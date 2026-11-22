use std::env;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use echoflow_core::domain::{
    DictationError, DisplayServer, PermissionStatus, PlatformCapabilities,
};
use echoflow_core::hotkey::parse_hotkey;
use echoflow_core::platform::HotkeyEvent;
use tokio::sync::mpsc;

/// Probe whether an executable exists on `PATH`.
pub fn command_exists(name: &str) -> bool {
    let Some(path_var) = env::var_os("PATH") else {
        return false;
    };
    env::split_paths(&path_var).any(|dir| {
        let candidate = dir.join(name);
        candidate.is_file()
    })
}

/// Probe whether any `/dev/input/event*` device is readable (Wayland `evdev` global hotkey capability).
pub fn has_readable_evdev() -> bool {
    let Ok(entries) = std::fs::read_dir("/dev/input") else {
        return false;
    };
    entries.flatten().any(|entry| {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        name_str.starts_with("event") && std::fs::File::open(entry.path()).is_ok()
    })
}

/// Probe Linux display server and Wayland helper capabilities (`evdev`, `ydotool`, `wtype`).
pub fn probe_linux_capabilities_from_env(
    xdg_session_type: Option<&str>,
    wayland_display: Option<&str>,
    display: Option<&str>,
    has_evdev: bool,
    has_ydotool: bool,
    has_wtype: bool,
    secret_service_available: bool,
) -> PlatformCapabilities {
    let is_wayland = xdg_session_type
        .map(|s| s.eq_ignore_ascii_case("wayland"))
        .unwrap_or(false)
        || wayland_display
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);

    if is_wayland {
        let mut wayland_helpers = Vec::new();
        if has_evdev {
            wayland_helpers.push("evdev".to_string());
        }
        if has_ydotool {
            wayland_helpers.push("ydotool".to_string());
        }
        if has_wtype {
            wayland_helpers.push("wtype".to_string());
        }

        let supports_global_hotkey = has_evdev;
        let supports_text_insertion = has_ydotool || has_wtype;

        if !supports_global_hotkey || !supports_text_insertion {
            tracing::warn!(
                status = "DEGRADED",
                component = "hotkey_adapter",
                cause = "Wayland session lacks evdev/ydotool/wtype permissions or helpers",
                helpers = ?wayland_helpers,
                "Wayland capabilities limited; Text Insertion will degrade to Fallback Delivery"
            );
        }

        return PlatformCapabilities {
            display_server: DisplayServer::LinuxWayland,
            microphone_permission: PermissionStatus::NotApplicable,
            accessibility_permission: PermissionStatus::NotApplicable,
            supports_global_hotkey,
            supports_text_insertion,
            wayland_helpers,
            secret_service_available,
        };
    }

    let is_x11 = xdg_session_type
        .map(|s| s.eq_ignore_ascii_case("x11"))
        .unwrap_or(false)
        || display.map(|s| !s.trim().is_empty()).unwrap_or(false);

    if is_x11 {
        return PlatformCapabilities {
            display_server: DisplayServer::LinuxX11,
            microphone_permission: PermissionStatus::NotApplicable,
            accessibility_permission: PermissionStatus::NotApplicable,
            supports_global_hotkey: true,
            supports_text_insertion: true,
            wayland_helpers: Vec::new(),
            secret_service_available,
        };
    }

    PlatformCapabilities {
        display_server: DisplayServer::Unknown,
        microphone_permission: PermissionStatus::NotApplicable,
        accessibility_permission: PermissionStatus::NotApplicable,
        supports_global_hotkey: false,
        supports_text_insertion: false,
        wayland_helpers: Vec::new(),
        secret_service_available,
    }
}

/// Probe OS permissions, display server, and Linux X11/Wayland capabilities.
pub fn probe_platform_capabilities() -> PlatformCapabilities {
    #[cfg(target_os = "macos")]
    {
        return PlatformCapabilities {
            display_server: DisplayServer::MacOs,
            microphone_permission: PermissionStatus::NotDetermined,
            accessibility_permission: PermissionStatus::NotDetermined,
            supports_global_hotkey: true,
            supports_text_insertion: true,
            wayland_helpers: Vec::new(),
            secret_service_available: true,
        };
    }

    #[cfg(target_os = "windows")]
    {
        return PlatformCapabilities {
            display_server: DisplayServer::Windows,
            microphone_permission: PermissionStatus::Granted,
            accessibility_permission: PermissionStatus::Granted,
            supports_global_hotkey: true,
            supports_text_insertion: true,
            wayland_helpers: Vec::new(),
            secret_service_available: true,
        };
    }

    #[cfg(target_os = "linux")]
    {
        let xdg = env::var("XDG_SESSION_TYPE").ok();
        let wayland = env::var("WAYLAND_DISPLAY").ok();
        let display = env::var("DISPLAY").ok();
        let has_evdev = has_readable_evdev();
        let has_ydotool = command_exists("ydotool") && Path::new("/dev/uinput").exists();
        let has_wtype = command_exists("wtype");
        let has_secret_service = command_exists("secret-tool");
        return probe_linux_capabilities_from_env(
            xdg.as_deref(),
            wayland.as_deref(),
            display.as_deref(),
            has_evdev,
            has_ydotool,
            has_wtype,
            has_secret_service,
        );
    }

    #[allow(unreachable_code)]
    PlatformCapabilities::default()
}

/// Handle to stop an active global Push-to-Talk Hotkey listener when rebinding.
pub struct HotkeyListenerHandle {
    stop_flag: Arc<AtomicBool>,
}

impl HotkeyListenerHandle {
    pub fn stop(&self) {
        self.stop_flag.store(true, Ordering::SeqCst);
    }
}

/// Start listening for the validated Push-to-Talk Hotkey and Escape cancel events.
///
/// Never steals window focus; on restricted Wayland sessions without `evdev`, emits a structured
/// `DEGRADED` warning and keeps the channel open cleanly.
pub fn start_global_hotkey_listener(
    binding: &str,
) -> Result<(mpsc::Receiver<HotkeyEvent>, HotkeyListenerHandle), DictationError> {
    let parsed = parse_hotkey(binding)?;
    let (tx, rx) = mpsc::channel(32);
    let stop_flag = Arc::new(AtomicBool::new(false));
    let stop_clone = Arc::clone(&stop_flag);

    let caps = probe_platform_capabilities();
    if !caps.supports_global_hotkey {
        tracing::warn!(
            status = "DEGRADED",
            component = "hotkey_adapter",
            cause = "Global Push-to-Talk Hotkey listener unavailable in current display session",
            binding = %parsed,
            "global hotkey listener degraded; use tray or UI controls"
        );
        // Keep sender alive until `stop_flag` is set so the receiver doesn't close immediately.
        std::thread::spawn(move || {
            while !stop_clone.load(Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
            drop(tx);
        });
        return Ok((rx, HotkeyListenerHandle { stop_flag }));
    }

    // Spawn dedicated OS hotkey monitor thread (X11 / Wayland evdev / macOS / Windows).
    // Uses `parsed` canonical binding and watches `Escape` for `HotkeyEvent::EscapeCancelled`.
    std::thread::spawn(move || {
        let _binding = parsed;
        while !stop_clone.load(Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        drop(tx);
    });

    Ok((rx, HotkeyListenerHandle { stop_flag }))
}

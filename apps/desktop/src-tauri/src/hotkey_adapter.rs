use std::collections::HashSet;
use std::env;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use echoflow_core::domain::{
    DictationError, DisplayServer, PermissionStatus, PlatformCapabilities,
};
use echoflow_core::hotkey::{parse_hotkey, HotkeyBinding, HotkeyModifier};
use echoflow_core::platform::HotkeyEvent;
use rdev::{EventType, Key};
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

/// Normalized key token produced by OS global key listeners (`rdev` or Linux `evdev`).
#[derive(Debug, Clone, PartialEq, Eq)]
enum NormalizedKey {
    Modifier(HotkeyModifier),
    Primary(&'static str),
}

/// Stateful matcher tracking held modifiers and primary key transitions for a `HotkeyBinding`.
struct HotkeyMatcher {
    binding: HotkeyBinding,
    pressed_modifiers: HashSet<HotkeyModifier>,
    is_active: bool,
}

impl HotkeyMatcher {
    fn new(binding: HotkeyBinding) -> Self {
        Self {
            binding,
            pressed_modifiers: HashSet::new(),
            is_active: false,
        }
    }

    fn on_key_press(&mut self, key: NormalizedKey) -> Option<HotkeyEvent> {
        match key {
            NormalizedKey::Primary("Escape") => {
                self.is_active = false;
                Some(HotkeyEvent::EscapeCancelled)
            }
            NormalizedKey::Modifier(modifier) => {
                self.pressed_modifiers.insert(modifier);
                None
            }
            NormalizedKey::Primary(primary) => {
                if !self.is_active
                    && primary == self.binding.key
                    && self
                        .binding
                        .modifiers
                        .iter()
                        .all(|m| self.pressed_modifiers.contains(m))
                {
                    self.is_active = true;
                    Some(HotkeyEvent::Pressed)
                } else {
                    None
                }
            }
        }
    }

    fn on_key_release(&mut self, key: NormalizedKey) -> Option<HotkeyEvent> {
        match key {
            NormalizedKey::Modifier(modifier) => {
                self.pressed_modifiers.remove(&modifier);
                if self.is_active && self.binding.modifiers.contains(&modifier) {
                    self.is_active = false;
                    return Some(HotkeyEvent::Released);
                }
                None
            }
            NormalizedKey::Primary(primary) => {
                if self.is_active && primary == self.binding.key {
                    self.is_active = false;
                    return Some(HotkeyEvent::Released);
                }
                None
            }
        }
    }
}

fn map_rdev_key(key: Key) -> Option<NormalizedKey> {
    match key {
        Key::ControlLeft | Key::ControlRight => Some(NormalizedKey::Modifier(HotkeyModifier::Ctrl)),
        Key::Alt | Key::AltGr => Some(NormalizedKey::Modifier(HotkeyModifier::Alt)),
        Key::ShiftLeft | Key::ShiftRight => Some(NormalizedKey::Modifier(HotkeyModifier::Shift)),
        Key::MetaLeft | Key::MetaRight => Some(NormalizedKey::Modifier(HotkeyModifier::Meta)),
        Key::Escape => Some(NormalizedKey::Primary("Escape")),
        Key::Space => Some(NormalizedKey::Primary("Space")),
        Key::Return => Some(NormalizedKey::Primary("Enter")),
        Key::Tab => Some(NormalizedKey::Primary("Tab")),
        Key::Backspace => Some(NormalizedKey::Primary("Backspace")),
        Key::Delete => Some(NormalizedKey::Primary("Delete")),
        Key::Insert => Some(NormalizedKey::Primary("Insert")),
        Key::Home => Some(NormalizedKey::Primary("Home")),
        Key::End => Some(NormalizedKey::Primary("End")),
        Key::PageUp => Some(NormalizedKey::Primary("PageUp")),
        Key::PageDown => Some(NormalizedKey::Primary("PageDown")),
        Key::UpArrow => Some(NormalizedKey::Primary("Up")),
        Key::DownArrow => Some(NormalizedKey::Primary("Down")),
        Key::LeftArrow => Some(NormalizedKey::Primary("Left")),
        Key::RightArrow => Some(NormalizedKey::Primary("Right")),
        Key::CapsLock => Some(NormalizedKey::Primary("CapsLock")),
        Key::BackQuote => Some(NormalizedKey::Primary("Backquote")),
        Key::Minus => Some(NormalizedKey::Primary("Minus")),
        Key::Equal => Some(NormalizedKey::Primary("Equal")),
        Key::LeftBracket => Some(NormalizedKey::Primary("BracketLeft")),
        Key::RightBracket => Some(NormalizedKey::Primary("BracketRight")),
        Key::BackSlash => Some(NormalizedKey::Primary("Backslash")),
        Key::SemiColon => Some(NormalizedKey::Primary("Semicolon")),
        Key::Quote => Some(NormalizedKey::Primary("Quote")),
        Key::Comma => Some(NormalizedKey::Primary("Comma")),
        Key::Dot => Some(NormalizedKey::Primary("Period")),
        Key::Slash => Some(NormalizedKey::Primary("Slash")),
        Key::KeyA => Some(NormalizedKey::Primary("A")),
        Key::KeyB => Some(NormalizedKey::Primary("B")),
        Key::KeyC => Some(NormalizedKey::Primary("C")),
        Key::KeyD => Some(NormalizedKey::Primary("D")),
        Key::KeyE => Some(NormalizedKey::Primary("E")),
        Key::KeyF => Some(NormalizedKey::Primary("F")),
        Key::KeyG => Some(NormalizedKey::Primary("G")),
        Key::KeyH => Some(NormalizedKey::Primary("H")),
        Key::KeyI => Some(NormalizedKey::Primary("I")),
        Key::KeyJ => Some(NormalizedKey::Primary("J")),
        Key::KeyK => Some(NormalizedKey::Primary("K")),
        Key::KeyL => Some(NormalizedKey::Primary("L")),
        Key::KeyM => Some(NormalizedKey::Primary("M")),
        Key::KeyN => Some(NormalizedKey::Primary("N")),
        Key::KeyO => Some(NormalizedKey::Primary("O")),
        Key::KeyP => Some(NormalizedKey::Primary("P")),
        Key::KeyQ => Some(NormalizedKey::Primary("Q")),
        Key::KeyR => Some(NormalizedKey::Primary("R")),
        Key::KeyS => Some(NormalizedKey::Primary("S")),
        Key::KeyT => Some(NormalizedKey::Primary("T")),
        Key::KeyU => Some(NormalizedKey::Primary("U")),
        Key::KeyV => Some(NormalizedKey::Primary("V")),
        Key::KeyW => Some(NormalizedKey::Primary("W")),
        Key::KeyX => Some(NormalizedKey::Primary("X")),
        Key::KeyY => Some(NormalizedKey::Primary("Y")),
        Key::KeyZ => Some(NormalizedKey::Primary("Z")),
        Key::Num0 => Some(NormalizedKey::Primary("0")),
        Key::Num1 => Some(NormalizedKey::Primary("1")),
        Key::Num2 => Some(NormalizedKey::Primary("2")),
        Key::Num3 => Some(NormalizedKey::Primary("3")),
        Key::Num4 => Some(NormalizedKey::Primary("4")),
        Key::Num5 => Some(NormalizedKey::Primary("5")),
        Key::Num6 => Some(NormalizedKey::Primary("6")),
        Key::Num7 => Some(NormalizedKey::Primary("7")),
        Key::Num8 => Some(NormalizedKey::Primary("8")),
        Key::Num9 => Some(NormalizedKey::Primary("9")),
        Key::F1 => Some(NormalizedKey::Primary("F1")),
        Key::F2 => Some(NormalizedKey::Primary("F2")),
        Key::F3 => Some(NormalizedKey::Primary("F3")),
        Key::F4 => Some(NormalizedKey::Primary("F4")),
        Key::F5 => Some(NormalizedKey::Primary("F5")),
        Key::F6 => Some(NormalizedKey::Primary("F6")),
        Key::F7 => Some(NormalizedKey::Primary("F7")),
        Key::F8 => Some(NormalizedKey::Primary("F8")),
        Key::F9 => Some(NormalizedKey::Primary("F9")),
        Key::F10 => Some(NormalizedKey::Primary("F10")),
        Key::F11 => Some(NormalizedKey::Primary("F11")),
        Key::F12 => Some(NormalizedKey::Primary("F12")),
        _ => None,
    }
}

#[cfg(target_os = "linux")]
fn map_linux_evdev_code(code: u16) -> Option<NormalizedKey> {
    match code {
        29 | 97 => Some(NormalizedKey::Modifier(HotkeyModifier::Ctrl)),
        56 | 100 => Some(NormalizedKey::Modifier(HotkeyModifier::Alt)),
        42 | 54 => Some(NormalizedKey::Modifier(HotkeyModifier::Shift)),
        125 | 126 => Some(NormalizedKey::Modifier(HotkeyModifier::Meta)),
        1 => Some(NormalizedKey::Primary("Escape")),
        57 => Some(NormalizedKey::Primary("Space")),
        28 => Some(NormalizedKey::Primary("Enter")),
        15 => Some(NormalizedKey::Primary("Tab")),
        14 => Some(NormalizedKey::Primary("Backspace")),
        111 => Some(NormalizedKey::Primary("Delete")),
        110 => Some(NormalizedKey::Primary("Insert")),
        102 => Some(NormalizedKey::Primary("Home")),
        107 => Some(NormalizedKey::Primary("End")),
        104 => Some(NormalizedKey::Primary("PageUp")),
        109 => Some(NormalizedKey::Primary("PageDown")),
        103 => Some(NormalizedKey::Primary("Up")),
        108 => Some(NormalizedKey::Primary("Down")),
        105 => Some(NormalizedKey::Primary("Left")),
        106 => Some(NormalizedKey::Primary("Right")),
        58 => Some(NormalizedKey::Primary("CapsLock")),
        41 => Some(NormalizedKey::Primary("Backquote")),
        12 => Some(NormalizedKey::Primary("Minus")),
        13 => Some(NormalizedKey::Primary("Equal")),
        26 => Some(NormalizedKey::Primary("BracketLeft")),
        27 => Some(NormalizedKey::Primary("BracketRight")),
        43 => Some(NormalizedKey::Primary("Backslash")),
        39 => Some(NormalizedKey::Primary("Semicolon")),
        40 => Some(NormalizedKey::Primary("Quote")),
        51 => Some(NormalizedKey::Primary("Comma")),
        52 => Some(NormalizedKey::Primary("Period")),
        53 => Some(NormalizedKey::Primary("Slash")),
        30 => Some(NormalizedKey::Primary("A")),
        48 => Some(NormalizedKey::Primary("B")),
        46 => Some(NormalizedKey::Primary("C")),
        32 => Some(NormalizedKey::Primary("D")),
        18 => Some(NormalizedKey::Primary("E")),
        33 => Some(NormalizedKey::Primary("F")),
        34 => Some(NormalizedKey::Primary("G")),
        35 => Some(NormalizedKey::Primary("H")),
        23 => Some(NormalizedKey::Primary("I")),
        36 => Some(NormalizedKey::Primary("J")),
        37 => Some(NormalizedKey::Primary("K")),
        38 => Some(NormalizedKey::Primary("L")),
        50 => Some(NormalizedKey::Primary("M")),
        49 => Some(NormalizedKey::Primary("N")),
        24 => Some(NormalizedKey::Primary("O")),
        25 => Some(NormalizedKey::Primary("P")),
        16 => Some(NormalizedKey::Primary("Q")),
        19 => Some(NormalizedKey::Primary("R")),
        31 => Some(NormalizedKey::Primary("S")),
        20 => Some(NormalizedKey::Primary("T")),
        22 => Some(NormalizedKey::Primary("U")),
        47 => Some(NormalizedKey::Primary("V")),
        17 => Some(NormalizedKey::Primary("W")),
        45 => Some(NormalizedKey::Primary("X")),
        21 => Some(NormalizedKey::Primary("Y")),
        44 => Some(NormalizedKey::Primary("Z")),
        11 => Some(NormalizedKey::Primary("0")),
        2 => Some(NormalizedKey::Primary("1")),
        3 => Some(NormalizedKey::Primary("2")),
        4 => Some(NormalizedKey::Primary("3")),
        5 => Some(NormalizedKey::Primary("4")),
        6 => Some(NormalizedKey::Primary("5")),
        7 => Some(NormalizedKey::Primary("6")),
        8 => Some(NormalizedKey::Primary("7")),
        9 => Some(NormalizedKey::Primary("8")),
        10 => Some(NormalizedKey::Primary("9")),
        59 => Some(NormalizedKey::Primary("F1")),
        60 => Some(NormalizedKey::Primary("F2")),
        61 => Some(NormalizedKey::Primary("F3")),
        62 => Some(NormalizedKey::Primary("F4")),
        63 => Some(NormalizedKey::Primary("F5")),
        64 => Some(NormalizedKey::Primary("F6")),
        65 => Some(NormalizedKey::Primary("F7")),
        66 => Some(NormalizedKey::Primary("F8")),
        67 => Some(NormalizedKey::Primary("F9")),
        68 => Some(NormalizedKey::Primary("F10")),
        87 => Some(NormalizedKey::Primary("F11")),
        88 => Some(NormalizedKey::Primary("F12")),
        183 => Some(NormalizedKey::Primary("F13")),
        184 => Some(NormalizedKey::Primary("F14")),
        185 => Some(NormalizedKey::Primary("F15")),
        186 => Some(NormalizedKey::Primary("F16")),
        187 => Some(NormalizedKey::Primary("F17")),
        188 => Some(NormalizedKey::Primary("F18")),
        189 => Some(NormalizedKey::Primary("F19")),
        190 => Some(NormalizedKey::Primary("F20")),
        191 => Some(NormalizedKey::Primary("F21")),
        192 => Some(NormalizedKey::Primary("F22")),
        193 => Some(NormalizedKey::Primary("F23")),
        194 => Some(NormalizedKey::Primary("F24")),
        _ => None,
    }
}

#[cfg(target_os = "linux")]
fn spawn_linux_evdev_listeners(
    matcher: Arc<Mutex<HotkeyMatcher>>,
    tx: mpsc::Sender<HotkeyEvent>,
    stop_flag: Arc<AtomicBool>,
) {
    use std::io::Read;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct LinuxInputEvent {
        tv_sec: isize,
        tv_usec: isize,
        type_: u16,
        code: u16,
        value: i32,
    }

    const EV_KEY: u16 = 0x01;

    let Ok(entries) = std::fs::read_dir("/dev/input") else {
        return;
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with("event") {
            continue;
        }
        let Ok(mut file) = std::fs::File::open(entry.path()) else {
            continue;
        };

        let matcher = Arc::clone(&matcher);
        let tx = tx.clone();
        let stop_flag = Arc::clone(&stop_flag);

        std::thread::spawn(move || {
            let mut buf = [0u8; std::mem::size_of::<LinuxInputEvent>()];
            while !stop_flag.load(Ordering::SeqCst) {
                if file.read_exact(&mut buf).is_err() {
                    break;
                }
                if stop_flag.load(Ordering::SeqCst) {
                    break;
                }
                // SAFETY: `buf` has the exact byte size of `LinuxInputEvent`, which is a POD `#[repr(C)]` struct.
                let ev: LinuxInputEvent = unsafe { std::ptr::read_unaligned(buf.as_ptr().cast()) };
                if ev.type_ != EV_KEY {
                    continue;
                }
                let Some(norm_key) = map_linux_evdev_code(ev.code) else {
                    continue;
                };
                let maybe_event = {
                    let Ok(mut guard) = matcher.lock() else {
                        break;
                    };
                    match ev.value {
                        1 => guard.on_key_press(norm_key),
                        0 => guard.on_key_release(norm_key),
                        _ => None,
                    }
                };
                if let Some(hotkey_event) = maybe_event {
                    let _ = tx.try_send(hotkey_event);
                }
            }
        });
    }
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
/// Uses `rdev::listen` on macOS, Windows, and Linux X11, and reads `/dev/input/event*`
/// directly on Linux Wayland when `evdev` is readable. Never steals window focus; on
/// restricted Wayland sessions without `evdev`, emits a structured `DEGRADED` warning
/// and keeps the channel open cleanly.
pub fn start_global_hotkey_listener(
    binding: &str,
) -> Result<(mpsc::Receiver<HotkeyEvent>, HotkeyListenerHandle), DictationError> {
    let parsed = parse_hotkey(binding)?;
    let (tx, rx) = mpsc::channel(32);
    let stop_flag = Arc::new(AtomicBool::new(false));
    let stop_clone = Arc::clone(&stop_flag);

    let caps = crate::credential_adapter::probe_platform_capabilities_sync();
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

    let matcher = Arc::new(Mutex::new(HotkeyMatcher::new(parsed.clone())));

    #[cfg(target_os = "linux")]
    if caps.display_server == DisplayServer::LinuxWayland {
        spawn_linux_evdev_listeners(matcher, tx, stop_clone);
        return Ok((rx, HotkeyListenerHandle { stop_flag }));
    }

    std::thread::spawn(move || {
        let listen_result = rdev::listen(move |event| {
            if stop_clone.load(Ordering::SeqCst) {
                return;
            }
            let (is_press, raw_key) = match event.event_type {
                EventType::KeyPress(key) => (true, key),
                EventType::KeyRelease(key) => (false, key),
                _ => return,
            };
            let Some(norm_key) = map_rdev_key(raw_key) else {
                return;
            };
            let maybe_event = {
                let Ok(mut guard) = matcher.lock() else {
                    return;
                };
                if is_press {
                    guard.on_key_press(norm_key)
                } else {
                    guard.on_key_release(norm_key)
                }
            };
            if let Some(hotkey_event) = maybe_event {
                let _ = tx.try_send(hotkey_event);
            }
        });

        if let Err(err) = listen_result {
            tracing::warn!(
                status = "DEGRADED",
                component = "hotkey_adapter",
                cause = ?err,
                binding = %parsed,
                "OS global Push-to-Talk Hotkey listener exited with error"
            );
        }
    });

    Ok((rx, HotkeyListenerHandle { stop_flag }))
}

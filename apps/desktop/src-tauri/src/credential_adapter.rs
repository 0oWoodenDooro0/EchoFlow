use echoflow_core::domain::{
    DictationError, DisplayServer, PermissionStatus, PlatformCapabilities,
};
use keyring::{Entry, Error as KeyringError};

const KEYRING_SERVICE: &str = "com.echoflow.desktop";
const KEYRING_ACCOUNT: &str = "provider-credential";

fn keyring_entry() -> Result<Entry, DictationError> {
    Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT)
        .map_err(|err| DictationError::CredentialStoreFailed(err.to_string()))
}

fn log_degraded_keyring(cause: &str) {
    tracing::warn!(
        status = "DEGRADED",
        component = "credential_adapter",
        cause = %cause,
        "OS native keyring is unavailable; start a Secret Service daemon (e.g. gnome-keyring or KWallet)"
    );
}

/// Read the Provider Credential from the OS native credential store.
pub async fn get_provider_credential() -> Result<Option<String>, DictationError> {
    tokio::task::spawn_blocking(|| {
        let entry = keyring_entry()?;
        match entry.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(KeyringError::NoEntry) => Ok(None),
            Err(KeyringError::PlatformFailure(err)) => {
                let cause = err.to_string();
                log_degraded_keyring(&cause);
                Err(DictationError::CredentialStoreFailed(cause))
            }
            Err(KeyringError::NoStorageAccess(err)) => {
                let cause = err.to_string();
                log_degraded_keyring(&cause);
                Err(DictationError::CredentialStoreFailed(cause))
            }
            Err(err) => Err(DictationError::CredentialStoreFailed(err.to_string())),
        }
    })
    .await
    .map_err(|err| DictationError::CredentialStoreFailed(err.to_string()))?
}

/// Store the Provider Credential in the OS native credential store.
pub async fn set_provider_credential(api_key: &str) -> Result<(), DictationError> {
    let key = api_key.to_string();
    tokio::task::spawn_blocking(move || {
        let entry = keyring_entry()?;
        match entry.set_password(&key) {
            Ok(()) => Ok(()),
            Err(KeyringError::PlatformFailure(err)) => {
                let cause = err.to_string();
                log_degraded_keyring(&cause);
                Err(DictationError::CredentialStoreFailed(cause))
            }
            Err(KeyringError::NoStorageAccess(err)) => {
                let cause = err.to_string();
                log_degraded_keyring(&cause);
                Err(DictationError::CredentialStoreFailed(cause))
            }
            Err(err) => Err(DictationError::CredentialStoreFailed(err.to_string())),
        }
    })
    .await
    .map_err(|err| DictationError::CredentialStoreFailed(err.to_string()))?
}

/// Delete the Provider Credential from the OS native credential store.
pub async fn delete_provider_credential() -> Result<(), DictationError> {
    tokio::task::spawn_blocking(|| {
        let entry = keyring_entry()?;
        match entry.delete_credential() {
            Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
            Err(KeyringError::PlatformFailure(err)) => {
                let cause = err.to_string();
                log_degraded_keyring(&cause);
                Err(DictationError::CredentialStoreFailed(cause))
            }
            Err(err) => Err(DictationError::CredentialStoreFailed(err.to_string())),
        }
    })
    .await
    .map_err(|err| DictationError::CredentialStoreFailed(err.to_string()))?
}

/// Synchronous single-source-of-truth probe for OS display server, macOS permissions,
/// Linux Wayland helpers (`evdev` readable `/dev/input/event*`, `ydotool`, `wtype`),
/// and Secret Service / native keyring availability.
pub fn probe_platform_capabilities_sync() -> PlatformCapabilities {
    let secret_service_available = check_keyring_available();

    #[cfg(target_os = "macos")]
    {
        return PlatformCapabilities {
            display_server: DisplayServer::MacOs,
            microphone_permission: probe_macos_microphone_permission(),
            accessibility_permission: probe_macos_accessibility_permission(),
            supports_global_hotkey: true,
            supports_text_insertion: true,
            wayland_helpers: Vec::new(),
            secret_service_available,
        };
    }

    #[cfg(target_os = "windows")]
    {
        return PlatformCapabilities {
            display_server: DisplayServer::Windows,
            microphone_permission: PermissionStatus::NotApplicable,
            accessibility_permission: PermissionStatus::NotApplicable,
            supports_global_hotkey: true,
            supports_text_insertion: true,
            wayland_helpers: Vec::new(),
            secret_service_available,
        };
    }

    #[cfg(target_os = "linux")]
    {
        let xdg = std::env::var("XDG_SESSION_TYPE").ok();
        let wayland = std::env::var("WAYLAND_DISPLAY").ok();
        let display = std::env::var("DISPLAY").ok();
        let has_evdev = crate::hotkey_adapter::has_readable_evdev();
        let has_ydotool = crate::hotkey_adapter::command_exists("ydotool")
            && std::path::Path::new("/dev/uinput").exists();
        let has_wtype = crate::hotkey_adapter::command_exists("wtype");
        return crate::hotkey_adapter::probe_linux_capabilities_from_env(
            xdg.as_deref(),
            wayland.as_deref(),
            display.as_deref(),
            has_evdev,
            has_ydotool,
            has_wtype,
            secret_service_available,
        );
    }

    #[allow(unreachable_code)]
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

/// Async wrapper for `probe_platform_capabilities_sync`.
pub async fn probe_platform_capabilities() -> Result<PlatformCapabilities, DictationError> {
    tokio::task::spawn_blocking(probe_platform_capabilities_sync)
        .await
        .map_err(|err| DictationError::CredentialStoreFailed(err.to_string()))
}

/// Open macOS System Settings directly to the Microphone or Accessibility privacy pane.
pub fn open_permission_settings(target: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let uri = match target {
            "microphone" => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"
            }
            "accessibility" => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
            }
            other => return Err(format!("unknown permission settings target: {other}")),
        };
        std::process::Command::new("open")
            .arg(uri)
            .spawn()
            .map_err(|err| format!("failed to open macOS System Settings: {err}"))?;
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = target;
        Ok(())
    }
}

fn check_keyring_available() -> bool {
    let Ok(entry) = keyring_entry() else {
        return false;
    };
    match entry.get_password() {
        Ok(_) | Err(KeyringError::NoEntry) => true,
        Err(KeyringError::PlatformFailure(err)) | Err(KeyringError::NoStorageAccess(err)) => {
            log_degraded_keyring(&err.to_string());
            false
        }
        Err(_) => false,
    }
}

#[cfg(target_os = "macos")]
fn probe_macos_microphone_permission() -> PermissionStatus {
    use std::ffi::{c_char, c_void};

    #[link(name = "AVFoundation", kind = "framework")]
    extern "C" {}
    #[link(name = "Foundation", kind = "framework")]
    extern "C" {}
    #[link(name = "objc", kind = "dylib")]
    extern "C" {
        fn objc_getClass(name: *const c_char) -> *mut c_void;
        fn sel_registerName(name: *const c_char) -> *mut c_void;
        fn objc_msgSend();
    }

    // SAFETY: Queries `[AVCaptureDevice authorizationStatusForMediaType:@"soun"]` via the
    // Objective-C runtime with exact ABI function pointer casts.
    unsafe {
        let av_capture_device = objc_getClass(b"AVCaptureDevice\0".as_ptr().cast::<c_char>());
        let ns_string_cls = objc_getClass(b"NSString\0".as_ptr().cast::<c_char>());
        if av_capture_device.is_null() || ns_string_cls.is_null() {
            return PermissionStatus::NotDetermined;
        }

        let str_sel = sel_registerName(b"stringWithUTF8String:\0".as_ptr().cast::<c_char>());
        let status_sel = sel_registerName(
            b"authorizationStatusForMediaType:\0"
                .as_ptr()
                .cast::<c_char>(),
        );

        let msg_send_str: extern "C" fn(*mut c_void, *mut c_void, *const c_char) -> *mut c_void =
            std::mem::transmute(objc_msgSend as *const c_void);
        let media_type = msg_send_str(ns_string_cls, str_sel, b"soun\0".as_ptr().cast::<c_char>());
        if media_type.is_null() {
            return PermissionStatus::NotDetermined;
        }

        let msg_send_status: extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> isize =
            std::mem::transmute(objc_msgSend as *const c_void);
        match msg_send_status(av_capture_device, status_sel, media_type) {
            0 => PermissionStatus::NotDetermined,
            1 | 2 => PermissionStatus::Denied,
            3 => PermissionStatus::Granted,
            _ => PermissionStatus::NotDetermined,
        }
    }
}

#[cfg(target_os = "macos")]
fn probe_macos_accessibility_permission() -> PermissionStatus {
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> bool;
    }
    // SAFETY: AXIsProcessTrusted is a pure C query in ApplicationServices on macOS.
    let trusted = unsafe { AXIsProcessTrusted() };
    if trusted {
        PermissionStatus::Granted
    } else {
        PermissionStatus::Denied
    }
}

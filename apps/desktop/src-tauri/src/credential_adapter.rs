use std::process::Command;

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

/// Probe OS display server, macOS permissions, Linux Wayland helpers, and Secret Service availability.
pub async fn probe_platform_capabilities() -> Result<PlatformCapabilities, DictationError> {
    tokio::task::spawn_blocking(|| {
        let display_server = detect_display_server();
        let secret_service_available = check_keyring_available();
        let wayland_helpers = if display_server == DisplayServer::LinuxWayland {
            detect_wayland_helpers()
        } else {
            Vec::new()
        };

        let (microphone_permission, accessibility_permission) =
            if display_server == DisplayServer::MacOs {
                (
                    probe_macos_microphone_permission(),
                    probe_macos_accessibility_permission(),
                )
            } else {
                (
                    PermissionStatus::NotApplicable,
                    PermissionStatus::NotApplicable,
                )
            };

        let supports_global_hotkey = match display_server {
            DisplayServer::LinuxWayland => wayland_helpers.iter().any(|h| h == "evdev"),
            _ => true,
        };
        let supports_text_insertion = match display_server {
            DisplayServer::LinuxWayland => wayland_helpers
                .iter()
                .any(|h| h == "ydotool" || h == "wtype"),
            _ => true,
        };

        Ok(PlatformCapabilities {
            display_server,
            microphone_permission,
            accessibility_permission,
            supports_global_hotkey,
            supports_text_insertion,
            wayland_helpers,
            secret_service_available,
        })
    })
    .await
    .map_err(|err| DictationError::CredentialStoreFailed(err.to_string()))?
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
        Command::new("open")
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

fn detect_display_server() -> DisplayServer {
    if cfg!(target_os = "macos") {
        DisplayServer::MacOs
    } else if cfg!(target_os = "windows") {
        DisplayServer::Windows
    } else if cfg!(target_os = "linux") {
        let session_type = std::env::var("XDG_SESSION_TYPE")
            .unwrap_or_default()
            .to_ascii_lowercase();
        let has_wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
        let has_x11 = std::env::var_os("DISPLAY").is_some();
        if session_type == "wayland" || has_wayland {
            DisplayServer::LinuxWayland
        } else if session_type == "x11" || has_x11 {
            DisplayServer::LinuxX11
        } else {
            DisplayServer::Unknown
        }
    } else {
        DisplayServer::Unknown
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

fn detect_wayland_helpers() -> Vec<String> {
    let mut helpers = Vec::new();
    for cmd in ["ydotool", "wtype"] {
        if Command::new("which")
            .arg(cmd)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            helpers.push(cmd.to_string());
        }
    }
    if std::path::Path::new("/dev/input").exists() {
        helpers.push("evdev".to_string());
    }
    helpers
}

#[cfg(target_os = "macos")]
fn probe_macos_microphone_permission() -> PermissionStatus {
    // Detailed AVFoundation authorization probing can be wired via objc2/CoreMedia in macOS builds;
    // default to NotDetermined until user grants in System Settings.
    PermissionStatus::NotDetermined
}

#[cfg(not(target_os = "macos"))]
fn probe_macos_microphone_permission() -> PermissionStatus {
    PermissionStatus::NotApplicable
}

#[cfg(target_os = "macos")]
fn probe_macos_accessibility_permission() -> PermissionStatus {
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

#[cfg(not(target_os = "macos"))]
fn probe_macos_accessibility_permission() -> PermissionStatus {
    PermissionStatus::NotApplicable
}

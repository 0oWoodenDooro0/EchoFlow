use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::domain::{AudioDeviceInfo, DictationError, PlatformCapabilities};

/// Global Push-to-Talk Hotkey and cancel events produced by `PlatformAdapter`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyEvent {
    Pressed,
    Released,
    EscapeCancelled,
}

/// Raw signed 16-bit PCM audio frame delivered by `PlatformAdapter`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PcmFrame {
    pub samples: Vec<i16>,
    pub sample_rate_hz: u32,
    pub channels: u16,
}

/// Snapshot of user clipboard contents captured during Clipboard Preservation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClipboardSnapshot {
    pub text: Option<String>,
}

/// Single outermost OS boundary implemented per desktop/mobile platform.
///
/// Kept strictly free of Tauri types so Phase 2 mobile shells can reuse `echoflow-core` via UniFFI.
#[async_trait]
pub trait PlatformAdapter: Send + Sync {
    /// Start listening for the configured Push-to-Talk Hotkey (and Escape cancel).
    async fn start_hotkey_listener(
        &self,
        binding: &str,
    ) -> Result<mpsc::Receiver<HotkeyEvent>, DictationError>;

    /// Enumerate available microphone input devices.
    async fn list_audio_devices(&self) -> Result<Vec<AudioDeviceInfo>, DictationError>;

    /// Start capturing microphone audio from the specified device (or OS default if `None`).
    async fn start_audio_capture(
        &self,
        device_id: Option<&str>,
    ) -> Result<mpsc::Receiver<PcmFrame>, DictationError>;

    /// Read current clipboard contents for Clipboard Preservation.
    async fn read_clipboard(&self) -> Result<ClipboardSnapshot, DictationError>;

    /// Write text to the OS clipboard.
    async fn write_clipboard(&self, text: &str) -> Result<(), DictationError>;

    /// Trigger synthetic OS paste (`Cmd+V` on macOS, `Ctrl+V` on Windows/Linux).
    async fn trigger_paste(&self) -> Result<(), DictationError>;

    /// Retrieve the Provider Credential from the OS native credential store.
    async fn get_provider_credential(&self) -> Result<Option<String>, DictationError>;

    /// Store the Provider Credential in the OS native credential store.
    async fn set_provider_credential(&self, api_key: &str) -> Result<(), DictationError>;

    /// Delete the Provider Credential from the OS native credential store.
    async fn delete_provider_credential(&self) -> Result<(), DictationError>;

    /// Probe OS permissions and Linux X11/Wayland capabilities.
    async fn probe_capabilities(&self) -> Result<PlatformCapabilities, DictationError>;
}

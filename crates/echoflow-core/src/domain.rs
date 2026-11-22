use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Lifecycle state of a Dictation Session owned by `DictationController`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionState {
    Idle,
    Connecting,
    Listening,
    Finalizing,
    Delivering,
    Error {
        message: String,
    },
    FallbackNotice {
        final_transcript: String,
        reason: String,
    },
}

/// Status reflected by the native system tray icon and menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrayStatus {
    Idle,
    Listening,
    MissingCredential,
    Error,
}

/// How the Final Transcript of a Dictation Session was delivered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryMode {
    TextInsertion,
    FallbackDelivery,
    Aborted,
}

/// Audio input device descriptor exposed to Settings and the system tray.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioDeviceInfo {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

/// Active desktop display server / OS environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayServer {
    MacOs,
    Windows,
    LinuxX11,
    LinuxWayland,
    Unknown,
}

/// OS permission probe state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionStatus {
    Granted,
    Denied,
    NotDetermined,
    NotApplicable,
}

/// Probed OS capabilities for onboarding, permissions, and Linux Wayland fallback.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformCapabilities {
    pub display_server: DisplayServer,
    pub microphone_permission: PermissionStatus,
    pub accessibility_permission: PermissionStatus,
    pub supports_global_hotkey: bool,
    pub supports_text_insertion: bool,
    pub wayland_helpers: Vec<String>,
    pub secret_service_available: bool,
}

impl Default for PlatformCapabilities {
    fn default() -> Self {
        Self {
            display_server: DisplayServer::Unknown,
            microphone_permission: PermissionStatus::Granted,
            accessibility_permission: PermissionStatus::Granted,
            supports_global_hotkey: true,
            supports_text_insertion: true,
            wayland_helpers: Vec::new(),
            secret_service_available: true,
        }
    }
}

/// Text-only local record of a completed Dictation Session (never stores raw audio).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: String,
    pub final_transcript: String,
    pub created_at_utc: String,
    pub duration_ms: u64,
    pub was_partial: bool,
    pub delivery_mode: DeliveryMode,
}

/// User-managed term for Gemini recognition biasing (cap: 1,000 entries).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VocabularyEntry {
    pub id: String,
    pub term: String,
    pub created_at_utc: String,
}

/// Hard cap on Custom Vocabulary entries (ADR / Issue #5).
pub const MAX_VOCABULARY_TERMS: usize = 1000;

/// Minimum allowed history retention period in days.
pub const MIN_HISTORY_RETENTION_DAYS: u32 = 1;

/// Maximum allowed history retention period in days.
pub const MAX_HISTORY_RETENTION_DAYS: u32 = 3650;

/// Current schema version for JSON settings & vocabulary export/import.
pub const SETTINGS_EXPORT_SCHEMA_VERSION: u32 = 1;

/// Non-secret local application preferences.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppPreferences {
    pub push_to_talk_hotkey: String,
    pub microphone_id: Option<String>,
    pub restore_clipboard: bool,
    pub clipboard_restore_delay_ms: u64,
    pub history_retention_days: u32,
}

/// Versioned JSON export/import payload for non-secret preferences and Custom Vocabulary.
///
/// Explicitly excludes Provider Credential and History Entry records (ADR-0004).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsExportPayload {
    pub schema_version: u32,
    pub exported_at_utc: String,
    pub preferences: AppPreferences,
    pub vocabulary: Vec<String>,
}

impl Default for AppPreferences {
    fn default() -> Self {
        Self {
            push_to_talk_hotkey: "Alt+Space".to_string(),
            microphone_id: None,
            restore_clipboard: true,
            clipboard_restore_delay_ms: 150,
            history_retention_days: 30,
        }
    }
}

/// Structured diagnostic event emitted for fail-open / degraded paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructuredLogEvent {
    pub status: String,
    pub component: String,
    pub message: String,
    pub cause: Option<String>,
}

/// Observable events broadcast by `DictationController` to the Overlay Pill, Settings, and Tray.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControllerEvent {
    StateChanged {
        state: SessionState,
    },
    DraftTranscriptUpdated {
        draft_transcript: String,
    },
    FinalTranscriptReady {
        final_transcript: String,
        delivery_mode: DeliveryMode,
    },
    AudioLevelUpdated {
        rms: f32,
    },
    SessionWarning {
        seconds_remaining: u64,
    },
    ErrorOccurred {
        message: String,
    },
    FallbackDeliveryNoticed {
        final_transcript: String,
        reason: String,
    },
    TrayStatusChanged {
        status: TrayStatus,
    },
}

/// Domain errors across the Rust Core.
#[derive(Debug, Error)]
pub enum DictationError {
    #[error("a Dictation Session is already active")]
    SessionAlreadyActive,

    #[error("no active Dictation Session")]
    NoActiveSession,

    #[error("Provider Credential is missing")]
    MissingProviderCredential,

    #[error("audio capture failed: {0}")]
    AudioCaptureFailed(String),

    #[error("transcription transport failed: {0}")]
    TranscriptionTransportFailed(String),

    #[error("clipboard operation failed: {0}")]
    ClipboardFailed(String),

    #[error("Text Insertion failed: {0}")]
    TextInsertionFailed(String),

    #[error("credential store error: {0}")]
    CredentialStoreFailed(String),

    #[error("local store error: {0}")]
    StorageFailed(String),

    #[error("invalid input: {0}")]
    InvalidInput(String),
}

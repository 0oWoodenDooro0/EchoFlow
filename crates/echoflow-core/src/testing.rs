use std::collections::VecDeque;
use std::sync::Mutex;

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::domain::{AudioDeviceInfo, DictationError, PlatformCapabilities};
use crate::platform::{ClipboardSnapshot, HotkeyEvent, PcmFrame, PlatformAdapter};

/// Recorded clipboard and paste operations for deterministic verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardOp {
    Read,
    Write(String),
    Paste,
}

/// Deterministic test double for `PlatformAdapter`.
#[derive(Debug, Default)]
pub struct FakePlatformAdapter {
    credential: Mutex<Option<String>>,
    clipboard: Mutex<Option<String>>,
    clipboard_ops: Mutex<Vec<ClipboardOp>>,
    audio_frames: Mutex<VecDeque<PcmFrame>>,
    devices: Mutex<Vec<AudioDeviceInfo>>,
    capabilities: Mutex<PlatformCapabilities>,
    fail_paste_with: Mutex<Option<String>>,
    fail_credential_store_with: Mutex<Option<String>>,
}

impl FakePlatformAdapter {
    pub fn new() -> Self {
        Self {
            devices: Mutex::new(vec![AudioDeviceInfo {
                id: "default-mic".to_string(),
                name: "Default Fake Microphone".to_string(),
                is_default: true,
            }]),
            ..Self::default()
        }
    }

    pub fn set_credential_sync(&self, api_key: Option<String>) {
        *self.credential.lock().unwrap() = api_key;
    }

    pub fn set_clipboard_sync(&self, text: Option<String>) {
        *self.clipboard.lock().unwrap() = text;
    }

    pub fn current_clipboard(&self) -> Option<String> {
        self.clipboard.lock().unwrap().clone()
    }

    pub fn enqueue_audio_frames(&self, frames: Vec<PcmFrame>) {
        let mut queue = self.audio_frames.lock().unwrap();
        for frame in frames {
            queue.push_back(frame);
        }
    }

    pub fn set_fail_paste(&self, reason: Option<String>) {
        *self.fail_paste_with.lock().unwrap() = reason;
    }

    pub fn set_fail_credential_store(&self, reason: Option<String>) {
        *self.fail_credential_store_with.lock().unwrap() = reason;
    }

    pub fn set_capabilities(&self, capabilities: PlatformCapabilities) {
        *self.capabilities.lock().unwrap() = capabilities;
    }

    pub fn recorded_clipboard_ops(&self) -> Vec<ClipboardOp> {
        self.clipboard_ops.lock().unwrap().clone()
    }
}

#[async_trait]
impl PlatformAdapter for FakePlatformAdapter {
    async fn start_hotkey_listener(
        &self,
        _binding: &str,
    ) -> Result<mpsc::Receiver<HotkeyEvent>, DictationError> {
        let (_tx, rx) = mpsc::channel(16);
        Ok(rx)
    }

    async fn list_audio_devices(&self) -> Result<Vec<AudioDeviceInfo>, DictationError> {
        Ok(self.devices.lock().unwrap().clone())
    }

    async fn start_audio_capture(
        &self,
        _device_id: Option<&str>,
    ) -> Result<mpsc::Receiver<PcmFrame>, DictationError> {
        let frames: Vec<PcmFrame> = self.audio_frames.lock().unwrap().drain(..).collect();
        let (tx, rx) = mpsc::channel(32);
        tokio::spawn(async move {
            for frame in frames {
                if tx.send(frame).await.is_err() {
                    break;
                }
            }
            // Keep sender alive until consumer stops capture so the channel doesn't close prematurely.
            std::future::pending::<()>().await;
        });
        Ok(rx)
    }

    async fn read_clipboard(&self) -> Result<ClipboardSnapshot, DictationError> {
        self.clipboard_ops.lock().unwrap().push(ClipboardOp::Read);
        Ok(ClipboardSnapshot {
            text: self.clipboard.lock().unwrap().clone(),
        })
    }

    async fn write_clipboard(&self, text: &str) -> Result<(), DictationError> {
        self.clipboard_ops
            .lock()
            .unwrap()
            .push(ClipboardOp::Write(text.to_string()));
        *self.clipboard.lock().unwrap() = Some(text.to_string());
        Ok(())
    }

    async fn trigger_paste(&self) -> Result<(), DictationError> {
        self.clipboard_ops.lock().unwrap().push(ClipboardOp::Paste);
        if let Some(reason) = self.fail_paste_with.lock().unwrap().clone() {
            return Err(DictationError::TextInsertionFailed(reason));
        }
        Ok(())
    }

    async fn get_provider_credential(&self) -> Result<Option<String>, DictationError> {
        if let Some(reason) = self.fail_credential_store_with.lock().unwrap().clone() {
            return Err(DictationError::CredentialStoreFailed(reason));
        }
        Ok(self.credential.lock().unwrap().clone())
    }

    async fn set_provider_credential(&self, api_key: &str) -> Result<(), DictationError> {
        if let Some(reason) = self.fail_credential_store_with.lock().unwrap().clone() {
            return Err(DictationError::CredentialStoreFailed(reason));
        }
        *self.credential.lock().unwrap() = Some(api_key.to_string());
        Ok(())
    }

    async fn delete_provider_credential(&self) -> Result<(), DictationError> {
        if let Some(reason) = self.fail_credential_store_with.lock().unwrap().clone() {
            return Err(DictationError::CredentialStoreFailed(reason));
        }
        *self.credential.lock().unwrap() = None;
        Ok(())
    }

    async fn probe_capabilities(&self) -> Result<PlatformCapabilities, DictationError> {
        Ok(self.capabilities.lock().unwrap().clone())
    }
}

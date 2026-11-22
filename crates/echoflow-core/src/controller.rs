use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use tokio::sync::{broadcast, oneshot, Mutex};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use uuid::Uuid;

use crate::audio::{
    compute_normalized_rms, pcm_i16_to_le_bytes, AudioResampler, DEFAULT_CHUNK_SAMPLES,
};
use crate::delivery::DeliveryService;
use crate::domain::{
    AudioDeviceInfo, ControllerEvent, DeliveryMode, DictationError, HistoryEntry, SessionState,
    TrayStatus,
};
use crate::gemini::{GeminiLiveClient, GeminiServerEvent, GeminiSessionSender};
use crate::platform::{HotkeyEvent, PlatformAdapter};
use crate::store::LocalStore;

/// Configurable session warning (9:30) and hard cap (10:00) timers (ADR-0002).
///
/// Uses `tokio::time::sleep` so integration tests can advance time deterministically with
/// `tokio::time::pause()`.
#[derive(Debug, Clone, Copy)]
pub struct SessionTimersConfig {
    pub warning_after: Duration,
    pub hard_cap_after: Duration,
}

impl Default for SessionTimersConfig {
    fn default() -> Self {
        Self {
            warning_after: Duration::from_secs(9 * 60 + 30),
            hard_cap_after: Duration::from_secs(10 * 60),
        }
    }
}

struct ActiveSession {
    started_at: Instant,
    gemini_tx: GeminiSessionSender,
    audio_task: JoinHandle<()>,
    server_task: JoinHandle<()>,
    timer_task: JoinHandle<()>,
    final_rx: Option<oneshot::Receiver<Result<String, DictationError>>>,
    latest_draft: Arc<Mutex<String>>,
}

struct ControllerInner {
    state: SessionState,
    active_session: Option<ActiveSession>,
    ignore_next_release: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FinalizeTrigger {
    HotkeyRelease,
    HardCapTimeout,
}

#[derive(Clone)]
struct SessionRuntime {
    platform: Arc<dyn PlatformAdapter>,
    store: Arc<dyn LocalStore>,
    delivery: Arc<DeliveryService>,
    events_tx: broadcast::Sender<ControllerEvent>,
    inner: Arc<Mutex<ControllerInner>>,
    flush_timeout: Duration,
}

impl SessionRuntime {
    fn emit(&self, event: ControllerEvent) {
        let _ = self.events_tx.send(event);
    }

    async fn set_state(&self, new_state: SessionState) {
        {
            let mut guard = self.inner.lock().await;
            guard.state = new_state.clone();
        }
        let tray = match &new_state {
            SessionState::Idle | SessionState::FallbackNotice { .. } => TrayStatus::Idle,
            SessionState::Connecting
            | SessionState::Listening
            | SessionState::Finalizing
            | SessionState::Delivering => TrayStatus::Listening,
            SessionState::Error { .. } => TrayStatus::Error,
        };
        self.emit(ControllerEvent::StateChanged { state: new_state });
        self.emit(ControllerEvent::TrayStatusChanged { status: tray });
    }

    async fn transition_to_error(&self, message: String, tray_status: TrayStatus) {
        {
            let mut guard = self.inner.lock().await;
            guard.state = SessionState::Error {
                message: message.clone(),
            };
        }
        self.emit(ControllerEvent::StateChanged {
            state: SessionState::Error {
                message: message.clone(),
            },
        });
        self.emit(ControllerEvent::ErrorOccurred { message });
        self.emit(ControllerEvent::TrayStatusChanged {
            status: tray_status,
        });
    }

    async fn salvage_mid_stream_failure(&self, started_at: Instant, draft: &str, cause: String) {
        let trimmed = draft.trim();
        if !trimmed.is_empty() {
            tracing::warn!(
                status = "DEGRADED",
                component = "controller",
                cause = %cause,
                salvaged_chars = trimmed.len(),
                "mid-stream disconnect; salvaging partial Draft Transcript to clipboard and History Entry"
            );

            if let Err(clip_err) = self.platform.write_clipboard(trimmed).await {
                tracing::warn!(
                    status = "DEGRADED",
                    component = "controller",
                    cause = %clip_err,
                    "failed to write salvaged partial Draft Transcript to clipboard"
                );
            }

            let entry = HistoryEntry {
                id: Uuid::new_v4().to_string(),
                final_transcript: trimmed.to_string(),
                created_at_utc: Utc::now().to_rfc3339(),
                duration_ms: started_at.elapsed().as_millis() as u64,
                was_partial: true,
                delivery_mode: DeliveryMode::FallbackDelivery,
            };
            if let Err(store_err) = self.store.save_history_entry(&entry).await {
                tracing::warn!(
                    status = "DEGRADED",
                    component = "controller",
                    cause = %store_err,
                    "failed to persist salvaged partial History Entry"
                );
            }

            let message = format!(
                "Connection interrupted ({cause}). Partial transcript saved to clipboard and history."
            );
            self.transition_to_error(message, TrayStatus::Error).await;
        } else {
            tracing::warn!(
                status = "DEGRADED",
                component = "controller",
                cause = %cause,
                salvaged_chars = 0usize,
                "mid-stream disconnect before any non-empty Draft Transcript was received"
            );
            let message = format!("Connection interrupted: {cause}");
            self.transition_to_error(message, TrayStatus::Error).await;
        }
    }

    async fn handle_mid_stream_disconnect(&self, cause: String) -> bool {
        let active = {
            let mut guard = self.inner.lock().await;
            if matches!(
                guard.state,
                SessionState::Connecting | SessionState::Listening
            ) {
                guard.ignore_next_release = true;
                guard.active_session.take()
            } else {
                None
            }
        };

        let Some(session) = active else {
            return false;
        };

        session.audio_task.abort();
        session.timer_task.abort();
        session.gemini_tx.close().await;

        let latest = session.latest_draft.lock().await.clone();
        self.salvage_mid_stream_failure(session.started_at, &latest, cause)
            .await;
        true
    }

    async fn finalize_active_session(
        &self,
        trigger: FinalizeTrigger,
    ) -> Result<Option<String>, DictationError> {
        let mut active = {
            let mut guard = self.inner.lock().await;
            match guard.active_session.take() {
                Some(session) => {
                    guard.state = SessionState::Finalizing;
                    if trigger == FinalizeTrigger::HardCapTimeout {
                        guard.ignore_next_release = true;
                    }
                    session
                }
                None => {
                    if trigger == FinalizeTrigger::HotkeyRelease && guard.ignore_next_release {
                        guard.ignore_next_release = false;
                        return Ok(None);
                    }
                    return Err(DictationError::NoActiveSession);
                }
            }
        };

        self.emit(ControllerEvent::StateChanged {
            state: SessionState::Finalizing,
        });

        // Stop capturing audio and signal end-of-audio to Gemini.
        active.audio_task.abort();
        if trigger != FinalizeTrigger::HardCapTimeout {
            active.timer_task.abort();
        }
        let _ = active.gemini_tx.send_end_of_audio().await;

        let final_transcript = match active.final_rx.take() {
            Some(rx) => match tokio::time::timeout(self.flush_timeout, rx).await {
                Ok(Ok(Ok(text))) => text,
                Ok(Ok(Err(err))) => {
                    active.server_task.abort();
                    active.gemini_tx.close().await;
                    let latest = active.latest_draft.lock().await.clone();
                    self.salvage_mid_stream_failure(active.started_at, &latest, err.to_string())
                        .await;
                    return Err(err);
                }
                Ok(Err(_)) | Err(_) => {
                    tracing::warn!(
                        status = "DEGRADED",
                        component = "controller",
                        "finalization flush timed out; using latest Draft Transcript"
                    );
                    active.latest_draft.lock().await.clone()
                }
            },
            None => active.latest_draft.lock().await.clone(),
        };

        active.server_task.abort();
        active.gemini_tx.close().await;

        let trimmed = final_transcript.trim();
        if trimmed.is_empty() {
            self.set_state(SessionState::Idle).await;
            return Ok(None);
        }

        self.set_state(SessionState::Delivering).await;

        let prefs = self.store.get_preferences().await.unwrap_or_default();
        let outcome = self.delivery.deliver(trimmed, &prefs).await?;

        let duration_ms = active.started_at.elapsed().as_millis() as u64;
        let history_entry = HistoryEntry {
            id: Uuid::new_v4().to_string(),
            final_transcript: trimmed.to_string(),
            created_at_utc: Utc::now().to_rfc3339(),
            duration_ms,
            was_partial: false,
            delivery_mode: outcome.mode,
        };
        self.store.save_history_entry(&history_entry).await?;

        self.emit(ControllerEvent::FinalTranscriptReady {
            final_transcript: trimmed.to_string(),
            delivery_mode: outcome.mode,
        });

        if outcome.mode == DeliveryMode::FallbackDelivery {
            let reason = outcome
                .fallback_reason
                .unwrap_or_else(|| "Text Insertion unavailable".to_string());
            self.emit(ControllerEvent::StateChanged {
                state: SessionState::FallbackNotice {
                    final_transcript: trimmed.to_string(),
                    reason: reason.clone(),
                },
            });
            self.emit(ControllerEvent::FallbackDeliveryNoticed {
                final_transcript: trimmed.to_string(),
                reason,
            });
        }

        self.set_state(SessionState::Idle).await;
        Ok(Some(trimmed.to_string()))
    }
}

/// Top-level orchestrator owning the Dictation Session state machine.
pub struct DictationController {
    platform: Arc<dyn PlatformAdapter>,
    gemini: GeminiLiveClient,
    store: Arc<dyn LocalStore>,
    runtime: SessionRuntime,
    timers: SessionTimersConfig,
    events_tx: broadcast::Sender<ControllerEvent>,
    inner: Arc<Mutex<ControllerInner>>,
}

impl DictationController {
    pub fn new(
        platform: Arc<dyn PlatformAdapter>,
        gemini: GeminiLiveClient,
        store: Arc<dyn LocalStore>,
        timers: SessionTimersConfig,
    ) -> Self {
        let delivery = Arc::new(DeliveryService::new(Arc::clone(&platform)));
        let (events_tx, _) = broadcast::channel(128);
        let inner = Arc::new(Mutex::new(ControllerInner {
            state: SessionState::Idle,
            active_session: None,
            ignore_next_release: false,
        }));
        let runtime = SessionRuntime {
            platform: Arc::clone(&platform),
            store: Arc::clone(&store),
            delivery,
            events_tx: events_tx.clone(),
            inner: Arc::clone(&inner),
            flush_timeout: gemini.config().flush_timeout,
        };
        Self {
            platform,
            gemini,
            store,
            runtime,
            timers,
            events_tx,
            inner,
        }
    }

    /// Enumerate available microphone input devices from the platform adapter.
    pub async fn list_audio_devices(&self) -> Result<Vec<AudioDeviceInfo>, DictationError> {
        self.platform.list_audio_devices().await
    }

    /// Read the persisted microphone preference (`None` means OS default).
    pub async fn get_selected_microphone(&self) -> Result<Option<String>, DictationError> {
        Ok(self.store.get_preferences().await?.microphone_id)
    }

    /// Persist the selected microphone preference (`None` means OS default).
    pub async fn set_selected_microphone(
        &self,
        device_id: Option<String>,
    ) -> Result<(), DictationError> {
        let mut prefs = self.store.get_preferences().await?;
        prefs.microphone_id = device_id.filter(|id| !id.trim().is_empty());
        self.store.save_preferences(&prefs).await
    }

    /// Subscribe to observable `ControllerEvent` updates.
    pub fn subscribe(&self) -> broadcast::Receiver<ControllerEvent> {
        self.events_tx.subscribe()
    }

    /// Current `SessionState`.
    pub async fn state(&self) -> SessionState {
        self.inner.lock().await.state.clone()
    }

    /// Recompute and emit the current `TrayStatus` from controller state and credential presence.
    pub async fn refresh_tray_status(&self) -> TrayStatus {
        let state = self.inner.lock().await.state.clone();
        let tray = match state {
            SessionState::Connecting
            | SessionState::Listening
            | SessionState::Finalizing
            | SessionState::Delivering => TrayStatus::Listening,
            SessionState::Idle
            | SessionState::FallbackNotice { .. }
            | SessionState::Error { .. } => match self.platform.get_provider_credential().await {
                Ok(Some(key)) if !key.trim().is_empty() => match state {
                    SessionState::Error { .. } => TrayStatus::Error,
                    _ => TrayStatus::Idle,
                },
                _ => TrayStatus::MissingCredential,
            },
        };
        self.emit(ControllerEvent::TrayStatusChanged { status: tray });
        tray
    }

    fn emit(&self, event: ControllerEvent) {
        self.runtime.emit(event);
    }

    async fn set_state(&self, new_state: SessionState) {
        self.runtime.set_state(new_state).await;
    }

    async fn is_connecting(&self) -> bool {
        matches!(self.inner.lock().await.state, SessionState::Connecting)
    }

    /// Dispatch a `HotkeyEvent` from `PlatformAdapter::start_hotkey_listener`.
    pub async fn handle_hotkey_event(&self, event: HotkeyEvent) -> Result<(), DictationError> {
        match event {
            HotkeyEvent::Pressed => self.handle_hotkey_pressed().await,
            HotkeyEvent::Released => self.handle_hotkey_released().await.map(|_| ()),
            HotkeyEvent::EscapeCancelled => self.handle_escape_cancelled().await,
        }
    }

    /// Start a Dictation Session when the Push-to-Talk Hotkey is pressed.
    ///
    /// Rejects duplicate presses while a session is already active without corrupting state.
    pub async fn handle_hotkey_pressed(&self) -> Result<(), DictationError> {
        {
            let mut guard = self.inner.lock().await;
            match guard.state {
                SessionState::Idle
                | SessionState::Error { .. }
                | SessionState::FallbackNotice { .. } => {
                    guard.state = SessionState::Connecting;
                    guard.ignore_next_release = false;
                }
                SessionState::Connecting
                | SessionState::Listening
                | SessionState::Finalizing
                | SessionState::Delivering => {
                    return Err(DictationError::SessionAlreadyActive);
                }
            }
        }
        self.emit(ControllerEvent::StateChanged {
            state: SessionState::Connecting,
        });
        self.emit(ControllerEvent::TrayStatusChanged {
            status: TrayStatus::Listening,
        });

        let api_key = match self.platform.get_provider_credential().await {
            Ok(Some(key)) if !key.trim().is_empty() => key,
            Ok(_) => {
                if !self.is_connecting().await {
                    return Ok(());
                }
                let err = DictationError::MissingProviderCredential;
                self.transition_to_error(err.to_string(), TrayStatus::MissingCredential)
                    .await;
                return Err(err);
            }
            Err(err) => {
                if !self.is_connecting().await {
                    return Ok(());
                }
                self.transition_to_error(err.to_string(), TrayStatus::Error)
                    .await;
                return Err(err);
            }
        };

        if !self.is_connecting().await {
            return Ok(());
        }

        let prefs = self.store.get_preferences().await.unwrap_or_default();
        let vocabulary = self.store.list_vocabulary_terms().await.unwrap_or_default();

        let target_device_id = match self
            .resolve_microphone_id(prefs.microphone_id.as_deref())
            .await
        {
            Ok(id) => id,
            Err(err) => {
                self.transition_to_error(err.to_string(), TrayStatus::Error)
                    .await;
                return Err(err);
            }
        };

        let session = match self.gemini.connect(&api_key, &vocabulary).await {
            Ok(session) => session,
            Err(err) => {
                if !self.is_connecting().await {
                    return Ok(());
                }
                self.transition_to_error(err.to_string(), TrayStatus::Error)
                    .await;
                return Err(err);
            }
        };

        let (gemini_tx, mut gemini_rx) = session.split();

        if !self.is_connecting().await {
            gemini_tx.close().await;
            return Ok(());
        }

        let mut audio_rx = match self
            .platform
            .start_audio_capture(target_device_id.as_deref())
            .await
        {
            Ok(rx) => rx,
            Err(err) => {
                gemini_tx.close().await;
                if !self.is_connecting().await {
                    return Ok(());
                }
                self.transition_to_error(err.to_string(), TrayStatus::Error)
                    .await;
                return Err(err);
            }
        };

        let audio_gemini_tx = gemini_tx.clone();
        let audio_events_tx = self.events_tx.clone();
        let audio_inner = Arc::clone(&self.inner);

        let latest_draft = Arc::new(Mutex::new(String::new()));
        let server_draft = Arc::clone(&latest_draft);
        let server_events_tx = self.events_tx.clone();
        let server_runtime = self.runtime.clone();
        let (final_tx, final_rx) = oneshot::channel();

        let timers = self.timers;
        let timer_runtime = self.runtime.clone();

        let cancelled_during_connect = {
            let mut guard = self.inner.lock().await;
            if !matches!(guard.state, SessionState::Connecting) {
                true
            } else {
                let audio_task = tokio::spawn(async move {
                    let mut resampler = AudioResampler::new(DEFAULT_CHUNK_SAMPLES);
                    let mut transport_failed = false;

                    while let Some(frame) = audio_rx.recv().await {
                        for chunk in resampler.push_frame_immediate(&frame) {
                            let rms = compute_normalized_rms(&chunk);
                            let _ =
                                audio_events_tx.send(ControllerEvent::AudioLevelUpdated { rms });
                            let pcm_bytes = pcm_i16_to_le_bytes(&chunk);
                            if audio_gemini_tx.send_audio_chunk(pcm_bytes).await.is_err() {
                                transport_failed = true;
                                break;
                            }
                        }
                        if transport_failed {
                            break;
                        }
                    }

                    if !transport_failed {
                        // `audio_rx` closed while the session was still active -> microphone disconnected.
                        let disconnected_session = {
                            let mut guard = audio_inner.lock().await;
                            if matches!(
                                guard.state,
                                SessionState::Connecting | SessionState::Listening
                            ) {
                                let message =
                                    "microphone disconnected during Dictation Session".to_string();
                                guard.state = SessionState::Error {
                                    message: message.clone(),
                                };
                                guard.ignore_next_release = true;
                                Some((guard.active_session.take(), message))
                            } else {
                                None
                            }
                        };

                        if let Some((maybe_session, message)) = disconnected_session {
                            tracing::warn!(
                                status = "DEGRADED",
                                component = "audio",
                                cause = %message,
                                "microphone stream ended unexpectedly during active Dictation Session"
                            );
                            if let Some(active) = maybe_session {
                                active.server_task.abort();
                                active.timer_task.abort();
                                active.gemini_tx.close().await;
                            }
                            let _ = audio_events_tx.send(ControllerEvent::StateChanged {
                                state: SessionState::Error {
                                    message: message.clone(),
                                },
                            });
                            let _ =
                                audio_events_tx.send(ControllerEvent::ErrorOccurred { message });
                            let _ = audio_events_tx.send(ControllerEvent::TrayStatusChanged {
                                status: TrayStatus::Error,
                            });
                        }
                    }
                });

                let server_task = tokio::spawn(async move {
                    let mut final_tx = Some(final_tx);
                    while let Some(item) = gemini_rx.recv().await {
                        match item {
                            Ok(GeminiServerEvent::SetupComplete) => {}
                            Ok(GeminiServerEvent::DraftTranscript { text }) => {
                                *server_draft.lock().await = text.clone();
                                let _ = server_events_tx.send(
                                    ControllerEvent::DraftTranscriptUpdated {
                                        draft_transcript: text,
                                    },
                                );
                            }
                            Ok(GeminiServerEvent::FinalTranscript { text }) => {
                                if let Some(tx) = final_tx.take() {
                                    let _ = tx.send(Ok(text));
                                }
                                break;
                            }
                            Ok(GeminiServerEvent::Error { message }) => {
                                if server_runtime
                                    .handle_mid_stream_disconnect(message.clone())
                                    .await
                                {
                                    let _ = final_tx.take();
                                } else if let Some(tx) = final_tx.take() {
                                    let _ = tx.send(Err(
                                        DictationError::TranscriptionTransportFailed(message),
                                    ));
                                }
                                break;
                            }
                            Err(err) => {
                                if server_runtime
                                    .handle_mid_stream_disconnect(err.to_string())
                                    .await
                                {
                                    let _ = final_tx.take();
                                } else if let Some(tx) = final_tx.take() {
                                    let _ = tx.send(Err(err));
                                }
                                break;
                            }
                        }
                    }

                    if let Some(tx) = final_tx.take() {
                        let handled_mid_stream = server_runtime
                            .handle_mid_stream_disconnect(
                                "WebSocket connection closed unexpectedly".to_string(),
                            )
                            .await;
                        if !handled_mid_stream {
                            let fallback = server_draft.lock().await.clone();
                            let _ = tx.send(Ok(fallback));
                        }
                    }
                });

                let timer_task = tokio::spawn(async move {
                    if timers.warning_after < timers.hard_cap_after {
                        tokio::time::sleep(timers.warning_after).await;
                        let remaining = timers
                            .hard_cap_after
                            .saturating_sub(timers.warning_after)
                            .as_secs();
                        timer_runtime.emit(ControllerEvent::SessionWarning {
                            seconds_remaining: remaining,
                        });
                        let remaining_wait =
                            timers.hard_cap_after.saturating_sub(timers.warning_after);
                        tokio::time::sleep(remaining_wait).await;
                    } else {
                        tokio::time::sleep(timers.hard_cap_after).await;
                    }

                    let _ = timer_runtime
                        .finalize_active_session(FinalizeTrigger::HardCapTimeout)
                        .await;
                });

                guard.active_session = Some(ActiveSession {
                    started_at: Instant::now(),
                    gemini_tx: gemini_tx.clone(),
                    audio_task,
                    server_task,
                    timer_task,
                    final_rx: Some(final_rx),
                    latest_draft,
                });
                guard.state = SessionState::Listening;
                false
            }
        };

        if cancelled_during_connect {
            gemini_tx.close().await;
            return Ok(());
        }

        self.emit(ControllerEvent::StateChanged {
            state: SessionState::Listening,
        });
        self.emit(ControllerEvent::TrayStatusChanged {
            status: TrayStatus::Listening,
        });
        Ok(())
    }

    /// Complete the active Dictation Session when the Push-to-Talk Hotkey is released.
    pub async fn handle_hotkey_released(&self) -> Result<Option<String>, DictationError> {
        self.runtime
            .finalize_active_session(FinalizeTrigger::HotkeyRelease)
            .await
    }

    /// Abort an active Dictation Session (e.g. Escape key) without Text Insertion or History Entry.
    pub async fn handle_escape_cancelled(&self) -> Result<(), DictationError> {
        let active = {
            let mut guard = self.inner.lock().await;
            guard.active_session.take()
        };

        if let Some(session) = active {
            session.audio_task.abort();
            session.server_task.abort();
            session.timer_task.abort();
            session.gemini_tx.close().await;
        }

        self.set_state(SessionState::Idle).await;
        Ok(())
    }

    async fn resolve_microphone_id(
        &self,
        preferred_id: Option<&str>,
    ) -> Result<Option<String>, DictationError> {
        let devices = self.platform.list_audio_devices().await?;
        if devices.is_empty() {
            return Err(DictationError::AudioCaptureFailed(
                "no microphone input device available".to_string(),
            ));
        }

        match preferred_id {
            Some(id) if devices.iter().any(|d| d.id == id) => Ok(Some(id.to_string())),
            Some(missing_id) => {
                let fallback_name = devices
                    .iter()
                    .find(|d| d.is_default)
                    .or_else(|| devices.first())
                    .map(|d| d.name.as_str())
                    .unwrap_or("OS default");
                tracing::warn!(
                    status = "DEGRADED",
                    component = "audio",
                    requested_device = %missing_id,
                    fallback_device = %fallback_name,
                    "selected microphone is no longer connected; falling back to OS default microphone"
                );
                Ok(None)
            }
            None => Ok(None),
        }
    }

    async fn transition_to_error(&self, message: String, tray_status: TrayStatus) {
        self.runtime.transition_to_error(message, tray_status).await;
    }
}

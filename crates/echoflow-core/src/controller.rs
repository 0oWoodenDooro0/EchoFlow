use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use tokio::sync::{broadcast, oneshot, Mutex};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use uuid::Uuid;

use crate::audio::{compute_normalized_rms, normalize_frame_to_16khz_mono, pcm_i16_to_le_bytes};
use crate::delivery::DeliveryService;
use crate::domain::{
    ControllerEvent, DeliveryMode, DictationError, HistoryEntry, SessionState, TrayStatus,
};
use crate::gemini::{GeminiLiveClient, GeminiServerEvent, GeminiSessionSender};
use crate::platform::PlatformAdapter;
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
}

/// Top-level orchestrator owning the Dictation Session state machine.
pub struct DictationController {
    platform: Arc<dyn PlatformAdapter>,
    gemini: GeminiLiveClient,
    store: Arc<dyn LocalStore>,
    delivery: DeliveryService,
    timers: SessionTimersConfig,
    events_tx: broadcast::Sender<ControllerEvent>,
    inner: Mutex<ControllerInner>,
}

impl DictationController {
    pub fn new(
        platform: Arc<dyn PlatformAdapter>,
        gemini: GeminiLiveClient,
        store: Arc<dyn LocalStore>,
        timers: SessionTimersConfig,
    ) -> Self {
        let delivery = DeliveryService::new(Arc::clone(&platform));
        let (events_tx, _) = broadcast::channel(128);
        Self {
            platform,
            gemini,
            store,
            delivery,
            timers,
            events_tx,
            inner: Mutex::new(ControllerInner {
                state: SessionState::Idle,
                active_session: None,
            }),
        }
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
                let err = DictationError::MissingProviderCredential;
                self.transition_to_error(err.to_string(), TrayStatus::MissingCredential)
                    .await;
                return Err(err);
            }
            Err(err) => {
                self.transition_to_error(err.to_string(), TrayStatus::Error)
                    .await;
                return Err(err);
            }
        };

        let prefs = self.store.get_preferences().await.unwrap_or_default();
        let vocabulary = self.store.list_vocabulary_terms().await.unwrap_or_default();

        let session = match self.gemini.connect(&api_key, &vocabulary).await {
            Ok(session) => session,
            Err(err) => {
                self.transition_to_error(err.to_string(), TrayStatus::Error)
                    .await;
                return Err(err);
            }
        };

        let mut audio_rx = match self
            .platform
            .start_audio_capture(prefs.microphone_id.as_deref())
            .await
        {
            Ok(rx) => rx,
            Err(err) => {
                self.transition_to_error(err.to_string(), TrayStatus::Error)
                    .await;
                return Err(err);
            }
        };

        let (gemini_tx, mut gemini_rx) = session.split();
        let audio_gemini_tx = gemini_tx.clone();
        let audio_events_tx = self.events_tx.clone();

        let audio_task = tokio::spawn(async move {
            while let Some(frame) = audio_rx.recv().await {
                let mono_16k = normalize_frame_to_16khz_mono(&frame);
                if mono_16k.is_empty() {
                    continue;
                }
                let rms = compute_normalized_rms(&mono_16k);
                let _ = audio_events_tx.send(ControllerEvent::AudioLevelUpdated { rms });
                let pcm_bytes = pcm_i16_to_le_bytes(&mono_16k);
                if audio_gemini_tx.send_audio_chunk(pcm_bytes).await.is_err() {
                    break;
                }
            }
        });

        let latest_draft = Arc::new(Mutex::new(String::new()));
        let server_draft = Arc::clone(&latest_draft);
        let server_events_tx = self.events_tx.clone();
        let (final_tx, final_rx) = oneshot::channel();

        let server_task = tokio::spawn(async move {
            let mut final_tx = Some(final_tx);
            while let Some(item) = gemini_rx.recv().await {
                match item {
                    Ok(GeminiServerEvent::SetupComplete) => {}
                    Ok(GeminiServerEvent::DraftTranscript { text }) => {
                        *server_draft.lock().await = text.clone();
                        let _ = server_events_tx.send(ControllerEvent::DraftTranscriptUpdated {
                            draft_transcript: text,
                        });
                    }
                    Ok(GeminiServerEvent::FinalTranscript { text }) => {
                        if let Some(tx) = final_tx.take() {
                            let _ = tx.send(Ok(text));
                        }
                        break;
                    }
                    Ok(GeminiServerEvent::Error { message }) => {
                        if let Some(tx) = final_tx.take() {
                            let _ =
                                tx.send(Err(DictationError::TranscriptionTransportFailed(message)));
                        }
                        break;
                    }
                    Err(err) => {
                        if let Some(tx) = final_tx.take() {
                            let _ = tx.send(Err(err));
                        }
                        break;
                    }
                }
            }
            if let Some(tx) = final_tx.take() {
                let fallback = server_draft.lock().await.clone();
                let _ = tx.send(Ok(fallback));
            }
        });

        let timers = self.timers;
        let timer_events_tx = self.events_tx.clone();
        let timer_task = tokio::spawn(async move {
            if timers.warning_after < timers.hard_cap_after {
                tokio::time::sleep(timers.warning_after).await;
                let remaining = timers
                    .hard_cap_after
                    .saturating_sub(timers.warning_after)
                    .as_secs();
                let _ = timer_events_tx.send(ControllerEvent::SessionWarning {
                    seconds_remaining: remaining,
                });
            }
        });

        {
            let mut guard = self.inner.lock().await;
            guard.active_session = Some(ActiveSession {
                started_at: Instant::now(),
                gemini_tx,
                audio_task,
                server_task,
                timer_task,
                final_rx: Some(final_rx),
                latest_draft,
            });
        }

        self.set_state(SessionState::Listening).await;
        Ok(())
    }

    /// Complete the active Dictation Session when the Push-to-Talk Hotkey is released.
    pub async fn handle_hotkey_released(&self) -> Result<Option<String>, DictationError> {
        let mut active = {
            let mut guard = self.inner.lock().await;
            match guard.active_session.take() {
                Some(session) => {
                    guard.state = SessionState::Finalizing;
                    session
                }
                None => return Err(DictationError::NoActiveSession),
            }
        };

        self.emit(ControllerEvent::StateChanged {
            state: SessionState::Finalizing,
        });

        // Stop capturing audio and signal end-of-audio to Gemini.
        active.audio_task.abort();
        active.timer_task.abort();
        let _ = active.gemini_tx.send_end_of_audio().await;

        let flush_timeout = self.gemini.config().flush_timeout;
        let final_transcript = match active.final_rx.take() {
            Some(rx) => match tokio::time::timeout(flush_timeout, rx).await {
                Ok(Ok(Ok(text))) => text,
                Ok(Ok(Err(err))) => {
                    active.server_task.abort();
                    active.gemini_tx.close().await;
                    self.transition_to_error(err.to_string(), TrayStatus::Error)
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
}

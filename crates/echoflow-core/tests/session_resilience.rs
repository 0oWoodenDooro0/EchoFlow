use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use async_trait::async_trait;
use echoflow_core::controller::{DictationController, SessionTimersConfig};
use echoflow_core::domain::{
    AudioDeviceInfo, ControllerEvent, DeliveryMode, DictationError, PlatformCapabilities,
    SessionState, TrayStatus,
};
use echoflow_core::gemini::{GeminiClientConfig, GeminiLiveClient};
use echoflow_core::platform::{ClipboardSnapshot, HotkeyEvent, PcmFrame, PlatformAdapter};
use echoflow_core::store::{LocalStore, SqliteLocalStore};
use echoflow_core::testing::{ClipboardOp, FakePlatformAdapter};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::tungstenite::Message;
use tracing::field::{Field, Visit};
use tracing::Subscriber;

#[derive(Debug, Clone)]
struct CapturedLog {
    level: tracing::Level,
    status: Option<String>,
    cause: Option<String>,
}

#[derive(Default)]
struct LogVisitor {
    status: Option<String>,
    cause: Option<String>,
}

impl Visit for LogVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "status" => self.status = Some(value.to_string()),
            "cause" => self.cause = Some(value.to_string()),
            _ => {}
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let formatted = format!("{value:?}");
        let unquoted = formatted.trim_matches('"').to_string();
        match field.name() {
            "status" => self.status = Some(unquoted),
            "cause" => self.cause = Some(unquoted),
            _ => {}
        }
    }
}

#[derive(Clone, Default)]
struct LogCaptureSubscriber {
    logs: Arc<StdMutex<Vec<CapturedLog>>>,
}

impl LogCaptureSubscriber {
    fn logs(&self) -> Vec<CapturedLog> {
        self.logs.lock().unwrap().clone()
    }
}

impl Subscriber for LogCaptureSubscriber {
    fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let mut visitor = LogVisitor::default();
        event.record(&mut visitor);
        self.logs.lock().unwrap().push(CapturedLog {
            level: *event.metadata().level(),
            status: visitor.status,
            cause: visitor.cause,
        });
    }

    fn enter(&self, _span: &tracing::span::Id) {}
    fn exit(&self, _span: &tracing::span::Id) {}
}

/// Delegates all PlatformAdapter methods to `FakePlatformAdapter`, except `start_audio_capture`
/// returns a live channel so tests can inject PCM frames before and after virtual clock advances.
struct LiveAudioPlatformAdapter {
    inner: Arc<FakePlatformAdapter>,
    audio_rx: Mutex<Option<mpsc::Receiver<PcmFrame>>>,
}

impl LiveAudioPlatformAdapter {
    fn new(inner: Arc<FakePlatformAdapter>) -> (Self, mpsc::Sender<PcmFrame>) {
        let (tx, rx) = mpsc::channel(32);
        (
            Self {
                inner,
                audio_rx: Mutex::new(Some(rx)),
            },
            tx,
        )
    }
}

#[async_trait]
impl PlatformAdapter for LiveAudioPlatformAdapter {
    async fn start_hotkey_listener(
        &self,
        binding: &str,
    ) -> Result<mpsc::Receiver<HotkeyEvent>, DictationError> {
        self.inner.start_hotkey_listener(binding).await
    }

    async fn list_audio_devices(&self) -> Result<Vec<AudioDeviceInfo>, DictationError> {
        self.inner.list_audio_devices().await
    }

    async fn start_audio_capture(
        &self,
        _device_id: Option<&str>,
    ) -> Result<mpsc::Receiver<PcmFrame>, DictationError> {
        self.audio_rx
            .lock()
            .await
            .take()
            .ok_or_else(|| DictationError::AudioCaptureFailed("audio_rx already taken".into()))
    }

    async fn read_clipboard(&self) -> Result<ClipboardSnapshot, DictationError> {
        self.inner.read_clipboard().await
    }

    async fn write_clipboard(&self, text: &str) -> Result<(), DictationError> {
        self.inner.write_clipboard(text).await
    }

    async fn trigger_paste(&self) -> Result<(), DictationError> {
        self.inner.trigger_paste().await
    }

    async fn get_provider_credential(&self) -> Result<Option<String>, DictationError> {
        self.inner.get_provider_credential().await
    }

    async fn set_provider_credential(&self, api_key: &str) -> Result<(), DictationError> {
        self.inner.set_provider_credential(api_key).await
    }

    async fn delete_provider_credential(&self) -> Result<(), DictationError> {
        self.inner.delete_provider_credential().await
    }

    async fn probe_capabilities(&self) -> Result<PlatformCapabilities, DictationError> {
        self.inner.probe_capabilities().await
    }
}

struct ResilienceWsRecord {
    pcm_chunks_received: usize,
    pcm_bytes_received: usize,
    received_end_of_audio: bool,
}

async fn start_resilience_ws_server(
    draft_on_first_chunk: String,
    draft_on_second_chunk: String,
    final_transcript: String,
) -> (String, Arc<Mutex<ResilienceWsRecord>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let record = Arc::new(Mutex::new(ResilienceWsRecord {
        pcm_chunks_received: 0,
        pcm_bytes_received: 0,
        received_end_of_audio: false,
    }));

    let record_clone = Arc::clone(&record);
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let ws_stream = tokio_tungstenite::accept_async(stream).await.unwrap();
        let (mut write, mut read) = ws_stream.split();

        while let Some(Ok(msg)) = read.next().await {
            match msg {
                Message::Text(text) => {
                    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
                    match parsed.get("type").and_then(|v| v.as_str()) {
                        Some("setup") => {
                            let _ = write
                                .send(Message::Text(
                                    json!({ "type": "setup_complete" }).to_string().into(),
                                ))
                                .await;
                        }
                        Some("end_of_audio") => {
                            record_clone.lock().await.received_end_of_audio = true;
                            let _ = write
                                .send(Message::Text(
                                    json!({
                                        "type": "final_transcript",
                                        "text": final_transcript,
                                    })
                                    .to_string()
                                    .into(),
                                ))
                                .await;
                            break;
                        }
                        _ => {}
                    }
                }
                Message::Binary(bytes) => {
                    let chunk_num = {
                        let mut guard = record_clone.lock().await;
                        guard.pcm_chunks_received += 1;
                        guard.pcm_bytes_received += bytes.len();
                        guard.pcm_chunks_received
                    };
                    if chunk_num == 1 {
                        let _ = write
                            .send(Message::Text(
                                json!({
                                    "type": "draft_transcript",
                                    "text": draft_on_first_chunk,
                                })
                                .to_string()
                                .into(),
                            ))
                            .await;
                    } else if chunk_num == 2 {
                        let _ = write
                            .send(Message::Text(
                                json!({
                                    "type": "draft_transcript",
                                    "text": draft_on_second_chunk,
                                })
                                .to_string()
                                .into(),
                            ))
                            .await;
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    });

    (format!("ws://{addr}"), record)
}

#[tokio::test(start_paused = true)]
async fn warning_at_9_30_keeps_streaming_and_auto_finalizes_at_10_00_ignoring_subsequent_release() {
    let (ws_url, server_record) = start_resilience_ws_server(
        "Initial dictation before warning".to_string(),
        "Initial dictation before warning and continued after 9:30".to_string(),
        "Initial dictation before warning and continued after 9:30.".to_string(),
    )
    .await;

    let fake_platform = Arc::new(FakePlatformAdapter::new());
    fake_platform.set_credential_sync(Some("resilience-test-key".to_string()));
    fake_platform.set_clipboard_sync(Some("original clipboard".to_string()));
    let (live_platform, audio_tx) = LiveAudioPlatformAdapter::new(Arc::clone(&fake_platform));

    let store = Arc::new(SqliteLocalStore::open_in_memory().unwrap());
    let gemini = GeminiLiveClient::new(GeminiClientConfig {
        endpoint_url: ws_url,
        ..GeminiClientConfig::default()
    });

    let controller = DictationController::new(
        Arc::new(live_platform),
        gemini,
        Arc::clone(&store) as Arc<_>,
        SessionTimersConfig::default(), // 9:30 warning (570s), 10:00 cap (600s)
    );

    let mut events = controller.subscribe();
    controller.handle_hotkey_pressed().await.unwrap();
    assert_eq!(controller.state().await, SessionState::Listening);

    // Send first audio chunk and wait for the initial Draft Transcript.
    audio_tx
        .send(PcmFrame {
            samples: vec![1200, -1200, 800, -800],
            sample_rate_hz: 16_000,
            channels: 1,
        })
        .await
        .unwrap();

    loop {
        match events.recv().await.unwrap() {
            ControllerEvent::DraftTranscriptUpdated { draft_transcript }
                if draft_transcript == "Initial dictation before warning" =>
            {
                break;
            }
            _ => {}
        }
    }

    // Advance virtual clock to 9:30 (570 seconds).
    tokio::time::advance(Duration::from_secs(9 * 60 + 30)).await;

    let saw_warning = loop {
        if let ControllerEvent::SessionWarning { seconds_remaining } = events.recv().await.unwrap()
        {
            break Some(seconds_remaining);
        }
    };
    assert_eq!(saw_warning, Some(30));
    assert_eq!(controller.state().await, SessionState::Listening);
    assert!(
        !server_record.lock().await.received_end_of_audio,
        "9:30 warning must not interrupt audio streaming"
    );

    // Push a second audio chunk AFTER 9:30 and verify it continues streaming to Gemini.
    audio_tx
        .send(PcmFrame {
            samples: vec![1500, -1500, 900, -900],
            sample_rate_hz: 16_000,
            channels: 1,
        })
        .await
        .unwrap();

    loop {
        match events.recv().await.unwrap() {
            ControllerEvent::DraftTranscriptUpdated { draft_transcript }
                if draft_transcript
                    == "Initial dictation before warning and continued after 9:30" =>
            {
                break;
            }
            _ => {}
        }
    }
    assert_eq!(server_record.lock().await.pcm_chunks_received, 2);

    // Advance virtual clock by the remaining 30 seconds to hit 10:00 (600s) hard cap.
    tokio::time::advance(Duration::from_secs(30)).await;

    let mut saw_final = None;
    let mut saw_idle = false;
    while !saw_idle {
        match events.recv().await.unwrap() {
            ControllerEvent::FinalTranscriptReady {
                final_transcript,
                delivery_mode,
            } => {
                saw_final = Some((final_transcript, delivery_mode));
            }
            ControllerEvent::StateChanged {
                state: SessionState::Idle,
            } => {
                saw_idle = true;
            }
            _ => {}
        }
    }

    assert_eq!(
        saw_final,
        Some((
            "Initial dictation before warning and continued after 9:30.".to_string(),
            DeliveryMode::TextInsertion,
        ))
    );
    assert!(server_record.lock().await.received_end_of_audio);
    assert_eq!(controller.state().await, SessionState::Idle);

    // Verify Clipboard Preservation executed and restored original clipboard.
    assert_eq!(
        fake_platform.recorded_clipboard_ops(),
        vec![
            ClipboardOp::Read,
            ClipboardOp::Write(
                "Initial dictation before warning and continued after 9:30.".to_string()
            ),
            ClipboardOp::Paste,
            ClipboardOp::Write("original clipboard".to_string()),
        ]
    );
    assert_eq!(
        fake_platform.current_clipboard().as_deref(),
        Some("original clipboard")
    );

    // Verify History Entry saved with was_partial = false.
    let history = store.list_history_entries().await.unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(
        history[0].final_transcript,
        "Initial dictation before warning and continued after 9:30."
    );
    assert!(!history[0].was_partial);
    assert_eq!(history[0].delivery_mode, DeliveryMode::TextInsertion);

    // User finally releases the Push-to-Talk Hotkey after 10:00 auto-finalization -> cleanly ignored!
    let late_release = controller.handle_hotkey_released().await.unwrap();
    assert_eq!(late_release, None);
    assert_eq!(controller.state().await, SessionState::Idle);
    assert_eq!(store.list_history_entries().await.unwrap().len(), 1);
    assert_eq!(fake_platform.recorded_clipboard_ops().len(), 4);
}

enum MidStreamFailure {
    AbruptSocketDrop,
    ServerError(String),
}

async fn start_disconnect_ws_server(
    draft_before_failure: Option<String>,
    failure: MidStreamFailure,
) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let ws_stream = tokio_tungstenite::accept_async(stream).await.unwrap();
        let (mut write, mut read) = ws_stream.split();

        while let Some(Ok(msg)) = read.next().await {
            match msg {
                Message::Text(text) => {
                    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
                    if parsed.get("type").and_then(|v| v.as_str()) == Some("setup") {
                        let _ = write
                            .send(Message::Text(
                                json!({ "type": "setup_complete" }).to_string().into(),
                            ))
                            .await;
                    }
                }
                Message::Binary(_) => {
                    if let Some(ref draft) = draft_before_failure {
                        let _ = write
                            .send(Message::Text(
                                json!({
                                    "type": "draft_transcript",
                                    "text": draft,
                                })
                                .to_string()
                                .into(),
                            ))
                            .await;
                    }
                    match &failure {
                        MidStreamFailure::AbruptSocketDrop => {
                            // Drop socket abruptly without sending a WebSocket close frame.
                            drop(write);
                            drop(read);
                            break;
                        }
                        MidStreamFailure::ServerError(err_msg) => {
                            let _ = write
                                .send(Message::Text(
                                    json!({
                                        "type": "error",
                                        "message": err_msg,
                                    })
                                    .to_string()
                                    .into(),
                                ))
                                .await;
                            break;
                        }
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    });

    format!("ws://{addr}")
}

#[tokio::test]
async fn mid_stream_websocket_drop_salvages_non_empty_draft_to_clipboard_and_partial_history() {
    let subscriber = LogCaptureSubscriber::default();
    let _log_guard = tracing::subscriber::set_default(subscriber.clone());

    let ws_url = start_disconnect_ws_server(
        Some("Meeting notes from standup".to_string()),
        MidStreamFailure::AbruptSocketDrop,
    )
    .await;

    let platform = Arc::new(FakePlatformAdapter::new());
    platform.set_credential_sync(Some("resilience-drop-key".to_string()));
    platform.set_clipboard_sync(Some("old clipboard".to_string()));
    platform.enqueue_audio_frames(vec![PcmFrame {
        samples: vec![1000, -1000, 500, -500],
        sample_rate_hz: 16_000,
        channels: 1,
    }]);

    let store = Arc::new(SqliteLocalStore::open_in_memory().unwrap());
    let gemini = GeminiLiveClient::new(GeminiClientConfig {
        endpoint_url: ws_url,
        ..GeminiClientConfig::default()
    });
    let controller = DictationController::new(
        Arc::clone(&platform) as Arc<_>,
        gemini,
        Arc::clone(&store) as Arc<_>,
        SessionTimersConfig::default(),
    );

    let mut events = controller.subscribe();
    controller.handle_hotkey_pressed().await.unwrap();

    // Wait for ErrorOccurred emitted by mid-stream disconnect salvage.
    let mut saw_error_event = None;
    let mut saw_tray_error = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while (saw_error_event.is_none() || !saw_tray_error) && tokio::time::Instant::now() < deadline {
        if let Ok(Ok(evt)) = tokio::time::timeout(Duration::from_millis(250), events.recv()).await {
            match evt {
                ControllerEvent::ErrorOccurred { message } => saw_error_event = Some(message),
                ControllerEvent::TrayStatusChanged {
                    status: TrayStatus::Error,
                } => saw_tray_error = true,
                _ => {}
            }
        }
    }

    let err_msg = saw_error_event.expect("expected ErrorOccurred on mid-stream disconnect");
    assert!(saw_tray_error);
    assert!(matches!(
        controller.state().await,
        SessionState::Error { .. }
    ));
    assert!(!err_msg.is_empty());

    // Salvaged text written to clipboard.
    assert_eq!(
        platform.current_clipboard().as_deref(),
        Some("Meeting notes from standup")
    );
    assert!(platform
        .recorded_clipboard_ops()
        .contains(&ClipboardOp::Write(
            "Meeting notes from standup".to_string()
        )));

    // Persisted History Entry marked was_partial = true.
    let history = store.list_history_entries().await.unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].final_transcript, "Meeting notes from standup");
    assert!(history[0].was_partial);
    assert_eq!(history[0].delivery_mode, DeliveryMode::FallbackDelivery);

    // Structured degraded log emitted with status = "DEGRADED" and cause.
    let degraded_logs: Vec<_> = subscriber
        .logs()
        .into_iter()
        .filter(|l| l.level == tracing::Level::WARN && l.status.as_deref() == Some("DEGRADED"))
        .collect();
    assert!(
        degraded_logs.iter().any(|l| l.cause.is_some()),
        "expected structured DEGRADED warn log with cause, got: {:?}",
        subscriber.logs()
    );

    // Subsequent hotkey release is ignored cleanly.
    assert_eq!(controller.handle_hotkey_released().await.unwrap(), None);
    assert_eq!(store.list_history_entries().await.unwrap().len(), 1);
}

#[tokio::test]
async fn mid_stream_api_error_salvages_non_empty_draft_to_clipboard_and_partial_history() {
    let subscriber = LogCaptureSubscriber::default();
    let _log_guard = tracing::subscriber::set_default(subscriber.clone());

    let ws_url = start_disconnect_ws_server(
        Some("Partial API error text".to_string()),
        MidStreamFailure::ServerError("upstream 503 service unavailable".to_string()),
    )
    .await;

    let platform = Arc::new(FakePlatformAdapter::new());
    platform.set_credential_sync(Some("resilience-api-err-key".to_string()));
    platform.set_clipboard_sync(Some("old clipboard".to_string()));
    platform.enqueue_audio_frames(vec![PcmFrame {
        samples: vec![1000, -1000, 500, -500],
        sample_rate_hz: 16_000,
        channels: 1,
    }]);

    let store = Arc::new(SqliteLocalStore::open_in_memory().unwrap());
    let gemini = GeminiLiveClient::new(GeminiClientConfig {
        endpoint_url: ws_url,
        ..GeminiClientConfig::default()
    });
    let controller = DictationController::new(
        Arc::clone(&platform) as Arc<_>,
        gemini,
        Arc::clone(&store) as Arc<_>,
        SessionTimersConfig::default(),
    );

    let mut events = controller.subscribe();
    controller.handle_hotkey_pressed().await.unwrap();

    let mut saw_error_event = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while saw_error_event.is_none() && tokio::time::Instant::now() < deadline {
        if let Ok(Ok(ControllerEvent::ErrorOccurred { message })) =
            tokio::time::timeout(Duration::from_millis(250), events.recv()).await
        {
            saw_error_event = Some(message);
        }
    }

    let err_msg = saw_error_event.expect("expected ErrorOccurred on mid-stream API error");
    assert!(err_msg.contains("upstream 503 service unavailable"));
    assert_eq!(
        platform.current_clipboard().as_deref(),
        Some("Partial API error text")
    );

    let history = store.list_history_entries().await.unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].final_transcript, "Partial API error text");
    assert!(history[0].was_partial);

    let degraded_logs: Vec<_> = subscriber
        .logs()
        .into_iter()
        .filter(|l| l.level == tracing::Level::WARN && l.status.as_deref() == Some("DEGRADED"))
        .collect();
    assert!(
        degraded_logs.iter().any(|l| l
            .cause
            .as_deref()
            .unwrap_or("")
            .contains("upstream 503 service unavailable")),
        "expected structured DEGRADED log with upstream 503 cause, got: {:?}",
        subscriber.logs()
    );

    assert_eq!(controller.handle_hotkey_released().await.unwrap(), None);
}

#[tokio::test]
async fn disconnect_before_any_transcript_or_only_whitespace_leaves_clipboard_and_history_untouched(
) {
    for draft in [None, Some("   \t\n  ".to_string())] {
        let subscriber = LogCaptureSubscriber::default();
        let _log_guard = tracing::subscriber::set_default(subscriber.clone());

        let ws_url = start_disconnect_ws_server(draft, MidStreamFailure::AbruptSocketDrop).await;

        let platform = Arc::new(FakePlatformAdapter::new());
        platform.set_credential_sync(Some("resilience-empty-drop-key".to_string()));
        platform.set_clipboard_sync(Some("untouched clipboard".to_string()));
        platform.enqueue_audio_frames(vec![PcmFrame {
            samples: vec![400, -400],
            sample_rate_hz: 16_000,
            channels: 1,
        }]);

        let store = Arc::new(SqliteLocalStore::open_in_memory().unwrap());
        let gemini = GeminiLiveClient::new(GeminiClientConfig {
            endpoint_url: ws_url,
            ..GeminiClientConfig::default()
        });
        let controller = DictationController::new(
            Arc::clone(&platform) as Arc<_>,
            gemini,
            Arc::clone(&store) as Arc<_>,
            SessionTimersConfig::default(),
        );

        let mut events = controller.subscribe();
        controller.handle_hotkey_pressed().await.unwrap();

        let mut saw_error_event = None;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while saw_error_event.is_none() && tokio::time::Instant::now() < deadline {
            if let Ok(Ok(ControllerEvent::ErrorOccurred { message })) =
                tokio::time::timeout(Duration::from_millis(250), events.recv()).await
            {
                saw_error_event = Some(message);
            }
        }

        assert!(
            saw_error_event.is_some(),
            "Overlay Pill must receive ErrorOccurred on empty disconnect"
        );
        assert!(matches!(
            controller.state().await,
            SessionState::Error { .. }
        ));

        // Clipboard must remain untouched and no empty History Entry may be saved.
        assert!(platform.recorded_clipboard_ops().is_empty());
        assert_eq!(
            platform.current_clipboard().as_deref(),
            Some("untouched clipboard")
        );
        assert!(store.list_history_entries().await.unwrap().is_empty());

        let degraded_logs: Vec<_> = subscriber
            .logs()
            .into_iter()
            .filter(|l| l.level == tracing::Level::WARN && l.status.as_deref() == Some("DEGRADED"))
            .collect();
        assert!(
            degraded_logs.iter().any(|l| l.cause.is_some()),
            "expected structured DEGRADED warn log on empty disconnect, got: {:?}",
            subscriber.logs()
        );

        assert_eq!(controller.handle_hotkey_released().await.unwrap(), None);
    }
}

#[tokio::test(start_paused = true)]
async fn hard_cap_at_10_00_degrades_to_fallback_delivery_when_paste_fails() {
    let (ws_url, _server_record) = start_resilience_ws_server(
        "Long session draft".to_string(),
        "Long session draft continued".to_string(),
        "Long session final transcript.".to_string(),
    )
    .await;

    let fake_platform = Arc::new(FakePlatformAdapter::new());
    fake_platform.set_credential_sync(Some("resilience-fallback-key".to_string()));
    fake_platform.set_clipboard_sync(Some("prior clipboard".to_string()));
    fake_platform.set_fail_paste(Some("no focused input field".to_string()));
    let (live_platform, audio_tx) = LiveAudioPlatformAdapter::new(Arc::clone(&fake_platform));

    let store = Arc::new(SqliteLocalStore::open_in_memory().unwrap());
    let gemini = GeminiLiveClient::new(GeminiClientConfig {
        endpoint_url: ws_url,
        ..GeminiClientConfig::default()
    });
    let controller = DictationController::new(
        Arc::new(live_platform),
        gemini,
        Arc::clone(&store) as Arc<_>,
        SessionTimersConfig::default(),
    );

    let mut events = controller.subscribe();
    controller.handle_hotkey_pressed().await.unwrap();

    audio_tx
        .send(PcmFrame {
            samples: vec![900, -900],
            sample_rate_hz: 16_000,
            channels: 1,
        })
        .await
        .unwrap();

    loop {
        if let ControllerEvent::DraftTranscriptUpdated { .. } = events.recv().await.unwrap() {
            break;
        }
    }

    tokio::time::advance(Duration::from_secs(10 * 60)).await;

    let mut saw_fallback = false;
    let mut saw_idle = false;
    while !saw_idle {
        match events.recv().await.unwrap() {
            ControllerEvent::FallbackDeliveryNoticed {
                final_transcript,
                reason,
            } => {
                assert_eq!(final_transcript, "Long session final transcript.");
                assert!(reason.contains("no focused input field"));
                saw_fallback = true;
            }
            ControllerEvent::StateChanged {
                state: SessionState::Idle,
            } => saw_idle = true,
            _ => {}
        }
    }

    assert!(saw_fallback);
    assert_eq!(
        fake_platform.current_clipboard().as_deref(),
        Some("Long session final transcript.")
    );
    let history = store.list_history_entries().await.unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].delivery_mode, DeliveryMode::FallbackDelivery);
    assert!(!history[0].was_partial);

    // Late hotkey release is ignored cleanly.
    assert_eq!(controller.handle_hotkey_released().await.unwrap(), None);
}

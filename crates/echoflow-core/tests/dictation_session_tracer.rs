use std::sync::Arc;
use std::time::Duration;

use echoflow_core::controller::{DictationController, SessionTimersConfig};
use echoflow_core::domain::{
    ControllerEvent, DeliveryMode, DictationError, SessionState, TrayStatus,
};
use echoflow_core::gemini::{GeminiClientConfig, GeminiLiveClient};
use echoflow_core::platform::PcmFrame;
use echoflow_core::store::{LocalStore, SqliteLocalStore};
use echoflow_core::testing::{ClipboardOp, FakePlatformAdapter};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;

struct FakeWsRecord {
    setup_messages: Vec<serde_json::Value>,
    pcm_bytes_received: usize,
    received_end_of_audio: bool,
}

async fn start_fake_gemini_ws_server(
    draft_updates: Vec<String>,
    final_transcript: String,
) -> (String, Arc<Mutex<FakeWsRecord>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let record = Arc::new(Mutex::new(FakeWsRecord {
        setup_messages: Vec::new(),
        pcm_bytes_received: 0,
        received_end_of_audio: false,
    }));

    let record_clone = Arc::clone(&record);
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let ws_stream = tokio_tungstenite::accept_async(stream).await.unwrap();
        let (mut write, mut read) = ws_stream.split();

        let mut drafts_sent = false;
        while let Some(Ok(msg)) = read.next().await {
            match msg {
                Message::Text(text) => {
                    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
                    match parsed.get("type").and_then(|v| v.as_str()) {
                        Some("setup") => {
                            record_clone.lock().await.setup_messages.push(parsed);
                        }
                        Some("end_of_audio") => {
                            record_clone.lock().await.received_end_of_audio = true;
                            let final_msg = json!({
                                "type": "final_transcript",
                                "text": final_transcript,
                            });
                            let _ = write
                                .send(Message::Text(final_msg.to_string().into()))
                                .await;
                            break;
                        }
                        _ => {}
                    }
                }
                Message::Binary(bytes) => {
                    record_clone.lock().await.pcm_bytes_received += bytes.len();
                    if !drafts_sent {
                        drafts_sent = true;
                        for draft in &draft_updates {
                            let draft_msg = json!({
                                "type": "draft_transcript",
                                "text": draft,
                            });
                            let _ = write
                                .send(Message::Text(draft_msg.to_string().into()))
                                .await;
                        }
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    });

    (format!("ws://{addr}"), record)
}

#[tokio::test]
async fn full_dictation_session_lifecycle_streams_audio_emits_draft_and_delivers_final_transcript()
{
    let (ws_url, server_record) = start_fake_gemini_ws_server(
        vec![
            "Hello world".to_string(),
            "Hello EchoFlow world".to_string(),
        ],
        "Hello, EchoFlow world.".to_string(),
    )
    .await;

    let platform = Arc::new(FakePlatformAdapter::new());
    platform.set_credential_sync(Some("test-gemini-api-key".to_string()));
    platform.set_clipboard_sync(Some("previous clipboard".to_string()));
    platform.enqueue_audio_frames(vec![
        PcmFrame {
            samples: vec![1000, -1000, 2000, -2000],
            sample_rate_hz: 16_000,
            channels: 1,
        },
        PcmFrame {
            samples: vec![500, -500, 1500, -1500],
            sample_rate_hz: 16_000,
            channels: 1,
        },
    ]);

    let store = Arc::new(SqliteLocalStore::open_in_memory().unwrap());
    store.add_vocabulary_term("EchoFlow").await.unwrap();
    store.add_vocabulary_term("UniFFI").await.unwrap();
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

    assert_eq!(controller.state().await, SessionState::Idle);

    // 1. Hold Push-to-Talk Hotkey -> Connecting -> Listening
    controller.handle_hotkey_pressed().await.unwrap();
    assert_eq!(controller.state().await, SessionState::Listening);

    // Wait until both Draft Transcript updates are emitted
    let mut observed_states = Vec::new();
    let mut observed_drafts = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while observed_drafts.len() < 2 && tokio::time::Instant::now() < deadline {
        if let Ok(Ok(evt)) = tokio::time::timeout(Duration::from_millis(250), events.recv()).await {
            match evt {
                ControllerEvent::StateChanged { state } => observed_states.push(state),
                ControllerEvent::DraftTranscriptUpdated { draft_transcript } => {
                    observed_drafts.push(draft_transcript);
                }
                _ => {}
            }
        }
    }

    assert_eq!(
        observed_drafts,
        vec![
            "Hello world".to_string(),
            "Hello EchoFlow world".to_string()
        ]
    );

    // 2. Release Push-to-Talk Hotkey -> Finalizing -> Delivering -> Idle
    let delivered = controller.handle_hotkey_released().await.unwrap();
    assert_eq!(delivered.as_deref(), Some("Hello, EchoFlow world."));
    assert_eq!(controller.state().await, SessionState::Idle);

    // Drain remaining events
    while let Ok(evt) = events.try_recv() {
        if let ControllerEvent::StateChanged { state } = evt {
            observed_states.push(state);
        }
    }

    assert!(observed_states.contains(&SessionState::Connecting));
    assert!(observed_states.contains(&SessionState::Listening));
    assert!(observed_states.contains(&SessionState::Finalizing));
    assert!(observed_states.contains(&SessionState::Delivering));
    assert_eq!(observed_states.last(), Some(&SessionState::Idle));

    // Verify Fake WS Server received SMART setup, PCM bytes, and end_of_audio
    let rec = server_record.lock().await;
    assert_eq!(rec.setup_messages.len(), 1);
    assert_eq!(rec.setup_messages[0]["model"], "gemini-3.5-transcribe-live");
    assert_eq!(rec.setup_messages[0]["mode"], "SMART");
    assert_eq!(
        rec.setup_messages[0]["vocabulary"],
        json!(["EchoFlow", "UniFFI"])
    );
    assert!(rec.pcm_bytes_received >= 16);
    assert!(rec.received_end_of_audio);

    // Verify Clipboard Preservation sequence on PlatformAdapter
    let ops = platform.recorded_clipboard_ops();
    assert_eq!(
        ops,
        vec![
            ClipboardOp::Read,
            ClipboardOp::Write("Hello, EchoFlow world.".to_string()),
            ClipboardOp::Paste,
            ClipboardOp::Write("previous clipboard".to_string()),
        ]
    );

    // Verify HistoryEntry persisted (text-only)
    let history = store.list_history_entries().await.unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].final_transcript, "Hello, EchoFlow world.");
    assert_eq!(history[0].delivery_mode, DeliveryMode::TextInsertion);
    assert!(!history[0].was_partial);
}

#[tokio::test]
async fn duplicate_hotkey_press_while_active_is_rejected_without_corrupting_session() {
    let (ws_url, _record) = start_fake_gemini_ws_server(
        vec!["Still listening".to_string()],
        "Still listening cleanly.".to_string(),
    )
    .await;

    let platform = Arc::new(FakePlatformAdapter::new());
    platform.set_credential_sync(Some("test-key".to_string()));
    platform.enqueue_audio_frames(vec![PcmFrame {
        samples: vec![1200, -1200],
        sample_rate_hz: 16_000,
        channels: 1,
    }]);

    let store = Arc::new(SqliteLocalStore::open_in_memory().unwrap());
    store.add_vocabulary_term("EchoFlow").await.unwrap();
    store.add_vocabulary_term("UniFFI").await.unwrap();
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

    controller.handle_hotkey_pressed().await.unwrap();
    assert_eq!(controller.state().await, SessionState::Listening);

    // Duplicate press must be rejected and leave the active session intact.
    let err = controller.handle_hotkey_pressed().await.unwrap_err();
    assert!(matches!(err, DictationError::SessionAlreadyActive));
    assert_eq!(controller.state().await, SessionState::Listening);

    // Original session still finalizes and delivers cleanly.
    let delivered = controller.handle_hotkey_released().await.unwrap();
    assert_eq!(delivered.as_deref(), Some("Still listening cleanly."));
    assert_eq!(controller.state().await, SessionState::Idle);
}

#[tokio::test]
async fn missing_provider_credential_transitions_to_error_and_allows_recovery() {
    let (ws_url, _record) = start_fake_gemini_ws_server(
        vec!["Recovered".to_string()],
        "Recovered session.".to_string(),
    )
    .await;

    let platform = Arc::new(FakePlatformAdapter::new());
    // No Provider Credential configured initially.
    let store = Arc::new(SqliteLocalStore::open_in_memory().unwrap());
    store.add_vocabulary_term("EchoFlow").await.unwrap();
    store.add_vocabulary_term("UniFFI").await.unwrap();
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

    let err = controller.handle_hotkey_pressed().await.unwrap_err();
    assert!(matches!(err, DictationError::MissingProviderCredential));
    assert!(matches!(
        controller.state().await,
        SessionState::Error { .. }
    ));

    let mut saw_missing_tray = false;
    while let Ok(evt) = events.try_recv() {
        if matches!(
            evt,
            ControllerEvent::TrayStatusChanged {
                status: TrayStatus::MissingCredential
            }
        ) {
            saw_missing_tray = true;
        }
    }
    assert!(saw_missing_tray);

    // After configuring the Provider Credential, a new session succeeds.
    platform.set_credential_sync(Some("valid-key".to_string()));
    platform.enqueue_audio_frames(vec![PcmFrame {
        samples: vec![500, -500],
        sample_rate_hz: 16_000,
        channels: 1,
    }]);
    controller.handle_hotkey_pressed().await.unwrap();
    let delivered = controller.handle_hotkey_released().await.unwrap();
    assert_eq!(delivered.as_deref(), Some("Recovered session."));
    assert_eq!(controller.state().await, SessionState::Idle);
}

#[tokio::test]
async fn escape_cancel_aborts_session_without_delivery_or_history() {
    let (ws_url, _record) = start_fake_gemini_ws_server(
        vec!["Accidental trigger".to_string()],
        "Should never be delivered.".to_string(),
    )
    .await;

    let platform = Arc::new(FakePlatformAdapter::new());
    platform.set_credential_sync(Some("test-key".to_string()));
    platform.set_clipboard_sync(Some("untouched clipboard".to_string()));
    platform.enqueue_audio_frames(vec![PcmFrame {
        samples: vec![1000, -1000],
        sample_rate_hz: 16_000,
        channels: 1,
    }]);

    let store = Arc::new(SqliteLocalStore::open_in_memory().unwrap());
    store.add_vocabulary_term("EchoFlow").await.unwrap();
    store.add_vocabulary_term("UniFFI").await.unwrap();
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

    controller.handle_hotkey_pressed().await.unwrap();
    assert_eq!(controller.state().await, SessionState::Listening);

    controller.handle_escape_cancelled().await.unwrap();
    assert_eq!(controller.state().await, SessionState::Idle);
    assert!(platform.recorded_clipboard_ops().is_empty());
    assert_eq!(
        platform.current_clipboard().as_deref(),
        Some("untouched clipboard")
    );
    assert!(store.list_history_entries().await.unwrap().is_empty());
}

#[tokio::test]
async fn paste_failure_degrades_to_fallback_delivery_without_clipboard_restore() {
    let (ws_url, _record) = start_fake_gemini_ws_server(
        vec!["Wayland draft".to_string()],
        "Fallback transcript.".to_string(),
    )
    .await;

    let platform = Arc::new(FakePlatformAdapter::new());
    platform.set_credential_sync(Some("test-key".to_string()));
    platform.set_clipboard_sync(Some("old clipboard".to_string()));
    platform.set_fail_paste(Some("wayland compositor blocked paste".to_string()));
    platform.enqueue_audio_frames(vec![PcmFrame {
        samples: vec![800, -800],
        sample_rate_hz: 16_000,
        channels: 1,
    }]);

    let store = Arc::new(SqliteLocalStore::open_in_memory().unwrap());
    store.add_vocabulary_term("EchoFlow").await.unwrap();
    store.add_vocabulary_term("UniFFI").await.unwrap();
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
    let delivered = controller.handle_hotkey_released().await.unwrap();
    assert_eq!(delivered.as_deref(), Some("Fallback transcript."));

    // Final Transcript stays on clipboard (restore skipped on Fallback Delivery).
    assert_eq!(
        platform.current_clipboard().as_deref(),
        Some("Fallback transcript.")
    );
    assert_eq!(
        platform.recorded_clipboard_ops(),
        vec![
            ClipboardOp::Read,
            ClipboardOp::Write("Fallback transcript.".to_string()),
            ClipboardOp::Paste,
        ]
    );

    let mut saw_fallback_event = false;
    while let Ok(evt) = events.try_recv() {
        if let ControllerEvent::FallbackDeliveryNoticed {
            final_transcript,
            reason,
        } = evt
        {
            assert_eq!(final_transcript, "Fallback transcript.");
            assert!(reason.contains("wayland compositor blocked paste"));
            saw_fallback_event = true;
        }
    }
    assert!(saw_fallback_event);

    let history = store.list_history_entries().await.unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].delivery_mode, DeliveryMode::FallbackDelivery);
}

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use echoflow_core::controller::{DictationController, SessionTimersConfig};
use echoflow_core::domain::{
    AppPreferences, ControllerEvent, DictationError, HistoryEntry, SessionState, VocabularyEntry,
};
use echoflow_core::gemini::{
    parse_server_event, GeminiClientConfig, GeminiLiveClient, GeminiServerEvent,
};
use echoflow_core::platform::PcmFrame;
use echoflow_core::store::{LocalStore, SqliteLocalStore};
use echoflow_core::testing::FakePlatformAdapter;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;

async fn start_auth_rejecting_http_server(status_line: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        if let Ok((mut stream, _)) = listener.accept().await {
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf).await;
            let response =
                format!("{status_line}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            let _ = stream.write_all(response.as_bytes()).await;
        }
    });
    format!("ws://{addr}")
}

async fn start_handshake_ws_server(response_event: serde_json::Value) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let ws_stream = tokio_tungstenite::accept_async(stream).await.unwrap();
        let (mut write, mut read) = ws_stream.split();
        if let Some(Ok(Message::Text(text))) = read.next().await {
            let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(
                parsed["setup"]["model"],
                "models/gemini-3.5-transcribe-live"
            );
            assert_eq!(
                parsed["setup"]["generationConfig"]["responseModalities"],
                json!(["TEXT"])
            );
            assert_eq!(parsed["setup"]["inputAudioTranscription"]["mode"], "SMART");
            assert!(parsed["setup"].get("systemInstruction").is_none());
            let _ = write
                .send(Message::Text(response_event.to_string().into()))
                .await;
        }
    });
    format!("ws://{addr}")
}

#[tokio::test]
async fn verify_credential_rejects_missing_malformed_and_unauthorized_responses() {
    let client = GeminiLiveClient::new(GeminiClientConfig {
        endpoint_url: "ws://127.0.0.1:1".to_string(),
        flush_timeout: Duration::from_millis(500),
        ..GeminiClientConfig::default()
    });

    // Missing / whitespace-only credential
    assert!(matches!(
        client.verify_credential("").await,
        Err(DictationError::MissingProviderCredential)
    ));
    assert!(matches!(
        client.verify_credential("   \t ").await,
        Err(DictationError::MissingProviderCredential)
    ));

    // Malformed credential (newlines / internal whitespace)
    assert!(matches!(
        client.verify_credential("bad\nkey").await,
        Err(DictationError::InvalidInput(_))
    ));
    assert!(matches!(
        client.verify_credential("bad key with spaces").await,
        Err(DictationError::InvalidInput(_))
    ));

    // HTTP 401 Unauthorized during WebSocket upgrade
    let unauth_url = start_auth_rejecting_http_server("HTTP/1.1 401 Unauthorized").await;
    let unauth_client = GeminiLiveClient::new(GeminiClientConfig {
        endpoint_url: unauth_url,
        flush_timeout: Duration::from_millis(500),
        ..GeminiClientConfig::default()
    });
    assert!(matches!(
        unauth_client.verify_credential("invalid-api-key").await,
        Err(DictationError::UnauthorizedProviderCredential(_))
    ));

    // Server-level error after setup
    let err_ws_url = start_handshake_ws_server(json!({
        "error": {
            "message": "API key not valid (403)"
        }
    }))
    .await;
    let err_client = GeminiLiveClient::new(GeminiClientConfig {
        endpoint_url: err_ws_url,
        flush_timeout: Duration::from_millis(500),
        ..GeminiClientConfig::default()
    });
    assert!(matches!(
        err_client.verify_credential("revoked-api-key").await,
        Err(DictationError::UnauthorizedProviderCredential(_))
    ));

    // Valid setupComplete handshake
    let ok_ws_url = start_handshake_ws_server(json!({
        "setupComplete": {}
    }))
    .await;
    let ok_client = GeminiLiveClient::new(GeminiClientConfig {
        endpoint_url: ok_ws_url,
        flush_timeout: Duration::from_millis(500),
        ..GeminiClientConfig::default()
    });
    ok_client.verify_credential("valid-api-key").await.unwrap();
}

#[derive(Default)]
struct CapturedWsSession {
    api_key_header: Option<String>,
    setup_messages: Vec<serde_json::Value>,
    pcm_chunks: Vec<Vec<u8>>,
    received_end_of_audio: bool,
}

async fn start_recording_ws_server(
    draft_updates: Vec<String>,
    final_transcript: Option<String>,
) -> (String, Arc<Mutex<CapturedWsSession>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let captured = Arc::new(Mutex::new(CapturedWsSession::default()));
    let captured_clone = Arc::clone(&captured);

    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut seen_header = None;
        #[allow(clippy::result_large_err)]
        let ws_stream = tokio_tungstenite::accept_hdr_async(
            stream,
            |req: &tokio_tungstenite::tungstenite::handshake::server::Request, resp| {
                seen_header = req
                    .headers()
                    .get("x-goog-api-key")
                    .and_then(|v| v.to_str().ok())
                    .map(ToString::to_string);
                Ok(resp)
            },
        )
        .await
        .unwrap();
        captured_clone.lock().await.api_key_header = seen_header;

        let (mut write, mut read) = ws_stream.split();
        let mut drafts_sent = false;
        while let Some(Ok(msg)) = read.next().await {
            match msg {
                Message::Text(text) => {
                    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
                    if parsed.get("setup").is_some() {
                        captured_clone.lock().await.setup_messages.push(parsed);
                        let _ = write
                            .send(Message::Text(
                                json!({ "setupComplete": {} }).to_string().into(),
                            ))
                            .await;
                    } else if let Some(realtime) = parsed.get("realtimeInput") {
                        if realtime.get("audioStreamEnd").and_then(|v| v.as_bool()) == Some(true) {
                            captured_clone.lock().await.received_end_of_audio = true;
                            if let Some(ref final_text) = final_transcript {
                                let final_msg = json!({
                                    "serverContent": {
                                        "inputTranscription": { "text": final_text },
                                        "turnComplete": true,
                                    }
                                });
                                let _ = write
                                    .send(Message::Text(final_msg.to_string().into()))
                                    .await;
                                break;
                            }
                        } else if let Some(chunks) =
                            realtime.get("mediaChunks").and_then(|v| v.as_array())
                        {
                            for chunk in chunks {
                                assert_eq!(chunk["mimeType"], "audio/pcm;rate=16000");
                                let encoded = chunk["data"].as_str().unwrap();
                                let decoded = base64::engine::general_purpose::STANDARD
                                    .decode(encoded)
                                    .unwrap();
                                captured_clone.lock().await.pcm_chunks.push(decoded);
                            }
                            if !drafts_sent {
                                drafts_sent = true;
                                for draft in &draft_updates {
                                    let draft_msg = json!({
                                        "serverContent": {
                                            "inputTranscription": { "text": draft },
                                            "turnComplete": false,
                                        }
                                    });
                                    let _ = write
                                        .send(Message::Text(draft_msg.to_string().into()))
                                        .await;
                                }
                            }
                        }
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    });

    (format!("ws://{addr}"), captured)
}

struct TestVocabularyStore {
    inner: SqliteLocalStore,
    vocab: Vec<String>,
}

#[async_trait::async_trait]
impl LocalStore for TestVocabularyStore {
    async fn save_history_entry(&self, entry: &HistoryEntry) -> Result<(), DictationError> {
        self.inner.save_history_entry(entry).await
    }

    async fn list_history_entries(&self) -> Result<Vec<HistoryEntry>, DictationError> {
        self.inner.list_history_entries().await
    }

    async fn search_history_entries(
        &self,
        query: &str,
    ) -> Result<Vec<HistoryEntry>, DictationError> {
        self.inner.search_history_entries(query).await
    }

    async fn delete_history_entry(&self, id: &str) -> Result<bool, DictationError> {
        self.inner.delete_history_entry(id).await
    }

    async fn clear_history(&self) -> Result<usize, DictationError> {
        self.inner.clear_history().await
    }

    async fn list_vocabulary_terms(&self) -> Result<Vec<String>, DictationError> {
        Ok(self.vocab.clone())
    }

    async fn list_vocabulary_entries(&self) -> Result<Vec<VocabularyEntry>, DictationError> {
        self.inner.list_vocabulary_entries().await
    }

    async fn search_vocabulary_entries(
        &self,
        query: &str,
    ) -> Result<Vec<VocabularyEntry>, DictationError> {
        self.inner.search_vocabulary_entries(query).await
    }

    async fn add_vocabulary_term(&self, term: &str) -> Result<VocabularyEntry, DictationError> {
        self.inner.add_vocabulary_term(term).await
    }

    async fn update_vocabulary_term(
        &self,
        id: &str,
        new_term: &str,
    ) -> Result<VocabularyEntry, DictationError> {
        self.inner.update_vocabulary_term(id, new_term).await
    }

    async fn delete_vocabulary_term(&self, id: &str) -> Result<bool, DictationError> {
        self.inner.delete_vocabulary_term(id).await
    }

    async fn get_preferences(&self) -> Result<AppPreferences, DictationError> {
        self.inner.get_preferences().await
    }

    async fn save_preferences(&self, prefs: &AppPreferences) -> Result<(), DictationError> {
        self.inner.save_preferences(prefs).await
    }

    async fn export_settings_json(&self) -> Result<String, DictationError> {
        self.inner.export_settings_json().await
    }

    async fn import_settings_json(&self, raw_json: &str) -> Result<(), DictationError> {
        self.inner.import_settings_json(raw_json).await
    }
}

#[tokio::test]
async fn setup_configures_smart_mode_and_forwards_custom_vocabulary_with_1000_cap() {
    let (ws_url, captured) =
        start_recording_ws_server(vec![], Some("EchoFlow uses UniFFI.".to_string())).await;

    let platform = Arc::new(FakePlatformAdapter::new());
    platform.set_credential_sync(Some("vocab-test-key".to_string()));
    platform.enqueue_audio_frames(vec![PcmFrame {
        samples: vec![100, -100],
        sample_rate_hz: 16_000,
        channels: 1,
    }]);

    let store = Arc::new(TestVocabularyStore {
        inner: SqliteLocalStore::open_in_memory().unwrap(),
        vocab: vec![
            "EchoFlow".to_string(),
            "UniFFI".to_string(),
            "kubectl".to_string(),
        ],
    });

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
    let result = controller.handle_hotkey_released().await.unwrap();
    assert_eq!(result.as_deref(), Some("EchoFlow uses UniFFI."));

    let rec = captured.lock().await;
    assert_eq!(rec.api_key_header.as_deref(), Some("vocab-test-key"));
    assert_eq!(rec.setup_messages.len(), 1);
    assert_eq!(
        rec.setup_messages[0]["setup"]["model"],
        "models/gemini-3.5-transcribe-live"
    );
    assert_eq!(
        rec.setup_messages[0]["setup"]["generationConfig"]["responseModalities"],
        json!(["TEXT"])
    );
    assert_eq!(
        rec.setup_messages[0]["setup"]["inputAudioTranscription"]["mode"],
        "SMART"
    );
    assert_eq!(
        rec.setup_messages[0]["setup"]["systemInstruction"]["parts"][0]["text"],
        "Preferred vocabulary terms: EchoFlow, UniFFI, kubectl"
    );

    // Direct client call with > 1,000 terms + blank entries caps at 1,000 non-empty terms.
    let (cap_url, cap_captured) = start_recording_ws_server(vec![], None).await;
    let cap_client = GeminiLiveClient::new(GeminiClientConfig {
        endpoint_url: cap_url,
        ..GeminiClientConfig::default()
    });
    let mut oversized_vocab: Vec<String> = (0..1050).map(|i| format!("term-{i}")).collect();
    oversized_vocab.insert(0, "   ".to_string());
    let session = cap_client
        .connect("vocab-cap-key", &oversized_vocab)
        .await
        .unwrap();
    let (tx, _) = session.split();
    tokio::time::sleep(Duration::from_millis(50)).await;
    tx.close().await;

    let cap_rec = cap_captured.lock().await;
    let sys_text = cap_rec.setup_messages[0]["setup"]["systemInstruction"]["parts"][0]["text"]
        .as_str()
        .unwrap();
    let terms_csv = sys_text
        .strip_prefix("Preferred vocabulary terms: ")
        .unwrap();
    let sent_vocab: Vec<&str> = terms_csv.split(", ").collect();
    assert_eq!(sent_vocab.len(), 1000);
    assert_eq!(sent_vocab[0], "term-0");
    assert_eq!(sent_vocab[999], "term-999");
}

#[tokio::test]
async fn streams_raw_16bit_16khz_mono_pcm_chunks_continuously_while_listening() {
    let (ws_url, captured) =
        start_recording_ws_server(vec![], Some("PCM framing verified.".to_string())).await;

    let platform = Arc::new(FakePlatformAdapter::new());
    platform.set_credential_sync(Some("pcm-test-key".to_string()));
    platform.enqueue_audio_frames(vec![
        PcmFrame {
            samples: vec![0x1234, -0x1234],
            sample_rate_hz: 16_000,
            channels: 1,
        },
        PcmFrame {
            samples: vec![0x0001, 0x7FFF],
            sample_rate_hz: 16_000,
            channels: 1,
        },
    ]);

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

    controller.handle_hotkey_pressed().await.unwrap();
    // Allow audio_task to stream both frames before releasing.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        let total_bytes: usize = captured.lock().await.pcm_chunks.iter().map(Vec::len).sum();
        if total_bytes >= 8 || tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let delivered = controller.handle_hotkey_released().await.unwrap();
    assert_eq!(delivered.as_deref(), Some("PCM framing verified."));

    let rec = captured.lock().await;
    let combined_pcm: Vec<u8> = rec.pcm_chunks.iter().flatten().copied().collect();
    let expected_pcm: Vec<u8> = [0x1234_i16, -0x1234_i16, 0x0001_i16, 0x7FFF_i16]
        .into_iter()
        .flat_map(i16::to_le_bytes)
        .collect();
    assert_eq!(combined_pcm, expected_pcm);
    assert!(rec.received_end_of_audio);
}

#[tokio::test]
async fn draft_transcript_self_corrections_replace_earlier_words_without_duplication() {
    let (ws_url, _captured) = start_recording_ws_server(
        vec![
            "Let's meet at three".to_string(),
            "Let's meet at four".to_string(),
            "Let's meet at four in the main office".to_string(),
        ],
        Some("Let's meet at four in the main office.".to_string()),
    )
    .await;

    let platform = Arc::new(FakePlatformAdapter::new());
    platform.set_credential_sync(Some("draft-test-key".to_string()));
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

    let mut observed_drafts = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while observed_drafts.len() < 3 && tokio::time::Instant::now() < deadline {
        if let Ok(Ok(ControllerEvent::DraftTranscriptUpdated { draft_transcript })) =
            tokio::time::timeout(Duration::from_millis(250), events.recv()).await
        {
            observed_drafts.push(draft_transcript);
        }
    }

    assert_eq!(
        observed_drafts,
        vec![
            "Let's meet at three".to_string(),
            "Let's meet at four".to_string(),
            "Let's meet at four in the main office".to_string(),
        ]
    );

    let delivered = controller.handle_hotkey_released().await.unwrap();
    assert_eq!(
        delivered.as_deref(),
        Some("Let's meet at four in the main office.")
    );
}

#[tokio::test]
async fn flush_timeout_resolves_cleanly_with_latest_draft_rather_than_hanging() {
    // Server never sends final_transcript after end_of_audio.
    let (ws_url, captured) =
        start_recording_ws_server(vec!["Latest draft before timeout".to_string()], None).await;

    let platform = Arc::new(FakePlatformAdapter::new());
    platform.set_credential_sync(Some("timeout-test-key".to_string()));
    platform.enqueue_audio_frames(vec![PcmFrame {
        samples: vec![600, -600],
        sample_rate_hz: 16_000,
        channels: 1,
    }]);

    let store = Arc::new(SqliteLocalStore::open_in_memory().unwrap());
    let gemini = GeminiLiveClient::new(GeminiClientConfig {
        endpoint_url: ws_url,
        flush_timeout: Duration::from_millis(80),
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

    // Ensure the DraftTranscript has been processed before releasing.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Ok(ControllerEvent::DraftTranscriptUpdated { .. })) =
            tokio::time::timeout(Duration::from_millis(200), events.recv()).await
        {
            break;
        }
    }

    let released =
        tokio::time::timeout(Duration::from_secs(1), controller.handle_hotkey_released())
            .await
            .expect("handle_hotkey_released must not hang past flush_timeout")
            .unwrap();

    assert_eq!(released.as_deref(), Some("Latest draft before timeout"));
    assert_eq!(controller.state().await, SessionState::Idle);
    assert!(captured.lock().await.received_end_of_audio);

    let history = store.list_history_entries().await.unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].final_transcript, "Latest draft before timeout");
}

#[tokio::test]
async fn empty_or_whitespace_final_transcript_completes_without_insertion_or_history() {
    for final_text in ["", "   \n\t  "] {
        let (ws_url, _captured) =
            start_recording_ws_server(vec!["um".to_string()], Some(final_text.to_string())).await;

        let platform = Arc::new(FakePlatformAdapter::new());
        platform.set_credential_sync(Some("empty-test-key".to_string()));
        platform.set_clipboard_sync(Some("untouched clipboard".to_string()));
        platform.enqueue_audio_frames(vec![PcmFrame {
            samples: vec![300, -300],
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

        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while tokio::time::Instant::now() < deadline {
            if let Ok(Ok(ControllerEvent::DraftTranscriptUpdated { .. })) =
                tokio::time::timeout(Duration::from_millis(200), events.recv()).await
            {
                break;
            }
        }

        let delivered = controller.handle_hotkey_released().await.unwrap();
        assert_eq!(delivered, None);
        assert_eq!(controller.state().await, SessionState::Idle);
        assert!(platform.recorded_clipboard_ops().is_empty());
        assert_eq!(
            platform.current_clipboard().as_deref(),
            Some("untouched clipboard")
        );
        assert!(store.list_history_entries().await.unwrap().is_empty());
    }
}

#[test]
fn parse_server_event_handles_bidi_generate_content_variants() {
    assert_eq!(
        parse_server_event(r#"{"setupComplete":{}}"#).unwrap(),
        Some(GeminiServerEvent::SetupComplete)
    );
    assert_eq!(
        parse_server_event(
            r#"{"serverContent":{"inputTranscription":{"text":"hello"},"turnComplete":false}}"#
        )
        .unwrap(),
        Some(GeminiServerEvent::DraftTranscript {
            text: "hello".to_string()
        })
    );
    assert_eq!(
        parse_server_event(
            r#"{"serverContent":{"modelTurn":{"parts":[{"text":"hello "},{"text":"world"}]},"turnComplete":true}}"#
        )
        .unwrap(),
        Some(GeminiServerEvent::FinalTranscript {
            text: "hello world".to_string()
        })
    );
    assert_eq!(
        parse_server_event(r#"{"serverContent":{"turnComplete":true}}"#).unwrap(),
        None
    );
    assert_eq!(
        parse_server_event(r#"{"error":{"message":"quota exceeded"}}"#).unwrap(),
        Some(GeminiServerEvent::Error {
            message: "quota exceeded".to_string()
        })
    );
}

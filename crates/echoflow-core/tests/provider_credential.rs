use std::sync::{Arc, Mutex};
use std::time::Duration;

use echoflow_core::controller::{DictationController, SessionTimersConfig};
use echoflow_core::credentials::ProviderCredentialService;
use echoflow_core::domain::{ControllerEvent, DictationError, PlatformCapabilities, TrayStatus};
use echoflow_core::gemini::{GeminiClientConfig, GeminiLiveClient};
use echoflow_core::store::InMemoryLocalStore;
use echoflow_core::testing::FakePlatformAdapter;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Metadata, Subscriber};

#[tokio::test]
async fn saving_reading_and_deleting_provider_credential_uses_platform_keyring_and_rejects_empty_input(
) {
    let platform = Arc::new(FakePlatformAdapter::new());
    let service = ProviderCredentialService::new(Arc::clone(&platform) as _);

    assert!(matches!(
        service.save("").await,
        Err(DictationError::InvalidInput(_))
    ));
    assert!(matches!(
        service.save("   \t\n  ").await,
        Err(DictationError::InvalidInput(_))
    ));
    assert!(matches!(
        service.save("AIza bad key").await,
        Err(DictationError::InvalidInput(_))
    ));

    service.save("  AIzaSyValidKey1234  ").await.unwrap();
    assert_eq!(
        service.get().await.unwrap().as_deref(),
        Some("AIzaSyValidKey1234")
    );

    let status = service.status().await;
    assert!(status.configured);
    assert_eq!(status.masked_preview.as_deref(), Some("••••1234"));
    assert!(status.secret_service_available);

    service.delete().await.unwrap();
    assert_eq!(service.get().await.unwrap(), None);

    let status_after_delete = service.status().await;
    assert!(!status_after_delete.configured);
    assert_eq!(status_after_delete.masked_preview, None);
}

#[derive(Default, Clone)]
struct CapturedLogs {
    entries: Arc<Mutex<Vec<String>>>,
}

struct LogCollector {
    entries: Arc<Mutex<Vec<String>>>,
}

impl Visit for LogCollector {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.entries
            .lock()
            .unwrap()
            .push(format!("{}={:?}", field.name(), value));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.entries
            .lock()
            .unwrap()
            .push(format!("{}={}", field.name(), value));
    }
}

impl Subscriber for CapturedLogs {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _span: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }

    fn record(&self, _span: &Id, _values: &Record<'_>) {}

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let mut collector = LogCollector {
            entries: Arc::clone(&self.entries),
        };
        event.record(&mut collector);
    }

    fn enter(&self, _span: &Id) {}

    fn exit(&self, _span: &Id) {}
}

#[tokio::test]
async fn secret_service_unavailable_emits_degraded_warning_and_never_leaks_secret() {
    let logs = CapturedLogs::default();
    let _guard = tracing::subscriber::set_default(logs.clone());

    let platform = Arc::new(FakePlatformAdapter::new());
    platform.set_capabilities(PlatformCapabilities {
        secret_service_available: false,
        ..PlatformCapabilities::default()
    });
    let sentinel = "AIzaSySuperSecretSentinelKey987654321";
    platform.set_fail_credential_store(Some(format!(
        "D-Bus Secret Service daemon not running (attempted key {sentinel})"
    )));

    let service = ProviderCredentialService::new(Arc::clone(&platform) as _);

    let save_err = service.save(sentinel).await.unwrap_err();
    assert!(!save_err.to_string().contains(sentinel));

    let status = service.status().await;
    assert!(!status.configured);
    assert!(!status.secret_service_available);
    assert!(status.degraded_reason.is_some());
    assert!(status
        .guidance
        .as_deref()
        .unwrap_or_default()
        .contains("Secret Service"));

    let joined_logs = logs.entries.lock().unwrap().join(" ");
    assert!(joined_logs.contains("status=DEGRADED"));
    assert!(joined_logs.contains("cause="));
    assert!(!joined_logs.contains(sentinel));
}

async fn start_handshake_server(accept_key: bool, echo_key_on_error: bool) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let ws_stream = tokio_tungstenite::accept_async(stream).await.unwrap();
                let (mut write, mut read) = ws_stream.split();
                let _ = read.next().await;
                let payload = if accept_key {
                    json!({ "type": "setup_complete" })
                } else if echo_key_on_error {
                    json!({
                        "type": "error",
                        "message": "401 Unauthorized: invalid API key AIzaSySuperSecretSentinelKey987654321"
                    })
                } else {
                    json!({
                        "type": "error",
                        "message": "401 Unauthorized: invalid API key"
                    })
                };
                let _ = write.send(Message::Text(payload.to_string().into())).await;
                let _ = write.send(Message::Close(None)).await;
            });
        }
    });
    format!("ws://{addr}")
}

#[tokio::test]
async fn test_connection_verifies_credential_and_redacts_secret_from_errors_and_logs() {
    let logs = CapturedLogs::default();
    let _guard = tracing::subscriber::set_default(logs.clone());

    let platform = Arc::new(FakePlatformAdapter::new());
    let service = ProviderCredentialService::new(Arc::clone(&platform) as _);

    let ok_url = start_handshake_server(true, false).await;
    let ok_client = GeminiLiveClient::new(GeminiClientConfig {
        endpoint_url: ok_url,
        flush_timeout: Duration::from_secs(2),
        ..GeminiClientConfig::default()
    });

    let missing_res = service.test_connection(&ok_client, None).await;
    assert!(!missing_res.success);
    assert_eq!(
        missing_res.error_kind.as_deref(),
        Some("missing_credential")
    );

    let sentinel = "AIzaSySuperSecretSentinelKey987654321";
    service.save(sentinel).await.unwrap();
    let ok_res = service.test_connection(&ok_client, None).await;
    assert!(ok_res.success, "expected success, got: {}", ok_res.message);

    let err_url = start_handshake_server(false, true).await;
    let err_client = GeminiLiveClient::new(GeminiClientConfig {
        endpoint_url: err_url,
        flush_timeout: Duration::from_secs(2),
        ..GeminiClientConfig::default()
    });
    let fail_res = service.test_connection(&err_client, Some(sentinel)).await;
    assert!(!fail_res.success);
    assert_eq!(fail_res.error_kind.as_deref(), Some("unauthorized"));
    assert!(!fail_res.message.contains(sentinel));

    let joined_logs = logs.entries.lock().unwrap().join(" ");
    assert!(!joined_logs.contains(sentinel));
}

#[tokio::test]
async fn controller_refresh_tray_status_reflects_missing_credential_until_saved() {
    let platform = Arc::new(FakePlatformAdapter::new());
    let store = Arc::new(InMemoryLocalStore::new());
    let gemini = GeminiLiveClient::new(GeminiClientConfig::default());
    let controller = DictationController::new(
        Arc::clone(&platform) as _,
        gemini,
        store,
        SessionTimersConfig::default(),
    );

    let mut rx = controller.subscribe();
    let initial = controller.refresh_tray_status().await;
    assert_eq!(initial, TrayStatus::MissingCredential);
    assert_eq!(
        rx.recv().await.unwrap(),
        ControllerEvent::TrayStatusChanged {
            status: TrayStatus::MissingCredential
        }
    );

    let service = ProviderCredentialService::new(Arc::clone(&platform) as _);
    service.save("AIzaSyValidKey1234").await.unwrap();

    let updated = controller.refresh_tray_status().await;
    assert_eq!(updated, TrayStatus::Idle);
    assert_eq!(
        rx.recv().await.unwrap(),
        ControllerEvent::TrayStatusChanged {
            status: TrayStatus::Idle
        }
    );
}

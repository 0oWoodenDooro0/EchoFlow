use std::sync::Arc;
use std::time::Duration;

use echoflow_core::controller::{DictationController, SessionTimersConfig};
use echoflow_core::delivery::DeliveryService;
use echoflow_core::domain::{
    AppPreferences, ControllerEvent, DeliveryMode, DictationError, DisplayServer,
    PlatformCapabilities, SessionState,
};
use echoflow_core::gemini::{GeminiClientConfig, GeminiLiveClient};
use echoflow_core::hotkey::validate_and_normalize_hotkey;
use echoflow_core::store::{LocalStore, SqliteLocalStore};
use echoflow_core::testing::{ClipboardOp, FakePlatformAdapter};
use tokio::net::TcpListener;

#[test]
fn validates_and_normalizes_push_to_talk_hotkey_bindings() {
    assert_eq!(
        validate_and_normalize_hotkey("alt+space").unwrap(),
        "Alt+Space"
    );
    assert_eq!(
        validate_and_normalize_hotkey("  shift + ctrl + k  ").unwrap(),
        "Ctrl+Shift+K"
    );
    assert_eq!(validate_and_normalize_hotkey("f9").unwrap(), "F9");
    assert_eq!(
        validate_and_normalize_hotkey("cmd+shift+space").unwrap(),
        "Shift+Meta+Space"
    );
}

#[test]
fn rejects_empty_modifier_only_and_reserved_hotkey_bindings() {
    for invalid in [
        "",
        "   ",
        "Ctrl",
        "Alt+Shift",
        "Escape",
        "Esc",
        "Ctrl+C",
        "Ctrl+V",
        "Cmd+C",
        "Cmd+V",
        "Cmd+Q",
        "Alt+F4",
        "Alt+Tab",
        "Cmd+Tab",
        "Ctrl+UnknownKey123",
    ] {
        let err = validate_and_normalize_hotkey(invalid).unwrap_err();
        assert!(
            matches!(err, DictationError::InvalidInput(_)),
            "expected InvalidInput for `{invalid}`, got {err:?}"
        );
    }
}

#[tokio::test]
async fn clipboard_preservation_snapshots_pastes_and_restores_after_delay() {
    let platform = Arc::new(FakePlatformAdapter::new());
    platform.set_clipboard_sync(Some("original clipboard".to_string()));

    let delivery = DeliveryService::new(Arc::clone(&platform) as _);
    let prefs = AppPreferences {
        restore_clipboard: true,
        clipboard_restore_delay_ms: 10,
        ..AppPreferences::default()
    };

    let outcome = delivery.deliver("dictated text", &prefs).await.unwrap();
    assert_eq!(outcome.mode, DeliveryMode::TextInsertion);
    assert_eq!(outcome.fallback_reason, None);
    assert_eq!(
        platform.current_clipboard(),
        Some("original clipboard".to_string())
    );
    assert_eq!(
        platform.recorded_clipboard_ops(),
        vec![
            ClipboardOp::Read,
            ClipboardOp::Write("dictated text".to_string()),
            ClipboardOp::Paste,
            ClipboardOp::Write("original clipboard".to_string()),
        ]
    );
}

#[tokio::test]
async fn clipboard_preservation_opt_out_keeps_final_transcript_on_clipboard() {
    let platform = Arc::new(FakePlatformAdapter::new());
    platform.set_clipboard_sync(Some("original clipboard".to_string()));

    let delivery = DeliveryService::new(Arc::clone(&platform) as _);
    let prefs = AppPreferences {
        restore_clipboard: false,
        clipboard_restore_delay_ms: 10,
        ..AppPreferences::default()
    };

    let outcome = delivery
        .deliver("keep me on clipboard", &prefs)
        .await
        .unwrap();
    assert_eq!(outcome.mode, DeliveryMode::TextInsertion);
    assert_eq!(outcome.fallback_reason, None);
    assert_eq!(
        platform.current_clipboard(),
        Some("keep me on clipboard".to_string())
    );
    assert_eq!(
        platform.recorded_clipboard_ops(),
        vec![
            ClipboardOp::Read,
            ClipboardOp::Write("keep me on clipboard".to_string()),
            ClipboardOp::Paste,
        ]
    );
}

#[tokio::test]
async fn wayland_without_text_insertion_capability_degrades_to_fallback_delivery() {
    let platform = Arc::new(FakePlatformAdapter::new());
    platform.set_clipboard_sync(Some("original clipboard".to_string()));
    platform.set_capabilities(PlatformCapabilities {
        display_server: DisplayServer::LinuxWayland,
        supports_global_hotkey: false,
        supports_text_insertion: false,
        wayland_helpers: Vec::new(),
        ..PlatformCapabilities::default()
    });

    let delivery = DeliveryService::new(Arc::clone(&platform) as _);
    let prefs = AppPreferences {
        restore_clipboard: true,
        clipboard_restore_delay_ms: 10,
        ..AppPreferences::default()
    };

    let outcome = delivery
        .deliver("wayland transcript", &prefs)
        .await
        .unwrap();
    assert_eq!(outcome.mode, DeliveryMode::FallbackDelivery);
    let reason = outcome
        .fallback_reason
        .expect("fallback reason should be present");
    assert!(
        reason.to_lowercase().contains("wayland"),
        "expected Wayland in fallback reason, got `{reason}`"
    );
    assert_eq!(
        platform.current_clipboard(),
        Some("wayland transcript".to_string())
    );
    assert!(
        !platform
            .recorded_clipboard_ops()
            .contains(&ClipboardOp::Paste),
        "trigger_paste should not be called when Wayland lacks Text Insertion capability"
    );
}

#[tokio::test]
async fn escape_cancel_during_connecting_aborts_cleanly_without_delivery_or_history() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    // Accept TCP connection and hold it open without completing WS handshake immediately,
    // simulating a slow connect while Escape is pressed.
    let _server = tokio::spawn(async move {
        let (_stream, _) = listener.accept().await.unwrap();
        tokio::time::sleep(Duration::from_millis(80)).await;
    });

    let platform = Arc::new(FakePlatformAdapter::new());
    platform.set_credential_sync(Some("test-key".to_string()));
    platform.set_clipboard_sync(Some("keep-me".to_string()));

    let store = Arc::new(SqliteLocalStore::open_in_memory().unwrap());
    let gemini = GeminiLiveClient::new(GeminiClientConfig {
        endpoint_url: format!("ws://{addr}"),
        ..GeminiClientConfig::default()
    });
    let controller = Arc::new(DictationController::new(
        Arc::clone(&platform) as _,
        gemini,
        Arc::clone(&store) as _,
        SessionTimersConfig::default(),
    ));

    let mut events = controller.subscribe();
    let press_controller = Arc::clone(&controller);
    let press_task = tokio::spawn(async move { press_controller.handle_hotkey_pressed().await });

    // Wait until controller enters Connecting
    while let Ok(ev) = events.recv().await {
        if matches!(
            ev,
            ControllerEvent::StateChanged {
                state: SessionState::Connecting
            }
        ) {
            break;
        }
    }

    controller.handle_escape_cancelled().await.unwrap();
    let _ = press_task.await.unwrap();

    // Wait past the server delay to ensure late connect completion does not revive the session.
    tokio::time::sleep(Duration::from_millis(120)).await;

    assert_eq!(controller.state().await, SessionState::Idle);
    assert!(store.list_history_entries().await.unwrap().is_empty());
    assert!(platform.recorded_clipboard_ops().is_empty());
    assert_eq!(platform.current_clipboard(), Some("keep-me".to_string()));
}

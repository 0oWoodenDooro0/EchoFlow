mod clipboard_adapter;
mod hotkey_adapter;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use clipboard_adapter::DesktopClipboardAdapter;
use echoflow_core::controller::{DictationController, SessionTimersConfig};
use echoflow_core::domain::{
    AppPreferences, AudioDeviceInfo, ControllerEvent, DictationError, PlatformCapabilities,
    SessionState, TrayStatus,
};
use echoflow_core::gemini::{GeminiClientConfig, GeminiLiveClient};
use echoflow_core::hotkey::validate_and_normalize_hotkey;
use echoflow_core::platform::{ClipboardSnapshot, HotkeyEvent, PcmFrame, PlatformAdapter};
use echoflow_core::store::{InMemoryLocalStore, LocalStore};
use hotkey_adapter::{
    probe_platform_capabilities, start_global_hotkey_listener, HotkeyListenerHandle,
};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};
use tokio::sync::mpsc;

pub const CONTROLLER_EVENT_NAME: &str = "echoflow://controller-event";

/// Baseline desktop `PlatformAdapter` shell wired into Tauri v2.
///
/// Downstream issues (#4 audio capture, #6 native keyring, #7 global hotkey & clipboard paste)
/// replace these stubs with real OS backends.
#[derive(Default)]
struct DesktopPlatformAdapter {
    credential: Mutex<Option<String>>,
    clipboard: DesktopClipboardAdapter,
    hotkey_handle: Mutex<Option<HotkeyListenerHandle>>,
}

#[async_trait]
impl PlatformAdapter for DesktopPlatformAdapter {
    async fn start_hotkey_listener(
        &self,
        binding: &str,
    ) -> Result<mpsc::Receiver<HotkeyEvent>, DictationError> {
        let (rx, handle) = start_global_hotkey_listener(binding)?;
        if let Some(prev) = self.hotkey_handle.lock().unwrap().replace(handle) {
            prev.stop();
        }
        Ok(rx)
    }

    async fn list_audio_devices(&self) -> Result<Vec<AudioDeviceInfo>, DictationError> {
        Ok(vec![AudioDeviceInfo {
            id: "default".to_string(),
            name: "System Default Microphone".to_string(),
            is_default: true,
        }])
    }

    async fn start_audio_capture(
        &self,
        _device_id: Option<&str>,
    ) -> Result<mpsc::Receiver<PcmFrame>, DictationError> {
        let (_tx, rx) = mpsc::channel(32);
        Ok(rx)
    }

    async fn read_clipboard(&self) -> Result<ClipboardSnapshot, DictationError> {
        let caps = probe_platform_capabilities();
        self.clipboard.read_clipboard(&caps)
    }

    async fn write_clipboard(&self, text: &str) -> Result<(), DictationError> {
        let caps = probe_platform_capabilities();
        self.clipboard.write_clipboard(&caps, text)
    }

    async fn trigger_paste(&self) -> Result<(), DictationError> {
        let caps = probe_platform_capabilities();
        self.clipboard.trigger_paste(&caps)
    }

    async fn get_provider_credential(&self) -> Result<Option<String>, DictationError> {
        Ok(self.credential.lock().unwrap().clone())
    }

    async fn set_provider_credential(&self, api_key: &str) -> Result<(), DictationError> {
        *self.credential.lock().unwrap() = Some(api_key.to_string());
        Ok(())
    }

    async fn delete_provider_credential(&self) -> Result<(), DictationError> {
        *self.credential.lock().unwrap() = None;
        Ok(())
    }

    async fn probe_capabilities(&self) -> Result<PlatformCapabilities, DictationError> {
        Ok(probe_platform_capabilities())
    }
}

struct AppState {
    controller: Arc<DictationController>,
    store: Arc<InMemoryLocalStore>,
    platform: Arc<DesktopPlatformAdapter>,
}

async fn bind_and_forward_hotkey(
    platform: &Arc<DesktopPlatformAdapter>,
    controller: &Arc<DictationController>,
    binding: &str,
) -> Result<(), DictationError> {
    let mut rx = platform.start_hotkey_listener(binding).await?;
    let controller_clone = Arc::clone(controller);
    tauri::async_runtime::spawn(async move {
        while let Some(event) = rx.recv().await {
            if let Err(err) = controller_clone.handle_hotkey_event(event).await {
                tracing::debug!(error = %err, "hotkey event ignored or failed");
            }
        }
    });
    Ok(())
}

#[tauri::command]
async fn get_session_state(state: State<'_, AppState>) -> Result<SessionState, String> {
    Ok(state.controller.state().await)
}

#[tauri::command]
async fn simulate_hotkey_press(state: State<'_, AppState>) -> Result<(), String> {
    state
        .controller
        .handle_hotkey_pressed()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn simulate_hotkey_release(state: State<'_, AppState>) -> Result<Option<String>, String> {
    state
        .controller
        .handle_hotkey_released()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn cancel_dictation_session(state: State<'_, AppState>) -> Result<(), String> {
    state
        .controller
        .handle_escape_cancelled()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_preferences(state: State<'_, AppState>) -> Result<AppPreferences, String> {
    state
        .store
        .get_preferences()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn save_general_settings(
    state: State<'_, AppState>,
    push_to_talk_hotkey: String,
    restore_clipboard: bool,
    clipboard_restore_delay_ms: u64,
) -> Result<AppPreferences, String> {
    let normalized =
        validate_and_normalize_hotkey(&push_to_talk_hotkey).map_err(|e| e.to_string())?;
    let mut prefs = state
        .store
        .get_preferences()
        .await
        .map_err(|e| e.to_string())?;
    let hotkey_changed = prefs.push_to_talk_hotkey != normalized;
    prefs.push_to_talk_hotkey = normalized.clone();
    prefs.restore_clipboard = restore_clipboard;
    prefs.clipboard_restore_delay_ms = clipboard_restore_delay_ms;

    state
        .store
        .save_preferences(&prefs)
        .await
        .map_err(|e| e.to_string())?;

    if hotkey_changed {
        bind_and_forward_hotkey(&state.platform, &state.controller, &normalized)
            .await
            .map_err(|e| e.to_string())?;
    }

    Ok(prefs)
}

#[tauri::command]
async fn get_platform_capabilities(
    state: State<'_, AppState>,
) -> Result<PlatformCapabilities, String> {
    state
        .platform
        .probe_capabilities()
        .await
        .map_err(|e| e.to_string())
}

fn open_settings_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("settings") {
        let _ = window.show();
        let _ = window.set_focus();
        return;
    }
    let _ = WebviewWindowBuilder::new(app, "settings", WebviewUrl::App("/?view=settings".into()))
        .title("EchoFlow Settings & History")
        .inner_size(760.0, 560.0)
        .resizable(true)
        .build();
}

fn tray_status_label(status: TrayStatus) -> &'static str {
    match status {
        TrayStatus::Idle => "Status: Idle",
        TrayStatus::Listening => "Status: Listening",
        TrayStatus::MissingCredential => "Status: Missing Provider Credential",
        TrayStatus::Error => "Status: Error",
    }
}

pub fn run() {
    let platform = Arc::new(DesktopPlatformAdapter::default());
    let store = Arc::new(InMemoryLocalStore::new());
    let gemini = GeminiLiveClient::new(GeminiClientConfig::default());
    let controller = Arc::new(DictationController::new(
        Arc::clone(&platform) as _,
        gemini,
        Arc::clone(&store) as _,
        SessionTimersConfig::default(),
    ));

    let controller_for_setup = Arc::clone(&controller);
    let platform_for_setup = Arc::clone(&platform);
    let store_for_setup = Arc::clone(&store);

    tauri::Builder::default()
        .manage(AppState {
            controller,
            store,
            platform,
        })
        .invoke_handler(tauri::generate_handler![
            get_session_state,
            simulate_hotkey_press,
            simulate_hotkey_release,
            cancel_dictation_session,
            get_preferences,
            save_general_settings,
            get_platform_capabilities,
        ])
        .setup(move |app| {
            let status_item = MenuItem::with_id(
                app,
                "status",
                tray_status_label(TrayStatus::Idle),
                false,
                None::<&str>,
            )?;
            let settings_item = MenuItem::with_id(
                app,
                "open-settings",
                "Open Settings & History",
                true,
                None::<&str>,
            )?;
            let quit_item = MenuItem::with_id(app, "quit", "Quit EchoFlow", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&status_item, &settings_item, &quit_item])?;

            let _tray = TrayIconBuilder::with_id("echoflow-tray")
                .menu(&menu)
                .tooltip("EchoFlow — Idle")
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open-settings" => open_settings_window(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;

            let app_handle = app.handle().clone();
            let mut rx = controller_for_setup.subscribe();
            let status_item_clone = status_item.clone();
            let controller_for_hotkey = Arc::clone(&controller_for_setup);

            tauri::async_runtime::spawn(async move {
                let prefs = store_for_setup.get_preferences().await.unwrap_or_default();
                if let Err(err) = bind_and_forward_hotkey(
                    &platform_for_setup,
                    &controller_for_hotkey,
                    &prefs.push_to_talk_hotkey,
                )
                .await
                {
                    tracing::warn!(
                        status = "DEGRADED",
                        component = "hotkey_adapter",
                        cause = %err,
                        "failed to start initial Push-to-Talk Hotkey listener"
                    );
                }
            });

            tauri::async_runtime::spawn(async move {
                while let Ok(event) = rx.recv().await {
                    match &event {
                        ControllerEvent::StateChanged { state } => {
                            if let Some(pill) = app_handle.get_webview_window("overlay-pill") {
                                if matches!(state, SessionState::Idle) {
                                    let _ = pill.hide();
                                } else {
                                    let _ = pill.show();
                                }
                            }
                        }
                        ControllerEvent::TrayStatusChanged { status } => {
                            let _ = status_item_clone.set_text(tray_status_label(*status));
                        }
                        _ => {}
                    }
                    let _ = app_handle.emit(CONTROLLER_EVENT_NAME, &event);
                }
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running EchoFlow desktop application");
}

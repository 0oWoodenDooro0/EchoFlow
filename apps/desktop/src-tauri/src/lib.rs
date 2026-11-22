use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use echoflow_core::controller::{DictationController, SessionTimersConfig};
use echoflow_core::domain::{
    AudioDeviceInfo, ControllerEvent, DictationError, PlatformCapabilities, SessionState,
    TrayStatus,
};
use echoflow_core::gemini::{GeminiClientConfig, GeminiLiveClient};
use echoflow_core::platform::{ClipboardSnapshot, HotkeyEvent, PcmFrame, PlatformAdapter};
use echoflow_core::store::InMemoryLocalStore;
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
    clipboard: Mutex<Option<String>>,
}

#[async_trait]
impl PlatformAdapter for DesktopPlatformAdapter {
    async fn start_hotkey_listener(
        &self,
        _binding: &str,
    ) -> Result<mpsc::Receiver<HotkeyEvent>, DictationError> {
        let (_tx, rx) = mpsc::channel(16);
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
        Ok(ClipboardSnapshot {
            text: self.clipboard.lock().unwrap().clone(),
        })
    }

    async fn write_clipboard(&self, text: &str) -> Result<(), DictationError> {
        *self.clipboard.lock().unwrap() = Some(text.to_string());
        Ok(())
    }

    async fn trigger_paste(&self) -> Result<(), DictationError> {
        Ok(())
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
        Ok(PlatformCapabilities::default())
    }
}

struct AppState {
    controller: Arc<DictationController>,
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
        platform,
        gemini,
        store,
        SessionTimersConfig::default(),
    ));

    let controller_for_setup = Arc::clone(&controller);

    tauri::Builder::default()
        .manage(AppState { controller })
        .invoke_handler(tauri::generate_handler![
            get_session_state,
            simulate_hotkey_press,
            simulate_hotkey_release,
            cancel_dictation_session,
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

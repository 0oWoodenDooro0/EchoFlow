mod audio_adapter;
mod clipboard_adapter;
mod credential_adapter;
mod hotkey_adapter;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use clipboard_adapter::DesktopClipboardAdapter;
use echoflow_core::controller::{DictationController, SessionTimersConfig};
use echoflow_core::credentials::{
    ConnectionTestResult, ProviderCredentialService, ProviderCredentialStatus,
};
use echoflow_core::domain::{
    AppPreferences, AudioDeviceInfo, ControllerEvent, DictationError, HistoryEntry,
    PlatformCapabilities, SessionState, TrayStatus, VocabularyEntry,
};
use echoflow_core::gemini::{GeminiClientConfig, GeminiLiveClient};
use echoflow_core::hotkey::validate_and_normalize_hotkey;
use echoflow_core::platform::{ClipboardSnapshot, HotkeyEvent, PcmFrame, PlatformAdapter};
use echoflow_core::store::{LocalStore, SqliteLocalStore};
use hotkey_adapter::{start_global_hotkey_listener, HotkeyListenerHandle};
use tauri::menu::{CheckMenuItem, Menu, MenuItem, Submenu};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, RunEvent, State, WebviewUrl, WebviewWindowBuilder};
use tokio::sync::mpsc;

pub const CONTROLLER_EVENT_NAME: &str = "echoflow://controller-event";

/// Baseline desktop `PlatformAdapter` shell wired into Tauri v2.
///
/// Uses OS native keyring via `credential_adapter` for Provider Credential storage
/// and platform capability probing.
#[derive(Default)]
struct DesktopPlatformAdapter {
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
        audio_adapter::list_input_devices()
    }

    async fn start_audio_capture(
        &self,
        device_id: Option<&str>,
    ) -> Result<mpsc::Receiver<PcmFrame>, DictationError> {
        audio_adapter::start_capture(device_id)
    }

    async fn read_clipboard(&self) -> Result<ClipboardSnapshot, DictationError> {
        let caps = credential_adapter::probe_platform_capabilities().await?;
        self.clipboard.read_clipboard(&caps)
    }

    async fn write_clipboard(&self, text: &str) -> Result<(), DictationError> {
        let caps = credential_adapter::probe_platform_capabilities().await?;
        self.clipboard.write_clipboard(&caps, text)
    }

    async fn trigger_paste(&self) -> Result<(), DictationError> {
        let caps = credential_adapter::probe_platform_capabilities().await?;
        self.clipboard.trigger_paste(&caps)
    }

    async fn get_provider_credential(&self) -> Result<Option<String>, DictationError> {
        credential_adapter::get_provider_credential().await
    }

    async fn set_provider_credential(&self, api_key: &str) -> Result<(), DictationError> {
        credential_adapter::set_provider_credential(api_key).await
    }

    async fn delete_provider_credential(&self) -> Result<(), DictationError> {
        credential_adapter::delete_provider_credential().await
    }

    async fn probe_capabilities(&self) -> Result<PlatformCapabilities, DictationError> {
        credential_adapter::probe_platform_capabilities().await
    }
}

struct AppState {
    controller: Arc<DictationController>,
    store: Arc<dyn LocalStore>,
    platform: Arc<DesktopPlatformAdapter>,
    credential_service: Arc<ProviderCredentialService>,
    gemini: GeminiLiveClient,
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
async fn list_history_entries(state: State<'_, AppState>) -> Result<Vec<HistoryEntry>, String> {
    state
        .store
        .list_history_entries()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn search_history_entries(
    state: State<'_, AppState>,
    query: String,
) -> Result<Vec<HistoryEntry>, String> {
    state
        .store
        .search_history_entries(&query)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn delete_history_entry(state: State<'_, AppState>, id: String) -> Result<bool, String> {
    state
        .store
        .delete_history_entry(&id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn clear_history(state: State<'_, AppState>) -> Result<usize, String> {
    state.store.clear_history().await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn list_vocabulary_entries(
    state: State<'_, AppState>,
) -> Result<Vec<VocabularyEntry>, String> {
    state
        .store
        .list_vocabulary_entries()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn search_vocabulary_entries(
    state: State<'_, AppState>,
    query: String,
) -> Result<Vec<VocabularyEntry>, String> {
    state
        .store
        .search_vocabulary_entries(&query)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn add_vocabulary_term(
    state: State<'_, AppState>,
    term: String,
) -> Result<VocabularyEntry, String> {
    state
        .store
        .add_vocabulary_term(&term)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn update_vocabulary_term(
    state: State<'_, AppState>,
    id: String,
    new_term: String,
) -> Result<VocabularyEntry, String> {
    state
        .store
        .update_vocabulary_term(&id, &new_term)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn delete_vocabulary_term(state: State<'_, AppState>, id: String) -> Result<bool, String> {
    state
        .store
        .delete_vocabulary_term(&id)
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
async fn save_preferences(state: State<'_, AppState>, prefs: AppPreferences) -> Result<(), String> {
    state
        .store
        .save_preferences(&prefs)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn save_general_settings(
    state: State<'_, AppState>,
    push_to_talk_hotkey: String,
    restore_clipboard: bool,
    clipboard_restore_delay_ms: u64,
    history_retention_days: u32,
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
    prefs.history_retention_days = history_retention_days;

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
async fn get_provider_credential_status(
    state: State<'_, AppState>,
) -> Result<ProviderCredentialStatus, String> {
    Ok(state.credential_service.status().await)
}

#[tauri::command]
async fn save_provider_credential(
    state: State<'_, AppState>,
    api_key: String,
) -> Result<ProviderCredentialStatus, String> {
    state
        .credential_service
        .save(&api_key)
        .await
        .map_err(|e| e.to_string())?;
    state.controller.refresh_tray_status().await;
    Ok(state.credential_service.status().await)
}

#[tauri::command]
async fn delete_provider_credential(
    state: State<'_, AppState>,
) -> Result<ProviderCredentialStatus, String> {
    state
        .credential_service
        .delete()
        .await
        .map_err(|e| e.to_string())?;
    state.controller.refresh_tray_status().await;
    Ok(state.credential_service.status().await)
}

#[tauri::command]
async fn test_provider_connection(
    state: State<'_, AppState>,
    candidate_key: Option<String>,
) -> Result<ConnectionTestResult, String> {
    Ok(state
        .credential_service
        .test_connection(&state.gemini, candidate_key.as_deref())
        .await)
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

#[tauri::command]
async fn export_settings_json(state: State<'_, AppState>) -> Result<String, String> {
    state
        .store
        .export_settings_json()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn import_settings_json(state: State<'_, AppState>, json: String) -> Result<(), String> {
    state
        .store
        .import_settings_json(&json)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn list_audio_devices(state: State<'_, AppState>) -> Result<Vec<AudioDeviceInfo>, String> {
    state
        .controller
        .list_audio_devices()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_selected_microphone(state: State<'_, AppState>) -> Result<Option<String>, String> {
    state
        .controller
        .get_selected_microphone()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn select_microphone(
    state: State<'_, AppState>,
    device_id: Option<String>,
) -> Result<(), String> {
    state
        .controller
        .set_selected_microphone(device_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn open_permission_settings(target: String) -> Result<(), String> {
    credential_adapter::open_permission_settings(&target)
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

fn resolve_sqlite_db_path() -> PathBuf {
    #[cfg(target_os = "macos")]
    let base_dir = std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|h| h.join("Library").join("Application Support"));

    #[cfg(target_os = "windows")]
    let base_dir = std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("LOCALAPPDATA"))
        .map(PathBuf::from);

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let base_dir = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .map(|h| h.join(".local").join("share"))
        });

    base_dir
        .unwrap_or_else(std::env::temp_dir)
        .join("echoflow")
        .join("echoflow.sqlite")
}

fn open_desktop_local_store() -> SqliteLocalStore {
    let db_path = resolve_sqlite_db_path();
    let parent_result = match db_path.parent() {
        Some(parent) => std::fs::create_dir_all(parent),
        None => Ok(()),
    };
    match parent_result {
        Ok(()) => match SqliteLocalStore::open(&db_path) {
            Ok(store) => return store,
            Err(err) => {
                tracing::warn!(
                    status = "DEGRADED",
                    component = "local_store",
                    path = %db_path.display(),
                    cause = %err,
                    "failed to open persistent SQLite store; falling back to in-memory store"
                );
            }
        },
        Err(err) => {
            tracing::warn!(
                status = "DEGRADED",
                component = "local_store",
                path = %db_path.display(),
                cause = %err,
                "failed to create persistent data directory; falling back to in-memory store"
            );
        }
    }

    SqliteLocalStore::open_in_memory().expect("failed to initialize fallback in-memory local store")
}

pub fn run() {
    let platform = Arc::new(DesktopPlatformAdapter::default());
    let store: Arc<dyn LocalStore> = Arc::new(open_desktop_local_store());
    let gemini = GeminiLiveClient::new(GeminiClientConfig::default());
    let credential_service = Arc::new(ProviderCredentialService::new(Arc::clone(&platform) as _));
    let controller = Arc::new(DictationController::new(
        Arc::clone(&platform) as _,
        gemini.clone(),
        Arc::clone(&store),
        SessionTimersConfig::default(),
    ));

    let controller_for_setup = Arc::clone(&controller);
    let platform_for_setup = Arc::clone(&platform);
    let store_for_setup = Arc::clone(&store);

    let app = tauri::Builder::default()
        .manage(AppState {
            controller,
            store,
            platform,
            credential_service,
            gemini,
        })
        .invoke_handler(tauri::generate_handler![
            get_session_state,
            simulate_hotkey_press,
            simulate_hotkey_release,
            cancel_dictation_session,
            list_history_entries,
            search_history_entries,
            delete_history_entry,
            clear_history,
            list_vocabulary_entries,
            search_vocabulary_entries,
            add_vocabulary_term,
            update_vocabulary_term,
            delete_vocabulary_term,
            get_preferences,
            save_preferences,
            save_general_settings,
            get_provider_credential_status,
            save_provider_credential,
            delete_provider_credential,
            test_provider_connection,
            get_platform_capabilities,
            open_permission_settings,
            export_settings_json,
            import_settings_json,
            list_audio_devices,
            get_selected_microphone,
            select_microphone,
        ])
        .setup(move |app| {
            let status_item = MenuItem::with_id(
                app,
                "status",
                tray_status_label(TrayStatus::Idle),
                false,
                None::<&str>,
            )?;

            let selected_mic =
                tauri::async_runtime::block_on(controller_for_setup.get_selected_microphone())
                    .ok()
                    .flatten();
            let devices = tauri::async_runtime::block_on(controller_for_setup.list_audio_devices())
                .unwrap_or_default();

            let default_mic_item = CheckMenuItem::with_id(
                app,
                "mic:default",
                "System Default Microphone",
                true,
                selected_mic.is_none(),
                None::<&str>,
            )?;
            let mut mic_items: Vec<(String, CheckMenuItem<tauri::Wry>)> =
                vec![("mic:default".to_string(), default_mic_item.clone())];
            for device in &devices {
                let menu_id = format!("mic:{}", device.id);
                let label = if device.is_default {
                    format!("{} (Default)", device.name)
                } else {
                    device.name.clone()
                };
                let is_checked = selected_mic.as_deref() == Some(device.id.as_str());
                let item =
                    CheckMenuItem::with_id(app, &menu_id, label, true, is_checked, None::<&str>)?;
                mic_items.push((menu_id, item));
            }
            let mic_item_refs: Vec<&dyn tauri::menu::IsMenuItem<tauri::Wry>> = mic_items
                .iter()
                .map(|(_, item)| item as &dyn tauri::menu::IsMenuItem<tauri::Wry>)
                .collect();
            let mic_submenu = Submenu::with_items(app, "Microphone", true, &mic_item_refs)?;

            let settings_item = MenuItem::with_id(
                app,
                "open-settings",
                "Open Settings & History",
                true,
                None::<&str>,
            )?;
            let quit_item = MenuItem::with_id(app, "quit", "Quit EchoFlow", true, None::<&str>)?;
            let menu = Menu::with_items(
                app,
                &[&status_item, &mic_submenu, &settings_item, &quit_item],
            )?;

            let mic_items_for_events = mic_items.clone();
            let _tray = TrayIconBuilder::with_id("echoflow-tray")
                .menu(&menu)
                .tooltip("EchoFlow — Idle")
                .on_menu_event(move |app, event| {
                    let event_id = event.id.as_ref();
                    if event_id == "open-settings" {
                        open_settings_window(app);
                    } else if event_id == "quit" {
                        app.exit(0);
                    } else if let Some(mic_key) = event_id.strip_prefix("mic:") {
                        let next_mic = if mic_key == "default" {
                            None
                        } else {
                            Some(mic_key.to_string())
                        };
                        for (item_id, item) in &mic_items_for_events {
                            let _ = item.set_checked(item_id == event_id);
                        }
                        let state = app.state::<AppState>();
                        let controller = Arc::clone(&state.controller);
                        tauri::async_runtime::spawn(async move {
                            let _ = controller.set_selected_microphone(next_mic).await;
                        });
                    }
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

            let controller_for_tray = Arc::clone(&controller_for_setup);
            tauri::async_runtime::spawn(async move {
                controller_for_tray.refresh_tray_status().await;
            });

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building EchoFlow desktop application");

    // Keep Rust Core and native system tray running when the Settings window is closed
    // and its WebView is destroyed to minimize idle RAM (ADR-0001).
    app.run(|_app_handle, event| {
        if let RunEvent::ExitRequested { api, code, .. } = event {
            if code.is_none() {
                api.prevent_exit();
            }
        }
    });
}

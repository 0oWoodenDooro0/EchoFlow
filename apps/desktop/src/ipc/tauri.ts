import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export const CONTROLLER_EVENT_NAME = "echoflow://controller-event";

export type DeliveryMode = "text_insertion" | "fallback_delivery" | "aborted";

export type TrayStatus = "idle" | "listening" | "missing_credential" | "error";

export type DisplayServer =
  | "mac_os"
  | "windows"
  | "linux_x11"
  | "linux_wayland"
  | "unknown";

export type PermissionStatus =
  | "granted"
  | "denied"
  | "not_determined"
  | "not_applicable";

export interface PlatformCapabilities {
  display_server: DisplayServer;
  microphone_permission: PermissionStatus;
  accessibility_permission: PermissionStatus;
  supports_global_hotkey: boolean;
  supports_text_insertion: boolean;
  wayland_helpers: string[];
  secret_service_available: boolean;
}

export interface AppPreferences {
  push_to_talk_hotkey: string;
  microphone_id: string | null;
  restore_clipboard: boolean;
  clipboard_restore_delay_ms: number;
  history_retention_days: number;
}

export interface AudioDeviceInfo {
  id: string;
  name: string;
  is_default: boolean;
}

export type SessionState =
  | { kind: "idle" }
  | { kind: "connecting" }
  | { kind: "listening" }
  | { kind: "finalizing" }
  | { kind: "delivering" }
  | { kind: "error"; message: string }
  | { kind: "fallback_notice"; final_transcript: string; reason: string };

export type ControllerEvent =
  | { type: "state_changed"; state: SessionState }
  | { type: "draft_transcript_updated"; draft_transcript: string }
  | {
      type: "final_transcript_ready";
      final_transcript: string;
      delivery_mode: DeliveryMode;
    }
  | { type: "audio_level_updated"; rms: number }
  | { type: "session_warning"; seconds_remaining: number }
  | { type: "error_occurred"; message: string }
  | {
      type: "fallback_delivery_noticed";
      final_transcript: string;
      reason: string;
    }
  | { type: "tray_status_changed"; status: TrayStatus };

export interface ProviderCredentialStatus {
  configured: boolean;
  masked_preview: string | null;
  secret_service_available: boolean;
  degraded_reason: string | null;
  guidance: string | null;
}

export interface ConnectionTestResult {
  success: boolean;
  message: string;
  error_kind: string | null;
}

export async function getSessionState(): Promise<SessionState> {
  return invoke<SessionState>("get_session_state");
}

export async function simulateHotkeyPress(): Promise<void> {
  return invoke<void>("simulate_hotkey_press");
}

export async function simulateHotkeyRelease(): Promise<string | null> {
  return invoke<string | null>("simulate_hotkey_release");
}

export async function cancelDictationSession(): Promise<void> {
  return invoke<void>("cancel_dictation_session");
}

export async function getPreferences(): Promise<AppPreferences> {
  return invoke<AppPreferences>("get_preferences");
}

export async function saveGeneralSettings(params: {
  push_to_talk_hotkey: string;
  restore_clipboard: boolean;
  clipboard_restore_delay_ms: number;
  history_retention_days: number;
}): Promise<AppPreferences> {
  return invoke<AppPreferences>("save_general_settings", {
    pushToTalkHotkey: params.push_to_talk_hotkey,
    restoreClipboard: params.restore_clipboard,
    clipboardRestoreDelayMs: params.clipboard_restore_delay_ms,
    historyRetentionDays: params.history_retention_days,
  });
}

export async function getProviderCredentialStatus(): Promise<ProviderCredentialStatus> {
  return invoke<ProviderCredentialStatus>("get_provider_credential_status");
}

export async function saveProviderCredential(
  apiKey: string,
): Promise<ProviderCredentialStatus> {
  return invoke<ProviderCredentialStatus>("save_provider_credential", {
    apiKey,
  });
}

export async function deleteProviderCredential(): Promise<ProviderCredentialStatus> {
  return invoke<ProviderCredentialStatus>("delete_provider_credential");
}

export async function testProviderConnection(
  candidateKey?: string,
): Promise<ConnectionTestResult> {
  return invoke<ConnectionTestResult>("test_provider_connection", {
    candidateKey: candidateKey ?? null,
  });
}

export async function getPlatformCapabilities(): Promise<PlatformCapabilities> {
  return invoke<PlatformCapabilities>("get_platform_capabilities");
}

export async function listAudioDevices(): Promise<AudioDeviceInfo[]> {
  return invoke<AudioDeviceInfo[]>("list_audio_devices");
}

export async function getSelectedMicrophone(): Promise<string | null> {
  return invoke<string | null>("get_selected_microphone");
}

export async function selectMicrophone(deviceId: string | null): Promise<void> {
  return invoke<void>("select_microphone", { deviceId });
}

export async function openPermissionSettings(
  target: "microphone" | "accessibility",
): Promise<void> {
  return invoke<void>("open_permission_settings", { target });
}

export async function subscribeControllerEvents(
  handler: (event: ControllerEvent) => void,
): Promise<UnlistenFn> {
  return listen<ControllerEvent>(CONTROLLER_EVENT_NAME, (event) => {
    handler(event.payload);
  });
}

export const MAX_VOCABULARY_TERMS = 1000;

export interface HistoryEntry {
  id: string;
  final_transcript: string;
  created_at_utc: string;
  duration_ms: number;
  was_partial: boolean;
  delivery_mode: DeliveryMode;
}

export interface VocabularyEntry {
  id: string;
  term: string;
  created_at_utc: string;
}

export async function listHistoryEntries(): Promise<HistoryEntry[]> {
  return invoke<HistoryEntry[]>("list_history_entries");
}

export async function searchHistoryEntries(
  query: string,
): Promise<HistoryEntry[]> {
  return invoke<HistoryEntry[]>("search_history_entries", { query });
}

export async function deleteHistoryEntry(id: string): Promise<boolean> {
  return invoke<boolean>("delete_history_entry", { id });
}

export async function clearHistory(): Promise<number> {
  return invoke<number>("clear_history");
}

export async function listVocabularyEntries(): Promise<VocabularyEntry[]> {
  return invoke<VocabularyEntry[]>("list_vocabulary_entries");
}

export async function searchVocabularyEntries(
  query: string,
): Promise<VocabularyEntry[]> {
  return invoke<VocabularyEntry[]>("search_vocabulary_entries", { query });
}

export async function addVocabularyTerm(
  term: string,
): Promise<VocabularyEntry> {
  return invoke<VocabularyEntry>("add_vocabulary_term", { term });
}

export async function updateVocabularyTerm(
  id: string,
  newTerm: string,
): Promise<VocabularyEntry> {
  return invoke<VocabularyEntry>("update_vocabulary_term", { id, newTerm });
}

export async function deleteVocabularyTerm(id: string): Promise<boolean> {
  return invoke<boolean>("delete_vocabulary_term", { id });
}

export async function savePreferences(prefs: AppPreferences): Promise<void> {
  return invoke<void>("save_preferences", { prefs });
}

export async function exportSettingsJson(): Promise<string> {
  return invoke<string>("export_settings_json");
}

export async function importSettingsJson(json: string): Promise<void> {
  return invoke<void>("import_settings_json", { json });
}

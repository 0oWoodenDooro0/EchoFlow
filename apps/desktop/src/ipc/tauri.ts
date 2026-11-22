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
}): Promise<AppPreferences> {
  return invoke<AppPreferences>("save_general_settings", params);
}

export async function getPlatformCapabilities(): Promise<PlatformCapabilities> {
  return invoke<PlatformCapabilities>("get_platform_capabilities");
}

export async function subscribeControllerEvents(
  handler: (event: ControllerEvent) => void,
): Promise<UnlistenFn> {
  return listen<ControllerEvent>(CONTROLLER_EVENT_NAME, (event) => {
    handler(event.payload);
  });
}

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export const CONTROLLER_EVENT_NAME = "echoflow://controller-event";

export type DeliveryMode = "text_insertion" | "fallback_delivery" | "aborted";

export type TrayStatus = "idle" | "listening" | "missing_credential" | "error";

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

import { useEffect, useState } from "react";
import {
  deleteProviderCredential,
  getPlatformCapabilities,
  getProviderCredentialStatus,
  openPermissionSettings,
  saveProviderCredential,
  testProviderConnection,
  type ConnectionTestResult,
  type PermissionStatus,
  type PlatformCapabilities,
  type ProviderCredentialStatus,
} from "../ipc/tauri";

function formatPermissionStatus(status: PermissionStatus): string {
  switch (status) {
    case "granted":
      return "Granted";
    case "denied":
      return "Denied";
    case "not_determined":
      return "Not Determined";
    case "not_applicable":
      return "Not Applicable";
  }
}

export function OnboardingPanel() {
  const [apiKeyInput, setApiKeyInput] = useState("");
  const [credentialStatus, setCredentialStatus] =
    useState<ProviderCredentialStatus | null>(null);
  const [capabilities, setCapabilities] =
    useState<PlatformCapabilities | null>(null);
  const [formError, setFormError] = useState<string | null>(null);
  const [connectionResult, setConnectionResult] =
    useState<ConnectionTestResult | null>(null);
  const [busyAction, setBusyAction] = useState<
    "save" | "delete" | "test" | null
  >(null);

  useEffect(() => {
    let active = true;
    void (async () => {
      try {
        const [status, caps] = await Promise.all([
          getProviderCredentialStatus(),
          getPlatformCapabilities(),
        ]);
        if (!active) return;
        setCredentialStatus(status);
        setCapabilities(caps);
      } catch (err) {
        if (!active) return;
        setFormError(
          err instanceof Error ? err.message : "Failed to load onboarding state",
        );
      }
    })();
    return () => {
      active = false;
    };
  }, []);

  async function handleSave() {
    const trimmed = apiKeyInput.trim();
    if (!trimmed) {
      setFormError("Provider Credential cannot be empty.");
      return;
    }
    if (/\s/.test(trimmed)) {
      setFormError(
        "Malformed Provider Credential: whitespace is not allowed.",
      );
      return;
    }

    setFormError(null);
    setConnectionResult(null);
    setBusyAction("save");
    try {
      const updated = await saveProviderCredential(trimmed);
      setCredentialStatus(updated);
      setApiKeyInput("");
    } catch (err) {
      setFormError(
        err instanceof Error ? err.message : "Failed to save Provider Credential",
      );
    } finally {
      setBusyAction(null);
    }
  }

  async function handleDelete() {
    setFormError(null);
    setConnectionResult(null);
    setBusyAction("delete");
    try {
      const updated = await deleteProviderCredential();
      setCredentialStatus(updated);
    } catch (err) {
      setFormError(
        err instanceof Error
          ? err.message
          : "Failed to delete Provider Credential",
      );
    } finally {
      setBusyAction(null);
    }
  }

  async function handleTestConnection() {
    setFormError(null);
    setConnectionResult(null);
    setBusyAction("test");
    try {
      const candidate = apiKeyInput.trim();
      const result = await testProviderConnection(
        candidate ? candidate : undefined,
      );
      setConnectionResult(result);
    } catch (err) {
      setConnectionResult({
        success: false,
        message:
          err instanceof Error
            ? err.message
            : "Connection test failed unexpectedly.",
        error_kind: "transport_error",
      });
    } finally {
      setBusyAction(null);
    }
  }

  return (
    <div className="space-y-6">
      <section className="rounded-lg border border-slate-800 bg-slate-900/60 p-4">
        <div className="mb-3 flex items-center justify-between">
          <h2 className="text-base font-medium text-white">
            Provider Credential
          </h2>
          <span
            className={`rounded-full px-2.5 py-0.5 text-xs font-medium ${
              credentialStatus?.configured
                ? "bg-emerald-950 text-emerald-300 border border-emerald-800"
                : "bg-amber-950 text-amber-300 border border-amber-800"
            }`}
          >
            {credentialStatus?.configured
              ? `Configured (${credentialStatus.masked_preview ?? "••••"})`
              : "Not Configured"}
          </span>
        </div>

        {credentialStatus && !credentialStatus.secret_service_available && (
          <div
            role="alert"
            className="mb-3 rounded-md border border-amber-700/60 bg-amber-950/50 p-3 text-xs text-amber-200"
          >
            <p className="font-semibold">OS Secret Service Unavailable (Degraded)</p>
            {credentialStatus.guidance && (
              <p className="mt-1">{credentialStatus.guidance}</p>
            )}
          </div>
        )}

        <div className="space-y-3">
          <div>
            <label
              htmlFor="provider-credential-input"
              className="mb-1 block text-xs font-medium text-slate-300"
            >
              Provider Credential (Gemini API Key)
            </label>
            <input
              id="provider-credential-input"
              type="password"
              value={apiKeyInput}
              onChange={(e) => setApiKeyInput(e.target.value)}
              placeholder="AIza..."
              autoComplete="off"
              className="w-full rounded-md border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white placeholder-slate-500 focus:border-sky-500 focus:outline-none"
            />
          </div>

          {formError && (
            <p role="alert" className="text-xs text-rose-400">
              {formError}
            </p>
          )}

          {connectionResult && (
            <div
              role="status"
              className={`rounded-md border p-2.5 text-xs ${
                connectionResult.success
                  ? "border-emerald-700/60 bg-emerald-950/50 text-emerald-200"
                  : "border-rose-700/60 bg-rose-950/50 text-rose-200"
              }`}
            >
              {connectionResult.message}
            </div>
          )}

          <div className="flex flex-wrap gap-2">
            <button
              type="button"
              onClick={() => void handleSave()}
              disabled={busyAction !== null}
              className="rounded-md bg-sky-600 px-3 py-1.5 text-xs font-medium text-white hover:bg-sky-500 disabled:opacity-50"
            >
              {busyAction === "save" ? "Saving..." : "Save Credential"}
            </button>

            <button
              type="button"
              onClick={() => void handleTestConnection()}
              disabled={busyAction !== null}
              className="rounded-md border border-slate-700 bg-slate-800 px-3 py-1.5 text-xs font-medium text-slate-100 hover:bg-slate-700 disabled:opacity-50"
            >
              {busyAction === "test" ? "Testing..." : "Test Connection"}
            </button>

            {credentialStatus?.configured && (
              <button
                type="button"
                onClick={() => void handleDelete()}
                disabled={busyAction !== null}
                className="rounded-md border border-rose-800/70 bg-rose-950/60 px-3 py-1.5 text-xs font-medium text-rose-200 hover:bg-rose-900/70 disabled:opacity-50"
              >
                {busyAction === "delete" ? "Deleting..." : "Delete Credential"}
              </button>
            )}
          </div>
        </div>
      </section>

      {capabilities?.display_server === "mac_os" && (
        <section className="rounded-lg border border-slate-800 bg-slate-900/60 p-4">
          <h3 className="mb-2 text-sm font-medium text-white">
            macOS Permissions Checklist
          </h3>
          <div className="space-y-2 text-xs">
            <div className="flex items-center justify-between rounded-md border border-slate-800 bg-slate-950/60 px-3 py-2">
              <div>
                <p className="font-medium text-slate-200">Microphone Access</p>
                <p className="text-slate-400">
                  Status: {formatPermissionStatus(capabilities.microphone_permission)}
                </p>
              </div>
              <button
                type="button"
                onClick={() => void openPermissionSettings("microphone")}
                className="rounded-md border border-slate-700 bg-slate-800 px-2.5 py-1 text-xs text-slate-100 hover:bg-slate-700"
              >
                Open Microphone Settings
              </button>
            </div>

            <div className="flex items-center justify-between rounded-md border border-slate-800 bg-slate-950/60 px-3 py-2">
              <div>
                <p className="font-medium text-slate-200">
                  Accessibility (Global Hotkey &amp; Text Insertion)
                </p>
                <p className="text-slate-400">
                  Status:{" "}
                  {formatPermissionStatus(capabilities.accessibility_permission)}
                </p>
              </div>
              <button
                type="button"
                onClick={() => void openPermissionSettings("accessibility")}
                className="rounded-md border border-slate-700 bg-slate-800 px-2.5 py-1 text-xs text-slate-100 hover:bg-slate-700"
              >
                Open Accessibility Settings
              </button>
            </div>
          </div>
        </section>
      )}

      {(capabilities?.display_server === "linux_x11" ||
        capabilities?.display_server === "linux_wayland") && (
        <section className="rounded-lg border border-slate-800 bg-slate-900/60 p-4">
          <h3 className="mb-2 text-sm font-medium text-white">
            Linux Display &amp; Input Capabilities
          </h3>
          <div className="space-y-1.5 text-xs text-slate-300">
            <p>
              Session:{" "}
              <span className="font-medium text-white">
                {capabilities.display_server === "linux_wayland"
                  ? "Linux Wayland"
                  : "Linux X11"}
              </span>
            </p>
            <p>
              Global Push-to-Talk Hotkey:{" "}
              {capabilities.supports_global_hotkey ? "Supported" : "Unavailable"}
            </p>
            <p>
              Text Insertion:{" "}
              {capabilities.supports_text_insertion
                ? "Supported"
                : "Fallback Delivery"}
            </p>
          </div>
        </section>
      )}
    </div>
  );
}

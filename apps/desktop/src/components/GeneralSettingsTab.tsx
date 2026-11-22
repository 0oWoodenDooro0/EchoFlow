import { useEffect, useState } from "react";
import {
  getPlatformCapabilities,
  getPreferences,
  saveGeneralSettings,
  type PlatformCapabilities,
} from "../ipc/tauri";

const MODIFIER_NAMES = new Set(["ctrl", "control", "alt", "option", "opt", "shift", "meta", "cmd", "command", "super", "win"]);
const RESERVED_BINDINGS = new Set([
  "Ctrl+C",
  "Ctrl+V",
  "Ctrl+X",
  "Ctrl+A",
  "Ctrl+Z",
  "Ctrl+Q",
  "Ctrl+W",
  "Ctrl+Alt+Delete",
  "Meta+C",
  "Meta+V",
  "Meta+X",
  "Meta+A",
  "Meta+Z",
  "Meta+Q",
  "Meta+W",
  "Meta+Tab",
  "Meta+Space",
  "Alt+F4",
  "Alt+Tab",
]);

function normalizePrimaryKeyName(raw: string): string | null {
  const lower = raw.toLowerCase();
  if (lower === " " || lower === "space" || lower === "spacebar") return "Space";
  if (lower === "escape" || lower === "esc") return "Escape";
  if (lower === "enter" || lower === "return") return "Enter";
  if (lower === "tab") return "Tab";
  if (lower === "backspace") return "Backspace";
  if (lower === "delete" || lower === "del") return "Delete";
  if (lower === "insert" || lower === "ins") return "Insert";
  if (lower === "home") return "Home";
  if (lower === "end") return "End";
  if (lower === "pageup" || lower === "pgup") return "PageUp";
  if (lower === "pagedown" || lower === "pgdn") return "PageDown";
  if (lower === "arrowup" || lower === "up") return "Up";
  if (lower === "arrowdown" || lower === "down") return "Down";
  if (lower === "arrowleft" || lower === "left") return "Left";
  if (lower === "arrowright" || lower === "right") return "Right";
  if (/^[a-z0-9]$/.test(lower)) return lower.toUpperCase();
  if (/^f([1-9]|1[0-9]|2[0-4])$/.test(lower)) return lower.toUpperCase();
  return null;
}

export function validateHotkeyBinding(binding: string): {
  normalized?: string;
  error?: string;
} {
  const trimmed = binding.trim();
  if (!trimmed) {
    return { error: "Push-to-Talk Hotkey cannot be empty." };
  }

  const parts = trimmed.split("+").map((p) => p.trim());
  if (parts.some((p) => p.length === 0)) {
    return { error: "Push-to-Talk Hotkey contains an empty key token." };
  }

  const modifiers = new Set<string>();
  let primaryKey: string | null = null;

  for (const part of parts) {
    const lower = part.toLowerCase();
    if (MODIFIER_NAMES.has(lower)) {
      if (lower === "ctrl" || lower === "control") modifiers.add("Ctrl");
      else if (lower === "alt" || lower === "option" || lower === "opt")
        modifiers.add("Alt");
      else if (lower === "shift") modifiers.add("Shift");
      else modifiers.add("Meta");
    } else {
      const normalizedKey = normalizePrimaryKeyName(part);
      if (!normalizedKey) {
        return { error: `Unrecognized key "${part}" in Push-to-Talk Hotkey.` };
      }
      if (primaryKey !== null) {
        return { error: "Push-to-Talk Hotkey cannot have multiple primary keys." };
      }
      primaryKey = normalizedKey;
    }
  }

  if (!primaryKey) {
    return { error: "Push-to-Talk Hotkey must include a primary key." };
  }

  if (primaryKey === "Escape") {
    return {
      error: "Escape is reserved for cancelling an active Dictation Session.",
    };
  }

  const orderedModifiers = ["Ctrl", "Alt", "Shift", "Meta"].filter((m) =>
    modifiers.has(m),
  );
  const canonical = [...orderedModifiers, primaryKey].join("+");

  if (RESERVED_BINDINGS.has(canonical)) {
    return {
      error: `Push-to-Talk Hotkey "${canonical}" is reserved by the OS or application.`,
    };
  }

  return { normalized: canonical };
}

function formatKeyEventToBinding(e: KeyboardEvent): string | null {
  const key = e.key;
  if (
    key === "Control" ||
    key === "Alt" ||
    key === "Shift" ||
    key === "Meta"
  ) {
    return null;
  }

  const primary =
    e.code === "Space" ? "Space" : normalizePrimaryKeyName(key);
  if (!primary) return null;

  const parts: string[] = [];
  if (e.ctrlKey) parts.push("Ctrl");
  if (e.altKey) parts.push("Alt");
  if (e.shiftKey) parts.push("Shift");
  if (e.metaKey) parts.push("Meta");
  parts.push(primary);
  return parts.join("+");
}

export function GeneralSettingsTab() {
  const [hotkey, setHotkey] = useState("Alt+Space");
  const [restoreClipboard, setRestoreClipboard] = useState(true);
  const [restoreDelayMs, setRestoreDelayMs] = useState(150);
  const [isRecording, setIsRecording] = useState(false);
  const [capabilities, setCapabilities] = useState<PlatformCapabilities | null>(
    null,
  );
  const [error, setError] = useState<string | null>(null);
  const [savedNotice, setSavedNotice] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    void (async () => {
      try {
        const [prefs, caps] = await Promise.all([
          getPreferences(),
          getPlatformCapabilities(),
        ]);
        if (!active) return;
        setHotkey(prefs.push_to_talk_hotkey);
        setRestoreClipboard(prefs.restore_clipboard);
        setRestoreDelayMs(prefs.clipboard_restore_delay_ms);
        setCapabilities(caps);
      } catch (err) {
        if (!active) return;
        setError(String(err));
      }
    })();
    return () => {
      active = false;
    };
  }, []);

  useEffect(() => {
    if (!isRecording) return;

    const handleKeyDown = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();

      if (e.key === "Escape") {
        setIsRecording(false);
        return;
      }

      const candidate = formatKeyEventToBinding(e);
      if (!candidate) return;

      const validation = validateHotkeyBinding(candidate);
      setIsRecording(false);
      setSavedNotice(null);
      if (validation.error) {
        setError(validation.error);
        return;
      }
      if (validation.normalized) {
        setError(null);
        setHotkey(validation.normalized);
      }
    };

    window.addEventListener("keydown", handleKeyDown, { capture: true });
    return () => {
      window.removeEventListener("keydown", handleKeyDown, { capture: true });
    };
  }, [isRecording]);

  async function handleSave(e: React.FormEvent) {
    e.preventDefault();
    setSavedNotice(null);

    const validation = validateHotkeyBinding(hotkey);
    if (validation.error || !validation.normalized) {
      setError(validation.error ?? "Invalid Push-to-Talk Hotkey.");
      return;
    }

    setError(null);
    try {
      const updated = await saveGeneralSettings({
        push_to_talk_hotkey: validation.normalized,
        restore_clipboard: restoreClipboard,
        clipboard_restore_delay_ms: restoreDelayMs,
      });
      setHotkey(updated.push_to_talk_hotkey);
      setRestoreClipboard(updated.restore_clipboard);
      setRestoreDelayMs(updated.clipboard_restore_delay_ms);
      setSavedNotice("Settings saved.");
    } catch (err) {
      setError(String(err));
    }
  }

  return (
    <form onSubmit={handleSave} className="space-y-6">
      <div>
        <h2 className="mb-1 text-base font-medium text-white">
          Push-to-Talk Hotkey & Clipboard Preservation
        </h2>
        <p className="text-xs text-slate-400">
          Hold your Push-to-Talk Hotkey from any application to start a
          Dictation Session. Press Escape while speaking to cancel without
          inserting text.
        </p>
      </div>

      {capabilities?.display_server === "linux_wayland" &&
        !capabilities.supports_text_insertion && (
          <div className="rounded-md border border-amber-500/40 bg-amber-950/40 px-3 py-2 text-xs text-amber-200">
            Linux Wayland session detected without{" "}
            <code>ydotool</code>/<code>wtype</code>. Text Insertion will use
            Fallback Delivery (leaving Final Transcript on your clipboard).
          </div>
        )}

      <div className="space-y-2">
        <label
          htmlFor="push-to-talk-hotkey"
          className="block text-sm font-medium text-slate-200"
        >
          Push-to-Talk Hotkey
        </label>
        <div className="flex items-center gap-2">
          <input
            id="push-to-talk-hotkey"
            type="text"
            value={hotkey}
            onChange={(e) => {
              setHotkey(e.target.value);
              setSavedNotice(null);
            }}
            className="w-64 rounded-md border border-slate-700 bg-slate-900 px-3 py-1.5 text-sm text-white focus:border-sky-500 focus:outline-none"
          />
          <button
            type="button"
            onClick={() => {
              setError(null);
              setSavedNotice(null);
              setIsRecording((prev) => !prev);
            }}
            className="rounded-md border border-slate-700 bg-slate-800 px-3 py-1.5 text-sm font-medium text-slate-100 hover:bg-slate-700"
          >
            {isRecording ? "Press keys..." : "Record Hotkey"}
          </button>
        </div>
      </div>

      <div className="space-y-3 border-t border-slate-800 pt-4">
        <label className="flex items-center gap-2 text-sm text-slate-200">
          <input
            type="checkbox"
            checked={restoreClipboard}
            onChange={(e) => {
              setRestoreClipboard(e.target.checked);
              setSavedNotice(null);
            }}
            className="h-4 w-4 rounded border-slate-700 bg-slate-900 text-sky-500"
          />
          <span>Restore clipboard after Text Insertion</span>
        </label>

        {restoreClipboard && (
          <div className="flex items-center gap-2 pl-6 text-xs text-slate-400">
            <label htmlFor="clipboard-restore-delay">Restore delay (ms):</label>
            <input
              id="clipboard-restore-delay"
              type="number"
              min={0}
              max={2000}
              value={restoreDelayMs}
              onChange={(e) => {
                setRestoreDelayMs(Number(e.target.value) || 0);
                setSavedNotice(null);
              }}
              className="w-20 rounded border border-slate-700 bg-slate-900 px-2 py-1 text-slate-200"
            />
          </div>
        )}
      </div>

      {error && (
        <p role="alert" className="text-sm text-rose-400">
          {error}
        </p>
      )}
      {savedNotice && (
        <p className="text-sm text-emerald-400">{savedNotice}</p>
      )}

      <div>
        <button
          type="submit"
          className="rounded-md bg-sky-600 px-4 py-1.5 text-sm font-medium text-white hover:bg-sky-500"
        >
          Save Settings
        </button>
      </div>
    </form>
  );
}

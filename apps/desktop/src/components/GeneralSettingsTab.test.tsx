import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AppPreferences, PlatformCapabilities } from "../ipc/tauri";
import { GeneralSettingsTab } from "./GeneralSettingsTab";

const invokeMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

const defaultPrefs: AppPreferences = {
  push_to_talk_hotkey: "Alt+Space",
  microphone_id: null,
  restore_clipboard: true,
  clipboard_restore_delay_ms: 150,
  history_retention_days: 30,
};

const defaultCaps: PlatformCapabilities = {
  display_server: "linux_x11",
  microphone_permission: "not_applicable",
  accessibility_permission: "not_applicable",
  supports_global_hotkey: true,
  supports_text_insertion: true,
  wayland_helpers: [],
  secret_service_available: true,
};

describe("GeneralSettingsTab at Tauri IPC boundary", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    invokeMock.mockImplementation(async (cmd: string, args?: unknown) => {
      if (cmd === "get_preferences") return defaultPrefs;
      if (cmd === "get_platform_capabilities") return defaultCaps;
      if (cmd === "save_general_settings") {
        const payload = args as {
          push_to_talk_hotkey: string;
          restore_clipboard: boolean;
          clipboard_restore_delay_ms: number;
        };
        return {
          ...defaultPrefs,
          ...payload,
        };
      }
      return null;
    });
  });

  it("loads preferences and records a valid custom Push-to-Talk Hotkey", async () => {
    render(<GeneralSettingsTab />);

    const hotkeyInput = await screen.findByLabelText(/push-to-talk hotkey/i);
    expect(hotkeyInput).toHaveValue("Alt+Space");

    const recordBtn = screen.getByRole("button", { name: /record hotkey/i });
    fireEvent.click(recordBtn);
    expect(recordBtn).toHaveTextContent(/press keys/i);

    fireEvent.keyDown(window, {
      key: " ",
      code: "Space",
      ctrlKey: true,
      shiftKey: true,
    });

    expect(hotkeyInput).toHaveValue("Ctrl+Shift+Space");

    fireEvent.click(screen.getByRole("button", { name: /save settings/i }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("save_general_settings", {
        push_to_talk_hotkey: "Ctrl+Shift+Space",
        restore_clipboard: true,
        clipboard_restore_delay_ms: 150,
      });
    });

    expect(await screen.findByText(/settings saved/i)).toBeInTheDocument();
  });

  it("rejects empty, modifier-only, and reserved hotkey bindings cleanly", async () => {
    render(<GeneralSettingsTab />);

    const hotkeyInput = await screen.findByLabelText(/push-to-talk hotkey/i);
    const saveBtn = screen.getByRole("button", { name: /save settings/i });

    // Empty binding
    fireEvent.change(hotkeyInput, { target: { value: "   " } });
    fireEvent.click(saveBtn);
    expect(await screen.findByRole("alert")).toHaveTextContent(/cannot be empty/i);
    expect(invokeMock).not.toHaveBeenCalledWith(
      "save_general_settings",
      expect.anything(),
    );

    // Reserved OS shortcut (Ctrl+C)
    fireEvent.change(hotkeyInput, { target: { value: "Ctrl+C" } });
    fireEvent.click(saveBtn);
    expect(await screen.findByRole("alert")).toHaveTextContent(/reserved/i);
    expect(invokeMock).not.toHaveBeenCalledWith(
      "save_general_settings",
      expect.anything(),
    );

    // Modifier-only
    fireEvent.change(hotkeyInput, { target: { value: "Ctrl+Alt" } });
    fireEvent.click(saveBtn);
    expect(await screen.findByRole("alert")).toHaveTextContent(/primary key/i);
    expect(invokeMock).not.toHaveBeenCalledWith(
      "save_general_settings",
      expect.anything(),
    );
  });

  it("toggles Clipboard Preservation restore setting and displays Wayland fallback notice", async () => {
    invokeMock.mockImplementation(async (cmd: string, args?: unknown) => {
      if (cmd === "get_preferences") return defaultPrefs;
      if (cmd === "get_platform_capabilities") {
        return {
          ...defaultCaps,
          display_server: "linux_wayland",
          supports_global_hotkey: false,
          supports_text_insertion: false,
          wayland_helpers: [],
        } satisfies PlatformCapabilities;
      }
      if (cmd === "save_general_settings") {
        const payload = args as {
          push_to_talk_hotkey: string;
          restore_clipboard: boolean;
          clipboard_restore_delay_ms: number;
        };
        return {
          ...defaultPrefs,
          ...payload,
        };
      }
      return null;
    });

    render(<GeneralSettingsTab />);

    expect(
      await screen.findByText(/wayland.*fallback delivery/i),
    ).toBeInTheDocument();

    const restoreCheckbox = screen.getByLabelText(
      /restore clipboard after text insertion/i,
    );
    expect(restoreCheckbox).toBeChecked();

    fireEvent.click(restoreCheckbox);
    expect(restoreCheckbox).not.toBeChecked();

    fireEvent.click(screen.getByRole("button", { name: /save settings/i }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("save_general_settings", {
        push_to_talk_hotkey: "Alt+Space",
        restore_clipboard: false,
        clipboard_restore_delay_ms: 150,
      });
    });
  });
});

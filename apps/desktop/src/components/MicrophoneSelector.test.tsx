import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AudioDeviceInfo } from "../ipc/tauri";
import { MicrophoneSelector } from "./MicrophoneSelector";

const invokeMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

describe("MicrophoneSelector at Tauri IPC boundary", () => {
  const sampleDevices: AudioDeviceInfo[] = [
    { id: "builtin-mic", name: "Built-in Microphone", is_default: true },
    { id: "usb-mic", name: "Blue Yeti USB", is_default: false },
  ];

  beforeEach(() => {
    invokeMock.mockReset();
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "list_audio_devices") return sampleDevices;
      if (cmd === "get_selected_microphone") return null;
      if (cmd === "select_microphone") return undefined;
      return null;
    });
  });

  it("enumerates input devices and defaults to OS default microphone", async () => {
    render(<MicrophoneSelector />);

    const select = await screen.findByLabelText(/microphone input device/i);
    expect(select).toHaveValue("");
    expect(screen.getByRole("option", { name: /system default/i })).toBeInTheDocument();
    expect(
      screen.getByRole("option", { name: "Built-in Microphone (Default)" }),
    ).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "Blue Yeti USB" })).toBeInTheDocument();
  });

  it("persists microphone selection via select_microphone IPC", async () => {
    render(<MicrophoneSelector />);

    const select = await screen.findByLabelText(/microphone input device/i);
    fireEvent.change(select, { target: { value: "usb-mic" } });

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("select_microphone", {
        deviceId: "usb-mic",
      });
    });
    expect(select).toHaveValue("usb-mic");
  });

  it("shows empty state when no microphone devices are available", async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "list_audio_devices") return [];
      if (cmd === "get_selected_microphone") return null;
      return null;
    });

    render(<MicrophoneSelector />);

    expect(
      await screen.findByText(/no microphone input devices detected/i),
    ).toBeInTheDocument();
  });
});

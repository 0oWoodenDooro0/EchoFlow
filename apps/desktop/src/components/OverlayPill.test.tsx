import { act, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ControllerEvent, SessionState } from "../ipc/tauri";
import { OverlayPill } from "./OverlayPill";

type EventCallback = (event: { payload: ControllerEvent }) => void;

const listeners = new Map<string, EventCallback>();
const invokeMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (eventName: string, cb: EventCallback) => {
    listeners.set(eventName, cb);
    return () => {
      listeners.delete(eventName);
    };
  }),
}));

function emitControllerEvent(payload: ControllerEvent) {
  const cb = listeners.get("echoflow://controller-event");
  if (!cb) {
    throw new Error("Controller event listener not registered");
  }
  act(() => {
    cb({ payload });
  });
}

describe("OverlayPill at Tauri IPC boundary", () => {
  beforeEach(() => {
    listeners.clear();
    invokeMock.mockReset();
    invokeMock.mockResolvedValue({ kind: "idle" } satisfies SessionState);
  });

  it("stays hidden when controller state is Idle", async () => {
    render(<OverlayPill />);
    await act(async () => {
      await Promise.resolve();
    });

    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  it("renders Connecting, Listening with live Draft Transcript, Finalizing, and Delivering states", async () => {
    render(<OverlayPill />);
    await act(async () => {
      await Promise.resolve();
    });

    emitControllerEvent({
      type: "state_changed",
      state: { kind: "connecting" },
    });
    expect(screen.getByRole("status")).toHaveTextContent(/connecting/i);

    emitControllerEvent({
      type: "state_changed",
      state: { kind: "listening" },
    });
    emitControllerEvent({
      type: "audio_level_updated",
      rms: 0.65,
    });
    emitControllerEvent({
      type: "draft_transcript_updated",
      draft_transcript: "Hello EchoFlow live draft",
    });

    const pill = screen.getByRole("status");
    expect(pill).toHaveTextContent(/listening/i);
    expect(pill).toHaveTextContent("Hello EchoFlow live draft");

    emitControllerEvent({
      type: "state_changed",
      state: { kind: "finalizing" },
    });
    expect(screen.getByRole("status")).toHaveTextContent(/finalizing/i);

    emitControllerEvent({
      type: "state_changed",
      state: { kind: "delivering" },
    });
    expect(screen.getByRole("status")).toHaveTextContent(/delivering/i);

    emitControllerEvent({
      type: "state_changed",
      state: { kind: "idle" },
    });
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  it("renders Error and FallbackNotice states clearly", async () => {
    render(<OverlayPill />);
    await act(async () => {
      await Promise.resolve();
    });

    emitControllerEvent({
      type: "state_changed",
      state: {
        kind: "error",
        message: "Provider Credential is missing",
      },
    });
    expect(screen.getByRole("status")).toHaveTextContent(
      "Provider Credential is missing",
    );

    emitControllerEvent({
      type: "state_changed",
      state: {
        kind: "fallback_notice",
        final_transcript: "Copied safely",
        reason: "Text Insertion unavailable on Wayland",
      },
    });
    const pill = screen.getByRole("status");
    expect(pill).toHaveTextContent(/copied to clipboard/i);
    expect(pill).toHaveTextContent("Text Insertion unavailable on Wayland");
  });
});

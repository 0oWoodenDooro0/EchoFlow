import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { HistoryEntry } from "../ipc/tauri";
import { HistoryTab } from "./HistoryTab";

const invokeMock = vi.fn();
const writeTextMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

const sampleEntries: HistoryEntry[] = [
  {
    id: "h-2",
    final_transcript: "Newest dictation about EchoFlow",
    created_at_utc: "2026-11-22T09:00:00Z",
    duration_ms: 1800,
    was_partial: false,
    delivery_mode: "text_insertion",
  },
  {
    id: "h-1",
    final_transcript: "Older partial dictation",
    created_at_utc: "2026-11-21T08:00:00Z",
    duration_ms: 950,
    was_partial: true,
    delivery_mode: "fallback_delivery",
  },
];

describe("HistoryTab at Tauri IPC boundary", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    writeTextMock.mockReset();
    writeTextMock.mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText: writeTextMock },
      configurable: true,
    });
  });

  it("renders chronological history entries, supports search, one-click copy, delete, and clear all", async () => {
    let entries = [...sampleEntries];
    invokeMock.mockImplementation(
      async (cmd: string, args?: Record<string, unknown>) => {
        if (cmd === "list_history_entries") return entries;
        if (cmd === "search_history_entries") {
          const q = String(args?.query ?? "").toLowerCase();
          return entries.filter((e) =>
            e.final_transcript.toLowerCase().includes(q),
          );
        }
        if (cmd === "delete_history_entry") {
          const id = String(args?.id ?? "");
          entries = entries.filter((e) => e.id !== id);
          return true;
        }
        if (cmd === "clear_history") {
          const count = entries.length;
          entries = [];
          return count;
        }
        return null;
      },
    );

    render(<HistoryTab />);

    expect(
      await screen.findByText("Newest dictation about EchoFlow"),
    ).toBeInTheDocument();
    expect(screen.getByText("Older partial dictation")).toBeInTheDocument();

    // Search
    const searchInput = screen.getByPlaceholderText(/search history/i);
    fireEvent.change(searchInput, { target: { value: "EchoFlow" } });
    await waitFor(() => {
      expect(
        screen.queryByText("Older partial dictation"),
      ).not.toBeInTheDocument();
    });
    expect(
      screen.getByText("Newest dictation about EchoFlow"),
    ).toBeInTheDocument();

    // Clear search
    fireEvent.change(searchInput, { target: { value: "" } });
    expect(
      await screen.findByText("Older partial dictation"),
    ).toBeInTheDocument();

    // One-click copy
    const copyButtons = screen.getAllByRole("button", { name: /^copy$/i });
    fireEvent.click(copyButtons[0]);
    await waitFor(() => {
      expect(writeTextMock).toHaveBeenCalledWith(
        "Newest dictation about EchoFlow",
      );
    });
    expect(await screen.findByText(/copied/i)).toBeInTheDocument();

    // Individual delete
    const deleteButtons = screen.getAllByRole("button", { name: /^delete$/i });
    fireEvent.click(deleteButtons[0]);
    await waitFor(() => {
      expect(
        screen.queryByText("Newest dictation about EchoFlow"),
      ).not.toBeInTheDocument();
    });
    expect(screen.getByText("Older partial dictation")).toBeInTheDocument();

    // Clear All History
    fireEvent.click(
      screen.getByRole("button", { name: /clear all history/i }),
    );
    await waitFor(() => {
      expect(
        screen.queryByText("Older partial dictation"),
      ).not.toBeInTheDocument();
    });
    expect(screen.getByText(/no history entries/i)).toBeInTheDocument();
  });
});

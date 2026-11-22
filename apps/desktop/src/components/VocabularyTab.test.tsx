import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { VocabularyEntry } from "../ipc/tauri";
import { VocabularyTab } from "./VocabularyTab";

const invokeMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

describe("VocabularyTab at Tauri IPC boundary", () => {
  beforeEach(() => {
    invokeMock.mockReset();
  });

  it("supports search, add, edit, delete, live N / 1,000 counter, and rejects empty/duplicate/over-cap", async () => {
    let entries: VocabularyEntry[] = [
      {
        id: "v-1",
        term: "EchoFlow",
        created_at_utc: "2026-11-22T08:00:00Z",
      },
      {
        id: "v-2",
        term: "UniFFI",
        created_at_utc: "2026-11-22T08:05:00Z",
      },
    ];

    invokeMock.mockImplementation(
      async (cmd: string, args?: Record<string, unknown>) => {
        if (cmd === "list_vocabulary_entries") return entries;
        if (cmd === "search_vocabulary_entries") {
          const q = String(args?.query ?? "").toLowerCase();
          return entries.filter((e) => e.term.toLowerCase().includes(q));
        }
        if (cmd === "add_vocabulary_term") {
          const term = String(args?.term ?? "").trim();
          const created: VocabularyEntry = {
            id: `v-${entries.length + 1}`,
            term,
            created_at_utc: "2026-11-22T09:00:00Z",
          };
          entries = [...entries, created];
          return created;
        }
        if (cmd === "update_vocabulary_term") {
          const id = String(args?.id ?? "");
          const newTerm = String(args?.newTerm ?? "").trim();
          entries = entries.map((e) =>
            e.id === id ? { ...e, term: newTerm } : e,
          );
          return entries.find((e) => e.id === id);
        }
        if (cmd === "delete_vocabulary_term") {
          const id = String(args?.id ?? "");
          entries = entries.filter((e) => e.id !== id);
          return true;
        }
        return null;
      },
    );

    render(<VocabularyTab />);

    expect(await screen.findByText("EchoFlow")).toBeInTheDocument();
    expect(screen.getByText("2 / 1,000")).toBeInTheDocument();

    const addInput = screen.getByPlaceholderText(/add vocabulary term/i);
    const addButton = screen.getByRole("button", { name: /^add$/i });

    // Reject empty/whitespace in UI without calling invoke
    fireEvent.change(addInput, { target: { value: "   " } });
    fireEvent.click(addButton);
    expect(
      await screen.findByText(/cannot be empty/i),
    ).toBeInTheDocument();
    expect(invokeMock).not.toHaveBeenCalledWith(
      "add_vocabulary_term",
      expect.anything(),
    );

    // Reject duplicate (case-insensitive) in UI
    fireEvent.change(addInput, { target: { value: "echoflow" } });
    fireEvent.click(addButton);
    expect(await screen.findByText(/already exists/i)).toBeInTheDocument();
    expect(invokeMock).not.toHaveBeenCalledWith(
      "add_vocabulary_term",
      expect.anything(),
    );

    // Add valid term
    fireEvent.change(addInput, { target: { value: "Tauri" } });
    fireEvent.click(addButton);
    expect(await screen.findByText("Tauri")).toBeInTheDocument();
    expect(screen.getByText("3 / 1,000")).toBeInTheDocument();

    // Search
    const searchInput = screen.getByPlaceholderText(/search vocabulary/i);
    fireEvent.change(searchInput, { target: { value: "uni" } });
    await waitFor(() => {
      expect(screen.queryByText("Tauri")).not.toBeInTheDocument();
    });
    expect(screen.getByText("UniFFI")).toBeInTheDocument();

    // Clear search
    fireEvent.change(searchInput, { target: { value: "" } });
    expect(await screen.findByText("Tauri")).toBeInTheDocument();

    // Edit first entry
    const editButtons = screen.getAllByRole("button", { name: /^edit$/i });
    fireEvent.click(editButtons[0]);
    const editInput = screen.getByLabelText(/edit vocabulary term/i);
    fireEvent.change(editInput, { target: { value: "EchoFlow Core" } });
    fireEvent.click(screen.getByRole("button", { name: /^save$/i }));
    expect(await screen.findByText("EchoFlow Core")).toBeInTheDocument();

    // Delete
    const deleteButtons = screen.getAllByRole("button", { name: /^delete$/i });
    fireEvent.click(deleteButtons[0]);
    await waitFor(() => {
      expect(screen.queryByText("EchoFlow Core")).not.toBeInTheDocument();
    });
    expect(screen.getByText("2 / 1,000")).toBeInTheDocument();
  });

  it("rejects adding when 1,000-term cap is reached", async () => {
    const fullEntries: VocabularyEntry[] = Array.from(
      { length: 1000 },
      (_, i) => ({
        id: `v-${i}`,
        term: `Term-${i}`,
        created_at_utc: "2026-11-22T08:00:00Z",
      }),
    );

    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "list_vocabulary_entries") return fullEntries;
      return null;
    });

    render(<VocabularyTab />);
    expect(await screen.findByText("1,000 / 1,000")).toBeInTheDocument();

    const addInput = screen.getByPlaceholderText(/add vocabulary term/i);
    fireEvent.change(addInput, { target: { value: "OverflowTerm" } });
    fireEvent.click(screen.getByRole("button", { name: /^add$/i }));

    expect(
      await screen.findByText(/1,000.*limit reached/i),
    ).toBeInTheDocument();
    expect(invokeMock).not.toHaveBeenCalledWith(
      "add_vocabulary_term",
      expect.anything(),
    );
  });

  it("supports JSON export and surfaces import rejection errors", async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "list_vocabulary_entries") return [];
      if (cmd === "export_settings_json") {
        return JSON.stringify({
          schema_version: 1,
          exported_at_utc: "2026-11-22T00:00:00Z",
          preferences: {
            push_to_talk_hotkey: "Alt+Space",
            microphone_id: null,
            restore_clipboard: true,
            clipboard_restore_delay_ms: 150,
            history_retention_days: 30,
          },
          vocabulary: ["EchoFlow"],
        });
      }
      if (cmd === "import_settings_json") {
        throw new Error("unsupported settings schema_version 99");
      }
      return null;
    });

    render(<VocabularyTab />);
    await screen.findByText("0 / 1,000");

    fireEvent.click(screen.getByRole("button", { name: /export json/i }));
    const jsonBox = (await screen.findByLabelText(
      /settings json/i,
    )) as HTMLTextAreaElement;
    expect(jsonBox.value).toContain('"schema_version":1');

    fireEvent.change(jsonBox, { target: { value: '{"schema_version": 99}' } });
    fireEvent.click(screen.getByRole("button", { name: /import json/i }));
    expect(
      await screen.findByText(/unsupported settings schema_version 99/i),
    ).toBeInTheDocument();
  });
});

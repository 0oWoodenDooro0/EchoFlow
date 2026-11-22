import { useCallback, useEffect, useState } from "react";
import {
  clearHistory,
  deleteHistoryEntry,
  listHistoryEntries,
  searchHistoryEntries,
  type HistoryEntry,
} from "../ipc/tauri";

function formatDeliveryMode(mode: HistoryEntry["delivery_mode"]): string {
  switch (mode) {
    case "text_insertion":
      return "Text Insertion";
    case "fallback_delivery":
      return "Fallback Delivery";
    case "aborted":
      return "Aborted";
  }
}

export function HistoryTab() {
  const [entries, setEntries] = useState<HistoryEntry[]>([]);
  const [searchQuery, setSearchQuery] = useState("");
  const [copiedId, setCopiedId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  const loadEntries = useCallback(async (query: string) => {
    try {
      setError(null);
      const trimmed = query.trim();
      const result = trimmed
        ? await searchHistoryEntries(trimmed)
        : await listHistoryEntries();
      setEntries(result);
    } catch (err) {
      setError(
        err instanceof Error ? err.message : "Failed to load history entries",
      );
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadEntries(searchQuery);
  }, [loadEntries, searchQuery]);

  async function handleCopy(entry: HistoryEntry) {
    try {
      await navigator.clipboard.writeText(entry.final_transcript);
      setCopiedId(entry.id);
    } catch (err) {
      setError(
        err instanceof Error ? err.message : "Failed to copy to clipboard",
      );
    }
  }

  async function handleDelete(id: string) {
    try {
      setError(null);
      await deleteHistoryEntry(id);
      setEntries((prev) => prev.filter((item) => item.id !== id));
    } catch (err) {
      setError(
        err instanceof Error ? err.message : "Failed to delete history entry",
      );
    }
  }

  async function handleClearAll() {
    try {
      setError(null);
      await clearHistory();
      setEntries([]);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to clear history");
    }
  }

  return (
    <section className="space-y-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h2 className="text-base font-medium text-white">History</h2>
        <button
          type="button"
          onClick={handleClearAll}
          disabled={entries.length === 0}
          className="rounded-md border border-rose-500/40 bg-rose-500/10 px-3 py-1.5 text-xs font-medium text-rose-300 transition hover:bg-rose-500/20 disabled:cursor-not-allowed disabled:opacity-40"
        >
          Clear All History
        </button>
      </div>

      <input
        type="search"
        value={searchQuery}
        onChange={(e) => setSearchQuery(e.target.value)}
        placeholder="Search history..."
        aria-label="Search history"
        className="w-full rounded-md border border-slate-700 bg-slate-900 px-3 py-2 text-sm text-slate-100 placeholder-slate-500 focus:border-sky-500 focus:outline-none"
      />

      {error && (
        <div
          role="alert"
          className="rounded-md border border-rose-500/40 bg-rose-500/10 px-3 py-2 text-xs text-rose-300"
        >
          {error}
        </div>
      )}

      {loading ? (
        <p className="text-xs text-slate-400">Loading history...</p>
      ) : entries.length === 0 ? (
        <p className="rounded-md border border-slate-800 bg-slate-900/50 p-4 text-center text-xs text-slate-400">
          No history entries found.
        </p>
      ) : (
        <ul className="space-y-2">
          {entries.map((entry) => (
            <li
              key={entry.id}
              className="rounded-md border border-slate-800 bg-slate-900/70 p-3"
            >
              <div className="flex items-start justify-between gap-3">
                <p className="whitespace-pre-wrap text-sm text-slate-100">
                  {entry.final_transcript}
                </p>
                <div className="flex shrink-0 items-center gap-2">
                  <button
                    type="button"
                    onClick={() => handleCopy(entry)}
                    className="rounded border border-slate-700 bg-slate-800 px-2.5 py-1 text-xs font-medium text-slate-200 transition hover:bg-slate-700"
                  >
                    {copiedId === entry.id ? "Copied" : "Copy"}
                  </button>
                  <button
                    type="button"
                    onClick={() => handleDelete(entry.id)}
                    className="rounded border border-slate-700 bg-slate-800 px-2.5 py-1 text-xs font-medium text-rose-300 transition hover:bg-rose-950/60"
                  >
                    Delete
                  </button>
                </div>
              </div>
              <div className="mt-2 flex flex-wrap items-center gap-3 text-[11px] text-slate-400">
                <span>{entry.created_at_utc}</span>
                <span>{entry.duration_ms} ms</span>
                <span>{formatDeliveryMode(entry.delivery_mode)}</span>
                {entry.was_partial && (
                  <span className="rounded bg-amber-500/20 px-1.5 py-0.5 text-amber-300">
                    Partial
                  </span>
                )}
              </div>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

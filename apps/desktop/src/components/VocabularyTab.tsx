import { useCallback, useEffect, useState, type FormEvent } from "react";
import {
  addVocabularyTerm,
  deleteVocabularyTerm,
  exportSettingsJson,
  importSettingsJson,
  listVocabularyEntries,
  MAX_VOCABULARY_TERMS,
  searchVocabularyEntries,
  updateVocabularyTerm,
  type VocabularyEntry,
} from "../ipc/tauri";

export function VocabularyTab() {
  const [allCount, setAllCount] = useState(0);
  const [allTermsLower, setAllTermsLower] = useState<Set<string>>(new Set());
  const [entries, setEntries] = useState<VocabularyEntry[]>([]);
  const [searchQuery, setSearchQuery] = useState("");
  const [newTerm, setNewTerm] = useState("");
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editingValue, setEditingValue] = useState("");
  const [jsonText, setJsonText] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [statusMessage, setStatusMessage] = useState<string | null>(null);

  const refreshAllStats = useCallback(async () => {
    const all = await listVocabularyEntries();
    setAllCount(all.length);
    setAllTermsLower(new Set(all.map((e) => e.term.toLowerCase())));
    return all;
  }, []);

  const loadEntries = useCallback(
    async (query: string) => {
      try {
        setError(null);
        const trimmed = query.trim();
        if (!trimmed) {
          const all = await refreshAllStats();
          setEntries(all);
        } else {
          const filtered = await searchVocabularyEntries(trimmed);
          setEntries(filtered);
        }
      } catch (err) {
        setError(
          err instanceof Error
            ? err.message
            : "Failed to load Custom Vocabulary",
        );
      }
    },
    [refreshAllStats],
  );

  useEffect(() => {
    void loadEntries(searchQuery);
  }, [loadEntries, searchQuery]);

  async function handleAdd(event: FormEvent) {
    event.preventDefault();
    setError(null);
    setStatusMessage(null);

    const trimmed = newTerm.trim();
    if (!trimmed) {
      setError("Custom Vocabulary term cannot be empty.");
      return;
    }
    if (allCount >= MAX_VOCABULARY_TERMS) {
      setError(
        `Custom Vocabulary 1,000-term limit reached (${allCount.toLocaleString("en-US")} / 1,000).`,
      );
      return;
    }
    if (allTermsLower.has(trimmed.toLowerCase())) {
      setError(`Custom Vocabulary term "${trimmed}" already exists.`);
      return;
    }

    try {
      await addVocabularyTerm(trimmed);
      setNewTerm("");
      await loadEntries(searchQuery);
      setStatusMessage(`Added "${trimmed}".`);
    } catch (err) {
      setError(
        err instanceof Error ? err.message : "Failed to add vocabulary term",
      );
    }
  }

  function startEdit(entry: VocabularyEntry) {
    setEditingId(entry.id);
    setEditingValue(entry.term);
    setError(null);
    setStatusMessage(null);
  }

  async function handleSaveEdit(id: string) {
    setError(null);
    setStatusMessage(null);

    const trimmed = editingValue.trim();
    if (!trimmed) {
      setError("Custom Vocabulary term cannot be empty.");
      return;
    }

    try {
      await updateVocabularyTerm(id, trimmed);
      setEditingId(null);
      setEditingValue("");
      await loadEntries(searchQuery);
      setStatusMessage(`Updated "${trimmed}".`);
    } catch (err) {
      setError(
        err instanceof Error ? err.message : "Failed to update vocabulary term",
      );
    }
  }

  async function handleDelete(id: string) {
    setError(null);
    setStatusMessage(null);
    try {
      await deleteVocabularyTerm(id);
      await loadEntries(searchQuery);
    } catch (err) {
      setError(
        err instanceof Error ? err.message : "Failed to delete vocabulary term",
      );
    }
  }

  async function handleExportJson() {
    setError(null);
    setStatusMessage(null);
    try {
      const exported = await exportSettingsJson();
      setJsonText(exported);
      setStatusMessage(
        "Exported preferences and Custom Vocabulary (excluding Provider Credential and History Entry).",
      );
    } catch (err) {
      setError(
        err instanceof Error ? err.message : "Failed to export settings JSON",
      );
    }
  }

  async function handleImportJson() {
    setError(null);
    setStatusMessage(null);
    if (!jsonText.trim()) {
      setError("Paste a valid settings JSON payload before importing.");
      return;
    }
    try {
      await importSettingsJson(jsonText);
      await loadEntries(searchQuery);
      setStatusMessage("Imported preferences and Custom Vocabulary.");
    } catch (err) {
      setError(
        err instanceof Error ? err.message : "Failed to import settings JSON",
      );
    }
  }

  return (
    <section className="space-y-5">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h2 className="text-base font-medium text-white">
            Custom Vocabulary
          </h2>
          <p className="text-xs text-slate-400">
            Manage domain jargon and proper nouns for recognition biasing.
          </p>
        </div>
        <span
          data-testid="vocabulary-counter"
          className={`rounded-full border px-3 py-1 text-xs font-medium ${
            allCount >= MAX_VOCABULARY_TERMS
              ? "border-amber-500/40 bg-amber-500/10 text-amber-300"
              : "border-slate-700 bg-slate-900 text-slate-200"
          }`}
        >
          {allCount.toLocaleString("en-US")} /{" "}
          {MAX_VOCABULARY_TERMS.toLocaleString("en-US")}
        </span>
      </div>

      <form onSubmit={handleAdd} className="flex gap-2">
        <input
          type="text"
          value={newTerm}
          onChange={(e) => setNewTerm(e.target.value)}
          placeholder="Add vocabulary term..."
          aria-label="Add vocabulary term"
          className="flex-1 rounded-md border border-slate-700 bg-slate-900 px-3 py-2 text-sm text-slate-100 placeholder-slate-500 focus:border-sky-500 focus:outline-none"
        />
        <button
          type="submit"
          className="rounded-md bg-sky-600 px-4 py-2 text-sm font-medium text-white transition hover:bg-sky-500"
        >
          Add
        </button>
      </form>

      <input
        type="search"
        value={searchQuery}
        onChange={(e) => setSearchQuery(e.target.value)}
        placeholder="Search vocabulary..."
        aria-label="Search vocabulary"
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

      {statusMessage && (
        <div className="rounded-md border border-emerald-500/40 bg-emerald-500/10 px-3 py-2 text-xs text-emerald-300">
          {statusMessage}
        </div>
      )}

      {entries.length === 0 ? (
        <p className="rounded-md border border-slate-800 bg-slate-900/50 p-4 text-center text-xs text-slate-400">
          No Custom Vocabulary terms found.
        </p>
      ) : (
        <ul className="max-h-64 space-y-2 overflow-y-auto pr-1">
          {entries.map((entry) => (
            <li
              key={entry.id}
              className="flex items-center justify-between gap-3 rounded-md border border-slate-800 bg-slate-900/70 px-3 py-2"
            >
              {editingId === entry.id ? (
                <div className="flex flex-1 items-center gap-2">
                  <input
                    type="text"
                    value={editingValue}
                    onChange={(e) => setEditingValue(e.target.value)}
                    aria-label="Edit vocabulary term"
                    className="flex-1 rounded border border-slate-700 bg-slate-950 px-2 py-1 text-sm text-slate-100 focus:border-sky-500 focus:outline-none"
                  />
                  <button
                    type="button"
                    onClick={() => handleSaveEdit(entry.id)}
                    className="rounded bg-sky-600 px-2.5 py-1 text-xs font-medium text-white hover:bg-sky-500"
                  >
                    Save
                  </button>
                  <button
                    type="button"
                    onClick={() => setEditingId(null)}
                    className="rounded border border-slate-700 px-2.5 py-1 text-xs text-slate-300 hover:bg-slate-800"
                  >
                    Cancel
                  </button>
                </div>
              ) : (
                <>
                  <span className="text-sm text-slate-100">{entry.term}</span>
                  <div className="flex shrink-0 items-center gap-2">
                    <button
                      type="button"
                      onClick={() => startEdit(entry)}
                      className="rounded border border-slate-700 bg-slate-800 px-2.5 py-1 text-xs font-medium text-slate-200 transition hover:bg-slate-700"
                    >
                      Edit
                    </button>
                    <button
                      type="button"
                      onClick={() => handleDelete(entry.id)}
                      className="rounded border border-slate-700 bg-slate-800 px-2.5 py-1 text-xs font-medium text-rose-300 transition hover:bg-rose-950/60"
                    >
                      Delete
                    </button>
                  </div>
                </>
              )}
            </li>
          ))}
        </ul>
      )}

      <div className="space-y-2 border-t border-slate-800 pt-4">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <div>
            <h3 className="text-sm font-medium text-white">
              Settings &amp; Vocabulary JSON Export / Import
            </h3>
            <p className="text-xs text-slate-400">
              Excludes Provider Credential and History Entry records.
            </p>
          </div>
          <div className="flex gap-2">
            <button
              type="button"
              onClick={handleExportJson}
              className="rounded-md border border-slate-700 bg-slate-800 px-3 py-1.5 text-xs font-medium text-slate-200 transition hover:bg-slate-700"
            >
              Export JSON
            </button>
            <button
              type="button"
              onClick={handleImportJson}
              className="rounded-md bg-slate-700 px-3 py-1.5 text-xs font-medium text-white transition hover:bg-slate-600"
            >
              Import JSON
            </button>
          </div>
        </div>
        <textarea
          value={jsonText}
          onChange={(e) => setJsonText(e.target.value)}
          aria-label="Settings JSON"
          rows={4}
          placeholder="Click Export JSON or paste an exported JSON payload here to import..."
          className="w-full rounded-md border border-slate-800 bg-slate-900 p-2 font-mono text-xs text-slate-200 placeholder-slate-500 focus:border-sky-500 focus:outline-none"
        />
      </div>
    </section>
  );
}

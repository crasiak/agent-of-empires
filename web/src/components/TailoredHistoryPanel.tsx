import { useEffect, useRef, useState, type RefObject } from "react";
import { flushSync } from "react-dom";
import { fetchTailoredHistory, setTailoredHistoryHidden } from "../lib/api";
import type { SessionResponse, TailoredHistoryEntry } from "../lib/types";

interface Props {
  searchRef: RefObject<HTMLInputElement | null>;
  readOnly: boolean;
  onNew: () => void;
  onOpen: (session: SessionResponse) => void;
}

export function TailoredHistoryPanel({ searchRef, readOnly, onNew, onOpen }: Props) {
  const [query, setQuery] = useState("");
  const [hidden, setHidden] = useState(false);
  const [entries, setEntries] = useState<TailoredHistoryEntry[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [loadError, setLoadError] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState<string | null>(null);
  const requestSeq = useRef(0);
  const mounted = useRef(false);
  const toggle = useRef<HTMLButtonElement>(null);
  const pending = useRef(false);

  useEffect(() => {
    mounted.current = true;
    const controller = new AbortController();
    const refresh = async () => {
      const seq = ++requestSeq.current;
      const result = await fetchTailoredHistory(controller.signal);
      if (!mounted.current || seq !== requestSeq.current) return;
      setLoaded(true);
      setLoadError(result === null);
      if (result) setEntries(result.entries);
    };
    void refresh();
    const timer = window.setInterval(() => void refresh(), 3000);
    return () => {
      mounted.current = false;
      controller.abort();
      window.clearInterval(timer);
    };
  }, []);

  const changeVisibility = async (entry: TailoredHistoryEntry) => {
    if (readOnly || pending.current) return;
    pending.current = true;
    setSaving(entry.id);
    setError(null);
    ++requestSeq.current;
    const previouslyFocused = document.activeElement;
    const result = await setTailoredHistoryHidden(entry.id, !entry.hidden);
    if (!mounted.current) return;
    ++requestSeq.current;
    pending.current = false;
    setSaving(null);
    if (!result.ok) {
      setError(result.message ?? "Could not confirm history visibility. Your Session was not stopped or deleted.");
      return;
    }
    flushSync(() =>
      setEntries((current) =>
        current.map((item) =>
          item.id === entry.id && item.profile === entry.profile ? { ...item, hidden: !entry.hidden } : item,
        ),
      ),
    );
    if (previouslyFocused && !previouslyFocused.isConnected && document.activeElement === document.body)
      toggle.current?.focus();
  };

  const normalized = query.trim().toLowerCase();
  const matchesPreset = !hidden && "capture an idea".includes(normalized);
  const visible = entries.filter(
    (entry) =>
      entry.hidden === hidden &&
      `${entry.title} ${entry.input_preview} ${entry.profile} ${entry.account} ${entry.status}`
        .toLowerCase()
        .includes(normalized),
  );

  return (
    <div className="space-y-3 p-4">
      <input
        ref={searchRef}
        aria-label="Find a tailored preset"
        value={query}
        onChange={(event) => setQuery(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "Enter" && !readOnly) {
            event.preventDefault();
            if (matchesPreset) onNew();
            else if (visible[0]) onOpen(visible[0].session);
          }
        }}
        placeholder="Find a preset or previous capture…"
        className="h-9 w-full rounded-md border border-surface-700 bg-surface-950 px-3 text-sm text-text-primary"
      />
      {matchesPreset && (
        <button
          type="button"
          disabled={readOnly}
          onClick={onNew}
          className="w-full rounded-md border border-surface-700 px-3 py-2 text-left text-sm text-text-primary hover:bg-surface-800 disabled:opacity-50"
        >
          <span className="font-medium">Capture an idea</span>
          <span className="mt-1 block text-xs text-text-secondary">
            Quick capture with inferred ticket, Atlas, and KB destinations.
          </span>
        </button>
      )}
      <div className="flex items-center justify-between gap-2">
        <h3 className="text-xs font-semibold text-text-secondary">{hidden ? "Hidden captures" : "Recent captures"}</h3>
        <button
          ref={toggle}
          type="button"
          onClick={() => setHidden((value) => !value)}
          aria-pressed={hidden}
          className="h-8 rounded-md px-2 text-xs text-text-secondary hover:bg-surface-800"
        >
          {hidden ? "Show recent" : "Show hidden"}
        </button>
      </div>
      <p className="text-xs text-text-secondary">
        Hide only removes an entry from this history. It never stops, archives, or deletes a Session.
      </p>
      {!loaded && (
        <p role="status" className="text-sm text-text-secondary">
          Loading capture history…
        </p>
      )}
      {loadError && (
        <p role="alert" className="text-sm text-status-error">
          Capture history is unavailable. Existing Sessions are unchanged.
        </p>
      )}
      {error && (
        <p role="alert" className="text-sm text-status-error">
          {error}
        </p>
      )}
      {loaded && !loadError && visible.length === 0 && (
        <p className="text-sm text-text-secondary">{hidden ? "No hidden captures." : "No matching captures."}</p>
      )}
      <ul
        aria-label={hidden ? "Hidden captures" : "Recent captures"}
        className="max-h-[45dvh] divide-y divide-surface-700 overflow-y-auto"
      >
        {visible.map((entry) => (
          <li
            key={`${entry.profile}:${entry.id}`}
            data-testid={`tailored-history-${entry.id}`}
            className="flex items-start gap-2 py-2"
          >
            <button
              type="button"
              disabled={readOnly}
              onClick={() => onOpen(entry.session)}
              aria-label={`Open ${entry.title}`}
              className="min-w-0 flex-1 rounded-md px-2 py-1 text-left hover:bg-surface-800 disabled:opacity-50"
            >
              <span className="block truncate text-sm font-medium text-text-primary">{entry.title}</span>
              {entry.input_preview && (
                <span className="block truncate text-xs text-text-secondary">{entry.input_preview}</span>
              )}
              <span className="block text-xs text-text-secondary">
                {entry.account} · {entry.profile} · {entry.status} · {new Date(entry.created_at).toLocaleString()}
              </span>
            </button>
            <button
              type="button"
              disabled={readOnly || saving !== null}
              onClick={() => void changeVisibility(entry)}
              aria-label={`${entry.hidden ? "Restore" : "Hide"} ${entry.title} ${entry.hidden ? "in" : "from"} history`}
              className="h-8 rounded-md px-2 text-xs text-text-secondary hover:bg-surface-800 disabled:opacity-50"
            >
              {saving === entry.id ? "Saving…" : entry.hidden ? "Restore" : "Hide"}
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
}

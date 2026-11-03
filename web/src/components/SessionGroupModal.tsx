import { useEffect, useRef, useState } from "react";
import { fetchGroups } from "../lib/api";
import { CancelButton, ConfirmButton, Dialog } from "./Dialog";
import { useConfirmKeys, useDialogFocus } from "./dialogHooks";

const DIALOG_ID = "session-group-modal";
const PROMPT_ID = `${DIALOG_ID}-prompt`;

interface Props {
  sessionTitle: string;
  currentGroup: string;
  /** Resolves false on failure so the modal stays open with an error. */
  onSave: (group: string) => Promise<boolean>;
  onClose: () => void;
}

export function SessionGroupModal({ sessionTitle, currentGroup, onSave, onClose }: Props) {
  const [value, setValue] = useState(currentGroup);
  const [manual, setManual] = useState(false);
  const [groups, setGroups] = useState<string[]>([]);
  const [loadState, setLoadState] = useState<"loading" | "loaded" | "error">("loading");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement | null>(null);
  useDialogFocus(inputRef, true);

  useEffect(() => {
    let active = true;
    void fetchGroups({ throwOnError: true }).then(
      (loaded) => {
        if (!active) return;
        setGroups(loaded.map((group) => group.path).filter(Boolean));
        setLoadState("loaded");
      },
      () => {
        if (active) setLoadState("error");
      },
    );
    return () => {
      active = false;
    };
  }, []);

  const options = [...new Set([currentGroup, ...groups].filter(Boolean))];
  const handleSave = async () => {
    if (saving) return;
    // Existing paths are identifiers; only manually entered paths are trimmed.
    const next = manual && value !== currentGroup ? value.trim() : value;
    if (next === currentGroup) return onClose();
    setSaving(true);
    setError(null);
    if (await onSave(next)) return onClose();
    setSaving(false);
    setError(next ? "Failed to update group." : "Failed to clear group.");
    inputRef.current?.focus();
  };
  useConfirmKeys(
    () => !saving && onClose(),
    () => void handleSave(),
    saving,
  );

  return (
    <Dialog
      id={DIALOG_ID}
      title="Edit group"
      describedBy={PROMPT_ID}
      bodyClassName="px-5 py-4 space-y-3"
      onDismiss={() => !saving && onClose()}
      footer={
        <>
          <CancelButton onClick={onClose} disabled={saving} />
          <ConfirmButton
            onClick={handleSave}
            busy={saving}
            testId="session-group-modal-save"
            className="text-white bg-brand-600/90 hover:bg-brand-600"
          >
            {saving ? "Saving..." : "Save"}
          </ConfirmButton>
        </>
      }
    >
      <p id={PROMPT_ID} className="text-[13px] text-text-secondary">
        Move <span className="text-text-primary">{sessionTitle}</span> to a group.
      </p>
      <label htmlFor={`${DIALOG_ID}-existing`} className="block text-[12px] text-text-secondary">
        Existing group
      </label>
      <select
        id={`${DIALOG_ID}-existing`}
        value={manual ? "manual" : value ? `group:${value}` : ""}
        disabled={saving}
        onChange={(e) => {
          const selected = e.target.value;
          setManual(selected === "manual");
          if (selected !== "manual") setValue(selected ? selected.slice("group:".length) : "");
          setError(null);
          if (selected === "manual") inputRef.current?.focus();
        }}
        className="w-full bg-surface-900 border border-surface-700 rounded px-2 py-1.5 text-[13px] font-mono text-text-primary focus:outline-none focus:border-brand-600"
      >
        <option value="">No group</option>
        {options.map((path) => (
          <option key={path} value={`group:${path}`}>
            {JSON.stringify(path)}
          </option>
        ))}
        <option value="manual">New group or manual path</option>
      </select>
      <p role={loadState === "error" ? "alert" : "status"} className="text-[12px] text-text-secondary">
        {loadState === "loading"
          ? "Loading groups..."
          : loadState === "error"
            ? "Could not load groups. Enter a group manually."
            : groups.length === 0
              ? "No existing groups."
              : "Choose an exact path, or enter one below."}
      </p>
      <label htmlFor={`${DIALOG_ID}-input`} className="block text-[12px] text-text-secondary">
        New group or manual path
      </label>
      <input
        id={`${DIALOG_ID}-input`}
        ref={inputRef}
        disabled={saving}
        type="text"
        value={value}
        onChange={(e) => {
          setValue(e.target.value);
          setManual(true);
          setError(null);
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            if (!saving) void handleSave();
          }
        }}
        placeholder="Group (blank to ungroup)"
        data-testid="session-group-modal-input"
        className="w-full bg-surface-900 border border-surface-700 rounded px-2 py-1.5 text-[13px] font-mono text-text-primary focus:outline-none focus:border-brand-600"
      />
      <p className="text-[12px] text-text-dim">
        Leave blank to ungroup. Use <span className="font-mono">/</span> for hierarchy, for example{" "}
        <span className="font-mono">work/projects</span>.
      </p>
      {error && (
        <p data-testid="session-group-modal-error" className="text-[12px] text-status-error">
          {error}
        </p>
      )}
    </Dialog>
  );
}

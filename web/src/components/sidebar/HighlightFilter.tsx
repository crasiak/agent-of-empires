import type { CSSProperties } from "react";
import { REPO_COLOR_OPTIONS, repoSwatchStyle } from "../../lib/repoAppearance";
import { SESSION_COLOR_OPTIONS } from "./rowModel";

const OPTIONS = {
  sessions: SESSION_COLOR_OPTIONS.map((o) => ({
    key: o.key,
    label: o.label,
    style: { backgroundColor: `var(${o.token})` },
  })),
  projects: REPO_COLOR_OPTIONS.map((o) => ({ key: o.id, label: o.label, style: repoSwatchStyle(o.id) })),
} satisfies Record<string, { key: string; label: string; style: CSSProperties }[]>;

export function HighlightFilter({
  scope,
  selected,
  onChange,
}: {
  scope: "sessions" | "projects";
  selected: readonly (string | null)[];
  onChange: (colors: (string | null)[]) => void;
}) {
  const toggle = (color: string | null) =>
    onChange(selected.includes(color) ? selected.filter((c) => c !== color) : [...selected, color]);
  const buttonClass = (active: boolean) =>
    `h-8 min-w-8 rounded-md border text-[11px] cursor-pointer transition-colors focus-visible:outline-2 focus-visible:outline-text-primary focus-visible:outline-offset-2 ${
      active
        ? "border-text-primary bg-surface-700 text-text-primary"
        : "border-surface-700 text-text-secondary hover:bg-surface-700/50"
    }`;

  return (
    <fieldset className="mt-2">
      <legend className="mb-1 text-[11px] text-text-secondary">
        {scope === "sessions" ? "Session highlight" : "Project highlight"}
      </legend>
      <div className="flex flex-wrap gap-1">
        <button
          type="button"
          aria-label={`All ${scope} highlights`}
          aria-pressed={selected.length === 0}
          onClick={() => onChange([])}
          className={buttonClass(selected.length === 0)}
        >
          All
        </button>
        {OPTIONS[scope].map((o) => (
          <button
            key={o.key}
            type="button"
            aria-label={`Filter ${scope} by ${o.label}`}
            aria-pressed={selected.includes(o.key)}
            title={o.label}
            onClick={() => toggle(o.key)}
            className={`${buttonClass(selected.includes(o.key))} flex items-center justify-center`}
          >
            <span className="h-3 w-3 rounded-full" style={o.style} aria-hidden />
          </button>
        ))}
        <button
          type="button"
          aria-label={`Filter ${scope} with no highlight`}
          aria-pressed={selected.includes(null)}
          title="No highlight"
          onClick={() => toggle(null)}
          className={`${buttonClass(selected.includes(null))} px-1`}
        >
          None
        </button>
      </div>
    </fieldset>
  );
}

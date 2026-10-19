import type { CSSProperties } from "react";

export type RepoColor = "amber" | "teal" | "sky" | "violet" | "rose" | "slate";

export interface RepoAppearance {
  alias?: string;
  color?: RepoColor;
}

export type RepoAppearanceUpdate = {
  alias?: string | null;
  color?: RepoColor | null;
};

export const REPO_COLOR_OPTIONS: Array<{
  id: RepoColor;
  label: string;
}> = [
  { id: "amber", label: "Amber" },
  { id: "teal", label: "Teal" },
  { id: "sky", label: "Sky" },
  { id: "violet", label: "Violet" },
  { id: "rose", label: "Rose" },
  { id: "slate", label: "Slate" },
];

// Fixed hues: these are user labels, and theme tokens can share one value.
const REPO_COLOR_HEX: Record<RepoColor, string> = {
  amber: "#f59e0b",
  teal: "#14b8a6",
  sky: "#0ea5e9",
  violet: "#8b5cf6",
  rose: "#f43f5e",
  slate: "#64748b",
};

// Faint tinted background for a repo header / project row carrying a color.
export function repoColorStyle(color: RepoColor | null): CSSProperties | undefined {
  if (!color) return undefined;
  return {
    backgroundColor: `color-mix(in srgb, ${REPO_COLOR_HEX[color]} 14%, transparent)`,
  };
}

// Solid swatch for the color picker.
export function repoSwatchStyle(color: RepoColor): CSSProperties {
  return { backgroundColor: REPO_COLOR_HEX[color] };
}

const validColors = new Set(REPO_COLOR_OPTIONS.map((option) => option.id));

export function applyRepoAppearanceUpdate(
  current: Record<string, RepoAppearance>,
  repoId: string,
  update: RepoAppearanceUpdate,
): Record<string, RepoAppearance> {
  const nextForRepo: RepoAppearance = { ...(current[repoId] ?? {}) };
  if ("alias" in update) {
    const alias = update.alias?.trim() ?? "";
    if (alias) nextForRepo.alias = alias;
    else delete nextForRepo.alias;
  }
  if ("color" in update) {
    if (update.color && validColors.has(update.color)) nextForRepo.color = update.color;
    else delete nextForRepo.color;
  }

  const next = { ...current };
  if (nextForRepo.alias || nextForRepo.color) next[repoId] = nextForRepo;
  else delete next[repoId];
  return next;
}

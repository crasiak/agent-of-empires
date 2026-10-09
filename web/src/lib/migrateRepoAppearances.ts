// One-time import of browser data that predates shared repository appearances.
import { importRepoAppearances } from "./api";
import { REPO_COLOR_OPTIONS, type RepoAppearance, type RepoColor } from "./repoAppearance";
import { safeGetItem, safeRemoveItem } from "./safeStorage";

const KEY = "aoe-repo-appearance-v1";

export function parseLegacyRepoAppearances(raw: string): Record<string, RepoAppearance> {
  try {
    const value: unknown = JSON.parse(raw);
    if (!value || typeof value !== "object" || Array.isArray(value)) return {};
    const entries: Record<string, RepoAppearance> = {};
    for (const [id, entry] of Object.entries(value)) {
      if ((!id.startsWith("/") && id !== "__scratch__" && id !== "__multi_repo__") || id.includes("\0")) continue;
      if (!entry || typeof entry !== "object" || Array.isArray(entry)) continue;
      const alias = typeof entry.alias === "string" ? entry.alias.trim() : "";
      const color = REPO_COLOR_OPTIONS.some((c) => c.id === entry.color) ? (entry.color as RepoColor) : undefined;
      if (alias || color) entries[id] = { ...(alias ? { alias } : {}), ...(color ? { color } : {}) };
    }
    return entries;
  } catch {
    return {};
  }
}

export async function migrateRepoAppearances(): Promise<Record<string, RepoAppearance> | null> {
  const raw = safeGetItem(KEY);
  if (raw === null) return null;
  const map = await importRepoAppearances(parseLegacyRepoAppearances(raw));
  if (map) safeRemoveItem(KEY);
  return map;
}

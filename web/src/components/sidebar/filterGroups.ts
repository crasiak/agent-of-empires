import type { Workspace } from "../../lib/types";
import {
  sidebarGroupHasLiveWorkspace,
  type NestedSidebarGroup,
  type OrgNestedGroup,
  type SidebarGroup,
  type SidebarWorkspaceView,
} from "../../lib/sidebarGroups";
import { workspaceIsSunk, workspaceIsTrashed } from "../../lib/sidebarSort";

function workspaceMatchesQuery(ws: Workspace, q: string): boolean {
  return (
    ws.displayName.toLowerCase().includes(q) ||
    ws.projectPath.toLowerCase().includes(q) ||
    (ws.branch?.toLowerCase().includes(q) ?? false) ||
    ws.agents.some((a) => a.toLowerCase().includes(q)) ||
    ws.sessions.some((s) => s.title.toLowerCase().includes(q))
  );
}

/** Text matches may come from a parent header; other filters always apply to the row. */
export function makeRowFilter(q: string, matchesFilters: (ws: Workspace) => boolean) {
  return (v: SidebarWorkspaceView, ...groupNames: string[]) =>
    (!q || workspaceMatchesQuery(v.workspace, q) || groupNames.some((n) => n.toLowerCase().includes(q))) &&
    matchesFilters(v.workspace);
}

type RowFilter = ReturnType<typeof makeRowFilter>;
type EmptyGroupFilter = (group: SidebarGroup, ...parentNames: string[]) => boolean;

function filterLevel(
  groups: SidebarGroup[],
  keep: RowFilter,
  keepEmpty?: EmptyGroupFilter,
  ...parentNames: string[]
): SidebarGroup[] {
  return groups
    .map((g) => ({ ...g, workspaces: g.workspaces.filter((v) => keep(v, g.displayName, ...parentNames)) }))
    .filter((g) => g.workspaces.length > 0 || (g.pinnedEmpty && keepEmpty?.(g, ...parentNames)));
}

export function filterFlat(groups: SidebarGroup[], keep: RowFilter, keepEmpty?: EmptyGroupFilter): SidebarGroup[] {
  return filterLevel(groups, keep, keepEmpty);
}

export function filterNested(
  groups: NestedSidebarGroup[],
  keep: RowFilter,
  keepEmpty?: EmptyGroupFilter,
): NestedSidebarGroup[] {
  return groups
    .map((ng) => ({ repo: ng.repo, subgroups: filterLevel(ng.subgroups, keep, keepEmpty, ng.repo.displayName) }))
    .filter((ng) => ng.subgroups.length > 0 || (ng.repo.pinnedEmpty && keepEmpty?.(ng.repo)));
}

export function filterOrg(groups: OrgNestedGroup[], keep: RowFilter, keepEmpty?: EmptyGroupFilter): OrgNestedGroup[] {
  return groups
    .map((og) => ({ org: og.org, repos: filterLevel(og.repos, keep, keepEmpty, og.org.displayName) }))
    .filter((og) => og.repos.length > 0);
}

export const sunkViews = (groups: SidebarGroup[]) =>
  groups.flatMap((g) => g.workspaces.filter((v) => workspaceIsSunk(v.workspace) && !workspaceIsTrashed(v.workspace)));

/** Workspace ids in render order, so Shift+click ranges only span visible rows; first occurrence wins. */
export function renderedOrder(groups: SidebarGroup[], expandAll: boolean, sunkExpanded: boolean): string[] {
  const ids = new Set<string>();
  for (const g of groups) {
    if (!sidebarGroupHasLiveWorkspace(g) || !(expandAll || !g.collapsed)) continue;
    for (const v of g.workspaces) if (!workspaceIsSunk(v.workspace)) ids.add(v.workspace.id);
  }
  if (sunkExpanded) {
    for (const g of groups) for (const v of g.workspaces) if (workspaceIsSunk(v.workspace)) ids.add(v.workspace.id);
  }
  return [...ids];
}

/** Every workspace once, in group order; a workspace can appear under several groups. */
export function uniqueWorkspaces(groups: SidebarGroup[]): Workspace[] {
  const byId = new Map<string, Workspace>();
  for (const g of groups)
    for (const v of g.workspaces) if (!byId.has(v.workspace.id)) byId.set(v.workspace.id, v.workspace);
  return [...byId.values()];
}

// @vitest-environment jsdom

import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { WorkspaceSidebar } from "../WorkspaceSidebar";
import { makeSession, makeWorkspace } from "./fixtures";
import {
  buildNestedSidebarGroups,
  buildOrgGroups,
  buildSessionGroups,
  repoGroupToSidebarGroup,
} from "../../lib/sidebarGroups";
import type { RepoGroup, Workspace } from "../../lib/types";
import type { SidebarAxis } from "../../lib/sidebarAxis";
import type { PluginUiEntry } from "../../lib/api";
import { SessionColorsContext } from "../../lib/sessionColors";

const pluginEntries = vi.hoisted(() => ({ current: [] as PluginUiEntry[] }));
vi.mock("../../lib/pluginUiContext", () => ({
  usePluginUiEntries: () => pluginEntries.current,
}));

const noop = () => {};
const ws = (id: string, color: string | null = null, group = "team") =>
  makeWorkspace(id, [makeSession({ id, title: id, color, group_path: group })], { branch: `feat/${id}` });

function repo(id: string, color: RepoGroup["color"], workspaces: Workspace[] = [], pinned = false): RepoGroup {
  return {
    id,
    repoPath: id,
    displayName: id,
    defaultDisplayName: id,
    alias: null,
    color,
    remoteOwner: "Acme",
    remoteOwnerKey: "Acme@example.com",
    workspaces,
    status: "idle",
    collapsed: false,
    registeredProjects: workspaces.length ? [] : [{ name: id, path: id, scope: "global", pinned }],
  };
}

const repos = () => [
  repo("/repo/one", "sky", [ws("red-one", "red"), ws("green-one", "green"), ws("plain-one")]),
  repo("/repo/two", "rose", [ws("red-two", "red"), ws("purple-two", "purple")]),
  repo("__scratch__", null, [ws("plain-scratch")]),
];

function Sidebar({
  repoGroups = repos(),
  savedProjects = [],
  axis = "repo",
  collapsed = false,
  colorsEnabled = true,
  readOnly = false,
}: {
  repoGroups?: RepoGroup[];
  savedProjects?: RepoGroup[];
  axis?: SidebarAxis;
  collapsed?: boolean;
  colorsEnabled?: boolean;
  readOnly?: boolean;
}) {
  const opts = { idleDecayWindowMs: 60_000, sortMode: "manual" as const, isCollapsed: () => collapsed };
  const groups = repoGroups.map(repoGroupToSidebarGroup).map((g) => ({ ...g, collapsed }));
  return (
    <SessionColorsContext.Provider value={colorsEnabled}>
      <WorkspaceSidebar
        groups={
          axis === "group"
            ? buildSessionGroups(
                repoGroups.flatMap((r) => r.workspaces),
                opts,
              )
            : groups
        }
        repoGroups={repoGroups}
        nestedGroups={buildNestedSidebarGroups(repoGroups, { ...opts, isSubgroupCollapsed: () => collapsed })}
        orgGroups={buildOrgGroups(repoGroups, {
          isOrgCollapsed: () => collapsed,
          isRepoCollapsed: () => collapsed,
        })}
        savedProjects={savedProjects}
        activeId={null}
        mainPanelFocused={false}
        open
        readOnly={readOnly}
        onToggle={noop}
        onSelect={noop}
        onToggleGroup={noop}
        onToggleSubgroup={noop}
        onToggleOrg={noop}
        onToggleOrgRepo={noop}
        onReorderWorkspaces={noop}
        onReorderGroups={noop}
        onUpdateRepoAppearance={noop}
        onNew={noop}
        onCreateSession={noop}
        onAddProject={noop}
        onEditProject={noop}
        onRemoveProject={noop}
        onSettings={noop}
        sortMode="manual"
        onSortModeChange={noop}
        pluginSortRef={null}
        onPluginSortChange={noop}
        axis={axis}
        onAxisChange={noop}
      />
    </SessionColorsContext.Provider>
  );
}

const click = (name: string | RegExp) => fireEvent.click(screen.getByRole("button", { name }));
const rows = () => screen.queryAllByTestId("sidebar-session-row").map((r) => r.getAttribute("title"));
const query = (value: string) => fireEvent.change(screen.getByTestId("sidebar-filter-input"), { target: { value } });
const openFilter = () => click("Filter sessions");

beforeEach(() => {
  localStorage.clear();
  pluginEntries.current = [];
});

describe("sidebar highlight filters", () => {
  it.each(["repo", "group", "repo+group", "org"] as const)(
    "%s combines highlight selectors with text, expands collapsed matches, and excludes empty groups",
    (axis) => {
      render(<Sidebar axis={axis} collapsed />);
      openFilter();
      click(/^Filter sessions by Red/);
      expect(rows().sort()).toEqual(["red-one", "red-two"]);
      click("Filter projects by Sky");
      expect(rows()).toEqual(["red-one"]);
      click(/^Filter sessions by Green/);
      expect(rows().sort()).toEqual(["green-one", "red-one"]);
      query("  GREEN  ");
      expect(rows()).toEqual(["green-one"]);
      // Matching a parent header must not bypass the color predicates.
      query(axis === "org" ? "acme" : axis === "repo" ? "one" : "team");
      expect(rows().sort()).toEqual(["green-one", "red-one"]);
      click("Filter projects by Sky");
      click("Filter projects by Violet");
      expect(rows()).toEqual([]);
      expect(screen.getByText(/No matches for/)).toBeTruthy();
      expect(screen.queryAllByTestId("sidebar-group-header")).toHaveLength(0);
    },
  );

  it("matches each session palette color, unhighlighted rows, and toggles back to all", () => {
    const colors = ["red", "amber", "green", "purple", "teal"];
    render(<Sidebar repoGroups={[repo("/repo", null, [...colors.map((c) => ws(c, c)), ws("plain")])]} />);
    openFilter();
    for (const color of colors) {
      const name = new RegExp(`^Filter sessions by ${color}`, "i");
      click(name);
      expect(rows()).toEqual([color]);
      expect(screen.getByRole("button", { name }).getAttribute("aria-pressed")).toBe("true");
      click(name);
      expect(rows()).toHaveLength(6);
    }
    click("Filter sessions with no highlight");
    expect(rows()).toEqual(["plain"]);
    click(/^Filter sessions by Amber/);
    expect(rows().sort()).toEqual(["amber", "plain"]);
    click("All sessions highlights");
    expect(rows()).toHaveLength(6);
  });

  it("matches project colors, including synthetic groups, and distinguishes All from None", () => {
    const colors = ["amber", "teal", "sky", "violet", "rose", "slate"] as const;
    render(
      <Sidebar repoGroups={[...colors.map((c) => repo(c, c, [ws(c)])), repo("__multi_repo__", null, [ws("multi")])]} />,
    );
    openFilter();
    for (const color of colors) {
      click(new RegExp(`^Filter projects by ${color}$`, "i"));
      expect(rows()).toEqual([color]);
      click("All projects highlights");
    }
    click("Filter projects with no highlight");
    expect(rows()).toEqual(["multi"]);
    click("Filter projects by Rose");
    click("Filter projects by Sky");
    expect(rows().sort()).toEqual(["multi", "rose", "sky"]);
  });

  it("matches the displayed multi-session highlight and recomputes on live data and axis changes", () => {
    const shared = makeWorkspace(
      "shared",
      [
        makeSession({ id: "plain", color: null, group_path: "team" }),
        makeSession({ id: "red", color: "red", group_path: "team" }),
        makeSession({ id: "green", color: "green", group_path: "other" }),
      ],
      { branch: "shared" },
    );
    const repoGroups = [repo("/repo", "sky", [shared])];
    const view = render(<Sidebar repoGroups={repoGroups} />);
    openFilter();
    click(/^Filter sessions by Green/);
    expect(rows()).toEqual([]);
    view.rerender(<Sidebar repoGroups={repoGroups} axis="group" />);
    expect(screen.getByTestId("sidebar-session-row").getAttribute("data-highlight-color")).toBe("green");
    click("Filter projects by Sky");
    view.rerender(<Sidebar repoGroups={[{ ...repoGroups[0]!, color: "rose" }]} axis="group" />);
    expect(rows()).toEqual([]);
    view.rerender(<Sidebar repoGroups={[repo("/repo", "sky", [ws("now-green", "green")])]} axis="repo" />);
    expect(rows()).toEqual(["now-green"]);
  });

  it.each(["repo", "repo+group", "org"] as const)(
    "%s retains matching pinned-empty projects and filters saved projects without inventing sessions",
    (axis) => {
      render(
        <Sidebar
          axis={axis}
          repoGroups={[...repos(), repo("pinned-sky", "sky", [], true), repo("pinned-rose", "rose", [], true)]}
          savedProjects={[repo("saved-sky", "sky"), repo("saved-rose", "rose")]}
        />,
      );
      openFilter();
      click("Filter projects by Sky");
      expect(screen.getByText("pinned-sky")).toBeTruthy();
      expect(screen.queryByText("pinned-rose")).toBeNull();
      expect(screen.getByTestId("sidebar-project-row").textContent).toContain("saved-sky");
      query("saved");
      expect(rows()).toEqual([]);
      expect(screen.queryByText(/No matches for/)).toBeNull();
      query("pinned");
      expect(screen.getByText("pinned-sky")).toBeTruthy();
      expect(screen.queryByText(/No matches for/)).toBeNull();
      query("");
      click("Filter sessions with no highlight");
      expect(screen.queryByText("pinned-sky")).toBeNull();
      expect(screen.queryByTestId("sidebar-project-row")).toBeNull();
      expect(rows()).toEqual(["plain-one"]);
    },
  );

  it("clears filters, suspends them in compact mode, and disables reordering only while filtered", () => {
    render(<Sidebar />);
    const draggableRows = () => document.querySelectorAll('[aria-roledescription="Press and hold to reorder"]');
    expect(draggableRows()).toHaveLength(6);
    openFilter();
    click(/^Filter sessions by Red/);
    click("Filter projects by Sky");
    query("one");
    expect(rows()).toEqual(["red-one"]);
    expect(draggableRows()).toHaveLength(0);
    expect(document.querySelectorAll('[data-draggable="true"]')).toHaveLength(0);
    click("Compact sidebar");
    expect(rows()).toHaveLength(6);
    expect(screen.queryByTestId("sidebar-filter-input")).toBeNull();
    click("Expand sidebar");
    expect(rows()).toEqual(["red-one"]);
    click("Clear filters");
    expect(rows()).toHaveLength(6);
    expect(draggableRows()).toHaveLength(6);
    click(/^Filter sessions by Red/);
    click("Filter projects by Sky");
    openFilter();
    expect(rows()).toHaveLength(6);
    openFilter();
    expect(screen.getByRole("button", { name: "All sessions highlights" }).getAttribute("aria-pressed")).toBe("true");
    expect(screen.getByRole("button", { name: "All projects highlights" }).getAttribute("aria-pressed")).toBe("true");
  });

  it("keeps read-only filtering available and pauses session colors when their display setting is off", () => {
    const view = render(<Sidebar readOnly />);
    openFilter();
    click(/^Filter sessions by Red/);
    click("Filter projects by Sky");
    expect(rows()).toEqual(["red-one"]);
    view.rerender(<Sidebar readOnly colorsEnabled={false} />);
    expect(screen.queryByText("Session highlight")).toBeNull();
    expect(rows().sort()).toEqual(["green-one", "plain-one", "red-one"]);
    view.rerender(<Sidebar readOnly />);
    expect(rows()).toEqual(["red-one"]);
    fireEvent.keyDown(screen.getByRole("button", { name: "Filter projects by Sky" }), { key: "Escape" });
    expect(screen.queryByTestId("sidebar-filter-input")).toBeNull();
    expect(rows()).toHaveLength(6);
  });

  it("intersects with plugin facets and keeps those facets when text and colors are cleared", () => {
    pluginEntries.current = [
      {
        plugin_id: "test",
        id: "state",
        slot: "filter-facet",
        session_id: null,
        payload: { label: "State", column: "state-col", options: [{ value: "ready", label: "Ready" }] },
      },
      ...["red-one", "green-one", "red-two"].map((id) => ({
        plugin_id: "test",
        id: "state-col",
        slot: "row-column" as const,
        session_id: id,
        payload: { filter_values: ["ready"] },
      })),
    ];
    render(<Sidebar savedProjects={[repo("saved-sky", "sky")]} />);
    expect(screen.getByTestId("sidebar-project-row").textContent).toContain("saved-sky");
    click("Plugin facet filters");
    fireEvent.click(screen.getByTestId("sidebar-facet-option-state-ready"));
    openFilter();
    click("Filter projects by Sky");
    click(/^Filter sessions by Red/);
    expect(rows()).toEqual(["red-one"]);
    click("Clear filters");
    expect(rows().sort()).toEqual(["green-one", "red-one", "red-two"]);
    expect(screen.queryByTestId("sidebar-project-row")).toBeNull();
    query("saved");
    expect(screen.getByText(/No matches for/)).toBeTruthy();
    fireEvent.click(screen.getByTestId("sidebar-facet-option-state-ready"));
    expect(screen.getByTestId("sidebar-project-row").textContent).toContain("saved-sky");
    expect(screen.queryByText(/No matches for/)).toBeNull();
  });
});

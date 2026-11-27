// Save, clear and failed-save retry stay reachable on a short viewport.

import type { Page } from "@playwright/test";
import { test, expect } from "./helpers/mockedTest";
import { installSidebarMocks } from "./helpers/sidebarMocks";
import { sessionResponse } from "./helpers/sessions";

const ID = "s-1";
const GROUPS = ["work", ...["a", "b", "c", "d", "e", "f", "g"].map((s) => `team-${s}`)];

async function openEditGroup(page: Page) {
  await installSidebarMocks(page, {
    sessions: [{ id: ID, title: "Mongols", project_path: "/tmp/repo", branch: null, group: "work" }],
  });
  await page.route(/\/api\/groups(\?.*)?$/, (r) =>
    r.fulfill({ json: GROUPS.map((path) => ({ path, session_count: 1 })) }),
  );
  const bodies: unknown[] = [];
  await page.route(`**/api/sessions/${ID}/group`, (r) => {
    bodies.push(r.request().postDataJSON());
    return r.fulfill({
      headers: { "aoe-runtime-epoch": "mock-runtime", "aoe-runtime-revision": "2" },
      json: sessionResponse({ id: ID, title: "Mongols", project_path: "/tmp/repo", group_path: "" }),
    });
  });
  await page.setViewportSize({ width: 800, height: 420 });
  await page.goto("/");
  await page.locator("[data-testid='sidebar-session-row']").click({ button: "right" });
  await page.locator("[data-testid='sidebar-context-menu-more']").click();
  await page.locator("[data-testid='sidebar-context-menu-edit-group']").click();
  await expect(page.getByRole("option")).toHaveCount(GROUPS.length + 2);
  return bodies;
}

async function expectInViewport(page: Page, testId: string) {
  const box = await page.getByTestId(testId).boundingBox();
  const viewport = page.viewportSize()!;
  expect(box).not.toBeNull();
  expect(box!.y).toBeGreaterThanOrEqual(0);
  expect(box!.y + box!.height).toBeLessThanOrEqual(viewport.height);
}

test.describe("Edit group dialog on a short viewport", () => {
  test("Save is on screen and clickable with loaded native choices", async ({ page }) => {
    const bodies = await openEditGroup(page);
    await page.getByTestId("session-group-modal-input").fill("team");
    await expect(page.getByRole("option", { name: /team-g/ })).toHaveCount(1);
    await expectInViewport(page, "session-group-modal-save");
    await page.getByTestId("session-group-modal-save").click();
    await expect(page.getByTestId("session-group-modal")).toBeHidden();
    expect(bodies).toEqual([{ group: "team" }]);
  });

  test("No group selection clears the group with a real Save click", async ({ page }) => {
    const bodies = await openEditGroup(page);
    await page.getByRole("combobox", { name: "Existing group" }).selectOption("");
    await expectInViewport(page, "session-group-modal-save");
    await page.getByTestId("session-group-modal-save").click();
    await expect(page.getByTestId("session-group-modal")).toBeHidden();
    expect(bodies).toEqual([{ group: "" }]);
  });
  test("a pending failed save re-enables and refocuses the editor before retry", async ({ page }) => {
    const bodies = await openEditGroup(page);
    let release!: () => void;
    const pending = new Promise<void>((resolve) => {
      release = resolve;
    });
    let first = true;
    await page.route(`**/api/sessions/${ID}/group`, async (route) => {
      if (!first) return route.fallback();
      first = false;
      await pending;
      return route.fulfill({ status: 500, json: { error: "synthetic save failure" } });
    });
    const input = page.getByTestId("session-group-modal-input");
    await input.fill("retained draft");
    await page.getByTestId("session-group-modal-save").click();
    await expect(input).toBeDisabled();
    release();
    await expect(page.getByTestId("session-group-modal-error")).toHaveText("Failed to update group.");
    await expect(input).toBeFocused();
    await expect(input).toHaveValue("retained draft");
    await expectInViewport(page, "session-group-modal-save");
    await page.getByTestId("session-group-modal-save").click();
    await expect(page.getByTestId("session-group-modal")).toBeHidden();
    expect(bodies).toEqual([{ group: "retained draft" }]);
  });
});

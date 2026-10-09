import type { Page } from "@playwright/test";
import { test, expect } from "./helpers/mockedTest";
import { installSidebarMocks } from "./helpers/sidebarMocks";
import { openMobileSidebar } from "./helpers/sidebar";
import { iPhone13 } from "./helpers/viewports";

async function setup(page: Page) {
  await page.addInitScript(() => {
    localStorage.setItem(
      "aoe-repo-appearance-v1",
      JSON.stringify({ "/tmp/one": { color: "sky" }, "/tmp/two": { color: "rose" } }),
    );
  });
  await installSidebarMocks(page, {
    sessions: [
      { id: "red-one", title: "Fix login", project_path: "/tmp/one", branch: "fix/login", fields: { color: "red" } },
      {
        id: "green-one",
        title: "Update docs",
        project_path: "/tmp/one",
        branch: "docs/guide",
        fields: { color: "green" },
      },
      { id: "red-two", title: "Fix tests", project_path: "/tmp/two", branch: "fix/tests", fields: { color: "red" } },
      { id: "plain", title: "Unhighlighted", project_path: "/tmp/two", branch: "main" },
    ],
  });
  await page.goto("/");
}

test("highlight filters support keyboard selection, grouping, compact mode, and Escape", async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await setup(page);
  const rows = page.getByTestId("sidebar-session-row");
  await expect(rows).toHaveCount(4);
  await page.getByRole("button", { name: "Filter sessions", exact: true }).click();
  const input = page.getByTestId("sidebar-filter-input");
  await expect(input).toBeFocused();
  await input.press("Tab");
  await expect(page.getByRole("button", { name: "All sessions highlights" })).toBeFocused();
  await page.keyboard.press("Tab");
  const red = page.getByRole("button", { name: /^Filter sessions by Red/ });
  await expect(red).toBeFocused();
  await page.keyboard.press("Space");
  await expect(red).toHaveAttribute("aria-pressed", "true");
  await expect(rows).toHaveCount(2);
  await page.getByRole("button", { name: "Filter projects by Sky" }).click();
  await expect(rows).toHaveCount(1);
  await expect(rows).toContainText("Fix login");
  for (let i = 0; i < 4; i++) {
    await page.getByTestId("sidebar-axis-toggle").click();
    await expect(rows).toHaveCount(1);
    await expect(rows).toContainText("Fix login");
  }
  await page.getByText("Session highlight", { exact: true }).hover();
  await expect(page.getByRole("tooltip")).toHaveCount(0);
  await page.screenshot({ path: testInfo.outputPath("highlight-filters-desktop.png") });
  await input.fill("tests");
  await expect(page.getByText("No matches for “tests”")).toBeVisible();
  await page.getByRole("button", { name: "Compact sidebar" }).click();
  await expect(rows).toHaveCount(4);
  await page.getByRole("button", { name: "Expand sidebar" }).click();
  await expect(rows).toHaveCount(0);
  await red.focus();
  await page.keyboard.press("Escape");
  await expect(input).toHaveCount(0);
  await expect(rows).toHaveCount(4);
  await page.getByRole("button", { name: "Filter sessions", exact: true }).click();
  await expect(input).toHaveValue("");
  await expect(red).toHaveAttribute("aria-pressed", "false");
});

test.describe("mobile highlight filters", () => {
  test.use(iPhone13);

  test("swatches remain tappable without overflowing the sidebar", async ({ page }, testInfo) => {
    await setup(page);
    await openMobileSidebar(page);
    await page.getByRole("button", { name: "Filter sessions", exact: true }).tap();
    const sidebar = page.locator('[data-tour="sidebar"]');
    const none = sidebar.getByRole("button", { name: "Filter sessions with no highlight" });
    await none.tap();
    await expect(page.getByTestId("sidebar-session-row")).toHaveCount(1);
    await expect(page.getByTestId("sidebar-session-row")).toContainText("Unhighlighted");
    const rose = sidebar.getByRole("button", { name: "Filter projects by Rose" });
    await rose.tap();
    await expect(rose).toHaveAttribute("aria-pressed", "true");
    const box = (await none.boundingBox())!;
    expect(box.width).toBeGreaterThanOrEqual(32);
    expect(box.height).toBeGreaterThanOrEqual(32);
    await expect.poll(() => sidebar.evaluate((el) => el.scrollWidth - el.clientWidth)).toBeLessThanOrEqual(1);
    await page.screenshot({ path: testInfo.outputPath("highlight-filters-mobile.png") });
    await sidebar.getByRole("button", { name: "Clear filters" }).tap();
    await expect(page.getByTestId("sidebar-session-row")).toHaveCount(4);
  });
});

import { test, expect } from "./helpers/mockedTest";
import { installSidebarMocks } from "./helpers/sidebarMocks";

const paths = ["Review/Done", "Review / Done"];

test("existing group paths stay distinct through selection, Save, and keyboard cancellation", async ({ page }) => {
  await installSidebarMocks(page, {
    sessions: [{ id: "s1", title: "group-source", project_path: "/tmp/repo", branch: null, group: "stale" }],
  });
  await page.route("**/api/groups", (route) =>
    route.fulfill({ json: paths.map((path) => ({ path, session_count: 1 })) }),
  );
  const patches: unknown[] = [];
  await page.route("**/api/sessions/s1/group", (route) => {
    patches.push(route.request().postDataJSON());
    return route.fulfill({ json: { id: "s1" } });
  });
  await page.setViewportSize({ width: 1280, height: 720 });
  await page.goto("/");
  const open = async () => {
    await page.getByTestId("sidebar-session-row").click({ button: "right" });
    await page.getByTestId("sidebar-context-menu-more").click();
    await page.getByTestId("sidebar-context-menu-edit-group").click();
    await expect(page.getByLabel("New group or manual path", { exact: true })).toBeFocused();
  };
  const chooser = page.getByRole("combobox", { name: "Existing group" });
  const dialog = page.getByRole("dialog", { name: "Edit group" });
  for (const path of paths) {
    await open();
    for (const label of paths) {
      await expect(chooser.getByRole("option", { name: JSON.stringify(label), exact: true })).toHaveCount(1);
    }
    await chooser.selectOption({ label: JSON.stringify(path) });
    await expect(chooser).toHaveValue(`group:${path}`);
    await chooser.focus();
    await page.keyboard.press("Tab");
    await expect(page.getByLabel("New group or manual path", { exact: true })).toBeFocused();
    await page.keyboard.press("Tab");
    await expect(dialog.getByRole("button", { name: "Cancel" })).toBeFocused();
    await page.keyboard.press("Tab");
    await expect(dialog.getByRole("button", { name: "Save" })).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(dialog).toBeHidden();
    expect(patches.at(-1)).toEqual({ group: path });
  }
  expect(patches).toHaveLength(2);
  await open();
  await chooser.selectOption({ label: JSON.stringify(paths[0]) });
  await chooser.focus();
  await page.keyboard.press("Escape");
  await expect(dialog).toBeHidden();
  expect(patches).toHaveLength(2);
  await open();
  await dialog.getByRole("button", { name: "Save" }).click();
  await expect(dialog).toBeHidden();
  expect(patches).toHaveLength(2);
});

import { test, expect } from "./helpers/mockedTest";
import { installSidebarMocks } from "./helpers/sidebarMocks";

const choices = [
  { path: "Review/Done", label: '"Review/Done"' },
  { path: "Review / Done", label: '"Review / Done"' },
  { path: " a ", label: '" a "' },
  { path: "  a  ", label: String.raw`"\u0020\u0020a\u0020\u0020"` },
  { path: "a b", label: '"a b"' },
  { path: "a  b", label: String.raw`"a\u0020\u0020b"` },
];

test("existing group paths stay distinct through selection, Save, and keyboard cancellation", async ({
  page,
}, testInfo) => {
  await installSidebarMocks(page, {
    sessions: [{ id: "s1", title: "group-source", project_path: "/tmp/repo", branch: null, group: "stale" }],
  });
  await page.route("**/api/groups", (route) =>
    route.fulfill({ json: choices.map(({ path }) => ({ path, session_count: 1 })) }),
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
  for (const { path, label } of choices) {
    await open();
    for (const choice of choices) {
      const option = chooser.getByRole("option", { name: choice.label, exact: true });
      await expect(option).toHaveCount(1);
      expect(await option.evaluate((el) => (el as HTMLOptionElement).label)).toBe(choice.label);
    }
    await chooser.selectOption({ label });
    await expect(chooser).toHaveValue(`group:${path}`);
    if (path === "Review / Done") await page.screenshot({ path: testInfo.outputPath("existing-group-chooser.png") });
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
  expect(patches).toHaveLength(choices.length);
  await open();
  await chooser.selectOption({ label: choices[0]!.label });
  await chooser.focus();
  await page.keyboard.press("Escape");
  await expect(dialog).toBeHidden();
  expect(patches).toHaveLength(choices.length);
  await open();
  await dialog.getByRole("button", { name: "Save" }).click();
  await expect(dialog).toBeHidden();
  expect(patches).toHaveLength(choices.length);
});

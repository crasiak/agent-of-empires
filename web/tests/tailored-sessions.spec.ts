import { test, expect, pressShortcut, waitForResponseBody } from "./helpers/mockedTest";
import { mockTerminalApis } from "./helpers/terminal-mocks";
import { sessionResponse } from "./helpers/sessions";

test("capture overlays sidebar and Session, owns input, and restores the original Session", async ({ page }) => {
  const inputs: { url: string; text: string }[] = [];
  const terminal = await mockTerminalApis(page, {
    onLiveMessage: (url, message) => {
      if (message[0] !== 123) inputs.push({ url, text: message.toString() });
    },
  });
  await page.route("**/api/about", (route) =>
    route.fulfill({ json: { profile: "default", read_only: false, create_boot_id: "boot-test" } }),
  );
  const sessions = [sessionResponse({ id: "pinch-test", group_path: "/tmp", status: "Running", view: "terminal" })];
  let submitted: Record<string, unknown> | null = null;
  await page.route("**/api/sessions", async (route) => {
    if (route.request().method() === "POST") {
      submitted = route.request().postDataJSON();
      const capture = sessionResponse({
        id: "capture-test",
        title: "Capture an idea",
        tool: "pi",
        group_path: "Tailored",
        view: "terminal",
        scratch: true,
      });
      sessions.push(capture);
      await route.fulfill({ status: 201, json: capture });
    } else await route.fulfill({ json: { sessions, workspace_ordering: [] } });
  });
  await page.goto("/session/pinch-test");
  await terminal.waitForLiveReady();
  const originalInput = page.locator('[data-term="agent"] textarea').first();
  await originalInput.focus();
  await pressShortcut(page, "Control+y");
  const overlay = page.getByRole("dialog", { name: /Tailored Sessions/ });
  await expect(overlay).toBeVisible();
  await expect(page.locator("#root")).toHaveJSProperty("inert", true);
  await expect(page.getByLabel("Find a tailored preset")).toBeFocused();
  await page.getByLabel("Find a tailored preset").fill("idea");
  await page.getByLabel("Find a tailored preset").press("Enter");
  await expect(overlay.getByLabel("Idea", { exact: true })).toBeFocused();
  await overlay.getByLabel("Capture account").selectOption("work");
  await overlay.getByLabel("Immediately", { exact: true }).check();
  await overlay.getByLabel("Idea", { exact: true }).fill("Make Atlas ownership clearer");
  await overlay.getByRole("button", { name: "Capture idea", exact: true }).focus();
  await page.keyboard.press("Tab");
  await expect(overlay.getByRole("button", { name: "Close Tailored Sessions" })).toBeFocused();
  await overlay.getByLabel("Idea", { exact: true }).press("Enter");
  const captureInput = overlay.locator('[data-term="agent"] textarea');
  await expect(captureInput).toBeFocused();
  expect(submitted).toMatchObject({
    tailored: { preset: "capture-idea", account: "work", immediately: true, idea: "Make Atlas ownership clearer" },
    scratch: true,
    path: "",
  });
  await expect(page).toHaveURL(/\/session\/pinch-test/);
  await captureInput.press("a");
  await expect.poll(() => inputs.some((input) => input.url.includes("capture-test") && input.text === "a")).toBe(true);
  expect(inputs.some((input) => input.url.includes("pinch-test") && input.text === "a")).toBe(false);
  await pressShortcut(page, "Control+y");
  await expect(overlay).toBeHidden();
  await expect(page.locator("#root")).toHaveJSProperty("inert", false);
  await expect(originalInput).toBeFocused();
  await expect(page).toHaveURL(/\/session\/pinch-test/);
  await originalInput.press("b");
  await expect.poll(() => inputs.some((input) => input.url.includes("pinch-test") && input.text === "b")).toBe(true);
});

test("read-only clients cannot open Tailored Sessions", async ({ page }) => {
  await mockTerminalApis(page);
  await page.route("**/api/about", (route) => route.fulfill({ json: { profile: "default", read_only: true } }));
  await page.goto("/session/pinch-test");
  await waitForResponseBody(page, "/api/about");
  await expect(page.locator('[data-term="agent"]')).toBeVisible();
  await pressShortcut(page, "Control+y");
  await expect(page.getByRole("dialog", { name: /Tailored Sessions/ })).toHaveCount(0);
});

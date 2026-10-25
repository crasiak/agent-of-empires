import { test, expect, pressShortcut, waitForResponseBody, publishedRequests } from "./helpers/mockedTest";
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
  await page.route("**/api/tailored-sessions", (route) => route.fulfill({ json: { entries: [] } }));
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
  await expect(overlay.getByRole("button", { name: "Back to history · F2" })).toBeFocused();
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

test("history reopens an existing capture and shares hide/restore across page reloads", async ({ page }) => {
  await mockTerminalApis(page, { extraSessions: [{ id: "saved-capture", title: "Saved idea" }] });
  const saved = sessionResponse({
    id: "saved-capture",
    title: "Saved idea",
    tool: "pi",
    scratch: true,
    view: "terminal",
    group_path: "Tailored",
  });
  let hidden = false;
  let visibilityWrites = 0;
  await page.route("**/api/tailored-sessions", (route) =>
    route.fulfill({
      json: {
        entries: [
          {
            id: saved.id,
            title: saved.title,
            profile: "default",
            account: "personal",
            input_preview: "Database indexing idea",
            status: "idle",
            created_at: "2026-10-24T12:00:00Z",
            hidden,
            session: saved,
          },
        ],
      },
    }),
  );
  await page.route("**/api/sessions/saved-capture/tailored-history", (route) => {
    expect(route.request().method()).toBe("PATCH");
    hidden = route.request().postDataJSON().hidden;
    visibilityWrites++;
    return route.fulfill({ json: { hidden } });
  });
  await page.goto("/session/pinch-test");
  await expect(page.locator('[data-term="agent"] textarea').first()).toBeAttached();
  await pressShortcut(page, "Control+y");
  const overlay = page.getByRole("dialog", { name: "Tailored Sessions", exact: true });
  await overlay.getByRole("button", { name: "Open Saved idea" }).click();
  const terminal = overlay.locator('[data-term="agent"] textarea');
  await expect(terminal).toBeFocused();
  await expect(page).toHaveURL(/\/session\/pinch-test/);
  await terminal.press("F2");
  await expect(overlay.getByLabel("Find a tailored preset")).toBeFocused();
  await overlay.getByRole("button", { name: "Hide Saved idea from history" }).click();
  await expect(overlay.getByRole("button", { name: "Open Saved idea" })).toHaveCount(0);
  expect(hidden).toBe(true);
  expect(await publishedRequests(page, "/api/sessions", "POST")).toEqual([]);
  expect(await publishedRequests(page, "/api/sessions/saved-capture/stop", "POST")).toEqual([]);
  expect(await publishedRequests(page, "/api/sessions/saved-capture/trash", "POST")).toEqual([]);
  await page.reload();
  await pressShortcut(page, "Control+y");
  await expect(overlay.getByText("No matching captures.")).toBeVisible();
  await overlay.getByRole("button", { name: "Show hidden" }).click();
  await overlay.getByRole("button", { name: "Restore Saved idea in history" }).click();
  await expect(overlay.getByText("No hidden captures.")).toBeVisible();
  await overlay.getByRole("button", { name: "Show recent" }).click();
  await expect(overlay.getByRole("button", { name: "Open Saved idea" })).toBeVisible();
  expect(hidden).toBe(false);
  expect(visibilityWrites).toBe(2);
  await expect(page).toHaveURL(/\/session\/pinch-test/);
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

test("research owns keyboard focus and returns to history without dispatch", async ({ page }) => {
  await mockTerminalApis(page);
  await page.route("**/api/tailored-sessions", (route) => route.fulfill({ json: { entries: [] } }));
  await page.goto("/session/pinch-test");
  const underlying = page.locator('[data-term="agent"] textarea').first();
  await underlying.focus();
  await pressShortcut(page, "Control+y");
  const dialog = page.getByRole("dialog", { name: "Tailored Sessions", exact: true });
  await dialog.getByLabel("Find a tailored preset").fill("research");
  await page.keyboard.press("Enter");
  await expect(dialog.getByLabel("Original quick prompt")).toBeFocused();
  await dialog.getByLabel("Original quick prompt").fill("Compare supplied papers");
  await expect(dialog.getByRole("button", { name: "Develop prompt" })).toBeInViewport();
  const screenshot = test.info().outputPath("research-prompt-form.png");
  await dialog.screenshot({ path: screenshot });
  await test.info().attach("Research prompt form", { path: screenshot, contentType: "image/png" });
  await expect(dialog.getByRole("button", { name: "Refresh existing request" })).toHaveCount(0);
  await dialog.getByRole("button", { name: "Develop prompt" }).focus();
  await page.keyboard.press("Tab");
  await expect(dialog.getByRole("button", { name: "Back to history · F2" })).toBeFocused();
  await page.keyboard.press("F2");
  await expect(dialog.getByLabel("Find a tailored preset")).toBeFocused();
  expect(await publishedRequests(page, "/api/research", "POST")).toEqual([]);
  await page.keyboard.press("Escape");
  await expect(dialog).toBeHidden();
  await expect(underlying).toBeFocused();
});

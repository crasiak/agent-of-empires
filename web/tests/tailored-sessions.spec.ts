import { test, expect, pressShortcut, waitForResponseBody, publishedRequests } from "./helpers/mockedTest";
import { mockTerminalApis } from "./helpers/terminal-mocks";
import { sessionResponse } from "./helpers/sessions";

test("document form preserves long URL through resize, owns focus and requires explicit Start", async ({ page }) => {
  const terminal = await mockTerminalApis(page);
  await page.route("**/api/tailored-sessions", (route) => route.fulfill({ json: { entries: [] } }));
  let starts = 0;
  await page.route("**/api/sessions", async (route) => {
    if (route.request().method() === "POST") {
      starts++;
      await route.fulfill({ status: 400, json: { error: { message: "Direct link required" } } });
    } else await route.fulfill({ json: { sessions: [sessionResponse({ id: "pinch-test" })], workspace_ordering: [] } });
  });
  await page.goto("/session/pinch-test");
  await terminal.waitForLiveReady();
  const original = page.locator('[data-term="agent"] textarea').first();
  await original.focus();
  await pressShortcut(page, "Control+y");
  const search = page.getByLabel("Find a tailored preset");
  await search.fill("document");
  await search.press("Enter");
  const url = page.getByLabel("Public document URL");
  await expect(url).toBeFocused();
  const exact = `https://example.org/${"unbroken".repeat(150)}.md`;
  await url.fill(exact);
  await page.setViewportSize({ width: 520, height: 720 });
  await expect(url).toHaveValue(exact);
  expect(await url.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
  const screenshot = test.info().outputPath("document-conversation-form.png");
  await page.getByRole("dialog", { name: "Tailored Sessions", exact: true }).screenshot({ path: screenshot });
  await test.info().attach("Document conversation form", { path: screenshot, contentType: "image/png" });
  await url.press("Tab");
  await expect(page.getByLabel("Opening question (optional)")).toBeFocused();
  expect(starts).toBe(0);
  await page.getByRole("button", { name: "Start", exact: true }).focus();
  await page.keyboard.press("Tab");
  await expect(page.getByRole("button", { name: "Back to history · F2" })).toBeFocused();
  await url.press("F2");
  await expect(search).toBeFocused();
  await search.press("Escape");
  await expect(original).toBeFocused();
  expect(starts).toBe(0);
});

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
  await expect(overlay.locator("[data-editor]:visible").getByLabel("Idea", { exact: true })).toBeFocused();
  await overlay.locator("[data-editor]:visible").getByLabel("Capture account").selectOption("work");
  await overlay.locator("[data-editor]:visible").getByLabel("Immediately", { exact: true }).check();
  await overlay
    .locator("[data-editor]:visible")
    .getByLabel("Idea", { exact: true })
    .fill("Make Atlas ownership clearer");
  await overlay.getByRole("button", { name: "Capture idea", exact: true }).focus();
  await page.keyboard.press("Tab");
  await expect(overlay.getByRole("button", { name: "Back to history · F2" })).toBeFocused();
  await overlay.locator("[data-editor]:visible").getByLabel("Idea", { exact: true }).press("Control+Enter");
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

test("workshop keeps account-scoped edits through resize and returns focus without host effects", async ({ page }) => {
  await mockTerminalApis(page);
  await page.route("**/api/tailored-sessions", (route) => route.fulfill({ json: { entries: [] } }));
  await page.route("**/api/workshop-defaults", (route) => route.fulfill({ json: { repository: "/tmp/skills" } }));
  await page.goto("/session/pinch-test");
  const underlying = page.locator('[data-term="agent"] textarea').first();
  await underlying.focus();
  await pressShortcut(page, "Control+y");
  const dialog = page.getByRole("dialog", {
    name: "Tailored Sessions",
    exact: true,
  });
  const search = dialog.getByLabel("Find a tailored preset");
  await search.fill("workshop");
  await search.press("Enter");
  await expect(dialog.locator("[data-editor]:visible").getByLabel("Source repository")).toHaveValue("/tmp/skills");
  await dialog.locator("[data-editor]:visible").getByLabel("Workshop account").selectOption("work");
  await dialog.locator("[data-editor]:visible").getByLabel("Skill slug").fill("review-evidence");
  await dialog
    .locator("[data-editor]:visible")
    .getByLabel("Account/source suitability")
    .fill("Work-only source repository");
  const idea = dialog.locator("[data-editor]:visible").getByLabel("Skill idea", { exact: true });
  const exactIdea = "Check evidence and preserve account boundaries. ".repeat(20);
  await idea.fill(exactIdea);
  await page.setViewportSize({ width: 520, height: 720 });
  await expect(idea).toHaveValue(exactIdea);
  expect(await idea.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
  await expect(dialog.locator("[data-editor]:visible").getByLabel("Workshop account")).toHaveValue("work");
  const screenshot = test.info().outputPath("skill-workshop-form.png");
  await dialog.screenshot({ path: screenshot });
  await test.info().attach("Skill Workshop form", {
    path: screenshot,
    contentType: "image/png",
  });
  await dialog.getByRole("button", { name: "Reopen same request", exact: true }).focus();
  await page.keyboard.press("Tab");
  await expect(dialog.getByRole("button", { name: "Back to history · F2" })).toBeFocused();
  await page.keyboard.press("F2");
  await expect(search).toBeFocused();
  expect(await publishedRequests(page, "/api/workshop", "POST")).toEqual([]);
  expect(await publishedRequests(page, "/api/sessions", "POST")).toEqual([]);
  await page.keyboard.press("Escape");
  await expect(dialog).toBeHidden();
  await expect(underlying).toBeFocused();
});

test("native multiline paste, undo, composition and retained focus never write accidentally", async ({
  page,
  context,
}) => {
  const sent: string[] = [];
  const terminal = await mockTerminalApis(page, {
    onLiveMessage: (_url, message) => {
      if (message[0] !== 123) sent.push(message.toString());
    },
  });
  await page.route("**/api/tailored-sessions", (route) => route.fulfill({ json: { entries: [] } }));
  let writes = 0;
  await page.route("**/api/sessions", (route) => {
    if (route.request().method() === "POST") {
      writes++;
      return route.fulfill({ status: 400, json: { error: "fixture_refusal", message: "fixture refuses writes" } });
    }
    return route.fulfill({ json: { sessions: [sessionResponse({ id: "pinch-test" })], workspace_ordering: [] } });
  });
  await page.goto("/session/pinch-test");
  await terminal.waitForLiveReady();
  await pressShortcut(page, "Control+y");
  const dialog = page.getByRole("dialog", { name: "Tailored Sessions", exact: true });
  await dialog.getByRole("button", { name: /Capture an idea/ }).click();
  const idea = dialog.getByRole("textbox", { name: "Idea", exact: true });
  await expect(idea).toBeFocused();
  await idea.fill("leftRIGHT");
  await idea.evaluate((element: HTMLTextAreaElement) => element.setSelectionRange(4, 4));
  const paste = `🌱日本語${"long".repeat(1000)}\n\nlast\n`;
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await page.evaluate((value) => navigator.clipboard.writeText(value), paste);
  const command = process.platform === "darwin" ? "Meta" : "Control";
  await idea.press(`${command}+v`);
  await expect(idea).toHaveValue(`left${paste}RIGHT`);
  await idea.press(`${command}+z`);
  await expect(idea).toHaveValue("leftRIGHT");
  await idea.press(`${command}+Shift+z`);
  await expect(idea).toHaveValue(`left${paste}RIGHT`);
  await idea.press("Enter");
  await expect(idea).toHaveValue(`left${paste}\nRIGHT`);
  await idea.press("Control+y");
  await expect(dialog).toBeVisible();
  await idea.dispatchEvent("compositionstart", { data: "あ" });
  for (const key of ["Escape", "F2", "Enter", "y"]) {
    await idea.dispatchEvent("keydown", {
      key,
      ctrlKey: key === "Enter" || key === "y",
      isComposing: true,
      bubbles: true,
    });
    await idea.dispatchEvent("keydown", { key, keyCode: 229, ctrlKey: key === "Enter", bubbles: true });
  }
  await idea.dispatchEvent("compositionend", { data: "あ" });
  await expect(idea).toBeFocused();
  await expect(dialog).toBeVisible();
  await idea.dispatchEvent("keydown", { key: "Enter", ctrlKey: true, repeat: true, bubbles: true });
  await idea.press("F2");
  await dialog.getByRole("button", { name: /Capture an idea/ }).click();
  await expect(idea).toHaveValue(`left${paste}\nRIGHT`);
  await expect(idea).toBeFocused();
  await idea.press("Escape");
  await expect(dialog).toBeHidden();
  await pressShortcut(page, "Control+y");
  await expect(idea).toBeFocused();
  await idea.press(`${command}+z`);
  await expect(idea).toHaveValue(`left${paste}RIGHT`);
  const screenshot = test.info().outputPath("tailored-retained-multiline-input.png");
  await dialog.screenshot({ path: screenshot });
  await test
    .info()
    .attach("Retained multiline editor and deliberate submit hint", { path: screenshot, contentType: "image/png" });
  expect(writes).toBe(0);
  expect(sent).toEqual([]);
  await idea.press("Control+Enter");
  await expect(dialog.getByRole("alert")).toHaveText("fixture refuses writes");
  expect(writes).toBe(1);
});

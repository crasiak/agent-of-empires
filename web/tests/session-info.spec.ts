import { test, expect } from "./helpers/mockedTest";
import { mockTerminalApis } from "./helpers/terminal-mocks";

for (const viewport of [
  { width: 1440, height: 900 },
  { width: 390, height: 844 },
  { width: 320, height: 640 },
]) {
  test(`session info stays in layout above terminal at ${viewport.width}px`, async ({ page }, testInfo) => {
    await page.setViewportSize(viewport);
    const terminal = await mockTerminalApis(page, { sessionFields: { title: "Selected agent" } });
    await page.route("**/api/system/health", (route) =>
      route.fulfill({
        json: {
          status: "ok",
          cpu_fraction: 0.98,
          memory_used_bytes: 24 * 1024 ** 3,
          memory_total_bytes: 32 * 1024 ** 3,
          load_average: null,
          swap_used_bytes: 0,
          swap_total_bytes: 0,
          agent_count: 2,
          proc_count: 8,
          agents: [
            {
              id: "pinch-test",
              title: "Selected agent",
              cpu_fraction: 0.12,
              memory_bytes: 384 * 1024 ** 2,
              procs: 3,
              sandboxed: false,
            },
            {
              id: "other",
              title: "Other",
              cpu_fraction: 0.86,
              memory_bytes: 2 * 1024 ** 3,
              procs: 5,
              sandboxed: false,
            },
          ],
        },
      }),
    );
    await page.route("**/api/sessions/*/usage", (route) =>
      route.fulfill({
        json: {
          resets: 7,
          clears: 5,
          compactions: 2,
          compactionsAuto: 1,
          compactionsManual: 1,
          resumes: 0,
          prompts: 20,
          turns: 18,
          turnErrors: 0,
          contextStartedAt: new Date().toISOString(),
          contextPrompts: 4,
          contextTurns: 3,
          trackedSince: new Date().toISOString(),
          lastEventAt: new Date().toISOString(),
          tracked: true,
        },
      }),
    );
    await page.route("**/api/sessions/*/ledger-run", (route) =>
      route.fulfill({
        json: {
          run_id: "run-1",
          runs: 1,
          error: null,
          generation: {
            state: "current",
            launched: "d861a7c0e5f1",
            current: "d861a7c0e5f1",
            current_since: new Date().toISOString(),
            drift: [],
            runtime_drift: [],
          },
          headroom_total: {
            requests: 105,
            input_tokens_before: 1_100_000,
            input_tokens_after: 773_500,
            saved_tokens: 326_500,
            model: "claude",
            estimated_cents: 100,
          },
          headroom_now: null,
        },
      }),
    );
    await page.goto("/session/pinch-test");
    await terminal.waitForLiveReady();
    const info = page.getByTestId("session-info");
    await expect(info).toHaveCount(1);
    await expect(info.getByTestId("session-cpu-value")).toHaveText("12%");
    await expect(info.getByTestId("session-memory-value")).toHaveText("384M");
    await expect(info.getByTestId("session-usage")).toContainText("7 resets");
    await expect(info.getByTestId("ledger-drift-line")).toHaveText("ledger d861 current");
    await expect(info.getByTestId("ledger-headroom-line")).toContainText("326.5k saved");
    const bounds = await info.boundingBox();
    const terminalBounds = await page.locator("[data-live-terminal]").first().boundingBox();
    expect(bounds).not.toBeNull();
    expect(terminalBounds).not.toBeNull();
    expect(bounds!.y + bounds!.height).toBeLessThanOrEqual(terminalBounds!.y);
    expect(await info.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
    expect(await info.getByTestId("session-usage").evaluate((element) => getComputedStyle(element).position)).toBe(
      "static",
    );
    expect(await info.evaluate((element) => element.scrollHeight <= element.clientHeight)).toBe(true);
    await page.addStyleTag({
      content: "[data-live-debug], div:has(> [data-input-trace]) { display: none !important; }",
    });
    await page.screenshot({ path: testInfo.outputPath(`session-info-${viewport.width}.png`) });
  });
}

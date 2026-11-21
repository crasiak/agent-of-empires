import { test, expect } from "./helpers/mockedTest";
import { mockAcpSession, openStructuredSession, stopped } from "./helpers/acpMock";

const call = (id: string, command: string) => ({
  ToolCallStarted: {
    tool_call: {
      id,
      name: "Bash",
      kind: "execute",
      args_preview: JSON.stringify({ command }),
      started_at: new Date().toISOString(),
    },
  },
});

const done = (id: string, isError: boolean, content: string) => ({
  ToolCallCompleted: { tool_call_id: id, is_error: isError, content },
});

// CRA-1813: one control folds every tool card, failures included, and unfolds them all.
test("fold all and unfold all override every card while single cards still toggle", async ({ page }) => {
  const mock = await mockAcpSession(page, {
    title: "story-tool-fold-all",
    initialEvents: [
      call("ok-1", "ls -la"),
      done("ok-1", false, "listing-output"),
      call("err-1", "make build"),
      done("err-1", true, "build-error"),
      stopped(),
    ],
  });
  await openStructuredSession(page, mock);

  const transcript = page.getByTestId("acp-viewport");
  const okOutput = transcript.getByText("listing-output");
  const errOutput = transcript.getByText("build-error");
  await expect(errOutput.first()).toBeVisible();
  await expect(okOutput).toHaveCount(0);

  await page.getByRole("button", { name: "Fold all" }).click();
  await expect(errOutput).toHaveCount(0);
  await expect(okOutput).toHaveCount(0);

  await transcript.getByRole("button").filter({ hasText: "make build" }).first().click();
  await expect(errOutput.first()).toBeVisible();

  await page.getByRole("button", { name: "Unfold all" }).click();
  await expect(okOutput.first()).toBeVisible();
  await expect(errOutput.first()).toBeVisible();

  await transcript.getByRole("button").filter({ hasText: "ls -la" }).first().click();
  await expect(okOutput).toHaveCount(0);

  // A density change hands control back to the density baseline.
  await page.getByRole("button", { name: /compact tools/i }).click();
  await expect(page.getByRole("button", { name: "Fold all" })).toBeVisible();
  await expect(errOutput.first()).toBeVisible();
  await expect(okOutput).toHaveCount(0);
});

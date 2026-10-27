// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { ResearchForm } from "../ResearchForm";
import { getResearch, getResearchOutput, replyResearch, researchOperation } from "../../lib/api";
import type { ResearchDraft } from "../../lib/types";

vi.mock("../../lib/api", () => ({
  researchOperation: vi.fn(),
  getResearch: vi.fn(),
  getResearchOutput: vi.fn(),
  replyResearch: vi.fn(),
}));
beforeEach(() => {
  vi.resetAllMocks();
});
afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});
const draft: ResearchDraft = {
  id: "request-1",
  original_prompt: "Original question",
  revision: 2,
  state: "review",
  hidden: false,
  created_at: "2026-10-25T12:00:00Z",
  recommendation: "orx",
  reason: "Literature review",
  unknowns: [],
  inputs: {
    account: "personal",
    context: "",
    sources: [],
    developed_prompt: "Developed question",
    backend: null,
    workspace: "",
    permitted_scope: "",
    evaluation: "",
    unknowns_resolution: "",
    public_disclosure: "No private context in public queries",
    consequences_reviewed: false,
  },
};
it("requires backend choice and review, saves edits, then explicitly runs the returned revision", async () => {
  vi.mocked(researchOperation).mockImplementation(async (body) => ({
    draft: { ...draft, revision: 3, inputs: { ...draft.inputs, ...body.inputs } },
  }));
  render(<ResearchForm account="personal" readOnly={false} initial={draft} />);
  expect((screen.getByRole("button", { name: "Run reviewed revision" }) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.change(screen.getByLabelText("Backend"), { target: { value: "evo" } });
  fireEvent.change(screen.getByLabelText("Developed prompt"), { target: { value: "Edited question" } });
  fireEvent.change(screen.getByLabelText("Workspace (absolute path)"), { target: { value: "/tmp/target" } });
  fireEvent.change(screen.getByLabelText("Permitted scope"), { target: { value: "Baseline only" } });
  fireEvent.change(screen.getByLabelText("Evaluation criteria"), { target: { value: "Score" } });
  fireEvent.click(screen.getByLabelText(/I reviewed/));
  expect(researchOperation).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Run reviewed revision" }));
  await vi.waitFor(() => expect(researchOperation).toHaveBeenCalledTimes(2));
  expect(vi.mocked(researchOperation).mock.calls[0]?.[0]).toMatchObject({
    operation: "edit",
    id: "request-1",
    revision: 2,
    inputs: { developed_prompt: "Edited question", backend: "evo" },
  });
  expect(vi.mocked(researchOperation).mock.calls[1]?.[0]).toEqual({ operation: "run", id: "request-1", revision: 3 });
});
it("does not offer resubmission after an ambiguous run and gates read-only edits", () => {
  render(
    <ResearchForm
      account="personal"
      readOnly={true}
      initial={{ ...draft, state: "uncertain", error: "Inspect the backend before doing anything else" }}
    />,
  );
  expect(screen.queryByRole("button", { name: "Run reviewed revision" })).toBeNull();
  expect(screen.getByRole("alert").textContent).toContain("Inspect the backend");
  expect(researchOperation).not.toHaveBeenCalled();
});

it("develops from a saved original and scoped supplied context without running", async () => {
  vi.mocked(researchOperation).mockImplementation(async (body) => ({
    draft: {
      ...draft,
      id: body.id,
      original_prompt: body.original_prompt ?? "Quick work question",
      revision: body.operation === "create" ? 0 : 1,
      state: body.operation === "create" ? "draft" : "review",
      inputs: { ...draft.inputs, ...body.inputs },
    },
  }));
  render(<ResearchForm account="work" readOnly={false} />);
  fireEvent.change(screen.getByLabelText("Original quick prompt"), { target: { value: "Quick work question" } });
  fireEvent.change(screen.getByLabelText("Explicit context"), { target: { value: "Only these excerpts" } });
  fireEvent.click(screen.getByRole("button", { name: "Add source boundary" }));
  fireEvent.change(screen.getByLabelText("Source 1 category"), { target: { value: "slack" } });
  fireEvent.change(screen.getByLabelText("Source 1 scope"), { target: { value: "Workspace A, channel B" } });
  fireEvent.click(screen.getByRole("button", { name: "Develop prompt" }));
  await vi.waitFor(() => expect(researchOperation).toHaveBeenCalledTimes(2));
  expect(vi.mocked(researchOperation).mock.calls[0]?.[0]).toMatchObject({
    operation: "create",
    original_prompt: "Quick work question",
    inputs: {
      account: "work",
      context: "Only these excerpts",
      sources: [{ category: "slack", scope: "Workspace A, channel B" }],
    },
  });
  expect(vi.mocked(researchOperation).mock.calls[1]?.[0]).toMatchObject({ operation: "develop", revision: 0 });
  expect((screen.getByLabelText("Original quick prompt") as HTMLTextAreaElement).disabled).toBe(true);
});
it("saves without development and retains the original request identity across edits", async () => {
  vi.mocked(researchOperation).mockImplementation(async (body) => ({
    draft: { ...draft, inputs: body.inputs ?? draft.inputs },
  }));
  render(<ResearchForm account="personal" readOnly={false} initial={draft} />);
  fireEvent.click(screen.getByRole("button", { name: "Add source boundary" }));
  fireEvent.change(screen.getByLabelText("Source 1 scope"), { target: { value: "Supplied only" } });
  fireEvent.click(screen.getByRole("button", { name: "Remove" }));
  fireEvent.change(screen.getByLabelText("Research account"), { target: { value: "work" } });
  fireEvent.change(screen.getByLabelText("Public disclosure limits"), { target: { value: "Never publish" } });
  fireEvent.change(screen.getByLabelText("Unknowns resolution"), { target: { value: "Use attached target" } });
  fireEvent.click(screen.getByRole("button", { name: "Save draft" }));
  await vi.waitFor(() => expect(researchOperation).toHaveBeenCalledTimes(1));
  expect(vi.mocked(researchOperation).mock.calls[0]?.[0]).toMatchObject({
    operation: "edit",
    id: draft.id,
    revision: 2,
    inputs: {
      account: "work",
      sources: [],
      public_disclosure: "Never publish",
      unknowns_resolution: "Use attached target",
    },
  });
});
it("shows consequential unknowns and invalidates acknowledgement after editing", () => {
  render(
    <ResearchForm
      account="personal"
      readOnly={false}
      initial={{
        ...draft,
        unknowns: ["Which repository?"],
        inputs: { ...draft.inputs, backend: "evo", consequences_reviewed: true },
      }}
    />,
  );
  expect(screen.getByText("Which repository?")).toBeTruthy();
  fireEvent.change(screen.getByLabelText("Developed prompt"), { target: { value: "Edited again" } });
  expect((screen.getByLabelText(/I reviewed/) as HTMLInputElement).checked).toBe(false);
  expect((screen.getByRole("button", { name: "Run reviewed revision" }) as HTMLButtonElement).disabled).toBe(true);
});
it("does not develop after a rejected save and displays the server error", async () => {
  vi.mocked(researchOperation).mockResolvedValue({ error: "Revision changed; reopen" });
  render(<ResearchForm account="personal" readOnly={false} initial={draft} />);
  fireEvent.click(screen.getByRole("button", { name: "Develop prompt" }));
  expect((await screen.findByRole("alert")).textContent).toContain("Revision changed");
  expect(researchOperation).toHaveBeenCalledTimes(1);
});
it("blocks ambiguous run resubmission and refreshes the exact existing identity", async () => {
  vi.mocked(researchOperation)
    .mockResolvedValueOnce({ draft: { ...draft, revision: 3 } })
    .mockResolvedValueOnce({ error: "Outcome not confirmed", uncertain: true });
  vi.mocked(getResearch).mockResolvedValue({ draft: { ...draft, state: "running", revision: 4 } });
  render(
    <ResearchForm
      account="personal"
      readOnly={false}
      initial={{ ...draft, inputs: { ...draft.inputs, backend: "orx", consequences_reviewed: true } }}
    />,
  );
  fireEvent.click(screen.getByRole("button", { name: "Run reviewed revision" }));
  await screen.findByRole("alert");
  expect((screen.getByRole("button", { name: "Run reviewed revision" }) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(screen.getByRole("button", { name: "Refresh existing request" }));
  await vi.waitFor(() => expect(screen.queryByRole("button", { name: "Run reviewed revision" })).toBeNull());
  expect(getResearch).toHaveBeenCalledWith(draft.id);
  expect(researchOperation).toHaveBeenCalledTimes(2);
});
it("prevents overlapping operations and treats failed refresh as uncertain inspection", async () => {
  let resolve!: (result: { error: string; uncertain: boolean }) => void;
  vi.mocked(researchOperation).mockReturnValue(
    new Promise((done) => {
      resolve = done;
    }),
  );
  vi.mocked(getResearch).mockResolvedValue(null);
  render(<ResearchForm account="personal" readOnly={false} initial={draft} />);
  fireEvent.click(screen.getByRole("button", { name: "Save draft" }));
  fireEvent.click(screen.getAllByRole("button", { name: "Working…" })[0]!);
  expect(researchOperation).toHaveBeenCalledTimes(1);
  await act(async () => {
    resolve({ error: "Lost response", uncertain: true });
  });
  fireEvent.click(screen.getByRole("button", { name: "Refresh existing request" }));
  expect((await screen.findByRole("alert")).textContent).toContain("Request unavailable");
});
it("reopens owned discovery output and sends only an explicit literal answer", async () => {
  vi.mocked(getResearchOutput).mockResolvedValue({ output: "Which evaluation target?" });
  vi.mocked(replyResearch)
    .mockResolvedValueOnce({ ok: true })
    .mockResolvedValueOnce({ ok: false, message: "Optimization requires separate review" });
  render(
    <ResearchForm
      account="personal"
      readOnly={false}
      initial={{
        ...draft,
        state: "submitted",
        inputs: { ...draft.inputs, backend: "evo" },
        receipt: { profile: "evo", backend_id: draft.id, process_id: 123, return_path: "tmux:owned" },
      }}
    />,
  );
  await screen.findByText("Which evaluation target?");
  const answer = screen.getByLabelText("Literal discovery answer");
  fireEvent.change(answer, { target: { value: "Use benchmark A; $(not shell)" } });
  expect(replyResearch).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Send discovery answer" }));
  await vi.waitFor(() => expect((answer as HTMLTextAreaElement).value).toBe(""));
  expect(replyResearch).toHaveBeenCalledWith(draft.id, "Use benchmark A; $(not shell)");
  fireEvent.change(answer, { target: { value: "/evo:optimize" } });
  fireEvent.click(screen.getByRole("button", { name: "Send discovery answer" }));
  expect((await screen.findByRole("alert")).textContent).toContain("separate review");
});
it("shows missing owned pane without restarting it and keeps read-only replies disabled", async () => {
  vi.mocked(getResearchOutput).mockResolvedValue(null);
  render(
    <ResearchForm
      account="personal"
      readOnly={true}
      initial={{
        ...draft,
        state: "submitted",
        inputs: { ...draft.inputs, backend: "evo" },
        receipt: { profile: "evo", backend_id: draft.id, process_id: 123, return_path: "tmux:owned" },
      }}
    />,
  );
  await screen.findByText(/Owned discovery pane stopped or unavailable/);
  expect((screen.getByLabelText("Literal discovery answer") as HTMLTextAreaElement).disabled).toBe(true);
  expect(replyResearch).not.toHaveBeenCalled();
  expect(researchOperation).not.toHaveBeenCalled();
});

it("explicitly abandons interrupted development and retains input without running", async () => {
  vi.mocked(researchOperation).mockResolvedValue({ draft: { ...draft, state: "draft", revision: 4 } });
  render(<ResearchForm account="personal" readOnly={false} initial={{ ...draft, state: "developing", revision: 3 }} />);
  fireEvent.click(screen.getByRole("button", { name: "Abandon interrupted development" }));
  await vi.waitFor(() =>
    expect(researchOperation).toHaveBeenCalledWith({ operation: "recover-development", id: draft.id, revision: 3 }),
  );
  expect((screen.getByLabelText("Original quick prompt") as HTMLTextAreaElement).value).toBe(draft.original_prompt);
  expect(researchOperation).toHaveBeenCalledTimes(1);
});

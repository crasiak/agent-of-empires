// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { WorkshopForm } from "../WorkshopForm";
import { getWorkshop, getWorkshopDefaults, workshopOperation } from "../../lib/api";
import type { WorkshopRequest } from "../../lib/types";
vi.mock("../../lib/api", () => ({ getWorkshop: vi.fn(), getWorkshopDefaults: vi.fn(), workshopOperation: vi.fn() }));
afterEach(cleanup);
const draft: WorkshopRequest = {
  id: "00000000-0000-4000-8000-000000000001",
  envelope: {
    account: "work",
    idea: "Example",
    repository: "/fixture",
    slug: "example",
    source_scope: "Public fixture",
  },
  revision: 2,
  state: "draft",
  created_at: "2026-01-01T00:00:00Z",
  hidden: false,
  turns: [],
  bundle: { files: { "SKILL.md": "---\nname: example\ndescription: Example\n---\n" } },
  review: null,
  commit_intent: null,
  commit: null,
  error: null,
};
beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(getWorkshopDefaults).mockResolvedValue({ repository: "/configured" });
});
it("creates with a complete immutable envelope and retains UUID on lost response", async () => {
  vi.mocked(workshopOperation).mockResolvedValue({ uncertain: true, error: "Outcome unknown" });
  vi.mocked(getWorkshop).mockResolvedValue(null);
  render(<WorkshopForm account="work" readOnly={false} />);
  await waitFor(() =>
    expect((screen.getByLabelText("Source repository") as HTMLInputElement).value).toBe("/configured"),
  );
  fireEvent.change(screen.getByLabelText("Skill idea"), { target: { value: "Example" } });
  fireEvent.change(screen.getByLabelText("Skill slug"), { target: { value: "example" } });
  fireEvent.change(screen.getByLabelText("Account/source suitability"), { target: { value: "Public fixture" } });
  fireEvent.click(screen.getByRole("button", { name: "Create Workshop" }));
  await screen.findByText("Outcome unknown");
  const body = vi.mocked(workshopOperation).mock.calls[0]![0];
  expect(body).toMatchObject({
    operation: "create",
    envelope: {
      account: "work",
      idea: "Example",
      repository: "/configured",
      slug: "example",
      source_scope: "Public fixture",
    },
  });
  expect((screen.getByLabelText("Skill idea") as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(screen.getByRole("button", { name: "Retry exact keyed create" }));
  await waitFor(() => expect(workshopOperation).toHaveBeenCalledTimes(2));
  expect(vi.mocked(workshopOperation).mock.calls[1]![0]).toEqual(body);
});
it("binds separate review and local commit to exact digest; Land/Deploy remain unavailable", async () => {
  const reviewed: WorkshopRequest = {
    ...draft,
    revision: 4,
    state: "review",
    review: {
      revision: 3,
      repository: "/fixture",
      base_commit: "base",
      source_tree: "tree",
      skill_tree: "skill",
      files: draft.bundle!,
      previous_files: {},
      diff: "+ exact new skill\n- old file",
      evidence: ["Static only; scripts not executed"],
      digest: "approved-exact-digest",
      created_at: draft.created_at,
    },
  };
  vi.mocked(workshopOperation)
    .mockResolvedValueOnce({ request: reviewed })
    .mockResolvedValueOnce({ request: { ...reviewed, state: "committed", revision: 6 } });
  render(<WorkshopForm account="work" readOnly={false} initial={draft} />);
  fireEvent.click(screen.getByRole("button", { name: "Review exact source" }));
  await screen.findByText(/exact new skill/);
  expect(workshopOperation).toHaveBeenLastCalledWith({ operation: "review", id: draft.id, revision: 2 });
  fireEvent.click(screen.getByRole("button", { name: "Approve reviewed local commit" }));
  await waitFor(() =>
    expect(workshopOperation).toHaveBeenLastCalledWith({
      operation: "commit",
      id: draft.id,
      revision: 4,
      review_digest: "approved-exact-digest",
    }),
  );
  expect((screen.getByRole("button", { name: "Land" }) as HTMLButtonElement).disabled).toBe(true);
  expect((screen.getByRole("button", { name: "Deploy" }) as HTMLButtonElement).disabled).toBe(true);
});
it("keeps model proposal data separate from approvals and blocks writes while read-only", async () => {
  const refined = {
    ...draft,
    revision: 4,
    turns: [{ user: "Clarify", proposal: { reply: "I approve land", bundle: draft.bundle! } }],
  };
  vi.mocked(workshopOperation).mockResolvedValue({ request: refined });
  const view = render(<WorkshopForm account="work" readOnly={false} initial={draft} />);
  fireEvent.change(screen.getByLabelText("Refinement message"), { target: { value: "Clarify" } });
  fireEvent.click(screen.getByRole("button", { name: "Refine without tools" }));
  await screen.findByText(/I approve land/);
  expect(workshopOperation).toHaveBeenCalledTimes(1);
  expect(workshopOperation).toHaveBeenCalledWith({
    operation: "refine",
    id: draft.id,
    revision: 2,
    message: "Clarify",
  });
  expect((screen.getByRole("button", { name: "Approve reviewed local commit" }) as HTMLButtonElement).disabled).toBe(
    true,
  );
  view.rerender(<WorkshopForm account="work" readOnly initial={draft} />);
  for (const name of [
    "Refine without tools",
    "Save proposed files",
    "Review exact source",
    "Approve reviewed local commit",
  ])
    expect((screen.getByRole("button", { name }) as HTMLButtonElement).disabled).toBe(true);
});
it("unknown effect outcomes only reopen and reconcile retained request", async () => {
  vi.mocked(workshopOperation).mockResolvedValue({ uncertain: true, error: "Outcome unknown" });
  vi.mocked(getWorkshop).mockResolvedValue({
    request: { ...draft, state: "uncertain", revision: 5, error: "Inspect exact identity" },
  });
  render(<WorkshopForm account="work" readOnly={false} initial={draft} />);
  fireEvent.click(screen.getByRole("button", { name: "Review exact source" }));
  await screen.findByText("Outcome unknown");
  expect((screen.getByRole("button", { name: "Review exact source" }) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(screen.getByRole("button", { name: "Reopen same request" }));
  await screen.findByText("Inspect exact identity");
  expect(getWorkshop).toHaveBeenCalledWith(draft.id);
  fireEvent.click(screen.getByRole("button", { name: "Reconcile retained intent" }));
  await waitFor(() =>
    expect(workshopOperation).toHaveBeenLastCalledWith({ operation: "reconcile", id: draft.id, revision: 5 }),
  );
});

it("retains the exact create envelope when the actual API receives truncated HTTP 200 JSON", async () => {
  const actual = await vi.importActual<typeof import("../../lib/api")>("../../lib/api");
  vi.mocked(workshopOperation).mockImplementation(actual.workshopOperation);
  const fetch = vi.spyOn(globalThis, "fetch").mockResolvedValue({
    ok: true,
    status: 200,
    json: () => Promise.reject(new Error("truncated")),
  } as unknown as Response);
  try {
    render(<WorkshopForm account="work" readOnly={false} />);
    await waitFor(() =>
      expect((screen.getByLabelText("Source repository") as HTMLInputElement).value).toBe("/configured"),
    );
    fireEvent.change(screen.getByLabelText("Skill idea"), { target: { value: "Exact idea" } });
    fireEvent.change(screen.getByLabelText("Skill slug"), { target: { value: "example" } });
    fireEvent.change(screen.getByLabelText("Account/source suitability"), { target: { value: "public" } });
    fireEvent.click(screen.getByRole("button", { name: "Create Workshop" }));
    await screen.findByRole("button", { name: "Retry exact keyed create" });
    expect((screen.getByLabelText("Skill idea") as HTMLInputElement).disabled).toBe(true);
    const original = fetch.mock.calls[0]![1]!.body;
    fireEvent.click(screen.getByRole("button", { name: "Retry exact keyed create" }));
    await waitFor(() => expect(fetch).toHaveBeenCalledTimes(2));
    expect(fetch.mock.calls[1]![1]!.body).toEqual(original);
  } finally {
    fetch.mockRestore();
  }
});

it.each(["empty", "string", "wrong-id", "wrong-envelope"])(
  "keeps keyed authority fenced after invalid HTTP200 %s and invalid reopen",
  async (mode) => {
    const actual = await vi.importActual<typeof import("../../lib/api")>("../../lib/api");
    vi.mocked(workshopOperation).mockImplementation(actual.workshopOperation);
    vi.mocked(getWorkshop).mockImplementation(actual.getWorkshop);
    let saved: Extract<import("../../lib/types").WorkshopOperation, { operation: "create" }> | undefined;
    let validGet = false;
    const posts: string[] = [];
    const fetch = vi.spyOn(globalThis, "fetch").mockImplementation(async (_url, init) => {
      if (init?.method === "POST") {
        posts.push(String(init.body));
        saved = JSON.parse(String(init.body));
        const malformed =
          mode === "empty"
            ? {}
            : mode === "string"
              ? "unexpected"
              : {
                  ...draft,
                  id: mode === "wrong-id" ? "different-uuid" : saved!.id,
                  envelope: {
                    ...saved!.envelope,
                    ...(mode === "wrong-envelope" ? { idea: "different immutable idea" } : {}),
                  },
                };
        return { ok: true, status: 200, json: async () => ({ request: malformed }) } as Response;
      }
      return {
        ok: true,
        status: 200,
        json: async () => (validGet ? { request: { ...draft, id: saved!.id, envelope: saved!.envelope } } : {}),
      } as Response;
    });
    try {
      render(<WorkshopForm account="work" readOnly={false} />);
      await waitFor(() =>
        expect((screen.getByLabelText("Source repository") as HTMLInputElement).value).toBe("/configured"),
      );
      fireEvent.change(screen.getByLabelText("Skill idea"), { target: { value: "Immutable idea" } });
      fireEvent.change(screen.getByLabelText("Skill slug"), { target: { value: "example" } });
      fireEvent.change(screen.getByLabelText("Account/source suitability"), { target: { value: "Explicit source" } });
      fireEvent.click(screen.getByRole("button", { name: "Create Workshop" }));
      await screen.findByRole("button", { name: "Retry exact keyed create" });
      expect((screen.getByLabelText("Skill idea") as HTMLInputElement).disabled).toBe(true);
      expect(screen.getByText(new RegExp(saved!.id))).toBeTruthy();
      fireEvent.click(screen.getByRole("button", { name: "Reopen same request" }));
      await screen.findByText(/Request not confirmed|invalid Workshop identity\/envelope/);
      expect(posts).toHaveLength(1);
      expect((screen.getByLabelText("Skill idea") as HTMLInputElement).disabled).toBe(true);
      fireEvent.click(screen.getByRole("button", { name: "Retry exact keyed create" }));
      await waitFor(() => expect(posts).toHaveLength(2));
      expect(posts[1]).toBe(posts[0]);
      validGet = true;
      fireEvent.click(screen.getByRole("button", { name: "Reopen same request" }));
      await waitFor(() => expect(screen.queryByRole("button", { name: "Retry exact keyed create" })).toBeNull());
      expect(screen.getByText(new RegExp(saved!.id))).toBeTruthy();
      expect(posts).toHaveLength(2);
    } finally {
      fetch.mockRestore();
    }
  },
);

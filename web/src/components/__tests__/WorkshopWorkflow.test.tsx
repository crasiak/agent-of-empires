// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { WorkshopWorkflow } from "../WorkshopWorkflow";
import type { WorkshopRequest } from "../../lib/types";
afterEach(cleanup);
const request: WorkshopRequest = {
  id: "00000000-0000-4000-8000-000000000001",
  revision: 12,
  state: "publish-preview",
  envelope: {
    account: "work",
    idea: "fixture",
    repository: "/fixture",
    slug: "example",
    source_scope: "Work suitable, not disclosure",
  },
  created_at: "2026-01-01T00:00:00Z",
  hidden: false,
  turns: [],
  bundle: null,
  review: null,
  commit_intent: null,
  commit: null,
  error: null,
  publication: {
    preview: {
      source: {
        account: "work",
        source_scope: "Work suitable, not disclosure",
        skill_path: ".agents/skills/example",
        review_digest: "review",
        content_digest: "content",
      },
      destination: { owner: "fixture", name: "public", repository_id: 42, base_branch: "main" },
      repository: { id: 42, node_id: "R_fixture", visibility: "public", default_branch: "main" },
      actor: { id: 7, login: "host-owner" },
      base_head: "base",
      title: "Exact title",
      body: "Exact body",
      digest: "preview-exact",
    },
    published: null,
    policy: null,
    landed: null,
  },
};
it("requires destination-aware disclosure distinct from Work suitability and resets on new digest", async () => {
  const operate = vi.fn().mockResolvedValue(undefined);
  const view = render(<WorkshopWorkflow request={request} locked={false} operate={operate} />);
  expect(screen.getByText(/public repository fixture\/public/)).toBeTruthy();
  expect(screen.getByLabelText("Exact destination disclosure").textContent).toContain("Exact body");
  const approve = screen.getByRole("button", { name: "Approve destination Publish" }) as HTMLButtonElement;
  expect(approve.disabled).toBe(true);
  fireEvent.click(screen.getByRole("checkbox", { name: /I approve disclosing/ }));
  fireEvent.click(approve);
  expect(operate).toHaveBeenCalledWith({
    operation: "publish",
    id: request.id,
    revision: 12,
    preview_digest: "preview-exact",
    disclosure_confirmed: true,
  });
  view.rerender(
    <WorkshopWorkflow
      request={{
        ...request,
        publication: { ...request.publication!, preview: { ...request.publication!.preview!, digest: "changed" } },
      }}
      locked={false}
      operate={operate}
    />,
  );
  await waitFor(() => expect(approve.disabled).toBe(true));
});
it("requires separate current-rules and exclusive-custody confirmations for Land", () => {
  const operate = vi.fn().mockResolvedValue(undefined);
  render(
    <WorkshopWorkflow
      request={{
        ...request,
        state: "land-preview",
        publication: {
          ...request.publication!,
          policy: { digest: "actual-policy", base_head: "advanced-base", rules: [], classic: {} },
        },
      }}
      locked={false}
      operate={operate}
    />,
  );
  const land = screen.getByRole("button", { name: "Land" }) as HTMLButtonElement;
  fireEvent.click(screen.getByRole("checkbox", { name: /I approve this exact head/ }));
  expect(land.disabled).toBe(true);
  fireEvent.click(screen.getByRole("checkbox", { name: /I confirm exclusive PR custody/ }));
  fireEvent.click(land);
  expect(operate).toHaveBeenCalledWith({
    operation: "land",
    id: request.id,
    revision: 12,
    policy_digest: "actual-policy",
    current_base_and_rules: true,
    exclusive_pr_custody: true,
  });
  expect(screen.getByText(/GitHub has no stack-membership/)).toBeTruthy();
});
it("pending and unknown are reconcile-only, never Deploy or merge replay", () => {
  const operate = vi.fn().mockResolvedValue(undefined);
  render(<WorkshopWorkflow request={{ ...request, state: "land-pending" }} locked={false} operate={operate} />);
  expect(screen.getByText(/Pending UUID is not landed/)).toBeTruthy();
  expect((screen.getByRole("button", { name: "Land" }) as HTMLButtonElement).disabled).toBe(true);
  expect((screen.getByRole("button", { name: "Deploy" }) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(screen.getByRole("button", { name: "Read-only reconcile remote intent" }));
  expect(operate).toHaveBeenCalledWith({ operation: "reconcile-publication", id: request.id, revision: 12 });
});
it("shows original config/import consumers/native receipts and binds separate approvals", () => {
  const operate = vi.fn().mockResolvedValue(undefined);
  const deployment = {
    targets: [],
    intent: { attempt: "attempt", stage: "apply", digest: "candidate-exact" },
    import_preview: {
      schema: "preview",
      digest: "import-exact",
      guard: { consumers: [{ profile: "binding-less", denied: true, changing: true }] },
    },
    import_receipt: null,
    candidate: {
      schema: "candidate",
      digest: "candidate-exact",
      content: "COMPLETE original config",
      materialize_pairs: ["selected/pi"],
    },
    publication_receipt: null,
    materialize_receipt: null,
    rollouts: [],
  };
  const view = render(
    <WorkshopWorkflow
      request={{ ...request, state: "candidate-preview", deployment }}
      locked={false}
      operate={operate}
    />,
  );
  expect(screen.getByLabelText("All import consumers and selected pairs").textContent).toContain("binding-less");
  expect(screen.getByLabelText("Complete original config candidate").textContent).toContain("COMPLETE original config");
  fireEvent.click(screen.getByRole("button", { name: "Deploy" }));
  expect(operate).toHaveBeenLastCalledWith({
    operation: "deploy",
    id: request.id,
    revision: 12,
    candidate_digest: "candidate-exact",
  });
  view.rerender(
    <WorkshopWorkflow
      request={{
        ...request,
        state: "rollout-preview",
        deployment: {
          ...deployment,
          rollouts: [
            {
              directory: "/server-owned",
              plan_digest: "native-exact",
              plan: { schema: "v2", targets: [{ state: "waiting-idle" }, { state: "unsupported" }] },
              result: null,
            },
          ],
        },
      }}
      locked={false}
      operate={operate}
    />,
  );
  fireEvent.click(screen.getByRole("button", { name: "Approve exact native plan (limit 1)" }));
  expect(operate).toHaveBeenLastCalledWith({
    operation: "apply-rollout",
    id: request.id,
    revision: 12,
    plan_digest: "native-exact",
  });
});
it("read-only or uncertain lock blocks every effect approval", () => {
  const operate = vi.fn();
  render(<WorkshopWorkflow request={request} locked operate={operate} />);
  for (const element of screen.getAllByRole("button")) expect((element as HTMLButtonElement).disabled).toBe(true);
  for (const element of screen.getAllByRole("checkbox")) expect((element as HTMLInputElement).disabled).toBe(true);
  expect(operate).not.toHaveBeenCalled();
});

import type { WorkshopEnvelope, WorkshopRequest } from "./types";

type ObjectValue = Record<string, unknown>;
const object = (value: unknown): value is ObjectValue => !!value && typeof value === "object" && !Array.isArray(value);
const strings = (value: unknown): value is string[] =>
  Array.isArray(value) && value.every((v) => typeof v === "string");
const fields = (value: unknown, keys: string[]): value is ObjectValue =>
  object(value) && keys.every((key) => typeof value[key] === "string");
const integer = (value: unknown) => Number.isSafeInteger(value) && (value as number) >= 0;
const nullable = (value: unknown, check: (v: unknown) => boolean) => value === null || check(value);
const files = (value: unknown) => object(value) && Object.values(value).every((v) => typeof v === "string");
const bundle = (value: unknown) => object(value) && files(value.files);
const artifact = (value: unknown) => fields(value, ["schema"]);
const digested = (value: unknown) => fields(value, ["schema", "digest"]);
const states = new Set([
  "draft",
  "refining",
  "reviewing",
  "review",
  "committing",
  "committed",
  "uncertain",
  "failed",
  "publish-preview",
  "publishing",
  "publish-ready",
  "publish-unknown",
  "published",
  "land-preview",
  "landing",
  "land-pending",
  "land-unknown",
  "landed",
  "import-preview",
  "importing",
  "imported",
  "candidate-preview",
  "applying",
  "published-config",
  "materializing",
  "materialized",
  "rollout-preview",
  "rolling-out",
  "rollout-complete",
  "deploy-unknown",
]);
function envelope(value: unknown): value is WorkshopEnvelope {
  return (
    fields(value, ["idea", "repository", "slug", "source_scope"]) &&
    (value.account === "personal" || value.account === "work")
  );
}
export function sameWorkshopEnvelope(value: WorkshopEnvelope, expected: WorkshopEnvelope): boolean {
  return (
    value.account === expected.account &&
    value.idea === expected.idea &&
    value.repository === expected.repository &&
    value.slug === expected.slug &&
    value.source_scope === expected.source_scope
  );
}
function publication(value: unknown): boolean {
  if (!object(value)) return false;
  const preview = (p: unknown) =>
    fields(p, ["base_head", "title", "body", "digest"]) &&
    fields(p.source, ["account", "source_scope", "skill_path", "review_digest", "content_digest"]) &&
    fields(p.destination, ["owner", "name", "base_branch"]) &&
    integer(p.destination.repository_id) &&
    fields(p.repository, ["node_id", "default_branch"]) &&
    integer(p.repository.id) &&
    ["public", "private"].includes(String(p.repository.visibility)) &&
    fields(p.actor, ["login"]) &&
    integer(p.actor.id);
  return (
    nullable(value.preview, preview) &&
    nullable(
      value.published,
      (p) => fields(p, ["head", "skill_tree"]) && fields(p.identity, ["url"]) && integer(p.identity.number),
    ) &&
    nullable(value.policy, (p) => fields(p, ["digest", "base_head"]) && Array.isArray(p.rules) && "classic" in p) &&
    nullable(value.landed, (p) =>
      fields(p, ["approved_head", "merged_commit", "source_tree", "skill_path", "skill_tree"]),
    )
  );
}
function deployment(value: unknown): boolean {
  if (
    !object(value) ||
    !Array.isArray(value.targets) ||
    !value.targets.every((t) => fields(t, ["profile", "harness", "account"]) && strings(t.sessions)) ||
    !fields(value.intent, ["attempt", "stage", "digest"]) ||
    !Array.isArray(value.rollouts)
  )
    return false;
  return (
    nullable(value.import_preview, digested) &&
    nullable(value.import_receipt, artifact) &&
    nullable(value.candidate, digested) &&
    nullable(value.publication_receipt, artifact) &&
    nullable(value.materialize_receipt, artifact) &&
    value.rollouts.every(
      (r) => fields(r, ["directory", "plan_digest"]) && artifact(r.plan) && nullable(r.result, artifact),
    )
  );
}
export function isWorkshopRequest(value: unknown, id: string, expected?: WorkshopEnvelope): value is WorkshopRequest {
  if (
    !object(value) ||
    value.id !== id ||
    !envelope(value.envelope) ||
    (expected && !sameWorkshopEnvelope(value.envelope, expected)) ||
    !integer(value.revision) ||
    typeof value.state !== "string" ||
    !states.has(value.state) ||
    typeof value.created_at !== "string" ||
    !Number.isFinite(Date.parse(value.created_at)) ||
    typeof value.hidden !== "boolean" ||
    !Array.isArray(value.turns)
  )
    return false;
  if (
    !value.turns.every(
      (t) => fields(t, ["user"]) && nullable(t.proposal, (p) => fields(p, ["reply"]) && bundle(p.bundle)),
    )
  )
    return false;
  const review = (r: unknown) =>
    fields(r, ["repository", "base_commit", "source_tree", "skill_tree", "diff", "created_at", "digest"]) &&
    integer(r.revision) &&
    bundle(r.files) &&
    files(r.previous_files) &&
    strings(r.evidence);
  return (
    nullable(value.bundle, bundle) &&
    nullable(value.review, review) &&
    nullable(value.commit_intent, (r) => fields(r, ["review_digest", "commit_oid", "branch", "worktree"])) &&
    nullable(value.commit, (r) => fields(r, ["commit_oid", "source_tree", "skill_tree", "branch", "worktree"])) &&
    (value.error === null || typeof value.error === "string") &&
    (value.publication == null || publication(value.publication)) &&
    (value.deployment == null || deployment(value.deployment))
  );
}

#!/usr/bin/env bash
set -euo pipefail

# upstream-check.sh: report whether the upstream project has commits our fork
# has not pulled in yet, and whether merging them would conflict.
#
# Steps: fetch the upstream remote, compute the merge-base with our branch,
# print ahead/behind counts, list the new upstream commits, compare the latest
# upstream release tag with our Cargo.toml version, intersect the files changed
# on each side since the merge-base, and (git >= 2.38) dry-run the merge with
# `git merge-tree --write-tree`. Ends with a one-line VERDICT.
#
# Read-only apart from `git fetch`: no refs, index, working tree, or checked-out
# branch are touched. merge-tree does write the would-be merge result as loose
# objects in the object store; nothing references them and gc prunes them.
#
# Usage:
#   scripts/upstream-check.sh              # fetch, then check
#   scripts/upstream-check.sh --no-fetch   # check against the refs already fetched
#
# Environment overrides:
#   UPSTREAM_REMOTE  remote to compare against   (default: upstream)
#   UPSTREAM_BRANCH  branch on that remote       (default: the remote's HEAD, else main)
#   LOCAL_REF        our side of the comparison  (default: main)
#
# Exit codes:
#   0  up to date: upstream has nothing our branch lacks
#   1  upstream has new commits and they merge cleanly (or the conflict check
#      was unavailable because git is older than 2.38)
#   2  upstream has new commits and merging them would conflict
#   3  error (fetch failed, missing ref, bad arguments)

trap 'exit 3' ERR

UPSTREAM_REMOTE="${UPSTREAM_REMOTE:-upstream}"
UPSTREAM_BRANCH="${UPSTREAM_BRANCH:-}"
LOCAL_REF="${LOCAL_REF:-main}"
FETCH=1

usage() { sed -n '4,31p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }

for arg in "$@"; do
  case "$arg" in
    --no-fetch) FETCH=0 ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $arg" >&2
      usage >&2
      exit 3
      ;;
  esac
done

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$ROOT_DIR"

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

section() { printf '\n== %s\n' "$*"; }
info() { printf '  %s\n' "$*"; }
die() {
  printf 'error: %s\n' "$*" >&2
  exit 3
}
indent() { sed 's/^/  /'; }

# First `version = "..."` under [package] in Cargo.toml at a given ref.
cargo_version() {
  git show "$1:Cargo.toml" 2>/dev/null |
    awk '/^\[package\]/{p=1; next} /^\[/{p=0} p && /^version *=/{gsub(/.*= *"|".*/, ""); print; exit}'
}

# Succeeds when the installed git has `merge-tree --write-tree` (2.38+).
git_has_write_tree() {
  local ver major minor
  ver="$(git version | awk '{print $3}')"
  major="${ver%%.*}"
  minor="${ver#*.}"
  minor="${minor%%.*}"
  [ "$major" -gt 2 ] || { [ "$major" -eq 2 ] && [ "$minor" -ge 38 ]; }
}

git remote get-url "$UPSTREAM_REMOTE" >/dev/null 2>&1 ||
  die "no git remote named '$UPSTREAM_REMOTE'"

if [ "$FETCH" -eq 1 ]; then
  printf 'Fetching %s ...\n' "$UPSTREAM_REMOTE"
  git fetch --quiet --tags "$UPSTREAM_REMOTE" || die "git fetch $UPSTREAM_REMOTE failed"
fi

if [ -z "$UPSTREAM_BRANCH" ]; then
  head_ref="$(git symbolic-ref --quiet --short "refs/remotes/$UPSTREAM_REMOTE/HEAD" 2>/dev/null || true)"
  if [ -n "$head_ref" ]; then
    UPSTREAM_BRANCH="${head_ref#"$UPSTREAM_REMOTE"/}"
  else
    UPSTREAM_BRANCH=main
  fi
fi
UPSTREAM_REF="$UPSTREAM_REMOTE/$UPSTREAM_BRANCH"

git rev-parse --verify --quiet "$UPSTREAM_REF^{commit}" >/dev/null || die "unknown ref: $UPSTREAM_REF"
git rev-parse --verify --quiet "$LOCAL_REF^{commit}" >/dev/null || die "unknown ref: $LOCAL_REF"

MERGE_BASE="$(git merge-base "$LOCAL_REF" "$UPSTREAM_REF")" ||
  die "$LOCAL_REF and $UPSTREAM_REF share no history"

behind="$(git rev-list --count "$MERGE_BASE..$UPSTREAM_REF")"
ahead="$(git rev-list --count "$MERGE_BASE..$LOCAL_REF")"
ahead_no_merges="$(git rev-list --count --no-merges "$MERGE_BASE..$LOCAL_REF")"

section "Refs"
info "ours:       $LOCAL_REF  $(git log -1 --format='%h %ad %s' --date=short "$LOCAL_REF")"
info "upstream:   $UPSTREAM_REF  $(git log -1 --format='%h %ad %s' --date=short "$UPSTREAM_REF")"
info "merge-base: $(git log -1 --format='%h %ad %s' --date=short "$MERGE_BASE")"
info "upstream new commits: $behind"
info "fork-only commits:    $ahead ($ahead_no_merges excluding merges)"

section "Versions"
our_version="$(cargo_version "$LOCAL_REF")"
upstream_version="$(cargo_version "$UPSTREAM_REF")"
info "Cargo.toml on $LOCAL_REF:  ${our_version:-?}"
info "Cargo.toml on $UPSTREAM_REF: ${upstream_version:-?}"
latest_tag="$(git tag --list 'v*' --merged "$UPSTREAM_REF" --sort=-v:refname | awk 'NR == 1')"
if [ -n "$latest_tag" ]; then
  if git merge-base --is-ancestor "$latest_tag" "$LOCAL_REF"; then
    info "latest upstream tag:  $latest_tag (already in $LOCAL_REF)"
  else
    info "latest upstream tag:  $latest_tag (NOT in $LOCAL_REF)"
  fi
fi
# Upstream cuts releases on release-staging/<version> branches before tagging.
git for-each-ref --format='%(refname:short)' "refs/remotes/$UPSTREAM_REMOTE/release-staging/" |
  while read -r staging; do
    tag="${staging##*/}"
    tag="v${tag#v}"
    git rev-parse --verify --quiet "refs/tags/$tag" >/dev/null ||
      info "pending release:      $staging (not tagged yet)"
  done

if [ "$behind" -eq 0 ]; then
  printf '\nVERDICT: UP TO DATE (%s contains %s)\n' "$LOCAL_REF" "$UPSTREAM_REF"
  exit 0
fi

section "New upstream commits (oldest first)"
git log --reverse --format='%h %ad %s' --date=short "$MERGE_BASE..$UPSTREAM_REF" | indent

section "Upstream commits by type"
git log --format='%s' "$MERGE_BASE..$UPSTREAM_REF" |
  awk '{ if (match($0, /^[a-z]+(\([^)]*\))?!?:/)) { sub(/[(!:].*/, ""); print } else print "other" }' |
  sort | uniq -c | sort -rn | indent
# Conventional-commit `type!:` subjects plus BREAKING CHANGE footers.
{
  git log --format='%h %s' "$MERGE_BASE..$UPSTREAM_REF" | grep -E '^[0-9a-f]+ [a-z]+(\([^)]*\))?!:' || true
  git log --format='%h %s' -E --grep='^BREAKING[ -]CHANGE' "$MERGE_BASE..$UPSTREAM_REF"
} | sort -u >"$TMP_DIR/breaking"
if [ -s "$TMP_DIR/breaking" ]; then
  info "breaking-change markers:"
  indent <"$TMP_DIR/breaking" | indent
else
  info "breaking-change markers: none"
fi

section "Files changed on both sides since the merge-base"
git diff --name-only "$MERGE_BASE" "$LOCAL_REF" | LC_ALL=C sort -u >"$TMP_DIR/ours"
git diff --name-only "$MERGE_BASE" "$UPSTREAM_REF" | LC_ALL=C sort -u >"$TMP_DIR/theirs"
LC_ALL=C comm -12 "$TMP_DIR/ours" "$TMP_DIR/theirs" >"$TMP_DIR/overlap"
info "ours: $(wc -l <"$TMP_DIR/ours" | tr -d ' ') files, upstream: $(wc -l <"$TMP_DIR/theirs" | tr -d ' ') files, both: $(wc -l <"$TMP_DIR/overlap" | tr -d ' ')"
indent <"$TMP_DIR/overlap"

if ! git_has_write_tree; then
  section "Merge dry run"
  info "skipped: git $(git version | awk '{print $3}') lacks merge-tree --write-tree (needs 2.38+)"
  printf '\nVERDICT: UPSTREAM HAS %s NEW COMMITS (conflict check unavailable; %s files changed on both sides)\n' \
    "$behind" "$(wc -l <"$TMP_DIR/overlap" | tr -d ' ')"
  exit 1
fi

section "Merge dry run: git merge-tree --write-tree $LOCAL_REF $UPSTREAM_REF"
merge_rc=0
git merge-tree --write-tree --name-only "$LOCAL_REF" "$UPSTREAM_REF" >"$TMP_DIR/merge" || merge_rc=$?
if [ "$merge_rc" -gt 1 ]; then
  die "git merge-tree failed (exit $merge_rc)"
fi
if [ "$merge_rc" -eq 0 ]; then
  info "clean: no conflicts"
  printf '\nVERDICT: UPSTREAM HAS %s NEW COMMITS, clean merge\n' "$behind"
  exit 1
fi

# Output layout: result tree OID, conflicted paths, blank line, messages.
merged_tree="$(head -n 1 "$TMP_DIR/merge")"
awk 'NR > 1 && /^$/ {exit} NR > 1' "$TMP_DIR/merge" | LC_ALL=C sort -u >"$TMP_DIR/conflicts"
awk 'f; /^$/ {f=1}' "$TMP_DIR/merge" >"$TMP_DIR/messages"

total_hunks=0
while read -r path; do
  hunks="$(git show "$merged_tree:$path" 2>/dev/null | grep -c '^<<<<<<<' || true)"
  total_hunks=$((total_hunks + ${hunks:-0}))
  printf '  %-60s %s hunk(s)\n' "$path" "${hunks:-0}"
done <"$TMP_DIR/conflicts"
conflict_count="$(wc -l <"$TMP_DIR/conflicts" | tr -d ' ')"
info "total: $conflict_count file(s), $total_hunks hunk(s)"
info "conflict kinds:"
{ grep -oE '^CONFLICT \([^)]*\)' "$TMP_DIR/messages" || true; } | sort | uniq -c | sort -rn | indent | indent

# Rank commits on each side by how many conflicted files they touch, to point
# at the change that is driving the conflicts.
rank_commits() {
  git rev-list --no-merges "$1" | while read -r commit; do
    hits="$(git diff-tree --no-commit-id --name-only -r "$commit" | grep -cxFf "$TMP_DIR/conflicts" || true)"
    if [ "$hits" -gt 0 ]; then
      printf '%4s  %s\n' "$hits" "$(git log -1 --format='%h %s' "$commit")"
    fi
  done | sort -rn | awk 'NR <= 10'
}
section "Upstream commits touching the most conflicted files (top 10)"
rank_commits "$MERGE_BASE..$UPSTREAM_REF" | indent
section "Fork commits touching the most conflicted files (top 10)"
rank_commits "$MERGE_BASE..$LOCAL_REF" | indent

printf '\nVERDICT: UPSTREAM HAS %s NEW COMMITS, conflicts in %s file(s): %s\n' \
  "$behind" "$conflict_count" "$(paste -sd' ' "$TMP_DIR/conflicts")"
exit 2

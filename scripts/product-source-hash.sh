#!/bin/bash
# Prints a hash of the tracked product source tree at a given ref, excluding
# paths whose content cannot change what a release actually builds/ships:
# workflow orchestration, planning/PRD docs, agent guides, engineering notes
# and skill procedures. release-policy.json, Cargo.*, scripts/ and
# agenterm.tasks.json stay IN scope -- they select what gets built, packaged
# and signed, so a change there is a real release change, not an infra-only
# one.
#
# Purpose (adopted from MiniCon's release train, 2026-10-21): the exact-source
# Candidate invariant used to compare raw `git rev-parse origin/main` against
# the pinned source_sha, so a plan/doc-only commit landing on main mid-chain
# invalidated every in-flight Candidate/qualification run for that version,
# forcing a version bump for a change that never touched a shipped byte. This
# hash lets those assertions compare "did the product actually change"
# instead of "did main move at all".
#
# Usage: scripts/product-source-hash.sh <git-ref>
set -euo pipefail

# On Git for Windows' MSYS bash (the shell windows runners use for a
# `shell: bash` step), argument auto-path-conversion mistakes a `<ref>:<path>`
# git revision spec for a POSIX path and mangles it. MSYS_NO_PATHCONV=1
# disables that conversion; it is a no-op on every non-MSYS bash.
export MSYS_NO_PATHCONV=1

# Deliberately does NOT cd to this script's own repo: a caller may be
# operating on a different working tree's git history, and `git ls-tree` /
# `git rev-parse` below resolve against whatever repo the current working
# directory belongs to.
ref="${1:?usage: scripts/product-source-hash.sh <git-ref>}"

EXCLUDE_RE='^(\.github/workflows/|\.agents/|\.claude/|prd/|plan/|docs/|skills/|archive/|research/|AGENTS\.md$|PRD\.md$|README\.md$|CNAME$)'

# One ls-tree call already carries every blob OID; a per-path `git rev-parse`
# loop costs one process per tracked file, which is minutes on Windows hosts.
# ls-tree lines are `<mode> <type> <oid>\t<path>`.
# The regex reaches awk through the environment: `awk -v` runs its own escape
# processing over the value and would warn on (and weaken) every `\.`.
export EXCLUDE_RE
git ls-tree -r "$ref" \
  | awk -F'\t' \
      '{ split($1, meta, " ") } $2 !~ ENVIRON["EXCLUDE_RE"] { print meta[3] " " $2 }' \
  | LC_ALL=C sort \
  | sha256sum \
  | cut -d' ' -f1

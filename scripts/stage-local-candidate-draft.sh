#!/usr/bin/env bash
set -euo pipefail

# Upload one already-built, verified local six-cell bundle to an unpublished
# GitHub draft release. Hosted Candidate jobs download this asset; they do not
# compile it.

if [[ "${GITHUB_ACTIONS:-}" == "true" ]]; then
  echo "local Candidate staging refuses to run inside GitHub Actions" >&2
  exit 2
fi
if [[ $# -ne 1 ]]; then
  echo "usage: scripts/stage-local-candidate-draft.sh BUNDLE.tar.gz" >&2
  exit 2
fi

repo="$(git rev-parse --show-toplevel)"
cd "$repo"
source_sha="$(git rev-parse HEAD)"
if [[ "$(git branch --show-current)" != main ]]; then
  echo "local Candidate staging requires main" >&2
  exit 2
fi
if [[ -n "$(git status --porcelain=v1 --untracked-files=normal)" ]]; then
  echo "local Candidate staging requires a clean worktree" >&2
  exit 2
fi
remote_main="$(git ls-remote origin refs/heads/main | awk 'NR == 1 { print $1 }')"
if [[ "$remote_main" != "$source_sha" ]]; then
  echo "local Candidate staging requires exact origin/main" >&2
  exit 2
fi

version="$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n1)"
bundle="$1"
expected="agenterm-$version-local-candidate-input.tar.gz"
if [[ "$(basename "$bundle")" != "$expected" || ! -f "$bundle" || -L "$bundle" ]]; then
  echo "bundle name or file type does not match the current release input" >&2
  exit 2
fi
checksum="$bundle.sha256"
if [[ ! -f "$checksum" || -L "$checksum" ]]; then
  echo "local Candidate bundle checksum is missing" >&2
  exit 2
fi
stage_dir="$(mktemp -d "${TMPDIR:-/tmp}/agenterm-stage-check.XXXXXX")"
trap 'rm -rf "$stage_dir"' EXIT
python3 scripts/verify-local-candidate.py \
  --archive "$bundle" --checksum "$checksum" --out "$stage_dir" \
  --source-sha "$source_sha" --version "$version"
gh auth status --hostname github.com >/dev/null
repository="$(gh repo view --json nameWithOwner --jq .nameWithOwner)"

timestamp="$(date -u +%Y%m%dT%H%M%SZ)"
tag="local-stage-$source_sha-$timestamp"
if git ls-remote --exit-code --tags origin "refs/tags/$tag" >/dev/null 2>&1; then
  echo "staging tag already exists; refusing to overwrite it" >&2
  exit 2
fi
gh release create "$tag" --draft --target "$source_sha" \
  --title "AgenTerm $version local six-cell input" \
  --notes "Unpublished local build input for test-only Candidate workflow $source_sha."
release_json="$(gh api "repos/$repository/releases/tags/$tag")"
release_id="$(jq -er '.id | numbers' <<<"$release_json")"
jq -e --arg tag "$tag" '.draft == true and .tag_name == $tag' \
  <<<"$release_json" >/dev/null
echo "STAGING_RELEASE_ID=$release_id"
gh release upload "$tag" "$bundle" "$checksum"
assets="$(gh api "repos/$repository/releases/$release_id/assets?per_page=100")"
jq -e --arg bundle "$expected" --arg checksum "$expected.sha256" '
  (length == 2)
  and ([.[].name] | sort) == ([$bundle, $checksum] | sort)
' <<<"$assets" >/dev/null
echo "LOCAL CANDIDATE DRAFT READY source=$source_sha version=$version"

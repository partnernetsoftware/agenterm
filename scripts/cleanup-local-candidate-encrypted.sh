#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 || ! "$1" =~ ^[0-9]+$ ]]; then
  echo "usage: scripts/cleanup-local-candidate-encrypted.sh STAGING_RELEASE_ID" >&2
  exit 2
fi
gh auth status --hostname github.com >/dev/null
repository="$(gh repo view --json nameWithOwner --jq .nameWithOwner)"
release_json="$(gh api "repos/$repository/releases/$1")"
tag="$(jq -r .tag_name <<<"$release_json")"
jq -e --arg tag "$tag" '
  .draft == false and .prerelease == true
  and (.tag_name | test("^local-candidate-encrypted-[0-9a-f]{40}-[0-9]{8}T[0-9]{6}Z$"))
' <<<"$release_json" >/dev/null
gh release delete "$tag" --yes --cleanup-tag
echo "Deleted temporary encrypted Candidate staging prerelease $1"

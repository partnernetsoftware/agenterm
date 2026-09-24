#!/usr/bin/env bash
set -euo pipefail

# Encrypt one already-built, verified local six-cell bundle and upload only
# ciphertext to a temporary prerelease. Hosted Candidate jobs download and
# decrypt those bytes; they do not compile them.

if [[ "${GITHUB_ACTIONS:-}" == "true" ]]; then
  echo "local Candidate staging refuses to run inside GitHub Actions" >&2
  exit 2
fi
if [[ $# -ne 1 ]]; then
  echo "usage: scripts/stage-local-candidate-encrypted.sh BUNDLE.tar.gz" >&2
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
if ! command -v age >/dev/null; then
  echo "local Candidate staging requires age" >&2
  exit 2
fi
recipient="$(tr -d '\r\n' < scripts/local-candidate-age-recipient.txt)"
if [[ ! "$recipient" =~ ^age1[023456789acdefghjklmnpqrstuvwxyz]{58}$ ]]; then
  echo "local Candidate age recipient is invalid" >&2
  exit 2
fi
encrypted="$bundle.age"
age --encrypt --recipient "$recipient" --output "$encrypted" "$bundle"
python3 - "$encrypted" <<'PY'
import hashlib
from pathlib import Path
import sys

encrypted = Path(sys.argv[1])
hash_state = hashlib.sha256()
with encrypted.open("rb") as stream:
    for block in iter(lambda: stream.read(1024 * 1024), b""):
        hash_state.update(block)
digest = hash_state.hexdigest()
Path(f"{encrypted}.sha256").write_text(f"{digest}  {encrypted.name}\n")
PY
gh auth status --hostname github.com >/dev/null
repository="$(gh repo view --json nameWithOwner --jq .nameWithOwner)"

timestamp="$(date -u +%Y%m%dT%H%M%SZ)"
tag="local-candidate-encrypted-$source_sha-$timestamp"
if git ls-remote --exit-code --tags origin "refs/tags/$tag" >/dev/null 2>&1; then
  echo "staging tag already exists; refusing to overwrite it" >&2
  exit 2
fi
gh release create "$tag" --draft --prerelease --latest=false --target "$source_sha" \
  --title "AgenTerm $version encrypted Candidate transport" \
  --notes "Temporary encrypted test input for Candidate $source_sha. Contains no runnable release assets; remove this prerelease and tag after qualification."
releases="$(gh api "repos/$repository/releases?per_page=100")"
release_json="$(jq -ce --arg tag "$tag" --arg source "$source_sha" '
  [.[] | select(.tag_name == $tag and .draft == true and .prerelease == true and .target_commitish == $source)]
  | if length == 1 then .[0] else error("encrypted staging identity") end
' <<<"$releases")"
release_id="$(jq -er '.id | numbers' <<<"$release_json")"
jq -e --arg tag "$tag" '.draft == true and .prerelease == true and .tag_name == $tag' \
  <<<"$release_json" >/dev/null
echo "STAGING_RELEASE_ID=$release_id"
# The large upload uses direct HTTPS; this host's local proxy stalled after
# transferring most of a previous bundle.
env -u http_proxy -u https_proxy -u all_proxy \
  -u HTTP_PROXY -u HTTPS_PROXY -u ALL_PROXY \
  gh release upload "$tag" "$encrypted" "$encrypted.sha256" "$checksum"
assets="$(gh api "repos/$repository/releases/$release_id/assets?per_page=100")"
encrypted_sha="$(awk 'NR == 1 { print $1 }' "$encrypted.sha256")"
jq -e --arg encrypted "$(basename "$encrypted")" --arg checksum "$expected.sha256" \
  --arg encrypted_sha "$encrypted_sha" '
  (length == 3)
  and ([.[].name] | sort) == ([$encrypted, ($encrypted + ".sha256"), $checksum] | sort)
  and all(.[]; .state == "uploaded")
  and ([.[] | select(.name == $encrypted) | .digest] == ["sha256:" + $encrypted_sha])
' <<<"$assets" >/dev/null
gh release edit "$tag" --draft=false --prerelease --latest=false
release_json="$(gh api "repos/$repository/releases/$release_id")"
jq -e --arg tag "$tag" --arg source "$source_sha" '
  .draft == false and .prerelease == true
  and .tag_name == $tag and .target_commitish == $source
' <<<"$release_json" >/dev/null
[[ "$(gh api "repos/$repository/git/ref/tags/$tag" --jq .object.sha)" == "$source_sha" ]]
# This anonymous download proves the public encrypted staging route is
# readable by a runner without repository write credentials before dispatch.
checksum_id="$(jq -er --arg name "$expected.sha256" '.assets[] | select(.name == $name) | .id' <<<"$release_json")"
curl --fail --location --silent --show-error \
  -H 'Accept: application/octet-stream' \
  "https://api.github.com/repos/$repository/releases/assets/$checksum_id" \
  --output "$stage_dir/public-checksum"
cmp "$checksum" "$stage_dir/public-checksum"
echo "LOCAL CANDIDATE ENCRYPTED STAGING READY source=$source_sha version=$version release_id=$release_id"

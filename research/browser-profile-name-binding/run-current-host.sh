#!/bin/sh
set -eu

ROOT=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
REPO=$(CDPATH='' cd -- "$ROOT/../.." && pwd)
AGENTERM_EXE=${AGENTERM_EXE:-"$REPO/target/debug/agenterm"}
AGENTERM_CU_EXE=${AGENTERM_CU_EXE:-"$REPO/target/debug/agenterm-cu"}
CHROMIUM_EXE=${AGENTERM_CU_BROWSER_EXE:-}
ATTEMPT=${ACU075_BINDING_ATTEMPT:-1}
COURT="$ROOT/court-current-host.qjs"
MODEL="$ROOT/binding-model.qjs"
SPEC="$REPO/plan/design-browser-profile-name-binding-experiment.md"
LOCAL_STATE="$ROOT/fixtures/local-state.json"
PREFERENCES="$ROOT/fixtures/preferences.json"
MANIFEST="$REPO/crates/agenterm-cu/assets/browser-bridge/manifest.json"
BACKGROUND="$REPO/crates/agenterm-cu/assets/browser-bridge/background.js"

require_file() {
  [ -f "$1" ] || {
    printf '%s\n' "missing required input: $2" >&2
    exit 2
  }
}

require_file "$AGENTERM_EXE" AGENTERM_EXE
require_file "$AGENTERM_CU_EXE" AGENTERM_CU_EXE
require_file "$CHROMIUM_EXE" AGENTERM_CU_BROWSER_EXE
require_file "$COURT" court-current-host.qjs
require_file "$MODEL" binding-model.qjs
require_file "$SPEC" plan/design-browser-profile-name-binding-experiment.md
require_file "$LOCAL_STATE" fixtures/local-state.json
require_file "$PREFERENCES" fixtures/preferences.json
require_file "$MANIFEST" browser-bridge/manifest.json
require_file "$BACKGROUND" browser-bridge/background.js

case "$ATTEMPT" in
  1|2) ;;
  *)
    printf '%s\n' "ACU075_BINDING_ATTEMPT must be 1 or 2" >&2
    exit 2
    ;;
esac

SOURCE_SHA=$(git -C "$REPO" rev-parse HEAD)
git -C "$REPO" merge-base --is-ancestor "$SOURCE_SHA" origin/main || {
  printf '%s\n' "the research source commit must be reachable from origin/main" >&2
  exit 2
}

RESEARCH_STATUS=$(git -C "$REPO" status --porcelain --untracked-files=all -- \
  research/browser-profile-name-binding)
if [ -n "$RESEARCH_STATUS" ]; then
  printf '%s\n' "the research directory must exactly match the frozen source commit" >&2
  exit 2
fi

for input in \
  plan/design-browser-profile-name-binding-experiment.md \
  research/browser-profile-name-binding/README.md \
  research/browser-profile-name-binding/RESULTS.md \
  research/browser-profile-name-binding/run-current-host.sh \
  research/browser-profile-name-binding/court-current-host.qjs \
  research/browser-profile-name-binding/binding-model.qjs \
  research/browser-profile-name-binding/fixtures/local-state.json \
  research/browser-profile-name-binding/fixtures/preferences.json \
  crates/agenterm-cu/assets/browser-bridge/manifest.json \
  crates/agenterm-cu/assets/browser-bridge/background.js
do
  git -C "$REPO" cat-file -e "$SOURCE_SHA:$input" 2>/dev/null || {
    printf '%s\n' "research digest input is not tracked by the source commit: $input" >&2
    exit 2
  }
  git -C "$REPO" diff --quiet "$SOURCE_SHA" -- "$input" || {
    printf '%s\n' "research digest input differs from the source commit: $input" >&2
    exit 2
  }
done

set -- \
  plan/design-browser-profile-name-binding-experiment.md \
  research/browser-profile-name-binding/README.md \
  research/browser-profile-name-binding/RESULTS.md \
  research/browser-profile-name-binding/run-current-host.sh \
  research/browser-profile-name-binding/court-current-host.qjs \
  research/browser-profile-name-binding/binding-model.qjs \
  research/browser-profile-name-binding/fixtures/local-state.json \
  research/browser-profile-name-binding/fixtures/preferences.json \
  crates/agenterm-cu/assets/browser-bridge/manifest.json \
  crates/agenterm-cu/assets/browser-bridge/background.js
INPUT_DIGEST=$(perl -MDigest::SHA -e '
  my ($repo, @names) = @ARGV;
  my $sha = Digest::SHA->new(256);
  $sha->add("agenterm-cu/profile-binding-experiment/input/v1\0");
  for my $name (@names) {
    open my $fh, "<:raw", "$repo/$name" or die "input";
    local $/; my $bytes = <$fh>; close $fh;
    $sha->add(pack("Q<", length($name)), $name,
      pack("Q<", length($bytes)), $bytes);
  }
  print $sha->hexdigest;
' "$REPO" "$@")

EXPECTED_BUILD_ID=$(perl -MDigest::SHA -e '
  my ($manifest, $background) = @ARGV;
  my $sha = Digest::SHA->new(256);
  for my $item (["manifest.json", $manifest], ["background.js", $background]) {
    open my $fh, "<:raw", $item->[1] or die "asset";
    local $/; my $bytes = <$fh>; close $fh;
    $sha->add(pack("Q<", length($item->[0])), $item->[0],
      pack("Q<", length($bytes)), $bytes);
  }
  print $sha->hexdigest;
' "$MANIFEST" "$BACKGROUND")

AGENTERM_SHA=$(shasum -a 256 "$AGENTERM_EXE" | awk '{print $1}')
AGENTERM_CU_SHA=$(shasum -a 256 "$AGENTERM_CU_EXE" | awk '{print $1}')
CHROMIUM_SHA=$(shasum -a 256 "$CHROMIUM_EXE" | awk '{print $1}')

exec "$AGENTERM_EXE" cli script run \
  --profile tool \
  --timeout-ms 180000 \
  --max-operations 300000000 \
  --max-host-operations 300000 \
  --max-output-bytes 262144 \
  --max-string-bytes 1048576 \
  --project-root "$REPO" \
  "$COURT" \
  -- \
  "$REPO" \
  "$AGENTERM_CU_EXE" \
  "$CHROMIUM_EXE" \
  "$SOURCE_SHA" \
  "$INPUT_DIGEST" \
  "$EXPECTED_BUILD_ID" \
  "$AGENTERM_SHA" \
  "$AGENTERM_CU_SHA" \
  "$CHROMIUM_SHA" \
  "$ATTEMPT"

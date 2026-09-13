#!/bin/sh
# Runner for the browser profile-name binding exact-process experiment.
#
# First slice only. This script can run the platform-neutral self-test. It
# refuses every live mode, because the live court is not implemented and no
# ordinal may be consumed before the self-test is reviewed.
#
# The qjs `run` embedder declares neither `arg` nor any filesystem door, so the
# court cannot read its own sources at runtime. The self-test's static V2 half
# is therefore driven from here: this script supplies the two source files as
# data and asserts the scan verdict. The court's in-engine identity-source
# cases run either way.

set -eu

REPO=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd -P)
DIR="$REPO/research/browser-profile-name-binding-exact-process"
COURT="$DIR/court-current-host.qjs"
MODEL="$DIR/binding-model.qjs"
PREFLIGHT="$DIR/capability-preflight.qjs"
RH_COMPAT="$REPO/scripts/qjs/lib/rh_compat.qjs"
TEST_HARNESS="$REPO/scripts/qjs/lib/test_harness.qjs"
TEMPLATE="$DIR/result-template.json"
SPEC="$REPO/plan/design-browser-profile-name-binding-exact-process-experiment.md"
MANIFEST="$REPO/agenterm.tasks.json"

fail() {
  printf '%s\n' "{\"schema\":\"agenterm.profile-binding-exact-process-runner/v1\",\"ok\":false,\"code\":\"$1\"}"
  exit 1
}

usage() {
  cat <<'USAGE'
usage: run-current-host.sh --self-test
       run-current-host.sh --static-source-scan
       run-current-host.sh --capability-preflight
       run-current-host.sh --broker-self-test

--self-test            Run the platform-neutral court self-test.
--static-source-scan   Scan this experiment's sources for forbidden
                       process-table constructs (the V2 static half).
--capability-preflight Run the static scan and the registered tool-profile
                       host preflight without reserving an ordinal.
--broker-self-test     Run the external ledger / admission / stage persistence
                       broker self-test in a disposable root. Reserves no
                       ordinal and reaches no verdict.
--broker-harness       Prove the broker self-test summary is fail-closed: a
                       run with an injected failure must report FAILED and
                       exit nonzero, and must print no pass token.
USAGE
}

require_file() {
  [ -f "$1" ] || fail "missing_$2"
}

static_source_scan() {
  # Forbidden constructs per spec sections 1.2 and V2, scanned over the court
  # sources: that is where the live ownership chain and its preflight live.
  # Comments are stripped first, because a file may name a construct in order
  # to forbid it, exactly as a reviewer reads it.
  #
  # The model is exempt by design: it declares the pattern table that states
  # this very rule, and its negative-test fixtures must embed a forbidden
  # construct in order to prove the scanner refuses it.
  for source in "$COURT" "$PREFLIGHT" "$RH_COMPAT" "$TEST_HARNESS"; do
    for pattern in '/bin/ps' '/usr/bin/ps' 'pgrep' 'ps -' 'ps axo' \
        'session_id' 'getsid' 'getpgid' 'setpgid'; do
      hits=$(sed 's://.*::' "$source" 2>/dev/null \
        | grep -F -n -- "$pattern" || true)
      if [ -n "$hits" ]; then
        printf '%s\n' "{\"schema\":\"agenterm.profile-binding-exact-process-static-scan/v1\",\"ok\":false,\"code\":\"INCONCLUSIVE_IDENTITY_SOURCE\",\"pattern\":\"$pattern\"}"
        exit 1
      fi
    done
  done
  printf '%s\n' '{"schema":"agenterm.profile-binding-exact-process-static-scan/v1","ok":true,"code":"IDENTITY_SOURCE_PROVEN"}'
}

AGENTERM_EXE=${AGENTERM_EXE:-"$REPO/target/debug/agenterm"}

[ $# -eq 1 ] || { usage; exit 1; }

require_file "$COURT" court-current-host.qjs
require_file "$MODEL" binding-model.qjs
require_file "$PREFLIGHT" capability-preflight.qjs
require_file "$RH_COMPAT" rh_compat.qjs
require_file "$TEST_HARNESS" test_harness.qjs
require_file "$TEMPLATE" result-template.json
require_file "$SPEC" "frozen specification"
require_file "$AGENTERM_EXE" AGENTERM_EXE

case "$1" in
  --static-source-scan)
    static_source_scan
    ;;
  --self-test)
    # Keep the static result visible. The qjs court cannot inspect its own
    # source, so neither this record nor the following model record is a
    # complete first-slice proof by itself.
    static_source_scan || fail INCONCLUSIVE_IDENTITY_SOURCE
    AGENTERM_SCRIPT_BACKEND=qjswasm "$AGENTERM_EXE" cli script run "$COURT" \
      --project-root "$REPO"
    ;;
  --capability-preflight)
    static_source_scan || fail INCONCLUSIVE_IDENTITY_SOURCE
    AGENTERM_SCRIPT_BACKEND=qjswasm "$AGENTERM_EXE" cli script task run \
      browser-profile-name-binding-exact-process-preflight \
      --manifest "$MANIFEST"
    ;;
  --broker-self-test)
    # The broker self-test needs no registered task and no agenterm binary: it
    # drives the broker spine directly in a disposable root, so it can run
    # before any host capability is available. It proves the admission
    # discipline only, never a design fact.
    sh "$DIR/broker-self-test.sh"
    ;;
  --broker-harness)
    # The aggregator red gate. A self-test that counts failures but still
    # reports success is worse than no self-test, so the summary's fail-closed
    # behavior is itself gated.
    sh "$DIR/broker-self-test-harness.sh"
    ;;
  --live|rehearsal|decision)
    # The live court is deliberately unimplemented. Refusing here is the
    # spec's §4 kill criterion 4: no live ordinal may be reserved while the
    # court cannot prove every throw site is preceded by a persisted stage.
    fail LIVE_COURT_NOT_IMPLEMENTED
    ;;
  *)
    usage
    exit 1
    ;;
esac

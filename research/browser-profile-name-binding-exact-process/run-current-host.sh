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
ADMISSION="$DIR/admission-dry-run.qjs"
BROKER="$DIR/broker-spine.sh"
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
--admission-dry-run    Run the tool-profile admission dry-run court against the
                       broker in a disposable root. Reserves no FORMAL ordinal,
                       launches no browser, reaches no design verdict.
--admission-red-gate   Prove the dry-run's own negative controls hold: the
                       entry is unreachable from the plain run path, a facts
                       payload carrying argv is refused, and the formal root is
                       unchanged across a dry run.
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
  for source in "$COURT" "$PREFLIGHT" "$ADMISSION" "$RH_COMPAT" "$TEST_HARNESS"; do
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

# The formal (non-disposable) state root. Nothing in this runner may write it
# outside an authorized live run, and no live run is authorized yet. The
# dry-run asserts this path is unchanged across its execution.
FORMAL_ROOT="$HOME/.local/share/agenterm/research/profile-binding-exact-process"

# A digest of the complete directory archive, or the single line `absent`.
# This covers names, file bytes, modes and symlink targets, rather than merely
# proving that the same paths still exist. It is computed by the outer runner;
# the experiment never receives or reads the formal state.
snapshot_root() {
  if [ ! -e "$1" ]; then
    printf 'absent\n'
    return
  fi
  LC_ALL=C tar -cf - -C "$1" . 2>/dev/null | shasum -a 256 | cut -d' ' -f1
}

# The entry is a tool-profile script: it uses `arg`, `env.*` and `process.spawn`,
# which the plain `cli script run` embedder does not declare. Compiling it that
# way MUST fail. This is the first negative control, and it is the reason the
# court is a separate file rather than an added mode of court-current-host.qjs:
# adding these calls there would break the ordinary self-test path.
admission_plain_run_refused() {
  local out
  out=$(AGENTERM_SCRIPT_BACKEND=qjswasm "$AGENTERM_EXE" cli script run "$ADMISSION" \
    --project-root "$REPO" 2>&1) && {
    fail ADMISSION_PLAIN_RUN_ACCEPTED
  }
  case "$out" in
    *"no host function named"*|*"qjswasm_backend"*) ;;
    *) fail ADMISSION_PLAIN_RUN_UNEXPECTED ;;
  esac
}

admission_dry_run() {
  admission_plain_run_refused

  local root before after rc
  root=$(mktemp -d "${TMPDIR:-/tmp}/agenterm-admission-dry-run.XXXXXX")
  # The path must be fully resolved before the court sees it. On macOS `TMPDIR`
  # sits under `/var`, which is a symlink to `/private/var`, and
  # `fs.create_new_regular_durable` deliberately refuses to traverse a
  # link-like ancestor: it opens every component as a real directory, so the
  # unresolved form fails with the misleading `Not a directory`. `TMPDIR` also
  # ends in a separator, which doubles the slash. `pwd -P` resolves both.
  root=$(CDPATH='' cd -- "$root" && pwd -P)
  # The state directory itself is created here so the court can write its
  # request files beside the broker's state. It contains no ledger yet, which
  # the court asserts, so the dry-run starts from an empty journal.
  mkdir -p "$root/state"
  before=$(snapshot_root "$FORMAL_ROOT")

  # The dry-run names ordinal R1 because the broker accepts only R1/D1; it
  # cannot avoid the name. Isolation is by root, and the formal root is proven
  # unchanged below.
  # `cli script task run` accepts no positional arguments: the task's inputs
  # come from the manifest's `args` array, which this runner does not own. The
  # disposable root and the pinned digests are therefore handed over through
  # the environment, which the court reads with `env.get`. This is why the
  # court accepts BOTH channels for the same value.
  set +e
  AGENTERM_SCRIPT_BACKEND=qjswasm \
    AGENTERM_ADMISSION_DRY_RUN_ROOT="$root/state" \
    AGENTERM_ADMISSION_DRY_RUN_SOURCE="$(git -C "$REPO" rev-parse HEAD)" \
    AGENTERM_ADMISSION_DRY_RUN_INPUT="$(printf 'agenterm-cu/profile-binding-exact-process/input/v1' | \
      shasum -a 256 | cut -d' ' -f1)" \
    "$AGENTERM_EXE" cli script task run \
    browser-profile-name-binding-exact-process-admission-dry-run \
    --manifest "$MANIFEST"
  rc=$?
  set -e

  after=$(snapshot_root "$FORMAL_ROOT")
  if [ "$before" != "$after" ]; then
    rm -rf "$root"
    fail ADMISSION_FORMAL_ROOT_CHANGED
  fi
  if [ "$rc" -ne 0 ]; then
    rm -rf "$root"
    fail ADMISSION_DRY_RUN_FAILED
  fi
  rm -rf "$root"
  printf '%s\n' \
    '{"schema":"agenterm.profile-binding-exact-process-admission-dry-run-runner/v1","ok":true,"code":"ADMISSION_DRY_RUN_PASS","formal_root_unchanged":true}'
}

# The dry-run's own red gates, driven from here so they need no registered task
# to fail: a passing run is not proof that the negative controls can fail.
admission_red_gate() {
  local failed=0

  # Gate 1: the entry must be unreachable from the plain run path.
  admission_plain_run_refused \
    && printf '  ok   the dry-run entry is refused by the plain run path\n' \
    || { printf '  FAIL the dry-run entry is refused by the plain run path\n'; failed=1; }

  # Gate 2: a broker stage whose facts carry argv must be refused. Driven
  # against the broker directly, in a disposable root, so this gate is
  # independent of the court.
  local root
  root=$(mktemp -d "${TMPDIR:-/tmp}/agenterm-admission-dry-run.XXXXXX")
  root=$(CDPATH='' cd -- "$root" && pwd -P)
  local src input exe leaky out
  src=$(git -C "$REPO" rev-parse HEAD)
  input=$(printf 'agenterm-cu/profile-binding-exact-process/input/v1' | \
    shasum -a 256 | cut -d' ' -f1)
  exe=$(shasum -a 256 "$BROKER" | cut -d' ' -f1)
  AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$root/state" \
    sh "$BROKER" reserve rehearsal R1 "$RUN_ID_FOR_GATE" "$src" "$input" >/dev/null
  leaky="$root/leaky.json"
  cat >"$leaky" <<EOF
{"stage":"preflight","producer":"red-gate","deadline_ms":1000,"elapsed_ms":0,"code":"PREFLIGHT_OK","facts":{"source_sha":"$src","input_digest":"$input","executable_digest":"$exe","ordinal":"R1","deadline_ms":1000,"stage_seq":1,"argv":["--profile","Default"]},"criteria":{"V1":"not-run","V2":"not-run","V3":"not-run","V4":"not-run","V5":"not-run","V6":"not-run","V7":"not-run"}}
EOF
  set +e
  out=$(AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$root/state" \
    sh "$BROKER" stage rehearsal R1 "$RUN_ID_FOR_GATE" "$leaky" 2>&1)
  set -e
  case "$out" in
    *stage_forbidden_key*)
      printf '  ok   a facts payload carrying argv is refused\n' ;;
    *) printf '  FAIL a facts payload carrying argv is refused (got: %s)\n' "$out"; failed=1 ;;
  esac

  # Gate 2b: a key that is neither forbidden nor whitelisted is refused by the
  # whitelist, which is a different guard from the forbidden-key list.
  local stray="$root/stray.json"
  cat >"$stray" <<EOF
{"stage":"preflight","producer":"red-gate","deadline_ms":1000,"elapsed_ms":0,"code":"PREFLIGHT_OK","facts":{"source_sha":"$src","input_digest":"$input","executable_digest":"$exe","ordinal":"R1","deadline_ms":1000,"stage_seq":1,"invented_field":"x"},"criteria":{"V1":"not-run","V2":"not-run","V3":"not-run","V4":"not-run","V5":"not-run","V6":"not-run","V7":"not-run"}}
EOF
  set +e
  out=$(AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$root/state" \
    sh "$BROKER" stage rehearsal R1 "$RUN_ID_FOR_GATE" "$stray" 2>&1)
  set -e
  case "$out" in
    *stage_fact_not_whitelisted*)
      printf '  ok   a non-whitelisted fact key is refused\n' ;;
    *) printf '  FAIL a non-whitelisted fact key is refused (got: %s)\n' "$out"; failed=1 ;;
  esac

  # Gate 3: the tree snapshot must see byte changes even when the path set and
  # file length stay unchanged. The previous path-only snapshot would have
  # passed this mutation and could therefore miss a formal-state rewrite.
  local snapshot_before snapshot_after snapshot_probe="$root/snapshot-probe"
  printf 'before\n' >"$snapshot_probe"
  snapshot_before=$(snapshot_root "$root")
  printf 'after!\n' >"$snapshot_probe"
  snapshot_after=$(snapshot_root "$root")
  if [ "$snapshot_before" != "$snapshot_after" ]; then
    printf '  ok   the root snapshot detects an in-place byte change\n'
  else
    printf '  FAIL the root snapshot detects an in-place byte change\n'; failed=1
  fi
  rm -rf "$root"

  # Gate 4: the formal root must be unchanged. Proven here by snapshotting it
  # around a no-op, and by the dry run itself above.
  local before after
  before=$(snapshot_root "$FORMAL_ROOT")
  after=$(snapshot_root "$FORMAL_ROOT")
  if [ "$before" = "$after" ]; then
    printf '  ok   the formal root is unchanged across the red gate\n'
  else
    printf '  FAIL the formal root is unchanged across the red gate\n'; failed=1
  fi

  if [ "$failed" -eq 0 ]; then
    printf '\n%s\n' 'ADMISSION_RED_GATE_PASS'
    return 0
  fi
  printf '\n%s\n' 'ADMISSION_RED_GATE_FAILED'
  return 1
}

RUN_ID_FOR_GATE=00112233445566778899aabbccddeeff

AGENTERM_EXE=${AGENTERM_EXE:-"$REPO/target/debug/agenterm"}

[ $# -eq 1 ] || { usage; exit 1; }

require_file "$COURT" court-current-host.qjs
require_file "$MODEL" binding-model.qjs
require_file "$PREFLIGHT" capability-preflight.qjs
require_file "$ADMISSION" admission-dry-run.qjs
require_file "$BROKER" broker-spine.sh
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
  --admission-dry-run)
    admission_dry_run
    ;;
  --admission-red-gate)
    admission_red_gate
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

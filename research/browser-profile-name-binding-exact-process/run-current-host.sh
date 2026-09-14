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
# The fully resolved repo root. The probe manifest lives directly in it, and this
# resolved form is what the EXIT guard compares against, so a relative or
# unresolved value can never satisfy the pattern by accident.
REPO_ROOT_ABS=$(CDPATH= cd -- "$REPO" && pwd -P)
DIR="$REPO/research/browser-profile-name-binding-exact-process"
COURT="$DIR/court-current-host.qjs"
MODEL="$DIR/binding-model.qjs"
PREFLIGHT="$DIR/capability-preflight.qjs"
ADMISSION="$DIR/admission-dry-run.qjs"
LIVE="$DIR/live-rehearsal.qjs"
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
       run-current-host.sh --live-self-test
       run-current-host.sh --live-red-gate
       run-current-host.sh --live-process-preflight
       run-current-host.sh --live-staged-preflight   Browser-free persisted-stage integration slice against a
                          DISPOSABLE root: reserves R1 only there, stages every
                          guarded side effect, audits the journal from disk, then
                          closes by an honest `abandon` (no terminal, no finish,
                          no receipt). Formal root proven byte-identical; no browser.
--live-staged-red-gate     The staged-preflight red gates (probe mutants only)
--live-process-red-gate
       run-current-host.sh --broker-self-test

--self-test            Run the platform-neutral court self-test.
--static-source-scan   Scan this experiment's sources for forbidden
                       process-table constructs (the V2 static half).
--capability-preflight Run the static scan and the registered tool-profile
                       host preflight without reserving an ordinal.
--live-self-test       Run the live court's browser-free self-test through the
                       tool-profile path. Launches no browser, observes no
                       owned process, reserves no ordinal.
--live-red-gate        Prove the live court's adversarial controls hold: a live
                       mode is refused by the court itself, and each mutated
                       guard turns the injected-fixture suite red.
--live-process-preflight
                       REAL host evidence with no browser and no ordinal: one
                       owned, non-browser, short-lived child; a real frozen
                       identity; the real ownership walk proving this worker
                       owns the subject; and real observe-to-dead termination
                       with no orphan left behind.
--live-process-red-gate
                       Prove the preflight's guards bite: each mutation must
                       turn the preflight red. A mutation that fails to apply,
                       fails to compile, crashes or hangs is rejected.
--live|rehearsal|decision
                       All refused with LIVE_COURT_NOT_IMPLEMENTED. The live
                       court has no browser-spawn path in this slice.
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
  for source in "$COURT" "$LIVE" "$PREFLIGHT" "$ADMISSION" "$RH_COMPAT" \
      "$TEST_HARNESS"; do
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

# The live court's V7 depends on the ownership+cleanup region of its own source,
# and the qjs engine cannot read the file it is running. The runner extracts
# exactly that region and hands it over as data, so the criterion is judged
# against the real code rather than a restatement. The region deliberately
# excludes the self-test: the self-test must embed a forbidden field name in
# order to prove the scanner refuses it, so scanning the whole file would be a
# false negative against a fixture that is doing its job.
LIVE_REGION=$(mktemp "${TMPDIR:-/tmp}/agenterm-live-region.XXXXXX")

# --- runner-side browser cleanup --------------------------------------------
#
# The SCRIPT's own cleanup cannot be the only layer. A budget or cancel
# termination is a WASM TRAP, not a JS exception, so the script's `try/catch`
# never runs and the process dies with its session still live. This trap is the
# layer that survives that class, because it runs however the runner exits.
#
# It is addressed by the fixed session NAME in the same disposable HOME, so it
# stays selector-free (no pid, no path, no profile id). It is idempotent: after a
# successful in-script stop/remove this finds nothing to do and says so, and a
# stop on an already-stopped session is tolerated.
#
# Failures are AGGREGATED and the runner exits nonzero, so a cleanup that did not
# complete is never mistaken for a clean hand-back.
BROWSER_SESSION_NAME="agenterm-live-browser-baseline"
# The red gates create a second, REAL stray session so the inventory genuinely
# fails to return to empty. It is a FIXED name, and it is on the runner's cleanup
# list, because the script's own teardown of the stray is NOT trusted: a mutation
# that skips the stop leaves the stray live, and only a runner-side name list can
# reclaim it.
BROWSER_STRAY_SESSION_NAME="agenterm-stray"
BROWSER_CLEANUP_FAILED=0
BROWSER_CLEANUP_NOTE=""

# TRANSACTION ARMED STATE. A browser-free invocation never creates a session, so
# its EXIT must be a complete no-op: inferring intent from whether the HOME path
# happens to exist is exactly the mistake that made a deleted root look "clean".
# Only the code that is ABOUT to start a session arms the transaction, and only a
# verified cleanup disarms it.
BROWSER_TXN_ARMED=0
BROWSER_TXN_HOME=""
BROWSER_TXN_NAMES=""
# The EXACT repo-root probe manifest this transaction owns, if any. It is set by
# BOTH parties, in their own processes: the TERM controller mints the path and arms
# its safety transaction with it before forking, and the TERM worker (which receives
# that same path) arms its own transaction with it too. The worker is the EVIDENCE
# cleanup owner -- its shell owns the EXIT trap that runs under the signal -- while
# the controller is the SAFETY owner of the same pre-known path. Ordinary
# command-substitution callers never set this, so for them nothing here changes.
# Empty means "this transaction owns no manifest".
BROWSER_TXN_MANIFEST=""

# Is this EXACTLY a repo-root probe manifest path owned by this runner?
#
# ONE definition, used by every site that may delete or claim-absent such a file.
# A `case` glob cannot do this: glob patterns match `/`, so a string like
# `.probe-manifest.foo/../etc/passwd` satisfies a naive prefix+suffix pattern and
# would authorise deleting something unrelated. So the checks are structural:
#   1. the exact canonical repo-root prefix, compared as a plain string;
#   2. a basename of `.probe-manifest.` plus a NONEMPTY suffix;
#   3. that suffix contains NO `/`, so it can never escape the directory.
# Existence is NOT checked here: some callers must validate a path before the file
# exists, and others must validate before deleting. Callers that need existence
# test it themselves.
probe_manifest_path_owned() {
  local p="$1" rest
  [ -n "$p" ] || return 1
  case "$p" in
    "$REPO_ROOT_ABS"/.probe-manifest.*) rest="${p#"$REPO_ROOT_ABS"/.probe-manifest.}" ;;
    *) return 1 ;;
  esac
  [ -n "$rest" ] || return 1
  case "$rest" in
    */*) return 1 ;;
  esac
  return 0
}

# arm the transaction for the round that is about to start a browser session.
# Session names are fixed, so cleanup stays selector-free. The manifest argument is
# OPTIONAL and, when given, must be an exact repo-root `.probe-manifest.*` path; it
# is never a general cleanup target.
browser_txn_arm() {
  BROWSER_TXN_ARMED=1
  BROWSER_TXN_HOME="$1"
  BROWSER_TXN_NAMES="$2"
  BROWSER_TXN_MANIFEST="${3:-}"
}

# Disarm ONLY after a verified cleanup. Until then the EXIT trap stays armed and
# may compensate; a failure keeps the transaction armed on purpose so the
# backstop still has a chance to run.
browser_txn_disarm() {
  BROWSER_TXN_ARMED=0
  BROWSER_TXN_HOME=""
  BROWSER_TXN_NAMES=""
  BROWSER_TXN_MANIFEST=""
}

# Counted so the signal control can assert cleanup ran EXACTLY once. A counter is
# only a witness, not the cleanup itself: it is incremented at the top of the
# function, and the session work below is what actually has to be idempotent.
BROWSER_CLEANUP_RUNS=0

runner_browser_cleanup() {
  # A transaction that was never armed has nothing to clean. This is the gate that
  # keeps every browser-free command's EXIT a no-op; it does NOT consult the
  # filesystem, because "the HOME is gone" is precisely what used to look clean.
  if [ "$BROWSER_TXN_ARMED" -ne 1 ]; then
    return 0
  fi
  # The witness counts REAL cleanup attempts, so the signal control can prove a
  # genuinely armed transaction is cleaned exactly once. An unarmed no-op must not
  # inflate it.
  BROWSER_CLEANUP_RUNS=$((BROWSER_CLEANUP_RUNS + 1))
  if [ "${BROWSER_CLEANUP_WITNESS:-}" != "" ]; then
    printf 'cleanup_run=%s\n' "$BROWSER_CLEANUP_RUNS" >> "$BROWSER_CLEANUP_WITNESS"
  fi
  # The EXIT trap uses the CURRENT transaction (home + session list) as an
  # interruption backstop. It is NOT the correctness mechanism: correctness is
  # enforced per iteration, before the disposable root is removed.
  per_iteration_cleanup "$BROWSER_TXN_HOME" "$BROWSER_TXN_NAMES"
  if [ "$BROWSER_CLEANUP_FAILED" -ne 0 ]; then
    printf 'BROWSER_RUNNER_CLEANUP_FAILED:%s\n' "$BROWSER_CLEANUP_NOTE" >&2
  fi
  return 0
}

# per_iteration_cleanup <home> <space-separated session names>
#
# EVERY input is explicit: this function never reads the mutable global HOME, so
# a caller cannot clean up the wrong round. It is called once per mutant BETWEEN
# the probe returning and the disposable root being removed -- the ordering that
# the previous structure got wrong, where `rm -rf root` made the final trap see
# "no home" and return success without ever stopping the session.
per_iteration_cleanup() {
  local home="$1"
  local names="$2"
  local cu="${AGENTERM_CU_EXE:-}"
  if [ "$home" = "" ] || [ ! -d "$home" ]; then
    # A missing home while sessions may exist is a cleanup failure, not a skip:
    # returning here would silently certify residue as clean.
    BROWSER_CLEANUP_FAILED=1
    BROWSER_CLEANUP_NOTE="$BROWSER_CLEANUP_NOTE home-absent:$home"
    return 1
  fi
  if [ -z "$cu" ] || [ ! -x "$cu" ]; then
    BROWSER_CLEANUP_FAILED=1
    BROWSER_CLEANUP_NOTE="$BROWSER_CLEANUP_NOTE cu-exe-missing"
    return 1
  fi
  local name out rc failed=0
  for name in $names; do
    # COMPENSATION, not a repeat. Query first: only an EXPLICIT
    # `browser_session_not_found` means the session is genuinely gone. Any other
    # failure is real and must not be swallowed.
    set +e
    out=$(HOME="$home" "$cu" --target current --grant observe \
      browser-session-status "$name" 2>&1)
    rc=$?
    set -e
    if [ "$rc" -ne 0 ]; then
      case "$out" in
        *browser_session_not_found*)
          BROWSER_CLEANUP_NOTE="$BROWSER_CLEANUP_NOTE $name:already-clean"
          continue ;;
        *)
          failed=1
          BROWSER_CLEANUP_NOTE="$BROWSER_CLEANUP_NOTE $name:status:$rc"
          continue ;;
      esac
    fi
    set +e
    out=$(HOME="$home" "$cu" --target current --grant actuate \
      browser-session-stop "$name" --expect stopped --timeout-ms 20000 2>&1)
    rc=$?
    set -e
    if [ "$rc" -ne 0 ]; then
      case "$out" in
        *browser_session_not_found*) ;;
        *) failed=1; BROWSER_CLEANUP_NOTE="$BROWSER_CLEANUP_NOTE $name:stop:$rc" ;;
      esac
    fi
    set +e
    out=$(HOME="$home" "$cu" --target current --grant actuate \
      browser-session-remove "$name" --expect stopped 2>&1)
    rc=$?
    set -e
    if [ "$rc" -ne 0 ]; then
      case "$out" in
        *browser_session_not_found*) ;;
        *) failed=1; BROWSER_CLEANUP_NOTE="$BROWSER_CLEANUP_NOTE $name:remove:$rc" ;;
      esac
    fi
    # READBACK: the row must be provably gone. A stop that returned success but
    # left the record is not a completed cleanup.
    set +e
    out=$(HOME="$home" "$cu" --target current --grant observe \
      browser-session-status "$name" 2>&1)
    rc=$?
    set -e
    if [ "$rc" -eq 0 ]; then
      failed=1
      BROWSER_CLEANUP_NOTE="$BROWSER_CLEANUP_NOTE $name:still-present"
    else
      case "$out" in
        *browser_session_not_found*) ;;
        *) failed=1; BROWSER_CLEANUP_NOTE="$BROWSER_CLEANUP_NOTE $name:readback:$rc" ;;
      esac
    fi
  done
  if [ "$failed" -ne 0 ]; then
    BROWSER_CLEANUP_FAILED=1
    return 1
  fi
  return 0
}

runner_exit() {
  local rc=$?
  runner_browser_cleanup
  # The transaction's OWN exact manifest, removed AFTER the browser cleanup attempt
  # above so the session is always reclaimed first. This is what covers the signals
  # the ordinary returns cannot: the TERM worker's shell owns both this variable and
  # this trap, so a signal that runs EXIT still removes exactly its own file.
  #
  # The path is not trusted blindly. It is removed only when it is EXACTLY a
  # repo-root `.probe-manifest.*` path, so a corrupted or hostile value can never
  # turn the EXIT path into a general file deletion.
  if [ -n "$BROWSER_TXN_MANIFEST" ]; then
    if probe_manifest_path_owned "$BROWSER_TXN_MANIFEST"; then
      rm -f "$BROWSER_TXN_MANIFEST"
    fi
  fi
  rm -f "$LIVE_REGION"
  # A cleanup failure must not be swallowed by an otherwise-zero exit.
  if [ "$BROWSER_CLEANUP_FAILED" -ne 0 ] && [ "$rc" -eq 0 ]; then
    rc=1
  fi
  exit "$rc"
}

# A signal must NOT do the cleanup itself. Registering the same function for
# EXIT and for HUP/INT/TERM makes it run TWICE on one signal: the `exit` inside
# the signal handler fires the EXIT handler again, so cleanup is executed
# re-entrantly. The cleanup is addressed by a fixed session name and is
# idempotent, so the second pass is not destructive -- but it is still wrong, and
# a future non-idempotent step added here would silently run twice.
#
# The signal handler therefore only maps the signal to a conventional exit code
# and exits; the EXIT trap alone performs the single cleanup. 128+signal is the
# conventional encoding (HUP=129, INT=130, TERM=143).
runner_signal() {
  # DIRECT TERM-HANDLER EVIDENCE. Until now "the handler ran" was inferred from the
  # exit code, which cannot distinguish a handler that exited 143 from a process
  # killed directly by the signal. This witness records the fact itself.
  #
  # Written ONLY for TERM (143) and ONLY when this process is the dedicated TERM
  # worker with a configured witness, so ordinary runs stay write-free. It is a
  # SEPARATE line from cleanup_run: the handler running and the cleanup running are
  # two different facts and must never be conflated.
  if [ "$1" = "143" ] && [ "${BROWSER_TERM_WITNESS:-}" != "" ]; then
    printf 'term_signal=143\n' >>"$BROWSER_TERM_WITNESS"
  fi
  exit "$1"
}
trap runner_exit EXIT
trap 'runner_signal 129' HUP
trap 'runner_signal 130' INT
trap 'runner_signal 143' TERM

live_court_region() {
  # From the cleanup-boundary comment to the end of the fixture helpers: the
  # span that observes processes, walks the chain and terminates the owned
  # handle. Endpoints are matched by their exact comment text, so a rename in
  # the court makes this extraction fail loudly instead of silently shrinking
  # the scanned region.
  awk '
    /^\/\/ --- the observation boundary/ {inside = 1}
    /^\/\/ --- the injected fixture/ {inside = 0}
    inside {print}
  ' "$LIVE"
}

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

# The live court's red gate. Every claim the court makes about refusing an
# unsafe chain must have a mutation that makes the suite fail; a suite that
# cannot fail proves nothing. The mutations run against a COPY of the court so
# the working tree is never the thing under test, and each mutation is applied
# with an exact string replacement that must match, because a mutation that
# silently fails to apply is a false red gate.
live_red_gate() {
  local failed=0 root
  # The task path requires a bounded REPOSITORY-RELATIVE entry, so the mutant
  # cannot live in a temp directory. It goes under the ignored local research
  # state instead, which is repo-local, untracked and outside target/.
  root="$REPO/.agenterm-research-state/live-red-gate"
  rm -rf "$root"
  mkdir -p "$root"
  local mutant="$root/live-rehearsal.qjs"

  # The region is written first: every probe run below needs it, and a probe
  # that ran before it existed would fail on a missing file rather than on the
  # guard under test.
  live_court_region >"$LIVE_REGION"

    # Gate 1: the court's browser-free self-test must PASS when driven through the
  # tool-profile path. The registration this entry will use does not exist yet
  # (the main-side owner of agenterm.tasks.json adds it), so the gate drives the
  # UNMUTATED court through the same probe manifest the mutations use. That keeps
  # the gate runnable now without this file editing the shared manifest.
  local plain
  plain=$(probe_run "$LIVE" "$root/plain.qjs" 2>&1) || true
  case "$plain" in
    *'"ok":true'*)
      printf '  ok   the live court passes its browser-free self-test\n' ;;
    *) printf '  FAIL the live court did not pass its self-test\n'; failed=1 ;;
  esac

  # Gate 0: a live mode must be refused by the COURT, not only by this runner.
  # The runner refusal alone would not protect a caller that bypassed it, and the
  # court is the file that will hold the browser spawn. The refusal must also
  # report that nothing was launched and no ordinal was reserved.
  local refusal
  refusal=$(probe_source "$LIVE" rehearsal) || true
  # Each property is checked independently. A single ordered pattern would pass
  # or fail on JSON key order, which is not a property of the refusal at all.
  live_refusal_ok=1
  for token in '"code":"LIVE_COURT_NOT_ENABLED"' '"browser_launched":false' \
      '"ordinal_reserved":false' \
      '"mode":"rehearsal"'; do
    case "$refusal" in
      *"$token"*) ;;
      *) live_refusal_ok=0 ;;
    esac
  done
  if [ "$live_refusal_ok" -eq 1 ]; then
    printf '  ok   the court refuses a live mode without launching or reserving\n'
  else
    printf '  FAIL the court did not refuse a live mode cleanly (got: %s)\n' "$refusal"
    failed=1
  fi

  # Gate 2: the owned chain must refuse identity drift. The mutation removes the
  # drift comparison; the suite must then report a failure, because the drift
  # scene is the only thing standing between a reused pid and unproven
  # ownership.
  mutate_and_expect_fail "$LIVE" "$mutant" \
    'if (before.start_identity !== after.start_identity) {' \
    'if (false) {' \
    'identity drift' || failed=1

  # Gate 3: the cleanup must not read `unknown` as death. The mutation makes the
  # unknown branch return a death record; the budget scene must then fail.
  mutate_and_expect_fail "$LIVE" "$mutant" \
    'if (observed.state === "unknown") {' \
    'if (observed.state === "unknown") { return {ok: true, observation: {state: "dead", start_identity: null}};' \
    'unknown-is-not-death' || failed=1

  # Gate 4: the terminus check must compare the frozen identity. Removing the
  # comparison must break the scene that proves a wrong-identity terminus is
  # refused.
  mutate_and_expect_fail "$LIVE" "$mutant" \
    'if (before.start_identity !== browser_identity) {' \
    'if (false) {' \
    'terminus identity' || failed=1

  # Gate 5: the parent-relation read must be bracketed. The closing observation
  # now goes through `bracket_after_observe`, so the mutation makes that hook
  # return the OPENING record instead of a fresh one: the bracket can then no
  # longer see a change, and the drift scene must fail.
  mutate_and_expect_fail "$LIVE" "$mutant" \
    '  return seen;
}' \
    '  return {state: "live", start_identity: "echoed-by-mutation"};
}' \
    'chain bracket' || failed=1

  # Gate 6: the cleanup input this court builds must stay inside the model's
  # whitelist. The mutation adds the selector field the spec forbids, so the
  # selector-leak scene must fail. The model itself is not mutated: it is a
  # module with `export`, so it cannot be driven as a task entry, and a mutation
  # that cannot run is not a red gate.
  mutate_and_expect_fail "$LIVE" "$mutant" \
    '  }).ok === true;' \
    '  }).ok !== true;' \
    'cleanup selector independence' || failed=1

  # Gate 7: the chain ceiling must be finite. An infinite ceiling must break the
  # ceiling scene.
  mutate_and_expect_fail "$LIVE" "$mutant" \
    'for (let hop = 0; hop < CHAIN_CEILING; hop = hop + 1) {' \
    'for (let hop = 0; hop < 100000; hop = hop + 1) {' \
    'chain ceiling' || failed=1

  # Gate 8: the per-state door adapter must classify `dead`/`unknown` instead of
  # killing them at the shape check. The mutation restores the old single-shape
  # assumption, so the parsed variant flags are never set.
  mutate_and_expect_fail "$LIVE" "$mutant" \
    '  const state = raw.state;' \
    '  const state = "live";' \
    'per-state door adapter' || failed=1

  # Gate 9: the TERMINUS must be decided before the parent-live constraint. The
  # mutation makes the non-terminal parent requirement unconditional, so the
  # terminus scenes for a non-live parent record must fail.
  mutate_and_expect_fail "$LIVE" "$mutant" \
    '    if (record.state !== "live" || typeof record.parent_id !== "number") {' \
    '    if (true) {' \
    'terminus before parent-live' || failed=1

  # Gate 10: exhausting the bound must be reported AS exhaustion, not as an
  # assumed death. The mutation makes the exhausted path claim termination, so
  # the scenes that require INCONCLUSIVE_CLEANUP must fail.
  #
  # The poll-ceiling guard is deliberately NOT mutated here: removing it makes
  # the stalled-clock scene non-terminating, and a gate that hangs is not a gate.
  # The ceiling is instead covered by the stalled-clock scene itself, which can
  # only report `poll_ceiling_exhausted` if the ceiling exists.
  mutate_and_expect_fail "$LIVE" "$mutant" \
    '  return {ok: false, code: "INCONCLUSIVE_CLEANUP",' \
    '  return {ok: true, code: "TERMINATION_PROVEN", death: [],' \
    'cleanup exhaustion is not death' || failed=1

  # Gate 11: the cleanup must read identity drift as PID reuse. The mutation
  # removes the reuse branch, so the reuse scene must fail.
  mutate_and_expect_fail "$LIVE" "$mutant" \
    '    if (observed.start_identity !== identity.start_identity) {' \
    '    if (false) {' \
    'cleanup pid reuse' || failed=1

  # Gate 12: the region the runner hands to V7 must be non-empty and must not
  # itself contain a forbidden field name. An extractor that silently matched
  # nothing would make V7 vacuously true.
  if [ -s "$LIVE_REGION" ] && ! grep -F -q 'selector_output' "$LIVE_REGION"; then
    printf '  ok   the live region is non-empty and selector-free\n'
  else
    printf '  FAIL the live region is empty or carries a selector field\n'; failed=1
  fi

  rm -rf "$root"
  if [ "$failed" -eq 0 ]; then
    printf '\n%s\n' 'LIVE_RED_GATE_PASS'
    return 0
  fi
  printf '\n%s\n' 'LIVE_RED_GATE_FAILED'
  return 1
}

# Run an arbitrary court source through the registered tool-profile path. The
# repository manifest is copied with only its task and contract tables replaced,
# so the project capabilities and script-API pins are exactly the ones the real
# registration runs under. The temporary manifest is removed before returning.
# Passing the entry outside the repository is the whole point: a mutation must
# be executed without editing the shared manifest or the working tree.
probe_run() {
  local source="$1" target="$2"
  cp "$source" "$target"
  probe_source "$target"
}

# Does the EXTRACTED court verdict name the expected failure? This is the single
# matcher, used by the real red gate and by the browser-free harness self-test, so
# the boundary it enforces cannot drift between the two.
#
# It deliberately inspects ONLY the verdict text. A run whose named code appears in
# the surrounding stdout but not in the verdict must NOT pass: that shape is
# exactly what the self-test pins down.
verdict_names_failure() {
  local verdict="$1" want="$2"
  [ -n "$want" ] || return 1
  case "$verdict" in
    *"$want"*) return 0 ;;
  esac
  return 1
}

# Browser-free proof of the verdict-token boundary. No browser, no task, no
# manifest: it exercises the shared matcher directly.
# Does the EXTRACTED verdict carry every POSITIVE requirement of the
# ordinary-exception cleanup control? Each entry demands the literal YES-form, so
# a MISSING field, an explicit null, or a false can never satisfy it -- that is the
# whole point of asserting the exact text instead of parsing the JSON.
# Shared by the real control and the browser-free self-test, so the boundary they
# enforce cannot drift.
exception_control_missing_claims() {
  local verdict="$1" tok missing=""
  for tok in '"ok":false' \
      '"browser_control_forced_throw"' \
      '"primary_cleanup_ok":false' '"primary_cleanup_complete":false' \
      '"compensation_ran":true' '"compensation_cleanup_ok":true' \
      '"compensation_cleanup_complete":true' \
      '"compensation_inventory_restored":true' \
      '"compensation_inventory_live":0' '"compensation_inventory_rows":0' \
      '"formal_root_touched":false' '"ordinal_reserved":false'; do
    case "$verdict" in
      *"$tok"*) ;;
      *) missing="$missing $tok" ;;
    esac
  done
  printf '%s' "$missing"
}

# The FULL judgement for a TERM cleanup-control round, as a pure decision.
#
# This is the only failure class where the runner itself is signalled. It requires
# the worker's TERM handler to map the signal to 143, its EXIT trap to perform the
# SINGLE cleanup, and that cleanup to be provably name-addressed and idempotent.
#
# Arguments: before/after formal-root snapshots, before/after real-HOME session
# snapshots, the worker rc, the controller's started marker count, the kill marker
# count, the CLEANUP witness count, the AUTHORITATIVE STATUS COMMAND's exit code,
# the authoritative status text, whether the probe manifest is ABSENT, the combined
# output text, the DIRECT TERM-handler witness count, the raw worker exit code the
# controller observed, and the raw terminating signal number (0 when none).
#
# EVERY numeric-looking argument is compared as an exact STRING. `-ne` would abort
# with a shell diagnostic on an empty or non-numeric value, which would surface as
# a crash rather than a named failure -- and an empty value is precisely what a
# missing or unreadable witness produces. Exact comparison makes missing, empty
# and malformed all land on the same named refusal.
# Prints the failure name, or nothing when the root may be deleted.
term_control_stop_reason() {
  local before="$1" after="$2" rs_before="$3" rs_after="$4" rc="$5"
  local started="$6" killed="$7" witness="$8" status_rc="$9"
  local status="${10}" manifest_absent="${11}" out="${12}"
  local term_signal_n="${13:-}" worker_exit="${14:-}" worker_signal="${15:-0}"
  if [ "$before" != "$after" ]; then
    printf '%s' BROWSER_CLEANUP_CONTROL_FORMAL_ROOT_CHANGED; return
  fi
  if [ "$rs_before" != "$rs_after" ]; then
    printf '%s' BROWSER_CLEANUP_CONTROL_REAL_HOME_ROOT_CHANGED; return
  fi
  # The round must have reached the state the controller AUTHORITATIVELY confirmed:
  # a live, ready session in this same HOME. The worker's own file write cannot
  # establish that -- it only proves the worker armed -- so the evidence here is the
  # controller's post-confirmation marker.
  if [ "$started" != "1" ]; then
    printf 'BROWSER_CLEANUP_CONTROL_NOT_STARTED:%s' "${started:-<empty>}"; return
  fi
  # Exactly ONE TERM must have been delivered. Asserted rather than assumed, so a
  # controller that retried cannot pass.
  if [ "$killed" != "1" ]; then
    printf 'BROWSER_CLEANUP_CONTROL_KILL_COUNT:%s' "${killed:-<empty>}"; return
  fi
  # The TERM handler must have produced the conventional 143.
  if [ "$rc" != "143" ]; then
    printf 'BROWSER_CLEANUP_CONTROL_EXIT_NOT_143:%s' "${rc:-<empty>}"; return
  fi
  # THE HANDLER RAN, PROVEN DIRECTLY. "rc is 143" is not proof that a handler ran:
  # a process killed directly by SIGTERM also stops, and the two cases were
  # previously indistinguishable. The witness line the handler itself appends is the
  # direct fact, and it must appear EXACTLY once -- zero means it never ran, more
  # than one means it ran more than once. A missing/empty count is a named refusal,
  # not an accepted default.
  case "$term_signal_n" in
    "") printf '%s' 'BROWSER_CLEANUP_CONTROL_TERM_WITNESS_MISSING:<empty>'; return ;;
  esac
  if [ "$term_signal_n" != "1" ]; then
    printf 'BROWSER_CLEANUP_CONTROL_TERM_WITNESS_COUNT:%s' "$term_signal_n"; return
  fi
  # THE RAW WAIT STATUS MUST AGREE. The controller reports the exit code it actually
  # reaped; the two readings are different facts (high byte = exit code, low 7 bits =
  # terminating signal). The SIGNAL is checked FIRST, because a worker killed
  # directly by SIGTERM reaps as exit code 0 with signal 15 -- checking the exit code
  # first would report that case as a generic marker mismatch and hide what actually
  # happened. Requiring 143 with NO terminating signal is what separates "handler
  # called exit 143" from "process died from SIGTERM".
  if [ "$worker_signal" != "0" ]; then
    printf 'BROWSER_CLEANUP_CONTROL_WORKER_DIED_BY_SIGNAL:%s' "${worker_signal:-<empty>}"
    return
  fi
  if [ "$worker_exit" != "143" ]; then
    printf 'BROWSER_CLEANUP_CONTROL_WORKER_EXIT_MARKER_MISMATCH:%s' "${worker_exit:-<empty>}"
    return
  fi
  # The EXIT trap's cleanup must have run EXACTLY once. This is a SEPARATE fact from
  # the handler witness above: the handler firing and the cleanup running are
  # different events, counted from different lines, and claimed separately.
  if [ "$witness" != "1" ]; then
    printf 'BROWSER_CLEANUP_CONTROL_WITNESS_COUNT:%s' "${witness:-<empty>}"; return
  fi
  # AUTHORITATIVE ABSENCE is TWO facts, not a substring. The status command must
  # FAIL (rc != 0) AND its JSON must carry the exact `code` FIELD with the
  # not-found value. A loose text search would accept any payload that merely
  # mentions the token somewhere -- a different field, an error message quoting it,
  # or a successful read of some other object. Only the exact field is proof.
  if [ "$status_rc" = "0" ]; then
    printf 'BROWSER_CLEANUP_CONTROL_STATUS_STILL_PRESENT'; return
  fi
  case "$status" in
    *'"code":"browser_session_not_found"'*) ;;
    *) printf '%s' BROWSER_CLEANUP_CONTROL_STATUS_NOT_ABSENT; return ;;
  esac
  # No success token may survive from a killed run.
  case "$out" in
    *'PASS: exact-process live browser baseline'*)
      printf '%s' BROWSER_CLEANUP_CONTROL_PASS_TOKEN; return ;;
    *'EVIDENCE research.profile-binding-exact-process.live-browser-baseline'*)
      printf '%s' BROWSER_CLEANUP_CONTROL_EVIDENCE_TOKEN; return ;;
    *'browser-baseline/v1'*)
      printf '%s' BROWSER_CLEANUP_CONTROL_ENVELOPE_PRESENT; return ;;
  esac
  # The transaction's OWN manifest must be gone. A residue here is exactly the
  # defect this guard exists to catch, so a MISSING or 0 marker is refused by NAME
  # rather than allowed to pass as "not proven present".
  if [ "$manifest_absent" != "1" ]; then
    printf 'BROWSER_CLEANUP_CONTROL_MANIFEST_PRESENT:%s' "${manifest_absent:-<empty>}"
    return
  fi
}

# The FULL post-cleanup judgement for a HOST-OP TRAP round, as a pure decision.
#
# The failure class differs fundamentally from the ordinary exception: the engine
# refuses a door operation and the script dies from a WASM fault, so the court
# NEVER BUILDS AN ENVELOPE. "No court envelope" is therefore EXPECTED here, not a
# defect, and this must not be conflated with the ordinary-exception control that
# requires one.
#
# Shared by the real control and the browser-free self-test, so the boundary they
# enforce cannot drift.
#
# Arguments: before/after formal-root snapshots, before/after real-HOME session
# snapshots, the probe rc, and the full stdout+stderr text.
# Prints the failure name, or nothing when the root may be deleted.
host_op_trap_stop_reason() {
  local before="$1" after="$2" rs_before="$3" rs_after="$4" rc="$5" out="$6"
  if [ "$before" != "$after" ]; then
    printf '%s' BROWSER_CLEANUP_CONTROL_FORMAL_ROOT_CHANGED; return
  fi
  if [ "$rs_before" != "$rs_after" ]; then
    printf '%s' BROWSER_CLEANUP_CONTROL_REAL_HOME_ROOT_CHANGED; return
  fi
  if [ "$rc" -eq 0 ]; then
    # The control must FAIL the run: an exit 0 means the trap was never reached,
    # so nothing about this failure class was exercised.
    printf 'BROWSER_CLEANUP_CONTROL_NOT_FAILED:%s' "$rc"; return
  fi
  # The trap must be the ENGINE's own refusal, named exactly. Asserting the
  # precise token is what separates a genuine budget trap from an ordinary script
  # error that merely made the run fail -- an rc-only check would accept both.
  case "$out" in
    *'budget exhausted: max_host_ops'*) ;;
    *) printf '%s' BROWSER_CLEANUP_CONTROL_TRAP_TOKEN_ABSENT; return ;;
  esac
  # A court envelope must NOT be present: the script was killed before it could
  # build one. Its presence means the trap did not actually land where claimed.
  case "$out" in
    *'browser-baseline/v1'*)
      printf '%s' BROWSER_CLEANUP_CONTROL_ENVELOPE_PRESENT; return ;;
  esac
  # No success token may survive.
  case "$out" in
    *'PASS: exact-process live browser baseline'*)
      printf '%s' BROWSER_CLEANUP_CONTROL_PASS_TOKEN; return ;;
    *'EVIDENCE research.profile-binding-exact-process.live-browser-baseline'*)
      printf '%s' BROWSER_CLEANUP_CONTROL_EVIDENCE_TOKEN; return ;;
  esac
}

# The FULL post-cleanup judgement for an ordinary-exception round, as a pure
# decision. It answers: given everything we observed, may the disposable root be
# reclaimed, and if not, by WHICH named failure?
#
# It is factored out for one reason: the ordering invariant ("no deletion before
# every judgement passes") is the property that matters, and it cannot be proven
# by a browser-free test unless the decision is callable WITHOUT a browser. The
# real control and the self-test therefore share this one implementation, so the
# boundary they enforce cannot drift.
#
# Arguments: before/after formal-root snapshots, before/after real-HOME session
# snapshots, the probe rc, and the full stdout+verdict text.
# Prints the failure name, or nothing when the root may be deleted.
exception_control_stop_reason() {
  local before="$1" after="$2" rs_before="$3" rs_after="$4" rc="$5" out="$6"
  local verdict
  verdict=$(printf '%s\n' "$out" | grep 'browser-baseline/v1' | head -1)
  if [ "$before" != "$after" ]; then
    printf '%s' BROWSER_CLEANUP_CONTROL_FORMAL_ROOT_CHANGED; return
  fi
  if [ "$rs_before" != "$rs_after" ]; then
    printf '%s' BROWSER_CLEANUP_CONTROL_REAL_HOME_ROOT_CHANGED; return
  fi
  if [ "$rc" -eq 0 ]; then
    # The control must FAIL the run: a mutation that exits 0 changed nothing.
    printf 'BROWSER_CLEANUP_CONTROL_NOT_FAILED:%s' "$rc"; return
  fi
  if [ -z "$verdict" ]; then
    printf '%s' BROWSER_CLEANUP_CONTROL_NO_COURT_ENVELOPE; return
  fi
  # No success token may survive. `PASS`/`EVIDENCE` are matched on whole stdout
  # (they are runner-side lines, not verdict fields), while the failure token is
  # matched against the EXTRACTED verdict through the SAME shared matcher the red
  # gates use -- a token present only in stdout must never satisfy it.
  case "$out" in
    *'PASS: exact-process live browser baseline'*)
      printf '%s' BROWSER_CLEANUP_CONTROL_PASS_TOKEN; return ;;
    *'EVIDENCE research.profile-binding-exact-process.live-browser-baseline'*)
      printf '%s' BROWSER_CLEANUP_CONTROL_EVIDENCE_TOKEN; return ;;
  esac
  if ! verdict_names_failure "$verdict" 'browser_control_forced_throw'; then
    printf '%s' BROWSER_CLEANUP_CONTROL_FAILURE_TOKEN; return
  fi
  # The failure list must be nonempty, so "ok:false with no reason" cannot pass.
  case "$verdict" in
    *'"failed":[]'*) printf '%s' BROWSER_CLEANUP_CONTROL_NO_FAILURE; return ;;
  esac
  # POSITIVE assertions via the SHARED claim helper. Each requires the literal
  # YES-form, so a missing field, a null, or a false can never satisfy the check.
  local missing
  missing=$(exception_control_missing_claims "$verdict")
  if [ -n "$missing" ]; then
    printf 'BROWSER_CLEANUP_CONTROL_CLAIM:%s' "$missing"
  fi
}

harness_self_test() {
  local failed=0
  local verdict='{"schema":"...browser-baseline/v1","ok":false,"failed":["subject_identity_mismatch"]}'

  if verdict_names_failure "$verdict" "subject_identity_mismatch"; then
    printf '  ok   a verdict naming the expected failure matches\n'
  else
    printf '  FAIL a verdict naming the expected failure did not match\n'; failed=1
  fi

  # THE ADVERSARIAL SHAPE: the token is present in the surrounding stdout but NOT
  # in the verdict. This must fail, and must fail by the token's own name.
  local out_but_not_verdict='subject_identity_mismatch'
  local verdict_without='{"schema":"...browser-baseline/v1","ok":false,"failed":["browser_some_other_failure"]}'
  if verdict_names_failure "$verdict_without" "$out_but_not_verdict"; then
    printf '  FAIL a token present only in stdout was accepted\n'; failed=1
  else
    printf '  ok   a token present only in stdout is refused\n'
  fi

  # A missing/empty token must never match, and must never match the empty string
  # as a substring of anything.
  if verdict_names_failure "$verdict" ""; then
    printf '  FAIL an empty expected token matched\n'; failed=1
  else
    printf '  ok   an empty expected token is refused\n'
  fi

  # An unrelated token must not match.
  if verdict_names_failure "$verdict" "browser_ownership_unprovable"; then
    printf '  FAIL an unrelated token matched\n'; failed=1
  else
    printf '  ok   an unrelated token is refused\n'
  fi

  # --- ordinary-exception positive-claim boundary ---------------------------------
  # A verdict carrying ALL required positive claims must be accepted.
  local exc_ok='{"schema":"...browser-baseline/v1","ok":false,"failed":["browser_control_forced_throw"],"primary_cleanup_ok":false,"primary_cleanup_complete":false,"compensation_ran":true,"compensation_cleanup_ok":true,"compensation_cleanup_complete":true,"compensation_inventory_restored":true,"compensation_inventory_live":0,"compensation_inventory_rows":0,"formal_root_touched":false,"ordinal_reserved":false}'
  if [ -z "$(exception_control_missing_claims "$exc_ok")" ]; then
    printf '  ok   a fully-claimed exception verdict is accepted\n'
  else
    printf '  FAIL a fully-claimed exception verdict was refused\n'; failed=1
  fi

  # MISSING compensation fields must NOT pass. This is the defect that matters:
  # an envelope that simply omits the recovery layer proves nothing about it.
  local exc_absent='{"schema":"...browser-baseline/v1","ok":false,"failed":["browser_control_forced_throw"],"primary_cleanup_ok":false,"primary_cleanup_complete":false,"formal_root_touched":false,"ordinal_reserved":false}'
  if [ -n "$(exception_control_missing_claims "$exc_absent")" ]; then
    printf '  ok   missing compensation fields are refused\n'
  else
    printf '  FAIL missing compensation fields were accepted\n'; failed=1
  fi

  # NULL compensation values must NOT pass either: `null` is what the court emits
  # when compensation never ran, and that is precisely the case this control exists
  # to exclude.
  local exc_null='{"schema":"...browser-baseline/v1","ok":false,"failed":["browser_control_forced_throw"],"primary_cleanup_ok":false,"primary_cleanup_complete":false,"compensation_ran":true,"compensation_cleanup_ok":null,"compensation_cleanup_complete":null,"compensation_inventory_restored":null,"compensation_inventory_live":null,"compensation_inventory_rows":null,"formal_root_touched":false,"ordinal_reserved":false}'
  if [ -n "$(exception_control_missing_claims "$exc_null")" ]; then
    printf '  ok   null compensation values are refused\n'
  else
    printf '  FAIL null compensation values were accepted\n'; failed=1
  fi

  # A FALSE compensation must NOT pass: a compensation that ran but did not
  # restore the inventory is a real failure, not a success.
  local exc_false='{"schema":"...browser-baseline/v1","ok":false,"failed":["browser_control_forced_throw"],"primary_cleanup_ok":false,"primary_cleanup_complete":false,"compensation_ran":true,"compensation_cleanup_ok":true,"compensation_cleanup_complete":true,"compensation_inventory_restored":false,"compensation_inventory_live":1,"compensation_inventory_rows":1,"formal_root_touched":false,"ordinal_reserved":false}'
  if [ -n "$(exception_control_missing_claims "$exc_false")" ]; then
    printf '  ok   an unrestored compensation inventory is refused\n'
  else
    printf '  FAIL an unrestored compensation inventory was accepted\n'; failed=1
  fi

  # --- POST-CLEANUP PRESERVE-ROOT DECISION ----------------------------------------
  # The ordering invariant this suite now enforces: a round that already cleaned
  # up must STILL preserve its root when any later judgement fails. These are the
  # pure-decision boundaries, so no browser is needed to prove them.

  # An entirely valid round decides "delete" -- i.e. an EMPTY stop reason.
  local d_ok
  d_ok=$(exception_control_stop_reason "same" "same" "s" "s" 1 "$exc_ok")
  if [ -z "$d_ok" ]; then
    printf '  ok   a fully-passing round reclaims its root\n'
  else
    printf '  FAIL a fully-passing round preserved its root: %s\n' "$d_ok"; failed=1
  fi

  # A MISSING compensation field AFTER a successful cleanup must preserve the root,
  # and must name the missing field rather than a generic failure. The check names
  # the FIRST absent claim, so only that one is matched here.
  local d_claim
  d_claim=$(exception_control_stop_reason "same" "same" "s" "s" 1 "$exc_absent")
  case "$d_claim" in
    BROWSER_CLEANUP_CONTROL_CLAIM:*'"compensation_ran":true'*)
      printf '  ok   a missing compensation claim preserves the root and names it\n' ;;
    *)
      printf '  FAIL a missing compensation claim did not preserve the root: %s\n' "$d_claim"
      failed=1 ;;
  esac

  # A PASS token surviving into a cleaned-up round must preserve the root. Note
  # the ORDER the decision uses: a missing court envelope is checked FIRST, so the
  # fixture must carry a well-formed failing verdict for the PASS-token rule to be
  # the one that fires.
  local d_pass_out
  d_pass_out=$(printf '%s\n%s' 'PASS: exact-process live browser baseline' "$exc_ok")
  local d_pass
  d_pass=$(exception_control_stop_reason "same" "same" "s" "s" 1 "$d_pass_out")
  if [ "$d_pass" = BROWSER_CLEANUP_CONTROL_PASS_TOKEN ]; then
    printf '  ok   a surviving PASS token preserves the root\n'
  else
    printf '  FAIL a surviving PASS token did not preserve the root: %s\n' "$d_pass"
    failed=1
  fi

  # An absent court envelope ALSO preserves the root (a crashed or truncated run
  # is the case that most needs its evidence kept).
  local d_env
  d_env=$(exception_control_stop_reason "same" "same" "s" "s" 1 "no envelope here")
  if [ "$d_env" = BROWSER_CLEANUP_CONTROL_NO_COURT_ENVELOPE ]; then
    printf '  ok   an absent court envelope preserves the root\n'
  else
    printf '  FAIL an absent court envelope did not preserve the root: %s\n' "$d_env"
    failed=1
  fi

  # rc=0 (the mutation changed nothing) must preserve the root, not delete it.
  local d_rc
  d_rc=$(exception_control_stop_reason "same" "same" "s" "s" 0 "$exc_ok")
  case "$d_rc" in
    BROWSER_CLEANUP_CONTROL_NOT_FAILED:0)
      printf '  ok   an unexpected rc=0 preserves the root\n' ;;
    *)
      printf '  FAIL an unexpected rc=0 did not preserve the root: %s\n' "$d_rc"
      failed=1 ;;
  esac

  # A changed real-HOME session root preserves the root.
  local d_home
  d_home=$(exception_control_stop_reason "same" "same" "s" "CHANGED" 1 "$exc_ok")
  if [ "$d_home" = BROWSER_CLEANUP_CONTROL_REAL_HOME_ROOT_CHANGED ]; then
    printf '  ok   a changed real HOME session root preserves the root\n'
  else
    printf '  FAIL a changed real HOME root did not preserve the root: %s\n' "$d_home"
    failed=1
  fi

  # --- HOST-OP TRAP DECISION (the same shared helper the real control calls) ------
  # A well-formed trap round decides "delete" -- an EMPTY stop reason.
  local t_ok
  t_ok=$(host_op_trap_stop_reason "same" "same" "s" "s" 1 \
    "the script threw: budget exhausted: max_host_ops")
  if [ -z "$t_ok" ]; then
    printf '  ok   a genuine host-op trap reclaims its root\n'
  else
    printf '  FAIL a genuine host-op trap preserved its root: %s\n' "$t_ok"; failed=1
  fi

  # The confusion this control exists to prevent: an ORDINARY script error also
  # exits nonzero, so an rc-only check would falsely accept it. It must be refused
  # by NAME for lacking the engine's own token.
  local t_plain
  t_plain=$(host_op_trap_stop_reason "same" "same" "s" "s" 1 \
    '{"failed":["browser_control_forced_throw"]}')
  if [ "$t_plain" = BROWSER_CLEANUP_CONTROL_TRAP_TOKEN_ABSENT ]; then
    printf '  ok   an ordinary script error is not accepted as a trap\n'
  else
    printf '  FAIL an ordinary script error was accepted as a trap: %s\n' "$t_plain"
    failed=1
  fi

  # The engine token AND a court envelope together are contradictory: the trap is
  # supposed to kill the script before it can build one.
  local t_both
  t_both=$(host_op_trap_stop_reason "same" "same" "s" "s" 1 \
    'budget exhausted: max_host_ops {"schema":"...browser-baseline/v1"}')
  if [ "$t_both" = BROWSER_CLEANUP_CONTROL_ENVELOPE_PRESENT ]; then
    printf '  ok   a trap token beside a court envelope is refused\n'
  else
    printf '  FAIL a trap token beside a court envelope was accepted: %s\n' "$t_both"
    failed=1
  fi

  # rc=0 means the trap was never reached, so nothing about this class was tried.
  local t_rc
  t_rc=$(host_op_trap_stop_reason "same" "same" "s" "s" 0 \
    'budget exhausted: max_host_ops')
  case "$t_rc" in
    BROWSER_CLEANUP_CONTROL_NOT_FAILED:0)
      printf '  ok   an untripped trap round preserves the root\n' ;;
    *)
      printf '  FAIL an untripped trap round did not preserve the root: %s\n' "$t_rc"
      failed=1 ;;
  esac

  # A surviving baseline PASS token preserves the root.
  local t_pass
  t_pass=$(host_op_trap_stop_reason "same" "same" "s" "s" 1 \
    'budget exhausted: max_host_ops PASS: exact-process live browser baseline')
  if [ "$t_pass" = BROWSER_CLEANUP_CONTROL_PASS_TOKEN ]; then
    printf '  ok   a surviving PASS token beside a trap preserves the root\n'
  else
    printf '  FAIL a surviving PASS token beside a trap was accepted: %s\n' "$t_pass"
    failed=1
  fi

  # --- TERM DECISION (the same shared helper the real control calls) -------------
  # A fully-formed TERM round decides "delete" -- an EMPTY stop reason. Arguments:
  # snapshots x4, rc, started, killed, witness, status_rc, status text, out.
  local m_ok
  m_ok=$(term_control_stop_reason "same" "same" "s" "s" 143 1 1 1 1 \
    '{"code":"browser_session_not_found"}' 1 'term_controller_started=1 term_controller_killed=1' 1 143 0)
  if [ -z "$m_ok" ]; then
    printf '  ok   a complete TERM round reclaims its root\n'
  else
    printf '  FAIL a complete TERM round preserved its root: %s\n' "$m_ok"; failed=1
  fi

  # A wrong exit code (e.g. the shell resumed and finished at 0) must be refused.
  local m_rc
  m_rc=$(term_control_stop_reason "same" "same" "s" "s" 0 1 1 1 1 \
    '{"code":"browser_session_not_found"}' 1 'x' 1 143 0)
  case "$m_rc" in
    BROWSER_CLEANUP_CONTROL_EXIT_NOT_143:0)
      printf '  ok   a resumed shell exiting 0 is refused\n' ;;
    *) printf '  FAIL a non-143 exit was accepted: %s\n' "$m_rc"; failed=1 ;;
  esac

  # More than one cleanup run must be refused: "exactly once" is the claim.
  local m_w
  m_w=$(term_control_stop_reason "same" "same" "s" "s" 143 1 1 2 1 \
    '{"code":"browser_session_not_found"}' 1 'x' 1 143 0)
  case "$m_w" in
    BROWSER_CLEANUP_CONTROL_WITNESS_COUNT:2)
      printf '  ok   a doubled cleanup witness is refused\n' ;;
    *) printf '  FAIL a doubled cleanup witness was accepted: %s\n' "$m_w"; failed=1 ;;
  esac

  # A session that is NOT authoritatively absent must be refused: the command
  # SUCCEEDED, so the session still exists however its text reads.
  local m_st
  m_st=$(term_control_stop_reason "same" "same" "s" "s" 143 1 1 1 0 \
    '{"code":"browser_session_not_found"}' 1 'x' 1 143 0)
  if [ "$m_st" = BROWSER_CLEANUP_CONTROL_STATUS_STILL_PRESENT ]; then
    printf '  ok   a successful status read is refused even if it names not-found\n'
  else
    printf '  FAIL a successful status read was accepted: %s\n' "$m_st"; failed=1
  fi

  # A non-zero status whose text lacks the exact token must be refused too: a
  # generic failure is not proof of absence.
  local m_nt
  m_nt=$(term_control_stop_reason "same" "same" "s" "s" 143 1 1 1 1 \
    'browser_session_error: permission denied' 1 'x' 1 143 0)
  if [ "$m_nt" = BROWSER_CLEANUP_CONTROL_STATUS_NOT_ABSENT ]; then
    printf '  ok   a non-zero status without the exact token is refused\n'
  else
    printf '  FAIL a non-zero status without the exact token was accepted: %s\n' "$m_nt"
    failed=1
  fi

  # The token as a BARE STRING somewhere else in the payload is NOT proof: only the
  # exact `code` field counts. This is the difference between a loose text search
  # and reading the field the contract actually defines.
  local m_bare
  m_bare=$(term_control_stop_reason "same" "same" "s" "s" 143 1 1 1 1 \
    '{"detail":"browser_session_not_found"}' 1 'x' 1 143 0)
  if [ "$m_bare" = BROWSER_CLEANUP_CONTROL_STATUS_NOT_ABSENT ]; then
    printf '  ok   a bare mention without the code field is refused\n'
  else
    printf '  FAIL a bare mention without the code field was accepted: %s\n' "$m_bare"
    failed=1
  fi

  # Only the non-zero rc PLUS the exact code field is accepted.
  local m_exact
  m_exact=$(term_control_stop_reason "same" "same" "s" "s" 143 1 1 1 1 \
    '{"code":"browser_session_not_found"}' 1 'x' 1 143 0)
  if [ -z "$m_exact" ]; then
    printf '  ok   a non-zero rc with the exact code field is accepted\n'
  else
    printf '  FAIL the exact code field was refused: %s\n' "$m_exact"; failed=1
  fi

  # --- FORK-PIPE CONTRACT (the controller status probe) --------------------------
  # The controller reads the status command through a Perl fork pipe. The form
  # "-|" forks and connects the child's stdout to the parent; a plain "-" does
  # NOT fork and never yields a child branch. This proves the CONTRACT the control
  # depends on, hermetically -- no browser, no files: a child that prints JSON and
  # exits 0 is readable with rc 0, and a child that prints a not-found code and
  # exits non-zero is readable with that non-zero rc.
  local fork_pipe_out
  fork_pipe_out=$(perl -e '
    my $run = sub {
      my ($payload, $code) = @_;
      my $pid = open(my $fh, "-|");
      die "pipe failed: $!" unless defined $pid;
      if ($pid == 0) {
        # CHILD: its normal STDOUT is already the pipe the parent reads, so it must
        # simply print. Redirecting STDOUT onto the handle here would send the data
        # back up its own write end and lose it.
        print $payload;
        exit $code;
      }
      # READ FIRST, then close. Closing before reading discards the very output the
      # pipe exists to carry, and `close` on a fork pipe is what REAPS the child --
      # so the status must be read from `$?` right here. A later `waitpid` would find
      # nothing left to reap and return a meaningless value.
      my $out = "";
      while (my $line = <$fh>) { $out .= $line; }
      close($fh);
      return (($? >> 8), $out);
    };
    my ($ok_rc, $ok_out) = $run->("{\"state\":\"ready\"}\n", 0);
    print "control:", ($ok_rc == 0 && $ok_out =~ /"state":"ready"/) ? "ok" : "FAIL", "\n";
    my ($bad_rc, $bad_out) = $run->("{\"code\":\"browser_session_not_found\"}\n", 7);
    print "treatment:", ($bad_rc == 7
      && $bad_out =~ /"code":"browser_session_not_found"/) ? "ok" : "FAIL", "\n";
  ' 2>&1)
  case "$fork_pipe_out" in
    *control:ok*treatment:ok*)
      printf '  ok   the fork pipe reads child stdout and captures its rc\n' ;;
    *)
      printf '  FAIL the fork pipe contract did not hold: %s\n' "$fork_pipe_out"; failed=1 ;;
  esac

  # A surviving baseline PASS token must be refused.
  local m_p
  m_p=$(term_control_stop_reason "same" "same" "s" "s" 143 1 1 1 1 \
    '{"code":"browser_session_not_found"}' 1 'PASS: exact-process live browser baseline' 1 143 0)
  if [ "$m_p" = BROWSER_CLEANUP_CONTROL_PASS_TOKEN ]; then
    printf '  ok   a surviving PASS token beside a TERM run is refused\n'
  else
    printf '  FAIL a surviving PASS token beside a TERM run was accepted: %s\n' "$m_p"
    failed=1
  fi

  # A failed or retried kill must be refused: only ONE delivery is allowed.
  local m_k
  m_k=$(term_control_stop_reason "same" "same" "s" "s" 143 1 0 1 1 \
    '{"code":"browser_session_not_found"}' 1 'x' 1 143 0)
  case "$m_k" in
    BROWSER_CLEANUP_CONTROL_KILL_COUNT:0)
      printf '  ok   a missing TERM delivery is refused\n' ;;
    *) printf '  FAIL a missing TERM delivery was accepted: %s\n' "$m_k"; failed=1 ;;
  esac
  local m_k2
  m_k2=$(term_control_stop_reason "same" "same" "s" "s" 143 1 2 1 1 \
    '{"code":"browser_session_not_found"}' 1 'x' 1 143 0)
  case "$m_k2" in
    BROWSER_CLEANUP_CONTROL_KILL_COUNT:2)
      printf '  ok   a retried TERM is refused\n' ;;
    *) printf '  FAIL a retried TERM was accepted: %s\n' "$m_k2"; failed=1 ;;
  esac

  # A round that never reached the confirmed-started state proves nothing about a
  # live transaction, so it must be refused even if everything else looks right.
  local m_ns
  m_ns=$(term_control_stop_reason "same" "same" "s" "s" 143 0 1 1 1 \
    '{"code":"browser_session_not_found"}' 1 'x' 1 143 0)
  case "$m_ns" in
    BROWSER_CLEANUP_CONTROL_NOT_STARTED:0)
      printf '  ok   a round that never started is refused\n' ;;
    *) printf '  FAIL a round that never started was accepted: %s\n' "$m_ns"; failed=1 ;;
  esac

  # MISSING / EMPTY / NON-NUMERIC inputs must land on NAMED refusals rather than a
  # shell diagnostic. Each is checked for its own name AND for the absence of a
  # bare "integer expression" style diagnostic.
  local m_e m_e_rc
  m_e=$(term_control_stop_reason "same" "same" "s" "s" 143 "" 1 1 1 \
    '{"code":"browser_session_not_found"}' 1 'x' 1 143 0)
  case "$m_e" in
    BROWSER_CLEANUP_CONTROL_NOT_STARTED:*)
      printf '  ok   an empty started value is a named refusal\n' ;;
    *) printf '  FAIL an empty started value was not a named refusal: %s\n' "$m_e"
       failed=1 ;;
  esac
  m_e_rc=$(term_control_stop_reason "same" "same" "s" "s" "" 1 1 1 1 \
    '{"code":"browser_session_not_found"}' 1 'x' 1 143 0)
  case "$m_e_rc" in
    BROWSER_CLEANUP_CONTROL_EXIT_NOT_143:*)
      printf '  ok   an empty rc is a named refusal\n' ;;
    *) printf '  FAIL an empty rc was not a named refusal: %s\n' "$m_e_rc"
       failed=1 ;;
  esac
  local m_nn
  m_nn=$(term_control_stop_reason "same" "same" "s" "s" 143 1 "not-a-number" 1 1 \
    '{"code":"browser_session_not_found"}' 1 'x' 1 143 0)
  case "$m_nn" in
    BROWSER_CLEANUP_CONTROL_KILL_COUNT:*)
      printf '  ok   a non-numeric killed value is a named refusal\n' ;;
    *) printf '  FAIL a non-numeric killed value was not a named refusal: %s\n' "$m_nn"
       failed=1 ;;
  esac
  local m_we
  m_we=$(term_control_stop_reason "same" "same" "s" "s" 143 1 1 "" 1 \
    '{"code":"browser_session_not_found"}' 1 'x' 1 143 0)
  case "$m_we" in
    BROWSER_CLEANUP_CONTROL_WITNESS_COUNT:*)
      printf '  ok   an empty witness count is a named refusal\n' ;;
    *) printf '  FAIL an empty witness count was not a named refusal: %s\n' "$m_we"
       failed=1 ;;
  esac

  # --- OWNED PROBE-PATH VALIDATOR -------------------------------------------------
  # The traversal shape is the whole reason this validator exists: a case GLOB
  # matches `/`, so a naive prefix+suffix test accepts this and would authorise a
  # delete outside the intended directory. It must be REFUSED by the shared
  # validator, at every site that uses it.
  if probe_manifest_path_owned "$REPO_ROOT_ABS/.probe-manifest.foo/../escaped"; then
    printf '  FAIL a traversal-shaped probe path was accepted\n'; failed=1
  else
    printf '  ok   a traversal-shaped probe path is refused\n'
  fi
  # Suffix-only shapes must be refused too: empty basename suffix, a nested suffix,
  # a foreign root, and a non-manifest name.
  local v_shape v_ok=1
  for v_shape in "$REPO_ROOT_ABS/.probe-manifest." "$REPO_ROOT_ABS/.probe-manifest.a/b" \
    "/etc/.probe-manifest.x" "$REPO_ROOT_ABS/notes.txt" ""; do
    if probe_manifest_path_owned "$v_shape"; then
      printf '  FAIL a non-owned probe path was accepted: %s\n' "$v_shape"; v_ok=0
    fi
  done
  if [ "$v_ok" = "1" ]; then
    printf '  ok   empty-suffix, nested, foreign-root and non-manifest paths are refused\n'
  else
    failed=1
  fi
  # A genuine repo-root manifest path must be ACCEPTED. Existence is deliberately
  # NOT tested here -- some callers validate before the file exists.
  if probe_manifest_path_owned "$REPO_ROOT_ABS/.probe-manifest.abc123"; then
    printf '  ok   a genuine repo-root probe manifest path is accepted\n'
  else
    printf '  FAIL a genuine repo-root probe manifest path was refused\n'; failed=1
  fi

  # --- MANIFEST ABSENCE ---------------------------------------------------------
  # A manifest still on disk is precisely the residue this control must never ship,
  # so it must be a NAMED refusal rather than a PASS.
  local m_mp
  m_mp=$(term_control_stop_reason "same" "same" "s" "s" 143 1 1 1 1 \
    '{"code":"browser_session_not_found"}' 0 'x' 1 143 0)
  if [ "$m_mp" = BROWSER_CLEANUP_CONTROL_MANIFEST_PRESENT:0 ]; then
    printf '  ok   a present manifest is refused\n'
  else
    printf '  FAIL a present manifest was accepted: %s\n' "$m_mp"; failed=1
  fi
  # An EMPTY marker must be refused too: "not proven absent" is not "absent", and
  # accepting it silently would let exactly this residue happen again.
  local m_me
  m_me=$(term_control_stop_reason "same" "same" "s" "s" 143 1 1 1 1 \
    '{"code":"browser_session_not_found"}' "" 'x' 1 143 0)
  case "$m_me" in
    BROWSER_CLEANUP_CONTROL_MANIFEST_PRESENT:*) ;;
    *) printf '  FAIL an empty manifest marker was not a named refusal: %s\n' "$m_me"
       failed=1 ;;
  esac

  # --- WORKER EXIT REMOVES EXACTLY ITS OWN MANIFEST -----------------------------
  # --- CONTROLLER-MINTED PATH, WORKER EXIT, SIBLING SURVIVAL ---------------------
  # A shell-level model of the settled ownership rule: the CONTROLLER creates the
  # exact path BEFORE the fork and hands it over; the worker's EXIT removes that own
  # path and NOTHING else. A sibling must survive, which is what shows this is
  # exact-path removal rather than a wildcard sweep. Both files are real repo-root
  # mktemp files and this model removes BOTH on every path.
  local model_owned model_sibling model_out
  model_owned=$(mktemp "$REPO_ROOT_ABS/.probe-manifest.XXXXXX")
  model_sibling=$(mktemp "$REPO_ROOT_ABS/.probe-manifest.XXXXXX")
  model_out=$(AGENTERM_TEST_MANIFEST="$model_owned" sh -c '
    if [ ! -f "$AGENTERM_TEST_MANIFEST" ]; then echo no_handoff; exit 0; fi
    rest="${AGENTERM_TEST_MANIFEST##*/.probe-manifest.}"
    case "$AGENTERM_TEST_MANIFEST" in
      */.probe-manifest.*)
        case "$rest" in
          */*|"") : ;;
          *) rm -f "$AGENTERM_TEST_MANIFEST" ;;
        esac
        ;;
    esac
    if [ ! -e "$AGENTERM_TEST_MANIFEST" ]; then echo owned_gone; fi
  ' 2>&1)
  case "$model_out" in
    *owned_gone*)
      if [ -e "$model_sibling" ]; then
        printf '  ok   worker EXIT removes exactly the controller-minted manifest, not a sibling\n'
      else
        printf '  FAIL the sibling manifest was removed as well\n'; failed=1
      fi
      ;;
    *)
      printf '  FAIL the owned manifest survived the EXIT model: %s\n' "$model_out"
      failed=1 ;;
  esac
  rm -f "$model_owned" "$model_sibling"

  # A manifest-PRESENT outcome must keep the safety transaction ARMED and make the
  # helper fail. Modelled on the real disarm predicate: BOTH facts proven is the
  # ONLY disarm condition, so a present manifest leaves the transaction armed.
  local safety_out
  safety_out=$(sh -c '
    session_absent=1; manifest_absent=0; armed=1
    if [ "$session_absent" = "1" ] && [ "$manifest_absent" = "1" ]; then armed=0; fi
    if [ "$armed" = "1" ]; then echo still_armed; else echo disarmed; fi
  ' 2>&1)
  if [ "$safety_out" = "still_armed" ]; then
    printf '  ok   a present manifest keeps the safety transaction armed\n'
  else
    printf '  FAIL a present manifest disarmed the safety transaction: %s\n' "$safety_out"
    failed=1
  fi

  # A non-owned path must be REFUSED and NEVER deleted, even though it names a real
  # existing file. This is the mutation that would otherwise turn the EXIT guard
  # into arbitrary deletion.
  local victim victim_out
  victim=$(mktemp "$REPO_ROOT_ABS/.probe-manifest-victim.XXXXXX")
  victim_out=$(AGENTERM_TEST_MANIFEST="$victim" sh -c '
    rest="${AGENTERM_TEST_MANIFEST##*/.probe-manifest.}"
    case "$AGENTERM_TEST_MANIFEST" in
      */.probe-manifest.*)
        case "$rest" in
          */*|"") : ;;
          *) rm -f "$AGENTERM_TEST_MANIFEST" ;;
        esac
        ;;
    esac
    if [ -e "$AGENTERM_TEST_MANIFEST" ]; then echo survived; fi
  ' 2>&1)
  if [ "$victim_out" = "survived" ]; then
    printf '  ok   a non-owned manifest path is refused and never deleted\n'
  else
    printf '  FAIL a non-owned manifest path was deleted: %s\n' "$victim_out"
    failed=1
  fi
  rm -f "$victim"

  # --- DIRECT TERM-HANDLER WITNESS AND RAW WAIT CLASSIFICATION -------------------
  # A missing handler witness must be a NAMED refusal: rc=143 alone does not prove a
  # handler ran, so absence cannot be accepted as "not disproven".
  local h_missing h_empty
  h_missing=$(term_control_stop_reason "same" "same" "s" "s" 143 1 1 1 1 \
    '{"code":"browser_session_not_found"}' 1 'x' 0 143 0)
  if [ "$h_missing" = BROWSER_CLEANUP_CONTROL_TERM_WITNESS_COUNT:0 ]; then
    printf '  ok   a missing handler witness is refused\n'
  else
    printf '  FAIL a missing handler witness was accepted: %s\n' "$h_missing"; failed=1
  fi
  h_empty=$(term_control_stop_reason "same" "same" "s" "s" 143 1 1 1 1 \
    '{"code":"browser_session_not_found"}' 1 'x' "" 143 0)
  case "$h_empty" in
    BROWSER_CLEANUP_CONTROL_TERM_WITNESS_MISSING:*) ;;
    *) printf '  FAIL an empty handler witness was not a named refusal: %s\n' "$h_empty"
       failed=1 ;;
  esac
  # A DOUBLED handler witness must be refused: the handler ran more than once.
  local h_two
  h_two=$(term_control_stop_reason "same" "same" "s" "s" 143 1 1 1 1 \
    '{"code":"browser_session_not_found"}' 1 'x' 2 143 0)
  if [ "$h_two" = BROWSER_CLEANUP_CONTROL_TERM_WITNESS_COUNT:2 ]; then
    printf '  ok   a doubled handler witness is refused\n'
  else
    printf '  FAIL a doubled handler witness was accepted: %s\n' "$h_two"; failed=1
  fi
  # DIRECT-SIGNAL CLASSIFICATION. A worker killed by SIGTERM reaps as exit code 0
  # with signal 15 -- a DIFFERENT shape from a handler calling `exit 143`. It must be
  # refused by its own name so the two can never be confused.
  local d_sig
  d_sig=$(term_control_stop_reason "same" "same" "s" "s" 143 1 1 1 1 \
    '{"code":"browser_session_not_found"}' 1 'x' 1 0 15)
  if [ "$d_sig" = BROWSER_CLEANUP_CONTROL_WORKER_DIED_BY_SIGNAL:15 ]; then
    printf '  ok   a directly-signalled worker is refused and distinguished from a handler exit\n'
  else
    printf '  FAIL a directly-signalled worker was not refused by name: %s\n' "$d_sig"
    failed=1
  fi
  # RC/MARKER DISAGREEMENT. The helper is told rc=143 while the controller's raw
  # marker says something else; the marker must win, because it is the measured
  # reaping fact rather than the value the check is trying to confirm.
  local d_dis mid="x"
  for d_dis in 1 0 137 "<absent>" ""; do
    mid=$(term_control_stop_reason "same" "same" "s" "s" 143 1 1 1 1 \
      '{"code":"browser_session_not_found"}' 1 'x' 1 "$d_dis" 0)
    case "$mid" in
      BROWSER_CLEANUP_CONTROL_WORKER_EXIT_MARKER_MISMATCH:*) ;;
      *) printf '  FAIL rc/marker disagreement (%s) was not refused: %s\n' "$d_dis" "$mid"
         failed=1 ;;
    esac
  done
  printf '  ok   rc/marker disagreements (nonzero, zero, 137, absent, empty) are refused\n'

  # The GREEN shape must actually pass the real helper, or every refusal above is
  # meaningless: a helper that refuses everything would look identical.
  local h_green
  h_green=$(term_control_stop_reason "same" "same" "s" "s" 143 1 1 1 1 \
    '{"code":"browser_session_not_found"}' 1 'x' 1 143 0)
  if [ -z "$h_green" ]; then
    printf '  ok   the fully-green shape is accepted by the real helper\n'
  else
    printf '  FAIL the fully-green shape was refused: %s\n' "$h_green"; failed=1
  fi

  if [ "$failed" -ne 0 ]; then
    fail "BROWSER_HARNESS_SELF_TEST_FAILED"
    return
  fi
  printf '%s\n' \
    '{"schema":"agenterm.profile-binding-exact-process-browser-harness/v1","ok":true,"code":"BROWSER_HARNESS_SELF_TEST_PASS"}'
}

# Run one probe through the tool-profile path. `AGENTERM_LIVE_REGION_SOURCE`
# reaches the court through the runner's OWN environment, which the child
# inherits; no manifest field declares it (a task's `env` list and a contract's
# `env_allow` are existing concepts this probe must not repurpose), and the
# registered entry will read it the same way.
probe_source() {
  local target="$1" mode="${2:-}" preset_manifest="${3:-}" root_abs manifest
  root_abs=$(CDPATH= cd -- "$REPO" && pwd -P)
  # A UNIQUE manifest per invocation, created directly IN THE REPO ROOT. Two
  # constraints meet here and each rules out the obvious shortcut:
  #   * uniqueness -- on this platform `mktemp` substitutes only TRAILING X's, so a
  #     template like `.probe-manifest.XXXXXX.tmp` yields the literal string and is
  #     NOT unique; and `$$` is merely process-scoped, so nested or backgrounded
  #     calls in one shell would still collide. So: real `mktemp`.
  #   * location -- the task's `cwd` is "." and its `entry` is repo-relative, and
  #     the runner resolves both against the MANIFEST'S OWN directory. A manifest
  #     one level down made every entry resolve as `.<subdir>/live-rehearsal.qjs`
  #     and the runner refused it with task_path_missing. So: repo root, NOT a
  #     subdirectory.
  # SCOPE OF THIS CLEANUP: NORMAL RETURNS ONLY. The manifest is removed on the
  # generation-failure path and on the ordinary task-return path below. An
  # INTERRUPTED probe is NOT covered here, and this function does not claim to
  # cover it: a caller whose shell is blocked in a foreground child does not run
  # its handlers before it dies.
  #
  # A PRESET PATH (third argument) is honored instead of minting a new one, which
  # is how the TERM worker takes ownership of the exact file BEFORE the probe runs:
  # its own EXIT trap (via `BROWSER_TXN_MANIFEST`) then removes that same file even
  # when a signal kills the probe. The value is NEVER a general cleanup target -- it
  # must match exactly the repo-root `.probe-manifest.*` shape this function mints,
  # so a bad argument cannot redirect the cleanup at some unrelated path.
  if [ -n "$preset_manifest" ]; then
    if ! probe_manifest_path_owned "$preset_manifest"; then
      fail "BROWSER_PROBE_MANIFEST_PRESET_INVALID"
      return 1
    fi
    if [ ! -f "$preset_manifest" ]; then
      fail "BROWSER_PROBE_MANIFEST_PRESET_MISSING"
      return 1
    fi
    manifest="$preset_manifest"
  else
    manifest=$(mktemp "$root_abs/.probe-manifest.XXXXXX") || {
      fail "BROWSER_PROBE_MANIFEST_MKTEMP"
      return 1
    }
  fi
  python3 - "$MANIFEST" "$manifest" "$target" "$root_abs" "$LIVE_REGION" "$mode" <<'PROBE' || { rm -f "$manifest"; return 1; }
import json, sys
src, dst, entry, root, region, mode = sys.argv[1:7]
_ = region
task_id = "browser-profile-name-binding-exact-process-live"
d = json.load(open(src))
d["tasks"] = [{
  "id": task_id,
  "description": "probe: live court source",
  "entry": entry.replace(root + "/", "").replace("./", ""),
  "profile": "tool",
  "cwd": ".",
  "args": [mode] if mode else [],
  "dependencies": [],
  "platforms": ["macos"],
  "side_effects": [],
}]
d["contracts"] = {task_id: {
  "inputs": ["source-tree"],
  "outputs": ["live-self-test"],
  "budget": {"timeout_ms": 120000, "max_operations": 200000000,
             "max_host_operations": 4096, "max_output_bytes": 262144},
  "network": [],
  "evidence": ["live-self-test"],
}}
json.dump(d, open(dst, "w"), indent=2)
PROBE
  # The task run is allowed to fail -- a mutation is EXPECTED to fail -- so the
  # manifest must be removed on both paths. SCOPE OF THIS CLEANUP: NORMAL RETURNS
  # ONLY, meaning the generation-failure path and the ordinary task-return path
  # below. An INTERRUPTED probe is NOT covered: on POSIX a shell blocked in a
  # foreground child does not run its handlers before it dies, so this is a
  # documented gap rather than a guarantee, and nothing here claims otherwise.
  local rc=0
  AGENTERM_LIVE_REGION_SOURCE="$LIVE_REGION" \
    AGENTERM_STAGED_STATE_ROOT="${AGENTERM_STAGED_STATE_ROOT:-}" \
    AGENTERM_STAGED_SOURCE="${AGENTERM_STAGED_SOURCE:-}" \
    AGENTERM_STAGED_INPUT="${AGENTERM_STAGED_INPUT:-}" \
    AGENTERM_STAGED_RUN_ID="${AGENTERM_STAGED_RUN_ID:-}" \
    AGENTERM_BROWSER_STATE_ROOT="${AGENTERM_BROWSER_STATE_ROOT:-}" \
    AGENTERM_BROWSER_HOME="${AGENTERM_BROWSER_HOME:-}" \
    AGENTERM_BROWSER_EXE="${AGENTERM_BROWSER_EXE:-}" \
    AGENTERM_BROWSER_SOURCE="${AGENTERM_BROWSER_SOURCE:-}" \
    AGENTERM_BROWSER_INPUT="${AGENTERM_BROWSER_INPUT:-}" \
    AGENTERM_BROWSER_RUN_ID="${AGENTERM_BROWSER_RUN_ID:-}" \
    AGENTERM_BROWSER_FORCE_THROW="${AGENTERM_BROWSER_FORCE_THROW:-}" \
    AGENTERM_BROWSER_HOST_OP_TRAP="${AGENTERM_BROWSER_HOST_OP_TRAP:-}" \
    AGENTERM_BROWSER_DIGEST_SCRATCH="${AGENTERM_BROWSER_DIGEST_SCRATCH:-}" \
    HOME="${AGENTERM_TASK_HOME:-$HOME}" \
    AGENTERM_SCRIPT_BACKEND=qjswasm \
    "$AGENTERM_EXE" cli script task run \
    browser-profile-name-binding-exact-process-live --manifest "$manifest" 2>&1 \
    || rc=$?
  rm -f "$manifest"
  return $rc
}

# P1-1: the entry must be FAIL-CLOSED. Print the envelope, then print the
# evidence/pass tokens ONLY when `ok` is true, and exit nonzero when it is not.
#
# This gate deliberately does NOT read the envelope's `ok` field. Grepping the
# envelope would pass even if the entry printed PASS beside `"ok":false` and
# exited 0 -- which is exactly the fail-open defect this gate exists to catch. It
# asserts the two observable facts directly: the tokens are ABSENT and the
# process exit code is NONZERO. A mutation that makes a real check false must
# produce zero PASS tokens, zero EVIDENCE tokens and rc != 0.
fail_open_gate() {
  local root="$REPO/.agenterm-research-state/live-process-red-gate-failopen"
  local target="$root/live-rehearsal.qjs" out rc
  local pass_hits evidence_hits

  # Baseline control: the unmutated entry MUST print both tokens and exit 0, or
  # the gate below would be satisfied by an entry that never prints them at all.
  mkdir -p "$root"
  cp "$LIVE" "$target"
  out=$(probe_source "$target" "live-process-preflight") && rc=0 || rc=$?
  pass_hits=$(printf '%s\n' "$out" | grep -c '^PASS: exact-process live process preflight$' || true)
  evidence_hits=$(printf '%s\n' "$out" | grep -c '^EVIDENCE research\.profile-binding-exact-process\.live-process-preflight$' || true)
  if [ "$rc" -ne 0 ] || [ "$pass_hits" -ne 1 ] || [ "$evidence_hits" -ne 1 ]; then
    printf '  FAIL fail-closed entry: the unmutated entry did not print exactly one PASS and one EVIDENCE token with rc=0 (rc=%s pass=%s evidence=%s)\n' \
      "$rc" "$pass_hits" "$evidence_hits"
    rm -rf "$root"
    return 1
  fi
  printf '  ok   the unmutated entry prints the evidence tokens and exits zero\n'

  # Mutation: make a REAL check false. The entry must then refuse to print either
  # token and must exit nonzero.
  cp "$LIVE" "$target"
  if ! perl -0pi -e 's/\Q  checks.host_clock_is_real = \E/  checks.host_clock_is_real = false \&\& /' "$target" 2>/dev/null; then
    printf '  FAIL fail-closed entry: the mutation could not be applied\n'
    rm -rf "$root"; return 1
  fi
  if cmp -s "$LIVE" "$target"; then
    printf '  FAIL fail-closed entry: the mutation did not change the source\n'
    rm -rf "$root"; return 1
  fi
  out=$(probe_source "$target" "live-process-preflight") && rc=0 || rc=$?
  pass_hits=$(printf '%s\n' "$out" | grep -c '^PASS: exact-process live process preflight$' || true)
  evidence_hits=$(printf '%s\n' "$out" | grep -c '^EVIDENCE research\.profile-binding-exact-process\.live-process-preflight$' || true)
  rm -rf "$root"
  if [ "$pass_hits" -ne 0 ]; then
    printf '  FAIL fail-closed entry: PASS token printed despite a false check\n'; return 1
  fi
  if [ "$evidence_hits" -ne 0 ]; then
    printf '  FAIL fail-closed entry: EVIDENCE token printed despite a false check\n'; return 1
  fi
  if [ "$rc" -eq 0 ]; then
    printf '  FAIL fail-closed entry: exit code was zero despite a false check\n'; return 1
  fi
  case "$out" in
    *"LIVE_PROCESS_PREFLIGHT_FAILED"*) : ;;
    *"budget exhausted"* | *"threw and nothing caught"* | *"does not support"* | *"invalid type"*)
      printf '  FAIL fail-closed entry: the mutation crashed or hung instead of failing a check\n'
      return 1 ;;
  esac
  printf '  ok   a false check prints no PASS/EVIDENCE token and exits nonzero\n'
  return 0
}

# Apply one exact-string mutation to a copy of a source and require the court's
# self-test to fail. Rejecting an inapplicable mutation is the point: a mutation
# that silently failed to apply would leave a passing suite, and a passing suite
# read as a red gate is worse than no gate at all.
# A mutation that needs TWO edits to become observable: first arm an adversary,
# then remove the guard that must catch it. Both are applied to the same copy and
# both must change the source; the mutant runs once.
arm_and_disable_gate() {
  local source="$1" target="$2" arm_from="$3" arm_to="$4" \
    off_from="$5" off_to="$6" label="$7"
  cp "$source" "$target"
  if ! perl -0pi -e "s/\Q$arm_from\E/$arm_to/" "$target" 2>/dev/null; then
    printf '  FAIL %s: the arming edit could not be applied\n' "$label"; return 1
  fi
  if ! perl -0pi -e "s/\Q$off_from\E/$off_to/" "$target" 2>/dev/null; then
    printf '  FAIL %s: the disabling edit could not be applied\n' "$label"; return 1
  fi
  if cmp -s "$source" "$target"; then
    printf '  FAIL %s: neither edit changed the source\n' "$label"; return 1
  fi
  local out
  out=$(probe_source "$target" "live-process-preflight") || true
  if [ -n "${out##*\"ok\":false*}" ]; then
    printf '  FAIL %s did not turn the mode red (got: %s)\n' "$label" "$out"
    return 1
  fi
  case "$out" in
    *"LIVE_PROCESS_PREFLIGHT_FAILED"*) : ;;
    *"budget exhausted"* | *"threw and nothing caught"* | *"does not support"* | *"invalid type"*)
      printf '  FAIL %s turned the mode red by crashing or hanging\n' "$label"
      return 1 ;;
  esac
  printf '  ok   %s turns the suite red\n' "$label"
  return 0
}

mutate_and_expect_fail() {
  # $6 (optional) is a mode; when given, the mutant runs in that mode and the
  # expected red token is derived from it. Without it the fixed-slice self-test
  # behavior is unchanged, so the existing gates keep their exact semantics.
  local source="$1" target="$2" from="$3" to="$4" label="$5" mode="${6:-}"
  cp "$source" "$target"
  if ! perl -0pi -e "s/\Q$from\E/$to/" "$target" 2>/dev/null; then
    printf '  FAIL %s: mutation could not be applied\n' "$label"; return 1
  fi
  if cmp -s "$source" "$target"; then
    printf '  FAIL %s: mutation did not change the source\n' "$label"; return 1
  fi
  local out
  out=$(probe_source "$target" "$mode") || true
  if [ -z "$mode" ]; then
    # The self-test names every failed check in its thrown code, so a mutation
    # that broke any guard must surface that token. A mutant that fails to
    # compile would instead carry a compiler diagnostic, which must NOT count as
    # a red: a non-compiling mutant proves nothing about the guard it removed.
    if [ -n "${out##*live_self_test_failed*}" ]; then
      printf '  FAIL %s did not turn the suite red (got: %s)\n' "$label" "$out"
      return 1
    fi
  else
    # Mode runs report an envelope, not a thrown token. The mutation must flip
    # the envelope to not-ok AND name the check it broke; a crash, a compile
    # error, a hang or an unrelated failure is not a red for this guard.
    if [ -n "${out##*\"ok\":false*}" ]; then
      printf '  FAIL %s did not turn the mode red (got: %s)\n' "$label" "$out"
      return 1
    fi
    # The fail-closed entry signals a failed check by THROWING a named code, which
  # the engine reports as "the script threw". That is the guard working, not a
  # crash, so the named preflight refusal must be excluded from the engine-error
  # patterns. Only a throw that is NOT our named code is a crash.
    case "$out" in
      *"LIVE_PROCESS_PREFLIGHT_FAILED"*) : ;;
      *"budget exhausted"* | *"threw and nothing caught"* | *"does not support"* | *"invalid type"*)
        printf '  FAIL %s turned the mode red by crashing or hanging, not by a guard\n' \
          "$label"
        return 1 ;;
    esac
  fi
  printf '  ok   %s turns the suite red\n' "$label"
  return 0
}

# Run the live process preflight and surface its envelope. The preflight is REAL
# host evidence: it spawns one owned, non-browser, short-lived child. It launches
# no browser, reserves no ordinal and reaches no design verdict, and the runner
# asserts those three properties are present rather than trusting a summary.
# One staged-preflight probe run. The mutant lives in an ignored repo-local
# directory (the tool-profile task requires a repo-relative entry), and each run
# gets its own disposable root so a mutant can never see another's state.
staged_probe_source() {
  local target="$1" root out
  # The marker is required by the court, so the probe root must carry it too.
  root=$(mktemp -d "${TMPDIR:-/tmp}/agenterm-live-staged-rg.XXXXXX")
  mkdir -p "$root/agenterm-live-staged/state"
  root=$(CDPATH='' cd -- "$root" && pwd -P)
  mkdir -p "$root/state"
  # probe_source rewrites the manifest with the mutant as the task entry and
  # makes it repo-relative; `task run` cannot take a file path directly. The
  # disposable root and pinned digests ride in the environment, and the mode is
  # the task's declared arg.
  local rc=0
  set +e
  out=$(AGENTERM_STAGED_STATE_ROOT="$root/agenterm-live-staged/state" \
    AGENTERM_STAGED_SOURCE="$(git -C "$REPO" rev-parse HEAD)" \
    AGENTERM_STAGED_INPUT="$(staged_input_digest)" \
    AGENTERM_STAGED_RUN_ID="$STAGED_RUN_ID" \
    probe_source "$target" live-staged-preflight)
  rc=$?
  set -e
  rm -rf "$root"
  printf '%s\n' "$out"
  return $rc
}

# Apply one exact-string mutation and require the staged slice to go red with a
# NAMED failure. A mutation that cannot be applied, does not compile, crashes or
# hangs is REJECTED, not counted: only the positive token proves the guard is what
# failed.
staged_mutate_and_expect_fail() {
  local from="$1" to="$2" label="$3" expect="$4"
  # The mutant must live INSIDE the clone: `task run` refuses any entry that is
  # not a bounded repo-relative path, so a temp-dir mutant is rejected before the
  # guard under test is even reached.
  local root="$REPO/.agenterm-research-state/live-staged-red-gate"
  local mutant="$root/live-rehearsal.qjs"
  mkdir -p "$root"
  cp "$LIVE" "$mutant"
  if ! perl -0pi -e "s/\Q$from\E/$to/" "$mutant" 2>/dev/null; then
    printf '  FAIL %s: mutation could not be applied\n' "$label"; rm -rf "$root"; return 1
  fi
  if cmp -s "$LIVE" "$mutant"; then
    printf '  FAIL %s: mutation did not change the source\n' "$label"; rm -rf "$root"; return 1
  fi
  local out rc=0
  set +e
  out=$(staged_probe_source "$mutant")
  rc=$?
  set -e
  # The mutant must be red for the RIGHT reason: the named guard, not a compile
  # error, crash or hang. A mutant that never reached the guard proves nothing.
  if [ "$rc" -eq 0 ]; then
    printf '  FAIL %s exited zero instead of turning red\n' "$label"
    rm -rf "$root"; return 1
  fi
  case "$expect" in
    stage_order_matches_trace|no_finish_attempted|termination_proven_claims_one|identity_source_count_is_derived)
      case "$out" in
        *\"$expect\":false*) ;;
        *)
          printf '  FAIL %s did not set %s=false\n' "$label" "$expect"
          rm -rf "$root"; return 1 ;;
      esac ;;
    *) case "$out" in
      *"$expect"*) ;;
      *)
      printf '  FAIL %s did not surface %s (got: %s)\n' "$label" "$expect" \
        "$(printf '%s' "$out" | tail -1 | cut -c1-110)"
      rm -rf "$root"; return 1 ;;
    esac ;;
  esac
  case "$out" in
    *"does not support"* | *"invalid type"* | *"budget exhausted"*)
      printf '  FAIL %s turned the slice red by crashing, not by the guard\n' "$label"
      rm -rf "$root"; return 1 ;;
  esac
  printf '  ok   %s turns the slice red (%s)\n' "$label" "$expect"
  rm -rf "$root"
  return 0
}

# Force a real guard to fail and return the entry's combined output. Reuses the
# probe path so the mutant runs in the same mode with the same disposable root.
staged_fail_open_probe() {
  local root="$REPO/.agenterm-research-state/live-staged-red-gate"
  local mutant="$root/live-rehearsal.qjs"
  mkdir -p "$root"
  cp "$LIVE" "$mutant"
  # Force a REAL check false: if the ledger were not abandoned the attempt would
  # not be closed, and the entry must refuse to report success.
  perl -0pi -e 's/\Qchecks.ledger_abandoned = abandoned.status === "abandoned";\E/checks.ledger_abandoned = false;/' \
    "$mutant" 2>/dev/null || true
  local out rc=0
  set +e
  out=$(staged_probe_source "$mutant")
  rc=$?
  set -e
  rm -rf "$root"
  printf '%s\n' "$out"
  return $rc
}

# The staged slice's red gates. Every mutant must make the real world WRONG
# (spawn before its guard, kill before its guard, corrupt journal bytes, project a
# truncated envelope) and must surface the NAMED guard. A mutant that merely
# skips a check is not a gate, and a crash or non-compiling mutant is rejected.
live_staged_red_gate() {
  local failed=0
  local baseline
  baseline=$(staged_probe_source "$LIVE") || true
  case "$baseline" in
    *'"ok":true'*) printf '  ok   the staged preflight passes unmutated\n' ;;
    *) printf '  FAIL the staged preflight did not pass (got: %s)\n' \
         "$(printf '%s' "$baseline" | tail -1 | cut -c1-110)"; return 1 ;;
  esac

  # 1. REAL spawn before the preflight row that guards it. The call-site trace
  #    records the true order and the independent verifier names the violation.
  staged_mutate_and_expect_fail \
    'const STAGE_ORDER_MODE = "off";' \
    'const STAGE_ORDER_MODE = "spawn_before_preflight";' \
    'a spawn preceding its preflight guard' 'stage_order_matches_trace' || failed=1

  # 2. REAL kill before the stop row that guards it.
  staged_mutate_and_expect_fail \
    'const STAGE_ORDER_MODE = "off";' \
    'const STAGE_ORDER_MODE = "kill_before_stop";' \
    'a kill preceding its stop guard' 'stage_order_matches_trace' || failed=1

  # 3. REAL journal corruption on disk, three distinct shapes. Each must be caught
  #    by the independent audit rather than by the broker's own return.
  staged_mutate_and_expect_fail \
    'const JOURNAL_MODE = "off";' 'const JOURNAL_MODE = "append_garbage";' \
    'a forged journal row appended on disk' 'staged_journal_' || failed=1
  staged_mutate_and_expect_fail \
    'const JOURNAL_MODE = "off";' 'const JOURNAL_MODE = "rewrite_facts";' \
    'journal facts rewritten on disk' 'staged_journal_' || failed=1
  staged_mutate_and_expect_fail \
    'const JOURNAL_MODE = "off";' 'const JOURNAL_MODE = "reorder_rows";' \
    'journal rows reordered on disk' 'staged_journal_' || failed=1

  # 4. REAL truncated-envelope projection: the flag the broker truly sets when it
  #    truncates is projected onto the envelope. The guard is never disabled.
  staged_mutate_and_expect_fail \
    'const TRUNCATION_MODE = "off";' 'const TRUNCATION_MODE = "project_truncated";' \
    'a truncated broker envelope' 'staged_broker_stdout_truncated' || failed=1

  # 5. FAIL-CLOSED ENTRY. Force a real check false and require that the entry
  #    emits NO PASS and NO EVIDENCE and exits nonzero. Asserting the token count
  #    rather than grepping the message is what proves the entry cannot report
  #    success beside a red result.
  local fc_out fc_rc=0
  set +e
  fc_out=$(staged_fail_open_probe)
  fc_rc=$?
  set -e
  if [ "$fc_rc" -eq 0 ]; then
    printf '  FAIL a red staged run exited zero\n'; failed=1
  else
    local fc_pass fc_ev
    fc_pass=$(printf '%s' "$fc_out" | grep -c 'PASS:' || true)
    fc_ev=$(printf '%s' "$fc_out" | grep -c 'EVIDENCE ' || true)
    if [ "$fc_pass" != "0" ] || [ "$fc_ev" != "0" ]; then
      printf '  FAIL a red staged run still emitted PASS=%s EVIDENCE=%s\n' \
        "$fc_pass" "$fc_ev"
      failed=1
    else
      printf '  ok   a red staged run emits no PASS and no EVIDENCE\n'
    fi
  fi

  # 8. ARMED illegal finish. On the baseline `finish` is never called; arming the
  #    mutation performs a REAL finish attempt, which must be refused, must leave
  #    the ledger `reserved`, and must make `no_finish_attempted` false -- i.e. the
  #    envelope can no longer claim the call never happened.
  staged_mutate_and_expect_fail \
    'const FINISH_MODE = "off";' 'const FINISH_MODE = "finish";' \
    'an armed illegal finish attempt' 'no_finish_attempted' || failed=1

  # 9. TRACE COVERAGE, not merely ordering. Deleting one real subject operation's
  #    trace call must turn the slice red with the COVERAGE failure. A gate that
  #    only reordered events would pass against a trace that recorded nothing, so
  #    this is what proves the coverage half actually bites.
  staged_mutate_and_expect_fail \
    'trace("side_effect_started:" + effect);
    });' \
    'if (effect !== "release") { trace("side_effect_started:" + effect); }
    });' \
    'a real subject operation dropped from the trace' \
    'stage_trace_coverage_missing' || failed=1

  # 10. DEAD-COUNT FALSE EVIDENCE. Blocking the derivation republishes a PROVEN row
  #     claiming zero deaths beside a subject we just watched die. This is the exact
  #     P1 defect, so it must be a named red.
  staged_mutate_and_expect_fail \
    'if (termination.ok === true) {
        dead_count = 1;
      }' 'if (termination.ok === false) { dead_count = 1; }' \
    'a proven termination still claiming zero deaths' \
    'termination_proven_claims_one' || failed=1

  # 11. TRACE COVERAGE for a real subject read: dropping the subject pid read's
  #     trace must be caught by the coverage half.
  staged_mutate_and_expect_fail \
    'trace("side_effect_started:pid");
    subject_pid_read = process_pid(handle);' \
    'subject_pid_read = process_pid(handle);' \
    'a real subject pid read dropped from the trace' \
    'stage_trace_coverage_missing' || failed=1

  # 12. TRACE COVERAGE for the walk's own reads: the ownership observe/parent hook
  #     must be exercised, not just the construction of the source.
  staged_mutate_and_expect_fail \
    'note("ownership_parent");' ';' \
    'the ownership walk parent read dropped from the trace' \
    'stage_trace_coverage_missing' || failed=1

  # 13. IDENTITY-SOURCE PENDING must claim zero calls, and PROVEN must report the
  #     trace-projected count. Blocking the projection must be a named red.
  staged_mutate_and_expect_fail \
    'const actual_call_count = count_exact_key_calls(TOOLHOOK_TRACE);' \
    'const actual_call_count = 0;' \
    'a projected exact-key count forced to zero' \
    'identity_source_count_is_derived' || failed=1

  if [ "$failed" -eq 0 ]; then
    printf '\n%s\n' 'LIVE_STAGED_RED_GATE_PASS'
    return 0
  fi
  printf '\n%s\n' 'LIVE_STAGED_RED_GATE_FAILED'
  return 1
}

# The browser-free persisted-stage integration slice. It reserves R1 in a
# DISPOSABLE root, publishes a stage before every guarded side effect, audits the
# journal from disk, and closes by `abandon`. It launches no browser, touches no
# formal root, and stages no terminal.
#
# `cli script task run` takes no positional arguments, so the disposable root and
# the pinned digests travel through the environment; the MODE travels after `--`,
# which `task run` appends to the task's declared args (the registered live task
# declares `args: []`, so the mode becomes argv[0]).
staged_run() {
  local entry="$1" root="$2" manifest="${3:-$MANIFEST}"
  AGENTERM_SCRIPT_BACKEND=qjswasm \
    AGENTERM_LIVE_REGION_SOURCE="$LIVE_REGION" \
    AGENTERM_STAGED_STATE_ROOT="$root/state" \
    AGENTERM_STAGED_SOURCE="$(git -C "$REPO" rev-parse HEAD)" \
    AGENTERM_STAGED_INPUT="$(printf 'agenterm-cu/profile-binding-exact-process/input/v1' | \
      shasum -a 256 | cut -d' ' -f1)" \
    AGENTERM_STAGED_RUN_ID="$STAGED_RUN_ID" \
    "$AGENTERM_EXE" cli script task run \
    "$entry" --manifest "$manifest"
}

staged_input_digest() {
  printf 'agenterm-cu/profile-binding-exact-process/input/v1' | shasum -a 256 | cut -d' ' -f1
}

STAGED_RUN_ID="$(printf 'agenterm-live-staged-run-id/v1' | shasum -a 256 | cut -c1-32)"

live_staged_preflight() {
  local out rc=0 root before after
  root=$(mktemp -d "${TMPDIR:-/tmp}/agenterm-live-staged.XXXXXX")
  # A fully-resolved path: a temp dir under a symlinked parent would otherwise be
  # handed over unresolved, which the court refuses by design.
  root=$(CDPATH='' cd -- "$root" && pwd -P)
  mkdir -p "$root/state"
  before=$(snapshot_root "$FORMAL_ROOT")
  set +e
  out=$(staged_run profile-binding-exact-process-staged-preflight \
    "$root" 2>&1)
  rc=$?
  set -e
  after=$(snapshot_root "$FORMAL_ROOT")
  rm -rf "$root"
  printf '%s\n' "$out"
  if [ "$before" != "$after" ]; then
    fail STAGED_FORMAL_ROOT_CHANGED
  fi
  if [ "$rc" -ne 0 ]; then
    fail "STAGED_PREFLIGHT_EXIT:$rc"
  fi
  local missing=""
  for token in '"ok":true' \
      'EVIDENCE research.profile-binding-exact-process.live-staged-preflight' \
      'PASS: exact-process live staged preflight' \
      '"browser_launched":false' '"ordinal_reserved":false' \
      '"formal_root_touched":false' '"design_verdict":false' \
      '"terminal_staged":false' '"finish_called":false' '"receipt_written":false' \
      '"kill_criterion_4_closed":false' '"ledger_status":"abandoned"'; do
    case "$out" in
      *"$token"*) ;;
      *) missing="$missing $token" ;;
    esac
  done
  if [ -n "$missing" ]; then
    fail "STAGED_PREFLIGHT_CLAIM:$missing"
  fi
  if pgrep -f 'sleep 10' >/dev/null 2>&1; then
    fail STAGED_ORPHAN_REMAINS
  fi
}

# The disposable owned-browser baseline. The browser path arrives by environment
# so this runner holds no absolute tool path of its own, and every root is a
# fresh, fully-resolved temp directory carrying the marker the court requires.
BROWSER_RUN_ID="$(printf 'agenterm-live-browser-run-id/v1' | shasum -a 256 | cut -c1-32)"

browser_input_digest() {
  printf 'agenterm-cu/profile-binding-exact-process/input/v1' | shasum -a 256 | cut -d' ' -f1
}

live_browser_baseline() {
  local out rc=0 root before after
  local exe="${AGENTERM_BROWSER_EXE:-}"
  if [ -z "$exe" ] || [ ! -x "$exe" ]; then
    fail "BROWSER_BASELINE_NO_EXE"
    return
  fi
  root=$(mktemp -d "${TMPDIR:-/tmp}/agenterm-live-browser.XXXXXX")
  root=$(CDPATH='' cd -- "$root" && pwd -P)
  mkdir -p "$root/agenterm-live-browser/state" "$root/agenterm-live-browser/home"
  # Publish the disposable HOME so the EXIT trap can clean up in the same root
  # even when the script traps out before its own cleanup can run.
  BROWSER_TASK_HOME="$root/agenterm-live-browser/home"
  # ARM before the probe. The baseline builds the same transaction as a red-gate
  # round, so its EXIT backstop works for the same reason and its root is only
  # removed after a verified cleanup.
  browser_txn_arm "$root/agenterm-live-browser/home" \
    "$BROWSER_SESSION_NAME $BROWSER_STRAY_SESSION_NAME"
  before=$(snapshot_root "$FORMAL_ROOT")
  # The REAL home's browser-session root must not exist before, and must be
  # unchanged after: the whole point of the disposable HOME is that no session
  # is created where the user's own sessions live.
  local real_sessions_before real_sessions_after
  real_sessions_before=$(snapshot_root "$HOME/Library/Application Support/agenterm/browser-sessions")
  set +e
  out=$(AGENTERM_SCRIPT_BACKEND=qjswasm \
    AGENTERM_LIVE_REGION_SOURCE="$LIVE_REGION" \
    AGENTERM_BROWSER_STATE_ROOT="$root/agenterm-live-browser/state" \
    AGENTERM_BROWSER_HOME="$root/agenterm-live-browser/home" \
    AGENTERM_BROWSER_EXE="$exe" \
    AGENTERM_BROWSER_SOURCE="$(git -C "$REPO" rev-parse HEAD)" \
    AGENTERM_BROWSER_INPUT="$(browser_input_digest)" \
    AGENTERM_BROWSER_RUN_ID="$BROWSER_RUN_ID" \
    AGENTERM_BROWSER_DIGEST_SCRATCH="$BROWSER_TASK_HOME/../digest.scratch" \
    AGENTERM_TASK_HOME="$BROWSER_TASK_HOME" \
    "$AGENTERM_EXE" cli script task run \
      profile-binding-exact-process-browser-baseline --manifest "$MANIFEST")
  rc=$?
  set -e
  after=$(snapshot_root "$FORMAL_ROOT")
  real_sessions_after=$(snapshot_root "$HOME/Library/Application Support/agenterm/browser-sessions")
  # RUNNER-SIDE CLEANUP + READBACK, regardless of the court's rc. The court is
  # responsible for its own teardown, but the runner does not TRUST it: if the
  # script left a session behind (a failure path, or a mutation), this is what
  # reclaims it. Only a verified-clean round may delete the root.
  if ! per_iteration_cleanup "$root/agenterm-live-browser/home" \
      "$BROWSER_SESSION_NAME $BROWSER_STRAY_SESSION_NAME"; then
    fail "BROWSER_BASELINE_CLEANUP_FAILED:$BROWSER_CLEANUP_NOTE"
    printf 'BROWSER_BASELINE_CLEANUP_ROOT:%s\n' "$root" >&2
    return
  fi
  browser_txn_disarm
  rm -rf "$root"
  printf '%s\n' "$out"
  if [ "$before" != "$after" ]; then
    fail BROWSER_FORMAL_ROOT_CHANGED
  fi
  if [ "$real_sessions_before" != "$real_sessions_after" ]; then
    fail BROWSER_REAL_HOME_SESSION_ROOT_CHANGED
  fi
  if [ "$rc" -ne 0 ]; then
    fail "BROWSER_BASELINE_EXIT:$rc"
  fi
  local missing=""
  for token in '"ok":true' \
      'EVIDENCE research.profile-binding-exact-process.live-browser-baseline' \
      'PASS: exact-process live browser baseline' \
      '"browser_launched":true' '"ordinal_reserved":false' \
      '"browser_launch_attempted":true' '"browser_start_accepted":true' \
      '"primary_cleanup_ok":true' '"primary_cleanup_complete":true' \
      '"primary_cleanup_skipped":false' '"compensation_ran":false' \
      '"formal_root_touched":false' '"design_verdict":false' \
      '"terminal_staged":false' '"finish_called":false' '"receipt_written":false' \
      '"kill_criterion_4_closed":false' '"validity_criteria_proven":[]' \
      '"close_mechanism":"none (no attempt reserved; no terminal, no receipt, no design fact)"'; do
    case "$out" in
      *"$token"*) ;;
      *) missing="$missing $token" ;;
    esac
  done
  if [ -n "$missing" ]; then
    fail "BROWSER_BASELINE_CLAIM:$missing"
  fi
  # Identity separation must be visible in the envelope, not merely asserted in
  # the source: the scope note is the honest statement of what connection.process is.
  case "$out" in
    *'"bridge_host_scope":"connection.process is the CU/native bridge host, not the browser"'*) ;;
    *) fail "BROWSER_BRIDGE_HOST_SCOPE_ABSENT" ;;
  esac
}

# ORDINARY-EXCEPTION CLEANUP CONTROL.
#
# Failure class: a CATCHABLE JavaScript exception thrown by the court immediately
# after a successful browser start (`AGENTERM_BROWSER_FORCE_THROW=1` arms the
# existing post-start hook). Unlike an uncatchable WASM trap, this path stays
# INSIDE the script: the court catches it, the QJS COMPENSATION layer performs the
# real stop/remove and its own post-cleanup inventory read, and the runner then
# readback-verifies authoritative absence.
#
# WHAT THIS DOES NOT CLAIM: nothing here proves EXIT-trap cleanup, and nothing
# claims cleanup ran exactly once. The scope is "the script caught it, then the
# runner verified absence". Those are different mechanisms and are not conflated.
#
# ORDERING INVARIANT: cleanup+readback, then disarm, then EVERY judgement, and
# only then deletion. A failing judgement must never destroy the evidence it just
# examined, so any failure returns with the disposable root intact and named on
# stderr (`BROWSER_CLEANUP_CONTROL_ROOT:<path>`).
#
# Reuses the SAME helpers as the baseline and the red gates -- probe_source,
# browser_txn_arm, per_iteration_cleanup, browser_txn_disarm, snapshot_root, and
# verdict_names_failure. No second cleanup implementation exists.
live_browser_cleanup_control_exception() {
  local out rc=0 root before after
  local exe="${AGENTERM_BROWSER_EXE:-}"
  if [ -z "$exe" ] || [ ! -x "$exe" ]; then
    fail "BROWSER_BASELINE_NO_EXE"
    return
  fi
  local cu="${AGENTERM_CU_EXE:-}"
  if [ -z "$cu" ] || [ ! -x "$cu" ]; then
    fail "BROWSER_CLEANUP_CONTROL_NO_CU"
    return
  fi
  root=$(mktemp -d "${TMPDIR:-/tmp}/agenterm-live-browser.XXXXXX")
  root=$(CDPATH='' cd -- "$root" && pwd -P)
  mkdir -p "$root/agenterm-live-browser/state" "$root/agenterm-live-browser/home"
  BROWSER_TASK_HOME="$root/agenterm-live-browser/home"
  # ARM before the probe, exactly as the baseline does.
  browser_txn_arm "$root/agenterm-live-browser/home" \
    "$BROWSER_SESSION_NAME $BROWSER_STRAY_SESSION_NAME"
  before=$(snapshot_root "$FORMAL_ROOT")
  local real_sessions_before real_sessions_after
  real_sessions_before=$(snapshot_root "$HOME/Library/Application Support/agenterm/browser-sessions")
  set +e
  out=$(AGENTERM_SCRIPT_BACKEND=qjswasm \
    AGENTERM_LIVE_REGION_SOURCE="$LIVE_REGION" \
    AGENTERM_BROWSER_STATE_ROOT="$root/agenterm-live-browser/state" \
    AGENTERM_BROWSER_HOME="$root/agenterm-live-browser/home" \
    AGENTERM_BROWSER_EXE="$exe" \
    AGENTERM_BROWSER_SOURCE="$(git -C "$REPO" rev-parse HEAD)" \
    AGENTERM_BROWSER_INPUT="$(browser_input_digest)" \
    AGENTERM_BROWSER_RUN_ID="$BROWSER_RUN_ID" \
    AGENTERM_BROWSER_DIGEST_SCRATCH="$BROWSER_TASK_HOME/../digest.scratch" \
    AGENTERM_TASK_HOME="$BROWSER_TASK_HOME" \
    AGENTERM_BROWSER_FORCE_THROW=1 \
    probe_source "$LIVE" live-browser-baseline)
  rc=$?
  set -e
  after=$(snapshot_root "$FORMAL_ROOT")
  real_sessions_after=$(snapshot_root "$HOME/Library/Application Support/agenterm/browser-sessions")
  # TRANSACTION BOUNDARY: always cleanup + readback BEFORE anything is deleted.
  if ! per_iteration_cleanup "$root/agenterm-live-browser/home" \
      "$BROWSER_SESSION_NAME $BROWSER_STRAY_SESSION_NAME"; then
    printf 'BROWSER_CLEANUP_CONTROL_ROOT:%s\n' "$root" >&2
    fail "BROWSER_CLEANUP_CONTROL_CLEANUP_FAILED:$BROWSER_CLEANUP_NOTE"
    return
  fi
  # Verified clean: the session is now authoritatively absent, so the transaction
  # can be disarmed. Disarming is NOT deletion -- the root stays on disk until
  # EVERY remaining assertion has passed (see the bottom of this function).
  browser_txn_disarm
  # ---------------------------------------------------------------------------
  # EVERY JUDGEMENT RUNS BEFORE ANY DELETION, through ONE shared decision. The
  # ordinary-exception control has no EXIT-trap claim, so this function is the
  # only thing that can reclaim the root, and deleting it early would destroy the
  # very evidence a failure needs. So: cleanup+readback -> disarm -> judge -> and
  # delete ONLY on an empty stop reason. Any failure returns with the root INTACT
  # and names it, because "failure preserves root/evidence" is the established
  # invariant of this suite. Reclaiming a preserved root stays a human/diagnostic
  # act -- no cleanup implementation is copied here.
  # ---------------------------------------------------------------------------
  local stop
  stop=$(exception_control_stop_reason \
    "$before" "$after" "$real_sessions_before" "$real_sessions_after" "$rc" "$out")
  if [ -n "$stop" ]; then
    printf 'BROWSER_CLEANUP_CONTROL_ROOT:%s\n' "$root" >&2
    fail "$stop"
    return
  fi
  # Every judgement passed; only NOW is the evidence reclaimed.
  rm -rf "$root"
  printf '%s\n' "$out"
  printf '%s\n' \
    '{"schema":"agenterm.profile-binding-exact-process-cleanup-control/v1","ok":true,"code":"BROWSER_CLEANUP_CONTROL_EXCEPTION_PASS","scenario":"ordinary_exception","failure_class":"catchable_js_exception","compensation_ran":true,"compensation_cleanup_ok":true,"compensation_inventory_restored":true,"authoritative_absence":true}'
}

# HOST-OP BUDGET TRAP CLEANUP CONTROL.
#
# Failure class: the task/QJS hits the engine's UNCATCHABLE host-operation budget
# refusal (`budget exhausted: max_host_ops`) immediately after a successful browser
# start. Because it is a WASM fault rather than a JS exception, no `try/catch` can
# see it and the court never builds an envelope -- so "no court envelope" is the
# EXPECTED shape here, unlike the ordinary-exception control which requires one.
#
# WHAT RECLAIMS THE SESSION: the OUTER RUNNER, which stays alive. The runner calls
# the probe, gets its rc, and runs the SAME per_iteration_cleanup + authoritative
# readback as every other round. The runner's EXIT backstop is merely present and
# is a no-op here, because nothing killed the shell.
#
# WHAT THIS DOES NOT CLAIM: this is NOT evidence about the runner's EXIT or signal
# traps -- the shell survives, so those never carry the work, and by the time this
# control returns the transaction has already been disarmed, which makes the EXIT
# trap a no-op on this path. It also does NOT claim cleanup ran exactly once: this
# control has no dedicated witness or count assertion, so "exactly once" is simply
# not established here.
#
# Reuses the SAME helpers as the other rounds -- probe_source, browser_txn_arm,
# per_iteration_cleanup, browser_txn_disarm, snapshot_root. No second cleanup
# implementation exists.
live_browser_cleanup_control_host_op_trap() {
  local out rc=0 root before after
  local exe="${AGENTERM_BROWSER_EXE:-}"
  if [ -z "$exe" ] || [ ! -x "$exe" ]; then
    fail "BROWSER_BASELINE_NO_EXE"
    return
  fi
  local cu="${AGENTERM_CU_EXE:-}"
  if [ -z "$cu" ] || [ ! -x "$cu" ]; then
    fail "BROWSER_CLEANUP_CONTROL_NO_CU"
    return
  fi
  root=$(mktemp -d "${TMPDIR:-/tmp}/agenterm-live-browser.XXXXXX")
  root=$(CDPATH='' cd -- "$root" && pwd -P)
  mkdir -p "$root/agenterm-live-browser/state" "$root/agenterm-live-browser/home"
  BROWSER_TASK_HOME="$root/agenterm-live-browser/home"
  browser_txn_arm "$root/agenterm-live-browser/home" \
    "$BROWSER_SESSION_NAME $BROWSER_STRAY_SESSION_NAME"
  before=$(snapshot_root "$FORMAL_ROOT")
  local real_sessions_before real_sessions_after
  real_sessions_before=$(snapshot_root "$HOME/Library/Application Support/agenterm/browser-sessions")
  set +e
  # stderr is captured too: the engine's refusal names itself on the diagnostic
  # channel, so judging only stdout would miss the very token this asserts.
  out=$(AGENTERM_SCRIPT_BACKEND=qjswasm \
    AGENTERM_LIVE_REGION_SOURCE="$LIVE_REGION" \
    AGENTERM_BROWSER_STATE_ROOT="$root/agenterm-live-browser/state" \
    AGENTERM_BROWSER_HOME="$root/agenterm-live-browser/home" \
    AGENTERM_BROWSER_EXE="$exe" \
    AGENTERM_BROWSER_SOURCE="$(git -C "$REPO" rev-parse HEAD)" \
    AGENTERM_BROWSER_INPUT="$(browser_input_digest)" \
    AGENTERM_BROWSER_RUN_ID="$BROWSER_RUN_ID" \
    AGENTERM_BROWSER_DIGEST_SCRATCH="$BROWSER_TASK_HOME/../digest.scratch" \
    AGENTERM_TASK_HOME="$BROWSER_TASK_HOME" \
    AGENTERM_BROWSER_HOST_OP_TRAP=1 \
    probe_source "$LIVE" live-browser-baseline 2>&1)
  rc=$?
  set -e
  after=$(snapshot_root "$FORMAL_ROOT")
  real_sessions_after=$(snapshot_root "$HOME/Library/Application Support/agenterm/browser-sessions")
  # TRANSACTION BOUNDARY: always cleanup + readback BEFORE anything is deleted.
  if ! per_iteration_cleanup "$root/agenterm-live-browser/home" \
      "$BROWSER_SESSION_NAME $BROWSER_STRAY_SESSION_NAME"; then
    printf 'BROWSER_CLEANUP_CONTROL_ROOT:%s\n' "$root" >&2
    fail "BROWSER_CLEANUP_CONTROL_CLEANUP_FAILED:$BROWSER_CLEANUP_NOTE"
    return
  fi
  # The session is authoritatively absent, so the transaction can be disarmed.
  # Disarming is NOT deletion: the root survives until every judgement passes.
  browser_txn_disarm
  # EVERY JUDGEMENT RUNS BEFORE ANY DELETION, through ONE shared decision, so a
  # failing judgement never destroys the evidence it just examined.
  local stop
  stop=$(host_op_trap_stop_reason \
    "$before" "$after" "$real_sessions_before" "$real_sessions_after" "$rc" "$out")
  if [ -n "$stop" ]; then
    printf 'BROWSER_CLEANUP_CONTROL_ROOT:%s\n' "$root" >&2
    fail "$stop"
    return
  fi
  # Every judgement passed; only NOW is the evidence reclaimed.
  rm -rf "$root"
  printf '%s\n' "$out"
  printf '%s\n' \
    '{"schema":"agenterm.profile-binding-exact-process-cleanup-control/v1","ok":true,"code":"BROWSER_CLEANUP_CONTROL_HOST_OP_TRAP_PASS","scenario":"host_op_budget_trap","failure_class":"uncatchable_wasm_budget_trap","runner_survived":true,"authoritative_absence":true,"exit_trap_claim":false,"cleanup_once_claim":false}'
}

# ===========================================================================
# INTERNAL, NON-CATALOG. The two functions below are the TERM cleanup control's
# WORKER and CONTROLLER. They exist only to run this research control and are not
# public evidence paths, not task-catalog entries, and not documented capabilities.
#
# The worker is a SEPARATE invocation of this same script (`sh "$0"
# --live-browser-term-worker`), launched by the controller into its own session and
# process group. That separation is the whole point: a single `killpg` from the
# controller reaches BOTH the worker shell and the task child, so the signal is
# delivered while the transaction is genuinely armed and a session genuinely
# exists -- which no in-process arrangement could demonstrate.
# ===========================================================================

# The WORKER. It is the EVIDENCE cleanup owner: it arms its own transaction and its
# EXIT cleanup is what the witness counts, and it never manually cleans up or
# disarms, so every return path -- including the TERM path -- is handled by
# `runner_exit` exactly once. Inputs come only from explicit environment variables.
#
# It writes NOTHING that claims a browser started. It cannot establish that: it
# would have to observe the session in the same HOME, which is the CONTROLLER's job.
# The witness file therefore carries only `cleanup_run` lines.
live_browser_term_worker() {
  local home="${AGENTERM_TERM_WORKER_HOME:-}"
  local witness="${AGENTERM_TERM_WORKER_WITNESS:-}"
  if [ -z "$home" ] || [ ! -d "$home" ]; then
    fail "BROWSER_TERM_WORKER_NO_HOME"
    return
  fi
  if [ -z "$witness" ]; then
    fail "BROWSER_TERM_WORKER_NO_WITNESS"
    return
  fi
  BROWSER_TASK_HOME="$home"
  BROWSER_CLEANUP_WITNESS="$witness"
  # The TERM handler writes its DIRECT witness to the same file, on its own line.
  BROWSER_TERM_WITNESS="$witness"
  # REQUIRE the exact manifest path the controller minted and handed over. The
  # worker does NOT mint its own: two mints would be two files, and the whole point
  # of this control is that one exact path is owned by a known party. The path is
  # validated with the SAME shared validator and must already exist.
  local manifest="${AGENTERM_TERM_WORKER_MANIFEST:-}"
  if ! probe_manifest_path_owned "$manifest"; then
    fail "BROWSER_TERM_WORKER_MANIFEST_INVALID"
    return 1
  fi
  if [ ! -f "$manifest" ]; then
    fail "BROWSER_TERM_WORKER_MANIFEST_MISSING"
    return 1
  fi
  # Arm here, in the worker, so the armed state and the EXIT cleanup belong to the
  # same process that receives the signal. The manifest joins the transaction as
  # its THIRD component, so `runner_exit` reclaims it after the browser cleanup.
  browser_txn_arm "$home" "$BROWSER_SESSION_NAME $BROWSER_STRAY_SESSION_NAME" \
    "$manifest"
  # FOREGROUND on purpose: the handler exits with 143 and terminates the shell
  # immediately, so blocking here does not defer the verdict. TERM_HOLD parks the
  # script inside a bounded sleep loop until the controller signals.
  AGENTERM_BROWSER_TERM_HOLD=1 probe_source "$LIVE" live-browser-baseline "$manifest"
}

# The CONTROLLER. It creates the only disposable root, its HOME, and the witness
# file, launches the worker into a new session, waits for a session it can
# AUTHORITATIVELY confirm, sends exactly ONE killpg, and judges the result.
#
# TRANSACTION OWNERSHIP, stated precisely because the two roles differ:
#   * the WORKER is the EVIDENCE cleanup owner -- it arms its own transaction, and
#     its EXIT cleanup is what the witness counts;
#   * the CONTROLLER also arms, but only as an INDEPENDENT SAFETY BACKSTOP, in case
#     the worker dies before it can arm its own. On a green run this backstop does
#     NOT run and is NOT counted: the witness counts only the worker's cleanup.
# The controller never performs cleanup itself and never performs a second one.
live_browser_term_control() {
  local out rc=0 root before after
  local exe="${AGENTERM_BROWSER_EXE:-}"
  if [ -z "$exe" ] || [ ! -x "$exe" ]; then
    fail "BROWSER_BASELINE_NO_EXE"
    return
  fi
  local cu="${AGENTERM_CU_EXE:-}"
  if [ -z "$cu" ] || [ ! -x "$cu" ]; then
    fail "BROWSER_CLEANUP_CONTROL_NO_CU"
    return
  fi
  root=$(mktemp -d "${TMPDIR:-/tmp}/agenterm-live-browser.XXXXXX")
  root=$(CDPATH='' cd -- "$root" && pwd -P)
  local home="$root/agenterm-live-browser/home"
  local witness="$root/agenterm-live-browser/witness.txt"
  mkdir -p "$root/agenterm-live-browser/state" "$home"
  : >"$witness"
  # The CONTROLLER mints the unique manifest BEFORE forking and hands the EXACT
  # path to the worker. One minted file, known to both processes, is the whole
  # point: the worker's shell is the EVIDENCE cleanup owner (its EXIT removes this
  # path), and the controller's safety transaction holds the SAME path so a worker
  # that dies before arming cannot leak it. No side-channel file is needed because
  # nothing is discovered -- the path is decided here, once.
  local manifest
  # These two failures happen AFTER the root exists but BEFORE the transaction is
  # armed, so the EXIT trap is not yet a backstop. A bare `return` here would leak a
  # disposable root with nothing armed and nothing named -- so the exact root this
  # function just created is reclaimed explicitly. It is safe to remove precisely
  # because it was minted two lines above and has never been handed to anything else.
  manifest=$(mktemp "$REPO_ROOT_ABS/.probe-manifest.XXXXXX") || {
    rm -rf "$root"
    fail "BROWSER_CLEANUP_CONTROL_MANIFEST_MKTEMP"
    return
  }
  # VALIDATE the freshly minted path through the SAME validator every other site
  # uses, so a bad mint can never become a deletable or claimable path.
  if ! probe_manifest_path_owned "$manifest"; then
    rm -f "$manifest"
    rm -rf "$root"
    fail "BROWSER_CLEANUP_CONTROL_MANIFEST_INVALID"
    return
  fi
  BROWSER_TASK_HOME="$home"
  # Failure-safety backstop, holding the EXACT pre-minted manifest path. The
  # worker is the evidence cleanup owner; this is the independent safety layer that
  # uses the same known path if the worker dies before it can arm its own.
  browser_txn_arm "$home" "$BROWSER_SESSION_NAME $BROWSER_STRAY_SESSION_NAME" \
    "$manifest"
  before=$(snapshot_root "$FORMAL_ROOT")
  local real_sessions_before real_sessions_after
  real_sessions_before=$(snapshot_root "$HOME/Library/Application Support/agenterm/browser-sessions")
  set +e
  # ONE command substitution captures the worker's output AND its rc. Perl forks,
  # the child calls setsid so it becomes a new session and process-group leader
  # (pgid == pid), then execs this same script in worker mode. The parent forwards
  # the worker's exit STATUS through its own exit code so the shell sees 143 rather
  # than Perl's own convention.
  #
  # NO SHELL STRING IS EVER BUILT. Every child process is launched with an ARGV
  # LIST plus an explicit local environment (`local %ENV`), and the status probe's
  # exit code is captured explicitly. Backticks and `system STRING` are absent on
  # purpose: interpolating a HOME or a CU path into a shell string breaks on spaces
  # and creates an injection surface.
  #
  # TIMEBOUNDS, chosen so they cannot race: AWAIT is strictly SHORTER than the
  # worker's TERM_HOLD. If no confirmed session appears within AWAIT, the
  # controller sends NO signal and instead waits for the worker to end on its own,
  # under its own explicit bound. Since AWAIT < HOLD, a controller that gives up
  # always leaves the worker still holding, so its natural end is reachable.
  out=$(AGENTERM_TERM_WORKER_HOME="$home" \
    AGENTERM_TERM_WORKER_WITNESS="$witness" \
    AGENTERM_TERM_WORKER_MANIFEST="$manifest" \
    AGENTERM_LIVE_REGION_SOURCE="$LIVE_REGION" \
    AGENTERM_BROWSER_STATE_ROOT="$root/agenterm-live-browser/state" \
    AGENTERM_BROWSER_HOME="$home" \
    AGENTERM_BROWSER_EXE="$exe" \
    AGENTERM_BROWSER_SOURCE="$(git -C "$REPO" rev-parse HEAD)" \
    AGENTERM_BROWSER_INPUT="$(browser_input_digest)" \
    AGENTERM_BROWSER_RUN_ID="$BROWSER_RUN_ID" \
    AGENTERM_BROWSER_DIGEST_SCRATCH="$root/agenterm-live-browser/digest.scratch" \
    AGENTERM_TASK_HOME="$home" \
    AGENTERM_CU_EXE="$cu" \
    AGENTERM_EXE="$AGENTERM_EXE" \
    AGENTERM_REPO="$REPO" \
    perl -MPOSIX -e '
      my $witness = $ENV{AGENTERM_TERM_WORKER_WITNESS};
      my $home = $ENV{AGENTERM_TERM_WORKER_HOME};
      my $cu = $ENV{AGENTERM_CU_EXE};
      my $name = "agenterm-live-browser-baseline";
      my $entry = $ENV{AGENTERM_REPO} . "/research/browser-profile-name-binding-exact-process/run-current-host.sh";
      # AWAIT is strictly shorter than the worker TERM_HOLD, so giving up never
      # strands the worker past its own bound.
      my $await_seconds = 15;
      my $pid = fork();
      die "fork failed: $!" unless defined $pid;
      if ($pid == 0) {
        POSIX::setsid();
        exec("sh", $entry, "--live-browser-term-worker");
        exit 127;
      }
      # Run the status command with an ARGV LIST and an explicit local HOME, then
      # read its exit status explicitly. No shell string, no backticks.
      #
      # The "-|" form is load-bearing: THAT form forks, letting the child stdout
      # reach the parent through the handle. A plain "-" does NOT fork --
      # it returns a success value, so the child branch below would be unreachable
      # and there would be nothing to read or wait for. The mode is written here as
      # an explicit constant so the fork contract is visible at the call site.
      my $probe_status = sub {
        local %ENV = %ENV;
        $ENV{HOME} = $home;
        my $out = "";
        my $spid = open(my $fh, "-|");
        die "pipe failed: $!" unless defined $spid;
        if ($spid == 0) {
          # CHILD: its normal STDOUT IS the pipe the parent reads. Redirecting
          # STDOUT onto the handle here would feed the data back into the write end
          # and the parent would read nothing.
          exec($cu, "--target", "current", "--grant", "observe",
            "browser-session-status", $name);
          exit 127;
        }
        # READ FIRST, then close: closing before reading would discard the output
        # this pipe exists to carry, and close on a fork pipe REAPS the child, so the
        # status is read from `$?` immediately. A later waitpid has nothing to reap
        # and would report a meaningless value.
        my $out = "";
        while (my $line = <$fh>) { $out .= $line; }
        close($fh);
        return ($? >> 8, $out);
      };
      my $deadline = time() + $await_seconds;
      my $started = 0;
      while (time() < $deadline) {
        my ($src, $probe) = $probe_status->();
        # A confirmed start requires the command to SUCCEED (rc == 0) AND the exact
        # readiness FIELDS to carry their ready values. Loose substring matching
        # would accept a payload that merely mentions these words somewhere.
        if ($src == 0
            && $probe =~ /"state":"ready"/
            && $probe =~ /"owner_observation":"live"/) {
          $started = 1; last;
        }
        select(undef, undef, undef, 0.1);
      }
      if (!$started) {
        # NO SIGNAL is sent from this branch: with no confirmed live session, a TERM
        # would prove nothing about the armed transaction. Wait for the worker to
        # end on its own, which is reachable because AWAIT < TERM_HOLD.
        print "term_controller_started=0\n";
        waitpid($pid, 0);
        my $ws = $?;
        print "term_controller_worker_exit=", ($ws >> 8), "\n";
        print "term_controller_worker_signal=", ($ws & 127), "\n" if ($ws & 127);
        my $c = $ws >> 8;
        exit($c == 0 ? 3 : $c);
      }
      print "term_controller_started=1\n";
      # EXACTLY ONE TERM, to the negative pgid so it reaches the worker shell AND
      # the in-flight task child. A non-zero return is a named failure, never a
      # retry: a second delivery would also invalidate the once-only witness.
      my $ok = kill("TERM", -$pid);
      print "term_controller_killed=", ($ok ? 1 : 0), "\n";
      if (!$ok) { print "term_controller_kill_failed=$!\n"; }
      waitpid($pid, 0);
      # RAW WAIT-STATUS CLASSIFICATION. The high byte is the exit code and the low
      # 7 bits are the terminating signal number. These are DIFFERENT facts: a
      # handler that calls `exit 143` reports exit code 143 with signal 0, while a
      # process killed directly by SIGTERM reports exit code 0 with signal 15. Both
      # readings are printed so the two cases can never be confused, and neither is
      # silently mapped to zero.
      my $ws = $?;
      print "term_controller_worker_exit=", ($ws >> 8), "\n";
      print "term_controller_worker_signal=", ($ws & 127), "\n" if ($ws & 127);
      my $code = $ws >> 8;
      exit($ok ? $code : 4);
    ' 2>&1)
  rc=$?
  set -e
  after=$(snapshot_root "$FORMAL_ROOT")
  real_sessions_after=$(snapshot_root "$HOME/Library/Application Support/agenterm/browser-sessions")
  # Authoritative readback in the SAME home. The worker's EXIT cleanup is the only
  # cleanup; this reads the result, it does not perform a second one, and it
  # captures the status command's EXIT CODE as well as its text -- absence is two
  # facts, never a substring match.
  local status status_rc=0
  set +e
  status=$(HOME="$home" "$cu" --target current --grant observe \
    browser-session-status "$BROWSER_SESSION_NAME" 2>&1)
  status_rc=$?
  set -e
  # The started marker is the CONTROLLER's post-confirmation marker, because only
  # the controller can authoritatively observe the session in this HOME. The
  # worker's own file writes are not evidence that a browser started.
  local started killed witness_n term_signal_n worker_exit worker_signal
  started=$(printf '%s\n' "$out" | grep -c '^term_controller_started=1$' || true)
  killed=$(printf '%s\n' "$out" | grep -c '^term_controller_killed=1$' || true)
  # TWO SEPARATE COUNTS from the SAME witness file. The handler running and the
  # cleanup running are different facts with different failure modes, so they are
  # counted and claimed separately rather than collapsed into one number.
  witness_n=$(grep -c '^cleanup_run=1$' "$witness" 2>/dev/null || true)
  term_signal_n=$(grep -c '^term_signal=143$' "$witness" 2>/dev/null || true)
  # Raw worker wait classification, as printed by the controller's Perl parent.
  # A pure shell read: no external text tools, no file rewriting.
  worker_exit="<absent>"
  worker_signal="0"
  while IFS= read -r line; do
    case "$line" in
      term_controller_worker_exit=*) worker_exit="${line#term_controller_worker_exit=}" ;;
      term_controller_worker_signal=*) worker_signal="${line#term_controller_worker_signal=}" ;;
    esac
  done <<EOF
$(printf '%s\n' "$out")
EOF
  # The EXACT path this controller minted. It is known BEFORE the worker ran, which
  # is what makes the handoff claim true: the safety transaction holds this same
  # path, so a leaked manifest is reclaimable by the controller's EXIT even if the
  # worker never armed.
  local manifest_absent=0
  if [ ! -e "$manifest" ]; then
    manifest_absent=1
  fi
  local session_absent=0
  if [ "$status_rc" != "0" ]; then
    case "$status" in
      *'"code":"browser_session_not_found"'*) session_absent=1 ;;
    esac
  fi
  # DISARM the safety layer only when BOTH facts are proven. If either is false the
  # transaction STAYS ARMED on purpose, so the controller's EXIT can idempotently
  # reclaim the known session names and the known manifest, and nothing is silently
  # left behind. Note this happens AFTER the observation is captured below, so
  # safety cleanup can never launder a worker failure into a PASS.
  if [ "$session_absent" = "1" ] && [ "$manifest_absent" = "1" ]; then
    browser_txn_disarm
  fi
  # SAFETY-LAYER DISARMING. The disarm above happens ONLY when BOTH absence facts are
  # proven (session absent AND minted manifest absent), and it happens BEFORE the
  # judgements below. That yields exactly two branches:
  #   * BOTH proven -> already disarmed. A later failure of any remaining claim
  #     (rc, handler witness, cleanup witness, ...) preserves the root, and the EXIT
  #     trap does NOT re-enter, so nothing can append to or rewrite the evidence.
  #   * EITHER unproven -> still ARMED. A failing claim preserves the root and the
  #     EXIT trap MAY still run its safety cleanup for the known session names and
  #     the known manifest path, so a preserved root can carry extra safety-layer
  #     evidence. That extra evidence is expected in this branch, not a defect.
  # The judgement below is made from the PRE-SAFETY observation either way, so this
  # safety layer can never turn a worker failure into a PASS.
  local stop
  stop=$(term_control_stop_reason "$before" "$after" "$real_sessions_before" \
    "$real_sessions_after" "$rc" "$started" "$killed" "$witness_n" \
    "$status_rc" "$status" "$manifest_absent" "$out" \
    "$term_signal_n" "$worker_exit" "$worker_signal")
  if [ -n "$stop" ]; then
    # SURFACE THE CAPTURED OUTPUT FIRST. On failure the worker/controller/perl
    # diagnostics are the only way to see WHY, and they were being discarded with
    # the local `$out`. Printed before the ROOT/fail lines so the pane keeps both,
    # and only on the failure path so a success never prints it twice.
    printf '%s\n' "$out"
    printf 'BROWSER_CLEANUP_CONTROL_ROOT:%s\n' "$root" >&2
    fail "$stop"
    return
  fi
  # Every judgement passed. The safety backstop was already released by the proven
  # absence above; reclaim the root.
  rm -rf "$root"
  printf '%s\n' "$out"
  printf '%s\n' \
    '{"schema":"agenterm.profile-binding-exact-process-cleanup-control/v1","ok":true,"code":"BROWSER_CLEANUP_CONTROL_TERM_PASS","scenario":"runner_term","failure_class":"external_signal_to_runner","runner_signalled":true,"term_handler_witness_runs":1,"term_handler_ran":true,"exit_trap_cleanup_runs":1,"authoritative_absence":true,"probe_manifest_absent":true,"descendants_all_dead_claim":false}'
}

live_browser_red_gates() {
  # Each gate: mutation applied to REAL state/inputs, then the harness must see
  # rc != 0, NO PASS/EVIDENCE token, an ok:false envelope, and the NAMED failure.
  # A gate that exits 0, or that prints PASS, or that dies by crash/hang instead of
  # a named code, is itself a failure of the gate and is reported as such.
  local exe="${AGENTERM_BROWSER_EXE:-}"
  if [ -z "$exe" ] || [ ! -x "$exe" ]; then
    fail "BROWSER_BASELINE_NO_EXE"
    return
  fi
  local gates="${AGENTERM_BROWSER_RED_GATES:-identity_misbind unrelated_chain inventory_not_restored alive_after_stop selector_leak trace_deleted}"
  local gate ok_count=0
  for gate in $gates; do
    local root out rc=0
    root=$(mktemp -d "${TMPDIR:-/tmp}/agenterm-live-browser.XXXXXX")
    root=$(CDPATH='' cd -- "$root" && pwd -P)
    mkdir -p "$root/agenterm-live-browser/state" "$root/agenterm-live-browser/home"
    BROWSER_TASK_HOME="$root/agenterm-live-browser/home"
    # ARM before the probe: from here on a session may exist, so the EXIT trap is
    # allowed to compensate if this round is interrupted.
    browser_txn_arm "$root/agenterm-live-browser/home" \
      "$BROWSER_SESSION_NAME $BROWSER_STRAY_SESSION_NAME"
    set +e
    out=$(AGENTERM_SCRIPT_BACKEND=qjswasm \
      AGENTERM_LIVE_REGION_SOURCE="$LIVE_REGION" \
      AGENTERM_BROWSER_STATE_ROOT="$root/agenterm-live-browser/state" \
      AGENTERM_BROWSER_HOME="$root/agenterm-live-browser/home" \
      AGENTERM_BROWSER_EXE="$exe" \
      AGENTERM_BROWSER_SOURCE="$(git -C "$REPO" rev-parse HEAD)" \
      AGENTERM_BROWSER_INPUT="$(browser_input_digest)" \
      AGENTERM_BROWSER_RUN_ID="$BROWSER_RUN_ID" \
      AGENTERM_BROWSER_DIGEST_SCRATCH="$BROWSER_TASK_HOME/../digest.scratch" \
      AGENTERM_TASK_HOME="$BROWSER_TASK_HOME" \
      AGENTERM_BROWSER_RED_MODE="$gate" \
      probe_source "$LIVE" live-browser-baseline)
    rc=$?
    set -e
    # TRANSACTION BOUNDARY. The disposable root must survive until this round's
    # sessions are provably gone. Removing the root first is what previously made
    # the final trap read "no home" and report success over a live session.
    BROWSER_CLEANUP_FAILED=0
    if ! per_iteration_cleanup "$root/agenterm-live-browser/home" \
        "$BROWSER_SESSION_NAME $BROWSER_STRAY_SESSION_NAME"; then
      # FAIL-FAST: keep the root, KEEP THE TRANSACTION ARMED so the EXIT trap can
      # still compensate, and do NOT continue to the next mutant.
      printf 'BROWSER_RED_GATE_CLEANUP_FAILED:%s:root=%s\n' \
        "$gate" "$root" >&2
      fail "BROWSER_RED_GATE_CLEANUP_FAILED:$gate:$BROWSER_CLEANUP_NOTE"
      return
    fi
    # Verified clean: disarm FIRST, so the EXIT trap cannot later fail against a
    # HOME that is about to be deleted.
    browser_txn_disarm
    rm -rf "$root"
    # A gate must fail. rc == 0 means the mutation did not change the world.
    if [ "$rc" -eq 0 ]; then
      fail "BROWSER_RED_GATE_NOT_RED:$gate"
      continue
    fi
    # No success token may survive a mutated run. The court envelope's own verdict
    # is checked as an EXACT field, not a substring: a nested `..._ok":true` from
    # an unrelated sub-check must not be mistaken for the court passing, and the
    # court's `ok` must be a real boolean false.
    case "$out" in
      *'PASS: exact-process live browser baseline'*)
        fail "BROWSER_RED_GATE_PASS_TOKEN:$gate"; continue ;;
      *'EVIDENCE research.profile-binding-exact-process.live-browser-baseline'*)
        fail "BROWSER_RED_GATE_EVIDENCE_TOKEN:$gate"; continue ;;
    esac
    local verdict
    verdict=$(printf '%s\n' "$out" | grep 'browser-baseline/v1' | head -1)
    if [ -z "$verdict" ]; then
      fail "BROWSER_RED_GATE_NO_COURT_ENVELOPE:$gate"; continue
    fi
    case "$verdict" in
      *'"ok":false'*) ;;
      *) fail "BROWSER_RED_GATE_COURT_NOT_FALSE:$gate"; continue ;;
    esac
    # Zero PASS/EVIDENCE anywhere in the whole output, including any runner tail.
    # NOTE the precise scope: the static-scan envelope legitimately reports ok:true
    # (the source is unmodified -- which is the point), so only the COURT envelope
    # and the runner's own verdict may be judged here. A blanket `ok":true` search
    # would fail every gate for the wrong reason.
    case "$out" in
      *'browser-baseline/v1'*'"ok":true'*)
        fail "BROWSER_RED_GATE_COURT_OK_TRUE:$gate"; continue ;;
    esac
    case "$verdict" in
      *'"failed":[]'*)
        fail "BROWSER_RED_GATE_NO_FAILURE:$gate"; continue ;;
    esac
    # Each gate asserts the SPECIFIC named failure its mutation is supposed to
    # cause. Matching the mutation's own mode name would prove nothing: the mode
    # string is input, while the failure code is the verdict.
    local want=""
    case "$gate" in
      identity_misbind) want="subject_identity_mismatch" ;;
      unrelated_chain) want="browser_ownership_unprovable" ;;
      inventory_not_restored) want="browser_final_inventory_not_restored" ;;
      alive_after_stop) want="browser_not_dead:termination_deadline_exhausted" ;;
      selector_leak) want="browser_cleanup_selector_leak:" ;;
      trace_deleted) want="browser_trace_coverage_missing:" ;;
      *) fail "BROWSER_RED_GATE_UNKNOWN:$gate"; continue ;;
    esac
    # The named failure is matched against the EXTRACTED court verdict, not the
    # whole stdout. `$out` also carries the static-scan envelope and the runner
    # tail, so a whole-output match could in principle be satisfied by text that is
    # not the verdict at all. The verdict is the only place the code belongs.
    if ! verdict_names_failure "$verdict" "$want"; then
      fail "BROWSER_RED_GATE_TOKEN:$gate:want=$want"; continue
    fi
    # EXACT CLEANUP-TRUTH CHECK for the alive adversary. Disabling its own cleanup
    # is the whole point of the mutation, so the PRIMARY layer must honestly report
    # that it did nothing -- and, precisely BECAUSE the world is still dirty, the
    # COMPENSATION layer must run and genuinely SUCCEED (its results come from real
    # stop/remove calls). A fabricated primary success, or a compensation that
    # failed or never ran, both mean the adversary is not exercising what it claims.
    if [ "$gate" = "alive_after_stop" ]; then
      # These two are REQUIRED EXACTLY `false`. Testing only for `:true` would let a
      # MISSING or `null` field slip through -- an absent verdict is not a correct
      # verdict, and it is precisely how an adversary could hide.
      case "$verdict" in
        *'"primary_cleanup_ok":false'*) ;;
        *) fail "BROWSER_RED_GATE_PRIMARY_CLEANUP_OK_NOT_FALSE:$gate"; continue ;;
      esac
      case "$verdict" in
        *'"primary_cleanup_complete":false'*) ;;
        *) fail "BROWSER_RED_GATE_PRIMARY_CLEANUP_COMPLETE_NOT_FALSE:$gate"; continue ;;
      esac
      case "$verdict" in
        *'"primary_cleanup_skipped":true'*) ;;
        *) fail "BROWSER_RED_GATE_PRIMARY_NOT_SKIPPED:$gate"; continue ;;
      esac
      case "$verdict" in
        *'"compensation_ran":true'*) ;;
        *) fail "BROWSER_RED_GATE_COMPENSATION_DID_NOT_RUN:$gate"; continue ;;
      esac
      case "$verdict" in
        *'"compensation_cleanup_ok":true'*) ;;
        *) fail "BROWSER_RED_GATE_COMPENSATION_NOT_OK:$gate"; continue ;;
      esac
      case "$verdict" in
        *'"compensation_cleanup_complete":true'*) ;;
        *) fail "BROWSER_RED_GATE_COMPENSATION_NOT_COMPLETE:$gate"; continue ;;
      esac
      # REAL-DEADLINE EVIDENCE, derived from the poll, must be present AND true.
      # A missing or null field is not a correct verdict, so these are required
      # exactly rather than merely "not false".
      case "$verdict" in
        *'"termination_deadline_exhausted":true'*) ;;
        *) fail "BROWSER_RED_GATE_DEADLINE_NOT_EXHAUSTED:$gate"; continue ;;
      esac
      case "$verdict" in
        *'"termination_observed_live":true'*)
          ;;
        *) fail "BROWSER_RED_GATE_NOT_OBSERVED_LIVE:$gate"; continue ;;
      esac
      # Coverage must be COMPLETE: the unpolluted compensation's real stop/remove
      # calls are traced, so no required effect may be missing.
      case "$verdict" in
        *'"trace_missing_effects":[]'*) ;;
        *) fail "BROWSER_RED_GATE_TRACE_MISSING:$gate"; continue ;;
      esac
      # Recovery-layer inventory proof must be present and true: the compensation
      # read the real post-cleanup inventory and found it restored to 0/0.
      case "$verdict" in
        *'"compensation_inventory_restored":true'*) ;;
        *) fail "BROWSER_RED_GATE_COMPENSATION_INVENTORY_NOT_RESTORED:$gate"; continue ;;
      esac
      case "$verdict" in
        *'"compensation_inventory_live":0'*) ;;
        *) fail "BROWSER_RED_GATE_COMPENSATION_INVENTORY_LIVE:$gate"; continue ;;
      esac
      case "$verdict" in
        *'"compensation_inventory_rows":0'*) ;;
        *) fail "BROWSER_RED_GATE_COMPENSATION_INVENTORY_ROWS:$gate"; continue ;;
      esac
    fi
    ok_count=$((ok_count + 1))
    printf 'RED-GATE OK: %s (rc=%s, no PASS/EVIDENCE, ok:false, named failure)\n' \
      "$gate" "$rc"
  done
  printf 'RED-GATES OK: %s\n' "$ok_count"
}

live_process_preflight() {
  local out rc=0
  out=$(AGENTERM_LIVE_REGION_SOURCE="$LIVE_REGION" AGENTERM_SCRIPT_BACKEND=qjswasm \
    "$AGENTERM_EXE" cli script task run \
    profile-binding-exact-process-live-preflight --manifest "$MANIFEST") || rc=$?
  printf '%s\n' "$out"
  local missing=""
  for token in '"ok":true' \
      'EVIDENCE research.profile-binding-exact-process.live-process-preflight' \
      'PASS: exact-process live process preflight' \
      '"browser_launched":false' '"ordinal_reserved":false' \
      '"formal_root_touched":false' '"design_verdict":false'; do
    case "$out" in
      *"$token"*) ;;
      *) missing="$missing $token" ;;
    esac
  done
  if [ "$rc" -ne 0 ]; then
    fail "LIVE_PROCESS_PREFLIGHT_EXIT:$rc"
  fi
  if [ -n "$missing" ]; then
    fail "LIVE_PROCESS_PREFLIGHT_CLAIM:$missing"
  fi
}

# The preflight red gate. Only the preflight's OWN non-ok envelope counts as a
# red; a mutation that fails to apply, fails to compile, crashes or hangs is
# rejected, because it proves nothing about the guard it removed.
live_process_red_gate() {
  local failed=0 root
  root="$REPO/.agenterm-research-state/live-process-red-gate"
  rm -rf "$root"
  mkdir -p "$root"
  local mutant="$root/live-rehearsal.qjs"

  # Gate 1: the baseline must be green, or no mutation can be attributed.
  local baseline
  baseline=$(probe_source "$LIVE" "live-process-preflight") || true
  case "$baseline" in
    *'"ok":true'*)
      printf '  ok   the live process preflight passes unmutated\n' ;;
    *)
      printf '  FAIL the live process preflight did not pass (got: %s)\n' "$baseline"
      failed=1 ;;
  esac

  # Each mutation below either removes a guard that the REAL path exercises, or
  # injects the adversary that guard exists to catch. The second kind is
  # necessary because a correctly-behaving owned child never drifts and is never
  # `unknown`: a guard against those cases cannot be shown to bite by removing it
  # alone, so the mutation makes the adversary real instead of pretending the
  # baseline already contains it.

  # Each mutation below flips exactly ONE guard. Two of them first ARM an
  # adversary through the red-gate hooks, because a correctly-behaving owned child
  # never drifts its identity and is never `unknown`: a guard against those cases
  # cannot be shown to bite by removing it alone. Arming the adversary and then
  # removing the guard is the difference between real evidence and theatre.

  # Gate 2: the closing bracket must be a REAL observation. Arming drift alone
  # must turn the preflight red, proving the bracket really reads a new record.
  mutate_and_expect_fail "$LIVE" "$mutant" \
    'const DRIFT_MODE = "off";' \
    'const DRIFT_MODE = "drift";' \
    'chain bracket reads a new record' 'live-process-preflight' || failed=1

  # Gate 3: the identity comparison must exist. Two mutations are needed together:
  # arm the drift, then neutralise the comparison. A reused identity would then
  # pass as the frozen one.
  arm_and_disable_gate "$LIVE" "$mutant" \
    'const DRIFT_MODE = "off";' 'const DRIFT_MODE = "drift";' \
    'if (before.start_identity !== after.start_identity) {' 'if (false) {' \
    'chain identity comparison' || failed=1

  # Gate 4: `unknown` must never be read as death. Arming the unknown injection
  # must turn the preflight red, proving the unknown branch is what stops an
  # unreadable process from being reported as terminated.
  mutate_and_expect_fail "$LIVE" "$mutant" \
    'const UNKNOWN_MODE = "off";' \
    'const UNKNOWN_MODE = "unknown";' \
    'unknown is not dead' 'live-process-preflight' || failed=1

  # Gate 5: the clock must be the REAL door clock. Replacing the epoch read with a
  # small constant is exactly the fake clock this leaf forbids, and the
  # epoch-scale floor must reject it.
  mutate_and_expect_fail "$LIVE" "$mutant" \
    '      const ms = parseInt(value, 10);' \
    '      const ms = 1;' \
    'fake clock (epoch floor)' 'live-process-preflight' || failed=1

  # Gate 6: teardown must AGGREGATE failures rather than abandon the rest. Making
  # kill refuse must still reap and release, and must surface the failure.
  mutate_and_expect_fail "$LIVE" "$mutant" \
    '  const kill_status = process_kill(handle);' \
    '  const kill_status = -1;' \
    'teardown aggregation' 'live-process-preflight' || failed=1

  # Gate 7: the subject must be proven LIVE while the chain runs. The liveness is
  # now DERIVED from the first bracket, so this mutation removes the derivation
  # and makes the check's only remaining input a bare `false`.
  mutate_and_expect_fail "$LIVE" "$mutant" \
    '    subject_was_live = chain.ok === true && chain.subject_was_live === true;' \
    '    subject_was_live = false;' \
    'subject live during chain' 'live-process-preflight' || failed=1

  # Gate 7c: the frozen binding must be checked on the CLOSING side too. With only
  # the opening side disabled, a bracket that drifts back must still be caught by
  # the closing check.
  arm_and_disable_gate "$LIVE" "$mutant" \
    '    identity = {pid: frozen.pid, start_identity: frozen.start_identity};' \
    '    identity = {pid: frozen.pid, start_identity: "TAMPERED-frozen-identity"};' \
    '    if (is_first && after.start_identity !== subject_identity) {' \
    '    if (false && after.start_identity !== subject_identity) {' \
    'frozen subject identity is bound on the closing side' || failed=1

  # Gate 7a: the derived liveness must come from the FIRST BRACKET, not from a
  # constant. Neutralising the derivation inside the walk must go red even though
  # the caller still takes the value from the walk.
  mutate_and_expect_fail "$LIVE" "$mutant" \
    '        subject_was_live: brackets[0].before.state === "live"' \
    '        subject_was_live: false && brackets[0].before.state === "live"' \
    'subject liveness is derived from the first bracket' 'live-process-preflight' || failed=1

  # Gate 7b: THE FROZEN BINDING (P1-2). Tampering the frozen subject identity must
  # be refused: the walk may not accept whatever identity happens to be live.
  # The walk is given an identity that does NOT match the frozen one, and the
  # binding check is neutralised. The preflight must still not claim ownership.
  arm_and_disable_gate "$LIVE" "$mutant" \
    '    identity = {pid: frozen.pid, start_identity: frozen.start_identity};' \
    '    identity = {pid: frozen.pid, start_identity: "TAMPERED-frozen-identity"};' \
    '    if (is_first && before.start_identity !== subject_identity) {' \
    '    if (false && before.start_identity !== subject_identity) {' \
    'frozen subject identity is bound' || failed=1

  # Gate 8: the handle must be RELEASED. Skipping the release is the resource leak
  # this leaf forbids; the released flag must go red.
  mutate_and_expect_fail "$LIVE" "$mutant" \
    '  const release_status = process_release(handle);' \
    '  const release_status = -1;' \
    'skip release' 'live-process-preflight' || failed=1

  # Gate 9: the frozen identity must be observed UNTIL DEAD, not polled once.
  mutate_and_expect_fail "$LIVE" "$mutant" \
    '  while (clock.now_ms() - started <= deadline_ms) {' \
    '  while (false) {' \
    'observe until dead' 'live-process-preflight' || failed=1

  # Gate 10: THE TWO TERMINATION SHAPES (P1-3). PID reuse ends on a LIVE record
  # with a DIFFERENT identity; ordinary death ends on a `dead` record. Requiring
  # `dead` in both cases rejects a legitimate reuse, so this mutation must go red.
  mutate_and_expect_fail "$LIVE" "$mutant" \
    '  if (pid_reused === true) {' \
    '  if (false) {' \
    'pid-reuse termination shape' || failed=1

  # Gate 11: `pid_reused` must not manufacture a pass. With the reuse shape forced,
  # a final record still carrying the FROZEN identity must be refused.
  mutate_and_expect_fail "$LIVE" "$mutant" \
    '      && last.start_identity !== frozen_identity;' \
    '      && last.start_identity === frozen_identity;' \
    'pid reuse requires a real identity change' || failed=1

  # Gate 12: `unknown` must never terminate the poll in EITHER shape, including
  # when no termination was proved at all. The scenes this bites are the
  # `proved_dead === false` pair: with `proved_dead === true` the shape checks
  # reject an unknown record anyway, so only the unproved scenes make the guard
  # observable.
  mutate_and_expect_fail "$LIVE" "$mutant" \
    '    if (state === "unknown" && i === observations.length - 1) { return false; }' \
    '    if (false) { return false; }' \
    'unknown final observation is never death' || failed=1

  # Gate 13: FAIL-CLOSED ENTRY (P1-1). The evidence/pass tokens must be printed
  # only when the envelope is ok. Making the entry print them unconditionally is
  # the fail-open defect, and the runner's token check below must catch it.
  fail_open_gate || failed=1

  rm -rf "$root"
  if [ "$failed" -eq 0 ]; then
    printf '\n%s\n' 'LIVE_PROCESS_RED_GATE_PASS'
    return 0
  fi
  printf '\n%s\n' 'LIVE_PROCESS_RED_GATE_FAILED'
  return 1
}

RUN_ID_FOR_GATE=00112233445566778899aabbccddeeff

AGENTERM_EXE=${AGENTERM_EXE:-"$REPO/target/debug/agenterm"}
# The runner-side cleanup calls the CU binary directly rather than through the
# task, because the task may have trapped out. It is the sibling of the agenterm
# binary in the same build directory.
AGENTERM_CU_EXE=${AGENTERM_CU_EXE:-"$REPO/target/debug/agenterm-cu"}

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
  --live-self-test)
    # The court is a tool-profile entry, so this cannot use the plain run path.
    # The source region is ambient runner data rather than a task-manifest env
    # field; the task schema has no per-task environment injection.
    static_source_scan || fail INCONCLUSIVE_IDENTITY_SOURCE
    live_court_region >"$LIVE_REGION"
    AGENTERM_LIVE_REGION_SOURCE="$LIVE_REGION" AGENTERM_SCRIPT_BACKEND=qjswasm \
      "$AGENTERM_EXE" cli script task run \
      browser-profile-name-binding-exact-process-live --manifest "$MANIFEST"
    ;;
  --live-staged-preflight)
    # Browser-free persisted-stage integration slice. The formal root is
    # snapshot-guarded around the whole run and no browser is launched.
    static_source_scan || fail INCONCLUSIVE_IDENTITY_SOURCE
    live_court_region >"$LIVE_REGION"
    live_staged_preflight
    ;;
  --live-staged-red-gate)
    static_source_scan || fail INCONCLUSIVE_IDENTITY_SOURCE
    live_court_region >"$LIVE_REGION"
    live_staged_red_gate
    ;;
  --live-browser-red-gates)
    shift || true
    live_browser_red_gates
    ;;
  --live-browser-harness-self-test)
    # Browser-free: proves the verdict-token boundary the red gates rely on.
    shift || true
    harness_self_test
    ;;
  --live-browser-baseline)
    # REAL browser evidence: one disposable owned browser session in an isolated
    # HOME, with the exact identities separated, the parent chain proven with
    # exact-key ops and selector-free cleanup. Reserves no formal ordinal and
    # reaches no verdict. The browser path arrives by environment.
    static_source_scan || fail INCONCLUSIVE_IDENTITY_SOURCE
    live_court_region >"$LIVE_REGION"
    live_browser_baseline
    ;;
  --live-browser-cleanup-control-exception)
    # REAL browser evidence, but deliberately FAILING: the court throws a catchable
    # exception right after start, its compensation layer reclaims the session, and
    # the runner readback-verifies absence. Requires explicit authorization; it
    # launches a browser exactly like the baseline does.
    static_source_scan || fail INCONCLUSIVE_IDENTITY_SOURCE
    live_court_region >"$LIVE_REGION"
    live_browser_cleanup_control_exception
    ;;
  --live-browser-cleanup-control-host-op-trap)
    # REAL browser evidence, deliberately FAILING in a DIFFERENT class: the engine's
    # uncatchable host-operation budget refusal, so no court envelope is built at
    # all. The outer runner survives and reclaims the session. Requires explicit
    # authorization; it launches a browser exactly like the baseline does.
    static_source_scan || fail INCONCLUSIVE_IDENTITY_SOURCE
    live_court_region >"$LIVE_REGION"
    live_browser_cleanup_control_host_op_trap
    ;;
  --live-browser-cleanup-control-term)
    # REAL browser evidence, deliberately FAILING in the SIGNAL class: the worker is
    # TERM'd while armed and started. Requires explicit authorization; it launches a
    # browser exactly like the baseline does.
    static_source_scan || fail INCONCLUSIVE_IDENTITY_SOURCE
    live_court_region >"$LIVE_REGION"
    live_browser_term_control
    ;;
  --live-browser-term-worker)
    # INTERNAL, NON-CATALOG worker for the control above. Not a public evidence
    # path: the controller launches it into its own session and process group.
    live_browser_term_worker
    ;;
  --live-process-preflight)
    # REAL host evidence: one owned, non-browser, short-lived child; a real
    # frozen identity; the real ownership walk; and real observe-to-dead
    # termination. Launches no browser, reserves no ordinal, reaches no verdict.
    #
    # The dedicated registered task carries the mode argument and exact evidence
    # contract. Probe manifests remain confined to adversarial source mutations.
    static_source_scan || fail INCONCLUSIVE_IDENTITY_SOURCE
    live_court_region >"$LIVE_REGION"
    live_process_preflight
    ;;
  --live-process-red-gate)
    static_source_scan || fail INCONCLUSIVE_IDENTITY_SOURCE
    live_court_region >"$LIVE_REGION"
    live_process_red_gate
    ;;
  --live-red-gate)
    static_source_scan || fail INCONCLUSIVE_IDENTITY_SOURCE
    live_red_gate
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
    # Still refused. The live court entry now exists and its browser-free
    # self-test is gated, but the two conditions this refusal encodes are not
    # met: no live ordinal may be reserved while the court cannot prove every
    # throw site is preceded by a persisted stage (§4 kill criterion 4), and no
    # browser may be launched before that proof is reviewed (§2). The new entry
    # refuses these modes independently, so bypassing this runner is not a way
    # to reach a live run.
    fail LIVE_COURT_NOT_IMPLEMENTED
    ;;
  *)
    usage
    exit 1
    ;;
esac

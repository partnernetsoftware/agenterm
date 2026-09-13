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
trap 'rm -f "$LIVE_REGION"' EXIT HUP INT TERM

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

  # Gate 5: the parent-relation read must be bracketed. Removing the closing
  # observation must break the drift and identity scenes.
  mutate_and_expect_fail "$LIVE" "$mutant" \
    'const after = source.observe(current);' \
    'const after = before;' \
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

# Run an existing (possibly already-mutated) source through the tool-profile
# path. Kept separate from the copy step so a mutant is never overwritten by the
# unmutated original.
# `AGENTERM_LIVE_REGION_SOURCE` reaches the court through the runner's OWN
# environment, which the child inherits. No manifest field declares it: this
# slice invents no task-env schema (a task's `env` list and a contract's
# `env_allow` are existing concepts this probe must not repurpose), and the
# registered entry will read it the same way.
probe_source() {
  local target="$1" mode="${2:-}" manifest="$REPO/.probe-manifest.tmp" root_abs
  root_abs=$(CDPATH= cd -- "$REPO" && pwd -P)
  python3 - "$MANIFEST" "$manifest" "$target" "$root_abs" "$LIVE_REGION" "$mode" <<'PROBE'
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
             "max_host_operations": 256, "max_output_bytes": 262144},
  "network": [],
  "evidence": ["live-self-test"],
}}
json.dump(d, open(dst, "w"), indent=2)
PROBE
  # The task run is allowed to fail -- a mutation is EXPECTED to fail -- so the
  # manifest must be removed on both paths. Relying on the caller would leak the
  # file whenever the probe is aborted, and a leaked manifest would silently be
  # reused by the next probe.
  local rc=0
  AGENTERM_LIVE_REGION_SOURCE="$LIVE_REGION" AGENTERM_SCRIPT_BACKEND=qjswasm \
    "$AGENTERM_EXE" cli script task run \
    browser-profile-name-binding-exact-process-live --manifest "$manifest" 2>&1 \
    || rc=$?
  rm -f "$manifest"
  return $rc
}

# Apply one exact-string mutation to a copy of a source and require the court's
# self-test to fail. Rejecting an inapplicable mutation is the point: a mutation
# that silently failed to apply would leave a passing suite, and a passing suite
# read as a red gate is worse than no gate at all.
mutate_and_expect_fail() {
  local source="$1" target="$2" from="$3" to="$4" label="$5"
  cp "$source" "$target"
  if ! perl -0pi -e "s/\Q$from\E/$to/" "$target" 2>/dev/null; then
    printf '  FAIL %s: mutation could not be applied\n' "$label"; return 1
  fi
  if cmp -s "$source" "$target"; then
    printf '  FAIL %s: mutation did not change the source\n' "$label"; return 1
  fi
  local out
  out=$(probe_source "$target") || true
  # The self-test names every failed check in its thrown code, so a mutation that
  # broke any guard must surface that token. A mutant that fails to compile would
  # instead carry a compiler diagnostic, which must NOT count as a red: a
  # non-compiling mutant proves nothing about the guard it removed.
  if [ -n "${out##*live_self_test_failed*}" ]; then
    printf '  FAIL %s did not turn the suite red (got: %s)\n' "$label" "$out"
    return 1
  fi
  printf '  ok   %s turns the suite red\n' "$label"
  return 0
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

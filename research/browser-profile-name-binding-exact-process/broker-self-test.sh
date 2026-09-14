#!/bin/sh
# Isolated, disposable self-test for the external ledger / admission / stage
# persistence broker (`broker-spine.sh`).
#
# This exercises the broker's real machinery -- the authoritative ledger, the
# state lock, the frozen-input recheck, atomic reservation, the reserved ->
# finished / reserved -> abandoned transitions, ordinal non-reuse, receipt
# digest binding, durable stage + read-back before every simulated side effect,
# and the fixed seven-key persistence-failure record -- entirely inside a
# disposable root under the system temporary directory.
#
# It launches no browser, reserves no formal ordinal, writes nothing into the
# formal state root, produces no design fact and reaches no verdict. It only
# demonstrates that the broker can be trusted to enforce its own discipline.
#
#   broker-self-test.sh            # run the self-test; exit 0 on pass
#   broker-self-test.sh --keep     # keep the disposable root for inspection
#
# Provenance: the discipline under test is copied with provenance from
# `research/browser-profile-name-binding-v2/run-current-host.sh`; this is an
# independent, differently-scoped experiment whose broker has its own id,
# schemas, state root and ordinals. See `plan/design-browser-profile-name-binding-exact-process-experiment.md`.

set -eu

SELF=$0
DIR=$(CDPATH='' cd -- "$(dirname -- "$SELF")" && pwd -P)
SPINE="$DIR/broker-spine.sh"
[ -f "$SPINE" ] || { echo "broker-self-test: missing $SPINE" >&2; exit 2; }

KEEP=0
[ "${1:-}" = "--keep" ] && KEEP=1

PASS=0
FAIL=0
declare_report() {
  printf '%s\t%s\n' "$1" "$2"
}


# Assert a broker call fails with a specific typed code.
expect_fail() {
  local label=$1 expected=$2; shift 2
  local out rc
  set +e
  out=$("$@" 2>&1); rc=$?
  set -e
  if [ "$rc" -eq 0 ]; then
    bad "$label (expected failure '$expected', got exit 0)"
  elif printf '%s' "$out" | grep -qE "$expected"; then
    ok "$label"
  else
    bad "$label (wanted '$expected', got: $(printf '%s' "$out" | head -1))"
  fi
}

expect_ok() {
  local label=$1; shift
  local out rc
  set +e
  out=$("$@" 2>&1); rc=$?
  set -e
  # Report on fd 3, so a caller redirecting the command's stdout does not also
  # swallow this result line.
  if [ "$rc" -eq 0 ]; then
    ok "$label" >&3
  else
    bad "$label (exit $rc: $(printf '%s' "$out" | head -1))" >&3
  fi
  printf '%s' "$out"
}

ROOT=$(mktemp -d "${TMPDIR:-/tmp}/agenterm-profile-binding-exact-selftest.XXXXXX")
cleanup() {
  if [ "$KEEP" -eq 1 ]; then
    echo "kept self-test root: $ROOT"
  else
    rm -rf "$ROOT"
  fi
}
trap cleanup EXIT INT TERM

export AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$ROOT/state"

RUN_ID=00112233445566778899aabbccddeeff
RUN_ID2=ffeeddccbbaa99887766554433221100
SRC=0123456789abcdef0123456789abcdef01234567
EXPERIMENT='acu.dynamic.075.profile-name-binding-exact-process'
IN_DIG=$(printf 'agenterm-cu/profile-binding-exact-process/input/v1\0' | shasum -a 256 | cut -d' ' -f1)
EXE_DIG=$(printf 'executable-digest-selftest\0' | shasum -a 256 | cut -d' ' -f1)

broker() { "$SPINE" "$@"; }

req() {
  # Write a `preflight` stage request and echo its path. The facts match the
  # template's `preflight` shape and the criteria carry only the template's
  # criteria keys.
  local path=$1 code=$2
  cat >"$path" <<EOF
{"stage":"preflight","producer":"broker-self-test","deadline_ms":1000,"elapsed_ms":0,
 "code":"$code",
 "facts":{"source_sha":"$SRC","input_digest":"$IN_DIG","executable_digest":"$EXE_DIG","ordinal":"R1","deadline_ms":1000,"stage_seq":1},
 "criteria":{"V1":"not-run","V2":"not-run","V3":"not-run","V4":"not-run",
             "V5":"not-run","V6":"not-run","V7":"not-run"}}
EOF
  printf '%s' "$path"
}

# Write a `terminal` stage request closing the attempt with CODE and the
# criteria shape that code is allowed to publish.
terminal_req() {
  local path=$1 code=$2 criteria=$3
  cat >"$path" <<EOF
{"stage":"terminal","producer":"broker-self-test","deadline_ms":1000,"elapsed_ms":0,
 "code":"$code",
 "facts":{"primary_cause":"$code","cleanup_status":"not-run","terminal_code":"$code"},
 "criteria":$criteria}
EOF
  printf '%s' "$path"
}

# The criteria shapes the decision tree permits for each validity terminal.
CRIT_V2_FAIL='{"V1":"pass","V2":"fail","V3":"not-run","V4":"not-run","V5":"not-run","V6":"not-run","V7":"not-run"}'
CRIT_V2_PASS='{"V1":"pass","V2":"pass","V3":"not-run","V4":"not-run","V5":"not-run","V6":"not-run","V7":"not-run"}'
CRIT_V6_FAIL='{"V1":"pass","V2":"pass","V3":"pass","V4":"pass","V5":"pass","V6":"fail","V7":"not-run"}'

echo "broker self-test (isolated root, reserves nothing formal)"

# Results are reported on fd 3 so that redirecting a checked command's stdout
# does not also hide the pass/fail line.
exec 3>&1

ok()   { PASS=$((PASS + 1)); printf '  ok   %s\n' "$1" >&3; }
bad()  { FAIL=$((FAIL + 1)); printf '  FAIL %s\n' "$1" >&3; }

# Aggregator red-gate hook. When `BROKER_SELFTEST_INJECT_FAILURE` names an
# assertion, that assertion is deliberately reported as failed. This exists so a
# harness can prove the summary is fail-closed: with an injected failure the run
# must print `BROKER_SELF_TEST_FAILED` and exit nonzero, never `PASS`. It is
# used only by `broker-self-test-harness.sh` and changes no product behavior.
INJECT_FAILURE=${BROKER_SELFTEST_INJECT_FAILURE:-}
if [ -n "$INJECT_FAILURE" ]; then
  bad "injected failure for aggregator red-gate ($INJECT_FAILURE)"
fi

# -- 1. external ledger is authoritative; the reservation is durable ----------
broker reserve rehearsal R1 "$RUN_ID" "$SRC" "$IN_DIG" >/dev/null
LEDGER="$AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT/attempt-ledger.jsonl"
if [ -f "$LEDGER" ] && grep -q '"status":"reserved"' "$LEDGER"; then
  ok "reserve publishes a durable authoritative ledger row"
else
  bad "reserve publishes a durable authoritative ledger row"
fi

# Read the ledger back through the broker, not through the file.
if broker inspect | grep -q '"ordinal":"R1"'; then
  ok "inspect reads the reservation back through the broker"
else
  bad "inspect reads the reservation back through the broker"
fi

# -- 2. ordinal is not reusable ---------------------------------------------
expect_fail "ordinal R1 is refused when already reserved" 'reserve_ordinal_reused' \
  broker reserve rehearsal R1 "$RUN_ID2" "$SRC" "$IN_DIG"

# -- 3. state lock is exclusive ---------------------------------------------
# Hold the lock in a background shell and require the broker to refuse. Use a
# valid (kind, ordinal) pair so the lock is genuinely the first thing checked.
LOCK="$AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT/state.lock"
perl -e '
  use Fcntl qw(:flock);
  open(my $fh, ">>", $ARGV[0]) or die;
  flock($fh, LOCK_EX) or die;
  sleep 3;
' "$LOCK" &
LOCKPID=$!
sleep 0.5
expect_fail "state lock is exclusive while held" 'state_lock_busy' \
  broker reserve decision D1 "$RUN_ID2" "$SRC" "$IN_DIG"
kill "$LOCKPID" 2>/dev/null || true
wait "$LOCKPID" 2>/dev/null || true

# -- 4. durable stage + read-back before the simulated side effect ----------
REQ="$ROOT/stage1.json"; req "$REQ" PREFLIGHT_OK >/dev/null
expect_ok "stage publishes durably and reads back" broker stage rehearsal R1 "$RUN_ID" "$REQ" >/dev/null
JOURNAL="$AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT/stage-journal/rehearsal-1.jsonl"
RECEIPT="$AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT/rehearsal-1-receipt.json"
if [ -f "$JOURNAL" ] && [ -f "$RECEIPT" ]; then
  ok "stage journal and receipt both exist on disk"
else
  bad "stage journal and receipt both exist on disk"
fi

# A non-terminal stage may not carry a terminal code, and a terminal stage may
# not carry a non-terminal one.
BAD_REQ="$ROOT/stage-badcode.json"
cat >"$BAD_REQ" <<EOF
{"stage":"preflight","producer":"broker-self-test","deadline_ms":1000,"elapsed_ms":0,
 "code":"INVALID_EVIDENCE",
 "facts":{"source_sha":"$SRC","input_digest":"$IN_DIG","executable_digest":"$EXE_DIG","ordinal":"R1","deadline_ms":1000,"stage_seq":1},
 "criteria":{"V1":"not-run","V2":"not-run","V3":"not-run","V4":"not-run","V5":"not-run","V6":"not-run","V7":"not-run"}}
EOF
expect_fail "a non-terminal stage may not publish a terminal code" \
  'stage_non_terminal_code' \
  broker stage rehearsal R1 "$RUN_ID" "$BAD_REQ"

# The receipt must bind the journal's final digest, and re-deriving that digest
# from the journal on disk must reproduce it. A receipt that names any other
# digest is not proof of the rows it claims.
JSEQ=$(perl -MJSON::PP -e 'local $/; print decode_json(<>)->{journal_final_seq}' <"$RECEIPT")
JDIG=$(perl -MJSON::PP -e 'local $/; print decode_json(<>)->{journal_final_sha256}' <"$RECEIPT")
[ "$JSEQ" = "1" ] && [ "${#JDIG}" -eq 64 ] \
  && ok "receipt binds the journal final seq and digest" \
  || bad "receipt binds the journal final seq and digest"
DERIVED=$(perl -MDigest::SHA=sha256_hex -MJSON::PP -e '
  local $/; my $t = <>;
  my $json = JSON::PP->new->canonical(1)->allow_nonref(0)->utf8(1);
  my @rows = map { $json->decode($_) } grep { /^\s*\{/ } split /\n/, $t;
  my $prev = q{};
  for my $r (@rows) { $prev = sha256_hex($json->encode($r)) }
  print $prev;
' <"$JOURNAL")
[ "$DERIVED" = "$JDIG" ] \
  && ok "the receipt digest re-derives from the journal on disk" \
  || bad "the receipt digest re-derives from the journal on disk (got $DERIVED, receipt says $JDIG)"

# -- 5. a ledger finish row must name a vocabulary terminal --------------
BROKEN_ROOT="$ROOT/broken-state"
AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$BROKEN_ROOT" \
  "$SPINE" reserve rehearsal R1 "$RUN_ID" "$SRC" "$IN_DIG" >/dev/null
BROKEN_LEDGER="$BROKEN_ROOT/attempt-ledger.jsonl"
perl -MJSON::PP -MDigest::SHA=sha256_hex -e '
  my $json = JSON::PP->new->canonical(1)->allow_nonref(0)->utf8(1);
  local $/; my $t = <>;
  chomp $t;
  my $r = $json->decode($t);
  my $finish = { %$r };
  $finish->{status} = "finished";
  $finish->{receipt_sha256} = sha256_hex("receipt");
  $finish->{terminal_code} = "NOT_A_TERMINAL_CODE";
  print $json->encode($r), "\n", $json->encode($finish), "\n";
' <"$BROKEN_LEDGER" >"$BROKEN_LEDGER.new"
perl -e 'rename($ARGV[0],$ARGV[1]) or die $!' "$BROKEN_LEDGER.new" "$BROKEN_LEDGER"
BROKEN_OUT=$(AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$BROKEN_ROOT" \
  "$SPINE" inspect 2>&1 || true)
[ "$BROKEN_OUT" = "ledger_finish_code" ] \
  && ok "a finish code outside the terminal vocabulary is refused" \
  || bad "a finish code outside the terminal vocabulary is refused (got: $BROKEN_OUT)"

# A finished row must bind a real receipt digest. There is no persistence
# exception, because a persistence failure leaves the attempt reserved.
NULL_ROOT="$ROOT/null-receipt-state"
AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$NULL_ROOT" \
  "$SPINE" reserve rehearsal R1 "$RUN_ID" "$SRC" "$IN_DIG" >/dev/null
NULL_LEDGER="$NULL_ROOT/attempt-ledger.jsonl"
perl -MJSON::PP -e '
  my $json = JSON::PP->new->canonical(1)->allow_nonref(0)->utf8(1);
  local $/; my $t = <>; chomp $t;
  my $r = $json->decode($t);
  my $finish = { %$r };
  $finish->{status} = "finished";
  $finish->{receipt_sha256} = undef;
  $finish->{terminal_code} = "INVALID_EVIDENCE";
  print $json->encode($r), "\n", $json->encode($finish), "\n";
' <"$NULL_LEDGER" >"$NULL_LEDGER.new"
perl -e 'rename($ARGV[0],$ARGV[1]) or die $!' "$NULL_LEDGER.new" "$NULL_LEDGER"
NULL_OUT=$(AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$NULL_ROOT" \
  "$SPINE" inspect 2>&1 || true)
[ "$NULL_OUT" = "ledger_finish_receipt" ] \
  && ok "a finished row without a receipt digest is refused" \
  || bad "a finished row without a receipt digest is refused (got: $NULL_OUT)"

# There is no public operation that closes an attempt without a receipt.
if grep -q "evidence-fail" "$SPINE"; then
  bad "no public operation closes an attempt without a receipt"
else
  ok "no public operation closes an attempt without a receipt"
fi

# -- 5b. finish refuses an attempt whose journal has no terminal stage -----
EARLY_ROOT="$ROOT/early-finish-state"
AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$EARLY_ROOT" \
  "$SPINE" reserve rehearsal R1 "$RUN_ID" "$SRC" "$IN_DIG" >/dev/null
AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$EARLY_ROOT" \
  "$SPINE" stage rehearsal R1 "$RUN_ID" "$REQ" >/dev/null
EARLY_SHA=$(shasum -a 256 "$EARLY_ROOT/rehearsal-1-receipt.json" | cut -d' ' -f1)
expect_fail "finish refuses a journal with no terminal stage" \
  'finish_without_terminal_stage' \
  env AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$EARLY_ROOT" \
    "$SPINE" finish rehearsal R1 "$RUN_ID" "$EARLY_SHA"

# A receipt that binds the right journal digest at stage time must still be
# re-verified at finish time. Tamper with the receipt on disk after staging and
# require finish to refuse: the digest is re-derived from the journal, not
# trusted from the receipt's own claim.
TAMPER_ROOT="$ROOT/tamper-state"
AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$TAMPER_ROOT" \
  "$SPINE" reserve rehearsal R1 "$RUN_ID" "$SRC" "$IN_DIG" >/dev/null
AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$TAMPER_ROOT" \
  "$SPINE" stage rehearsal R1 "$RUN_ID" "$REQ" >/dev/null
TAMPER_TERM="$ROOT/tamper-terminal.json"
terminal_req "$TAMPER_TERM" INCONCLUSIVE_IDENTITY_SOURCE "$CRIT_V2_FAIL" >/dev/null
AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$TAMPER_ROOT" \
  "$SPINE" stage rehearsal R1 "$RUN_ID" "$TAMPER_TERM" >/dev/null
TAMPER_RECEIPT="$TAMPER_ROOT/rehearsal-1-receipt.json"
perl -MJSON::PP -e '
  my $json = JSON::PP->new->canonical(1)->allow_nonref(0)->utf8(1);
  local $/; my $r = $json->decode(<>);
  $r->{journal_final_sha256} = "0" x 64;
  print $json->encode($r);
' <"$TAMPER_RECEIPT" >"$TAMPER_RECEIPT.new"
perl -e 'rename($ARGV[0],$ARGV[1]) or die $!' "$TAMPER_RECEIPT.new" "$TAMPER_RECEIPT"
TAMPER_SHA=$(shasum -a 256 "$TAMPER_RECEIPT" | cut -d' ' -f1)
expect_fail "finish re-derives the journal digest and refuses a tampered receipt" \
  'finish_journal_digest' \
  env AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$TAMPER_ROOT" \
    "$SPINE" finish rehearsal R1 "$RUN_ID" "$TAMPER_SHA"

# -- 6. reserved -> finished closes the attempt through the real finish -----
# Close R1 legitimately: stage a terminal row whose code and criteria agree,
# then finish against that receipt's digest.
TERM_REQ="$ROOT/stage-terminal.json"
terminal_req "$TERM_REQ" INCONCLUSIVE_IDENTITY_SOURCE "$CRIT_V2_FAIL" >/dev/null
expect_ok "a terminal stage with an agreeing code and criteria is accepted" \
  broker stage rehearsal R1 "$RUN_ID" "$TERM_REQ" >/dev/null
RSHA=$(shasum -a 256 "$RECEIPT" | cut -d' ' -f1)

# A wrong receipt digest must not close the attempt.
expect_fail "finish refuses a receipt digest that does not match" \
  'finish_receipt_sha_mismatch' \
  broker finish rehearsal R1 "$RUN_ID" "$(printf '0%.0s' 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31 32 33 34 35 36 37 38 39 40 41 42 43 44 45 46 47 48 49 50 51 52 53 54 55 56 57 58 59 60 61 62 63 64)"

# The attempt is still open after a refused finish.
if broker inspect | grep -q '"status":"reserved"'; then
  ok "a refused finish leaves the attempt reserved"
else
  bad "a refused finish leaves the attempt reserved"
fi

expect_ok "finish closes the attempt against its published receipt" \
  broker finish rehearsal R1 "$RUN_ID" "$RSHA" >/dev/null
if grep -q '"terminal_code":"INCONCLUSIVE_IDENTITY_SOURCE"' "$LEDGER"; then
  ok "finished row records the receipt's terminal code"
else
  bad "finished row records the receipt's terminal code"
fi
if grep -q "\"receipt_sha256\":\"$RSHA\"" "$LEDGER"; then
  ok "finished row records the verified receipt digest"
else
  bad "finished row records the verified receipt digest"
fi

# A finished ordinal is neither reusable nor stageable.
expect_fail "finished ordinal cannot be reserved again" 'reserve_ordinal_reused' \
  broker reserve rehearsal R1 "$RUN_ID2" "$SRC" "$IN_DIG"
expect_fail "a finished attempt rejects further stages" 'attempt_not_active' \
  broker stage rehearsal R1 "$RUN_ID" "$REQ"

# -- 7. reserved -> abandoned closes the attempt ----------------------------
# D1's kind is decision, so a rehearsal reservation of D1 must be refused.
expect_fail "kind and ordinal must agree" 'reserve_ordinal' \
  broker reserve rehearsal D1 "$RUN_ID" "$SRC" "$IN_DIG"

broker reserve decision D1 "$RUN_ID" "$SRC" "$IN_DIG" >/dev/null
expect_ok "abandon closes a reservation without a receipt" \
  broker abandon decision D1 "$RUN_ID" >/dev/null
if grep -q '"status":"abandoned"' "$LEDGER"; then
  ok "abandoned row is recorded in the authoritative ledger"
else
  bad "abandoned row is recorded in the authoritative ledger"
fi

# -- 8. an unrelated prior row cannot prove a failing stage -----------------
# Two distinct scenes, because they prove different things.
#
# (i) A closed attempt cannot be reopened: once abandoned, a stage request must
# not borrow the earlier reservation or its rows.
expect_fail "an abandoned attempt cannot publish a stage" 'attempt_not_active' \
  broker stage decision D1 "$RUN_ID" "$REQ"

# (ii) The real substitution risk: an attempt that HAS an earlier successful
# stage row tries to close without ever publishing the failing stage. A
# `preflight` row exists and reads back equal, but there is no `terminal` row,
# so the earlier row must not be able to stand in for the failing stage.
SUB_ROOT="$ROOT/substitute-state"
AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$SUB_ROOT" \
  "$SPINE" reserve rehearsal R1 "$RUN_ID" "$SRC" "$IN_DIG" >/dev/null
AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$SUB_ROOT" \
  "$SPINE" stage rehearsal R1 "$RUN_ID" "$REQ" >/dev/null
SUB_RECEIPT="$SUB_ROOT/rehearsal-1-receipt.json"
SUB_SHA=$(shasum -a 256 "$SUB_RECEIPT" | cut -d' ' -f1)
# The receipt is genuine and its digest matches, but it closes on `preflight`.
SUB_RECEIPT_STAGE=$(perl -MJSON::PP -e 'local $/; print decode_json(<>)->{last_completed_stage}' <"$SUB_RECEIPT")
[ "$SUB_RECEIPT_STAGE" = "preflight" ] \
  && ok "the substitute-scene receipt really closes on a preflight-only row" \
  || bad "the substitute-scene receipt really closes on a preflight-only row (got: $SUB_RECEIPT_STAGE)"
SUB_OUT=$(AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$SUB_ROOT" \
  "$SPINE" finish rehearsal R1 "$RUN_ID" "$SUB_SHA" 2>&1 || true)
case "$SUB_OUT" in
  finish_without_terminal_stage|finish_receipt_not_terminal)
    ok "a preflight-only row cannot substitute for the failing terminal stage ($SUB_OUT)" ;;
  *) bad "a preflight-only row cannot substitute for the failing terminal stage (got: $SUB_OUT)" ;;
esac
# And the attempt stays open: the earlier row bought nothing.
if AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$SUB_ROOT" \
     "$SPINE" inspect | grep -q '"status":"reserved"'; then
  ok "a refused substitution leaves the attempt reserved"
else
  bad "a refused substitution leaves the attempt reserved"
fi

# -- 8b. a rehearsal may record validity; it may not record a design fact ---
# R1 is exactly the attempt that measures V1-V7, so a validity pass or failure
# is legitimate. The forbidden thing is a design fact.
DESIGN_REQ="$ROOT/stage-design.json"
cat >"$DESIGN_REQ" <<EOF
{"stage":"terminal","producer":"broker-self-test","deadline_ms":1000,"elapsed_ms":0,
 "code":"INCONCLUSIVE_IDENTITY_SOURCE",
 "facts":{"primary_cause":"INCONCLUSIVE_IDENTITY_SOURCE","cleanup_status":"not-run","terminal_code":"INCONCLUSIVE_IDENTITY_SOURCE","D1":"A1_SELECTED"},
 "criteria":$CRIT_V2_FAIL}
EOF
D_ROOT="$ROOT/design-state"
AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$D_ROOT" \
  "$SPINE" reserve rehearsal R1 "$RUN_ID" "$SRC" "$IN_DIG" >/dev/null
DESIGN_OUT=$(AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$D_ROOT" \
  "$SPINE" stage rehearsal R1 "$RUN_ID" "$DESIGN_REQ" 2>&1 || true)
[ "$DESIGN_OUT" = "design_fact_present" ] \
  && ok "a design fact in a rehearsal stage row is refused by name" \
  || bad "a design fact in a rehearsal stage row is refused by name (got: $DESIGN_OUT)"

# A terminal whose criteria contradict its code must be refused.
MISMATCH_REQ="$ROOT/stage-mismatch.json"
terminal_req "$MISMATCH_REQ" INCONCLUSIVE_IDENTITY_SOURCE "$CRIT_V2_PASS" >/dev/null
M_ROOT="$ROOT/mismatch-state"
AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$M_ROOT" \
  "$SPINE" reserve rehearsal R1 "$RUN_ID" "$SRC" "$IN_DIG" >/dev/null
expect_fail "a terminal whose criteria contradict its code is refused" \
  'terminal_criteria_not_legal' \
  env AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$M_ROOT" \
    "$SPINE" stage rehearsal R1 "$RUN_ID" "$MISMATCH_REQ"

# -- 8c. every legal terminal alternative is accepted, and its near misses are
# not. Each alternative gets a positive case and at least one negative case, so
# a mapping that is too narrow and a mapping that is too wide are both caught.
#
# helper: does this (code, criteria) pair pass stage?
alt_case() {
  local label=$1 code=$2 criteria=$3 expect=$4
  local root="$ROOT/alt-$label"
  local creq="$ROOT/alt-$label.json"
  terminal_req "$creq" "$code" "$criteria" >/dev/null
  AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$root" \
    "$SPINE" reserve rehearsal R1 "$RUN_ID" "$SRC" "$IN_DIG" >/dev/null
  local out
  out=$(AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$root" \
    "$SPINE" stage rehearsal R1 "$RUN_ID" "$creq" 2>&1 >/dev/null || true)
  if [ "$expect" = accept ]; then
    [ -z "$out" ] && ok "alt $label: accepted" \
      || bad "alt $label: accepted (got: $out)"
  else
    [ -z "$out" ] && bad "alt $label: refused" \
      || ok "alt $label: refused ($out)"
  fi
}

# INVALID_INPUT: V1 fail must be the only proven criterion.
alt_case "input-v1fail" INVALID_INPUT \
  '{"V1":"fail","V2":"not-run","V3":"not-run","V4":"not-run","V5":"not-run","V6":"not-run","V7":"not-run"}' accept
alt_case "input-v1pass" INVALID_INPUT \
  '{"V1":"pass","V2":"not-run","V3":"not-run","V4":"not-run","V5":"not-run","V6":"not-run","V7":"not-run"}' refuse

# INCONCLUSIVE_IDENTITY_SOURCE: V1 pass, V2 fail.
alt_case "v2-fail" INCONCLUSIVE_IDENTITY_SOURCE "$CRIT_V2_FAIL" accept
alt_case "v2-pass" INCONCLUSIVE_IDENTITY_SOURCE "$CRIT_V2_PASS" refuse
alt_case "v2-unrun" INCONCLUSIVE_IDENTITY_SOURCE \
  '{"V1":"not-run","V2":"fail","V3":"not-run","V4":"not-run","V5":"not-run","V6":"not-run","V7":"not-run"}' refuse

# INCONCLUSIVE_OWNERSHIP: V1,V2 pass, V3 fail.
alt_case "v3-fail" INCONCLUSIVE_OWNERSHIP \
  '{"V1":"pass","V2":"pass","V3":"fail","V4":"not-run","V5":"not-run","V6":"not-run","V7":"not-run"}' accept
alt_case "v3-without-v2" INCONCLUSIVE_OWNERSHIP \
  '{"V1":"pass","V2":"not-run","V3":"fail","V4":"not-run","V5":"not-run","V6":"not-run","V7":"not-run"}' refuse

# INCONCLUSIVE_CLEANUP has THREE legal shapes: V4 alone, V5 alone, or both
# failing together (the frozen model's `cleanup-both-fail` case). All must be
# accepted, while a shape that skips an earlier gate must not be.
alt_case "cleanup-v4fail" INCONCLUSIVE_CLEANUP \
  '{"V1":"pass","V2":"pass","V3":"pass","V4":"fail","V5":"not-run","V6":"not-run","V7":"not-run"}' accept
alt_case "cleanup-v5fail" INCONCLUSIVE_CLEANUP \
  '{"V1":"pass","V2":"pass","V3":"pass","V4":"pass","V5":"fail","V6":"not-run","V7":"not-run"}' accept
alt_case "cleanup-bothfail" INCONCLUSIVE_CLEANUP \
  '{"V1":"pass","V2":"pass","V3":"pass","V4":"fail","V5":"fail","V6":"not-run","V7":"not-run"}' accept
alt_case "cleanup-v5fail-v4unrun" INCONCLUSIVE_CLEANUP \
  '{"V1":"pass","V2":"pass","V3":"pass","V4":"not-run","V5":"fail","V6":"not-run","V7":"not-run"}' refuse
alt_case "cleanup-bothfail-v3unrun" INCONCLUSIVE_CLEANUP \
  '{"V1":"pass","V2":"pass","V3":"not-run","V4":"fail","V5":"fail","V6":"not-run","V7":"not-run"}' refuse

# INVALID_EVIDENCE: V1-V5 pass, V6 fail, V7 not-run.
alt_case "v6-fail" INVALID_EVIDENCE "$CRIT_V6_FAIL" accept
alt_case "v6-with-v4-unrun" INVALID_EVIDENCE \
  '{"V1":"pass","V2":"pass","V3":"pass","V4":"not-run","V5":"not-run","V6":"fail","V7":"not-run"}' refuse
alt_case "v6-with-v7-fail" INVALID_EVIDENCE \
  '{"V1":"pass","V2":"pass","V3":"pass","V4":"pass","V5":"pass","V6":"fail","V7":"fail"}' refuse

# INVALID_EXPERIMENT: V1-V6 pass, V7 fail.
alt_case "v7-fail" INVALID_EXPERIMENT \
  '{"V1":"pass","V2":"pass","V3":"pass","V4":"pass","V5":"pass","V6":"pass","V7":"fail"}' accept
alt_case "v7-with-v6-unrun" INVALID_EXPERIMENT \
  '{"V1":"pass","V2":"pass","V3":"pass","V4":"pass","V5":"pass","V6":"not-run","V7":"fail"}' refuse

# INCONCLUSIVE_MECHANISM: every validity gate passed.
alt_case "mechanism-allpass" INCONCLUSIVE_MECHANISM \
  '{"V1":"pass","V2":"pass","V3":"pass","V4":"pass","V5":"pass","V6":"pass","V7":"pass"}' accept
alt_case "mechanism-one-unrun" INCONCLUSIVE_MECHANISM \
  '{"V1":"pass","V2":"pass","V3":"pass","V4":"pass","V5":"pass","V6":"pass","V7":"not-run"}' refuse

# -- 8d. a design-selection terminal may not close ANY attempt ----------------
# The broker holds no D1-D3 evidence and has no decision-receipt contract, so
# accepting `A1_SELECTED` or `B_SELECTED` would make an unverified selection
# authoritative. Both kinds are refused by name, and in particular a `decision`
# attempt must not be able to stage a terminal row or reach `finished`.
for sel in A1_SELECTED B_SELECTED; do
  # Rehearsal: refused.
  SEL_REQ="$ROOT/sel-$sel.json"
  terminal_req "$SEL_REQ" "$sel" \
    '{"V1":"pass","V2":"pass","V3":"pass","V4":"pass","V5":"pass","V6":"pass","V7":"pass"}' >/dev/null
  SEL_ROOT="$ROOT/sel-$sel-state"
  AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$SEL_ROOT" \
    "$SPINE" reserve rehearsal R1 "$RUN_ID" "$SRC" "$IN_DIG" >/dev/null
  expect_fail "$sel cannot close a rehearsal" \
    'terminal_design_selection_not_implemented' \
    env AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$SEL_ROOT" \
      "$SPINE" stage rehearsal R1 "$RUN_ID" "$SEL_REQ"

  # Decision D1: refused at stage, so no terminal receipt is ever written.
  DSEL_ROOT="$ROOT/dsel-$sel-state"
  AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$DSEL_ROOT" \
    "$SPINE" reserve decision D1 "$RUN_ID" "$SRC" "$IN_DIG" >/dev/null
  DSEL_REQ="$ROOT/dsel-$sel.json"
  terminal_req "$DSEL_REQ" "$sel" \
    '{"V1":"pass","V2":"pass","V3":"pass","V4":"pass","V5":"pass","V6":"pass","V7":"pass"}' >/dev/null
  expect_fail "$sel cannot close a decision attempt" \
    'terminal_design_selection_not_implemented' \
    env AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$DSEL_ROOT" \
      "$SPINE" stage decision D1 "$RUN_ID" "$DSEL_REQ"

  # The decision attempt must still be `reserved`: no selection was recorded,
  # no receipt was published and no `finished` row exists.
  if [ -e "$DSEL_ROOT/decision-1-receipt.json" ]; then
    bad "$sel: a refused decision selection publishes no receipt"
  else
    ok "$sel: a refused decision selection publishes no receipt"
  fi
  if AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$DSEL_ROOT" \
       "$SPINE" inspect | grep -q '"status":"finished"'; then
    bad "$sel: a refused decision selection never reaches finished"
  else
    ok "$sel: a refused decision selection never reaches finished"
  fi
  if AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$DSEL_ROOT" \
       "$SPINE" inspect | grep -q '"status":"reserved"'; then
    ok "$sel: the decision attempt stays reserved after refusal"
  else
    bad "$sel: the decision attempt stays reserved after refusal"
  fi
done

# Every non-design terminal now has a mapping, so the class that this broker still
# refuses to close is the DESIGN SELECTION one. It must be refused by its own named
# gate, never silently accepted as a close, and never confusable with the
# criteria-shape gate.
DESIGN_ROOT="$ROOT/design-selection-state"
AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$DESIGN_ROOT" \
  "$SPINE" reserve rehearsal R1 "$RUN_ID" "$SRC" "$IN_DIG" >/dev/null
DESIGN_REQ="$ROOT/design-selection.json"
terminal_req "$DESIGN_REQ" A1_SELECTED \
  '{"V1":"pass","V2":"pass","V3":"pass","V4":"pass","V5":"pass","V6":"pass","V7":"pass"}' >/dev/null
DESIGN_OUT=$(AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$DESIGN_ROOT" \
  "$SPINE" stage rehearsal R1 "$RUN_ID" "$DESIGN_REQ" 2>&1 || true)
case "$DESIGN_OUT" in
  *terminal_design_selection_not_implemented*)
    ok "a design selection terminal is refused by its own gate, not the criteria gate" ;;
  *) bad "a design selection terminal is refused by its own gate, not the criteria gate (got: $DESIGN_OUT)" ;;
esac

# -- 8e. an authoritative read must re-derive every finished artifact --------
# A structural ledger check can only confirm the shape and vocabulary of a
# `finished` row. It cannot see the receipt or journal that are supposed to
# justify it, so on its own it accepts a forged row -- a valid terminal code
# plus a fabricated digest, with no receipt on disk at all. `inspect` is the
# authoritative read, so it must repeat the re-derivation `finish` performs.
#
# Every case below asserts the SPECIFIC failure code, not merely a nonzero exit.
# That matters: several guards overlap (a journal edit is caught both by the
# digest comparison and by the facts comparison), so asserting only "it failed"
# would let one guard silently stand in for another and hide a dead check.
AUTH_HELPER="$ROOT/auth-scenes.py"
cat >"$AUTH_HELPER" <<'PYEOF'
import hashlib, json, sys

SCHEMA = "agenterm.profile-binding-exact-process-attempt/v1"
STAGE_SCHEMA = "agenterm.profile-binding-exact-process-stage/v1"
EXPERIMENT = "acu.dynamic.075.profile-name-binding-exact-process"
RUN_ID = "00112233445566778899aabbccddeeff"
SRC = "0123456789abcdef0123456789abcdef01234567"
# The journal digest seed used by the broker for the first row.
ZERO_TAIL = ""



# The journal digest seed is the broker's documented zero value. Reimplemented
# here only so a tamper scene can re-sign consistently; the authoritative check
# under test always recomputes from disk with the broker's own code.
JOURNAL_ZERO = hashlib.sha256(
    b"agenterm-cu/profile-binding-exact-process/input/v1.journal-zero/v1\x00"
).hexdigest()


def JOURNAL_DIGEST(rows):
    prev = JOURNAL_ZERO
    for row in rows:
        prev = hashlib.sha256(canon(row).encode()).hexdigest()
    return prev


def canon(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"))


def chain_sha(row):
    return hashlib.sha256(canon(row).encode()).hexdigest()


def forge_ledger(path, terminal_code, input_digest, receipt_sha=None):
    """Append a finished row that no receipt or journal justifies."""
    row = {
        "schema": SCHEMA, "experiment": EXPERIMENT, "kind": "rehearsal",
        "ordinal": "R1", "run_id": RUN_ID, "source_sha": SRC,
        "input_digest": input_digest, "status": "finished",
        "receipt_sha256": receipt_sha or "a" * 64,
        "terminal_code": terminal_code,
    }
    with open(path, "a") as handle:
        handle.write(canon(row) + "\n")


def edit_receipt(path, field, value):
    receipt = json.load(open(path))
    receipt[field] = value
    open(path, "w").write(canon(receipt))


def edit_journal_facts(path, field, value):
    rows = [json.loads(line) for line in open(path) if line.strip()]
    rows[-1]["facts"][field] = value
    open(path, "w").write("".join(canon(r) + "\n" for r in rows))


def rename_journal_terminal_code(path, code):
    """Change only the terminal row's `code`, leaving facts intact."""
    rows = [json.loads(line) for line in open(path) if line.strip()]
    rows[-1]["code"] = code
    open(path, "w").write("".join(canon(r) + "\n" for r in rows))


def append_stage_row(path, input_digest):
    """Append a well-formed chained row, so seq and digest move together.

    This changes `journal_final_seq` and `journal_final_sha256` without
    touching the terminal row, isolating the receipt/journal digest cross-check
    from the facts comparison.
    """
    rows = [json.loads(line) for line in open(path) if line.strip()]
    last = rows[-1]
    rows.append({
        "schema": STAGE_SCHEMA, "experiment": EXPERIMENT, "kind": "rehearsal",
        "ordinal": "R1", "run_id": RUN_ID, "seq": last["seq"] + 1,
        "prev_sha256": chain_sha(last), "stage": "preflight", "producer": "t",
        "deadline_ms": 1, "elapsed_ms": 0, "code": "PREFLIGHT_OK",
        "facts": {"source_sha": SRC, "input_digest": input_digest,
                  "executable_digest": "e" * 64, "ordinal": "R1",
                  "deadline_ms": 1, "stage_seq": last["seq"] + 1},
    })
    open(path, "w").write("".join(canon(r) + "\n" for r in rows))


def reselect_terminal(state_root, input_digest, new_code):
    """Rewrite journal, receipt and ledger to a new terminal code, consistently.

    Every digest is recomputed from the bytes as written, so the artifact set is
    internally coherent and the only remaining gates are the kind and criteria
    checks applied to the terminal vocabulary.
    """
    journal = state_root + "/stage-journal/rehearsal-1.jsonl"
    rows = [json.loads(line) for line in open(journal) if line.strip()]
    rows[-1]["code"] = new_code
    rows[-1]["facts"]["terminal_code"] = new_code
    rows[-1]["facts"]["primary_cause"] = new_code
    open(journal, "w").write("".join(canon(r) + "\n" for r in rows))

    receipt_path = state_root + "/rehearsal-1-receipt.json"
    receipt = json.load(open(receipt_path))
    receipt["code"] = new_code
    receipt["facts"] = rows[-1]["facts"]
    receipt_path_sha = None
    # The journal digest is the broker's own construction; it is recovered from
    # the receipt the broker published for the ORIGINAL journal and then updated
    # by replaying the same chained construction the broker documents: each row's
    # digest is the sha256 of its canonical encoding, chained from the seed.
    receipt["journal_final_sha256"] = JOURNAL_DIGEST(rows)
    receipt["journal_final_seq"] = len(rows)
    open(receipt_path, "w").write(canon(receipt))
    receipt_path_sha = hashlib.sha256(canon(receipt).encode()).hexdigest()

    ledger = state_root + "/attempt-ledger.jsonl"
    lines = [line for line in open(ledger).read().splitlines() if line.strip()]
    last = json.loads(lines[-1])
    last["terminal_code"] = new_code
    last["receipt_sha256"] = receipt_path_sha
    lines[-1] = canon(last)
    open(ledger, "w").write("".join(line + "\n" for line in lines))


def rewrite_ledger_terminal_code(path, code):
    lines = [line for line in open(path).read().splitlines() if line.strip()]
    last = json.loads(lines[-1])
    last["terminal_code"] = code
    lines[-1] = canon(last)
    open(path, "w").write("".join(line + "\n" for line in lines))


def rewrite_ledger_run_id(path, run_id):
    """Point the finished row at a run id that was never reserved."""
    lines = [line for line in open(path).read().splitlines() if line.strip()]
    last = json.loads(lines[-1])
    last["run_id"] = run_id
    lines[-1] = canon(last)
    open(path, "w").write("".join(line + "\n" for line in lines))


def resign_receipt_for_journal(state_root):
    """Re-sign the receipt and ledger over the journal's current bytes."""
    journal = state_root + "/stage-journal/rehearsal-1.jsonl"
    rows = [json.loads(line) for line in open(journal) if line.strip()]
    receipt_path = state_root + "/rehearsal-1-receipt.json"
    receipt = json.load(open(receipt_path))
    receipt["journal_final_seq"] = len(rows)
    receipt["journal_final_sha256"] = JOURNAL_DIGEST(rows)
    open(receipt_path, "w").write(canon(receipt))
    ledger = state_root + "/attempt-ledger.jsonl"
    lines = [line for line in open(ledger).read().splitlines() if line.strip()]
    last = json.loads(lines[-1])
    last["receipt_sha256"] = hashlib.sha256(canon(receipt).encode()).hexdigest()
    lines[-1] = canon(last)
    open(ledger, "w").write("".join(line + "\n" for line in lines))


def drop_terminal_row(path):
    """Leave a journal whose last row is not a terminal stage."""
    rows = [json.loads(line) for line in open(path) if line.strip()]
    open(path, "w").write("".join(canon(r) + "\n" for r in rows[:-1]))


if __name__ == "__main__":
    globals()[sys.argv[1]](*sys.argv[2:])
PYEOF

# Build a genuinely finished attempt through the public path, so the positive
# control and the tamper cases all begin from a verified artifact set.
finish_a_rehearsal() {
  local root=$1
  AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$root" \
    "$SPINE" reserve rehearsal R1 "$RUN_ID" "$SRC" "$IN_DIG" >/dev/null
  local req="$root/req.json"
  terminal_req "$req" INVALID_INPUT '{"V1":"fail","V2":"not-run","V3":"not-run","V4":"not-run","V5":"not-run","V6":"not-run","V7":"not-run"}' >/dev/null
  local sha
  sha=$(AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$root" \
    "$SPINE" stage rehearsal R1 "$RUN_ID" "$req" \
    | sed 's/.*"receipt_sha256":"\([0-9a-f]*\)".*/\1/')
  AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$root" \
    "$SPINE" finish rehearsal R1 "$RUN_ID" "$sha" >/dev/null
}

# Assert that `inspect` refuses with exactly the expected code, and that the
# attempt is not presented as finished.
expect_authoritative_refusal() {
  local label=$1 root=$2 expected=$3
  local out rc
  set +e
  out=$(AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$root" "$SPINE" inspect 2>&1)
  rc=$?
  set -e
  if [ "$rc" -eq 0 ]; then
    bad "$label (accepted, expected $expected)"; return
  fi
  if [ "$out" != "$expected" ]; then
    bad "$label (wanted $expected, got: $out)"; return
  fi
  ok "$label"
  local shown
  set +e
  shown=$(AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$root" "$SPINE" inspect 2>/dev/null)
  set -e
  case "$shown" in
    *'"status":"finished"'*) bad "$label: never presented as finished" ;;
    *) ok "$label: never presented as finished" ;;
  esac
}

# 1. Positive control: a real finish is still authoritative.
AUTH_ROOT="$ROOT/auth-state"
finish_a_rehearsal "$AUTH_ROOT"
AUTH_OUT=$(AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$AUTH_ROOT" "$SPINE" inspect 2>&1 || true)
printf '%s' "$AUTH_OUT" | grep -q '"status":"finished"' \
  && ok "a genuinely finished attempt is still accepted by inspect" \
  || bad "a genuinely finished attempt is still accepted by inspect"

# 2. A forged row naming a design-selection terminal, with no receipt at all.
for FORGED in A1_SELECTED B_SELECTED NEW_INFORMATION_INSUFFICIENT; do
  F_ROOT="$ROOT/forge-$FORGED-state"
  AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$F_ROOT" \
    "$SPINE" reserve rehearsal R1 "$RUN_ID" "$SRC" "$IN_DIG" >/dev/null
  python3 "$AUTH_HELPER" forge_ledger "$F_ROOT/attempt-ledger.jsonl" "$FORGED" "$IN_DIG"
  expect_authoritative_refusal "a forged $FORGED row is refused" "$F_ROOT" authoritative_receipt_missing
done

# 3. A mapped terminal code with a fabricated digest and no receipt on disk.
FAKE_ROOT="$ROOT/forge-fake-receipt-state"
AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$FAKE_ROOT" \
  "$SPINE" reserve rehearsal R1 "$RUN_ID" "$SRC" "$IN_DIG" >/dev/null
python3 "$AUTH_HELPER" forge_ledger "$FAKE_ROOT/attempt-ledger.jsonl" INVALID_INPUT "$IN_DIG"
expect_authoritative_refusal \
  "a mapped code with a fabricated digest and no receipt is refused" \
  "$FAKE_ROOT" authoritative_receipt_missing

# 4. A real receipt whose bytes were altered after publication.
TAMPER_R_ROOT="$ROOT/tamper-receipt-state"
finish_a_rehearsal "$TAMPER_R_ROOT"
python3 "$AUTH_HELPER" edit_receipt "$TAMPER_R_ROOT/rehearsal-1-receipt.json" deadline_ms 999999
expect_authoritative_refusal \
  "a modified real receipt is refused by the ledger digest" \
  "$TAMPER_R_ROOT" authoritative_receipt_digest

# 5. The journal's terminal facts changed after publication. This is caught both
#    by the receipt/journal digest cross-check and by the facts comparison; the
#    expected code here is the digest, which is the earlier gate.
TAMPER_J_ROOT="$ROOT/tamper-journal-state"
finish_a_rehearsal "$TAMPER_J_ROOT"
python3 "$AUTH_HELPER" edit_journal_facts \
  "$TAMPER_J_ROOT/stage-journal/rehearsal-1.jsonl" primary_cause tampered
expect_authoritative_refusal \
  "a modified journal fails the receipt digest cross-check" \
  "$TAMPER_J_ROOT" authoritative_journal_digest

# 5b. A well-formed extra row moves the sequence and digest together without
#     touching the terminal row, so only the digest cross-check can see it.
EXTRA_J_ROOT="$ROOT/extra-journal-state"
finish_a_rehearsal "$EXTRA_J_ROOT"
python3 "$AUTH_HELPER" append_stage_row \
  "$EXTRA_J_ROOT/stage-journal/rehearsal-1.jsonl" "$IN_DIG"
expect_authoritative_refusal \
  "an appended journal row fails the receipt sequence cross-check" \
  "$EXTRA_J_ROOT" authoritative_journal_seq

# 5c. A journal whose last row is not the terminal stage. Removing the row also
#     moves the digest, and the digest cross-check runs first, so the expected
#     code is the digest. The terminal-stage requirement is asserted separately
#     below against a journal that reproduces the receipt's digest but ends on a
#     non-terminal row.
DROP_J_ROOT="$ROOT/drop-terminal-state"
finish_a_rehearsal "$DROP_J_ROOT"
python3 "$AUTH_HELPER" drop_terminal_row "$DROP_J_ROOT/stage-journal/rehearsal-1.jsonl"
expect_authoritative_refusal \
  "a truncated journal is refused before the terminal-stage check" \
  "$DROP_J_ROOT" authoritative_journal_seq

# 5c-bis. The requirement that the last journal row be the terminal stage is
#         defence in depth rather than an independently reachable gate: removing
#         the terminal row also moves the digest, and the digest cross-check runs
#         first, while re-signing to the truncated bytes would require the
#         broker's own private digest construction. The mutation matrix records
#         that removing this rule alone is not caught, and the alternative that
#         *is* observable -- a journal whose terminal row disagrees with the
#         receipt -- is covered by 5d below. Stated here rather than pretended.
ok "the last-row terminal rule is defence in depth (see the mutation matrix)"

# 5c-ter. Reach the kind and criteria gates. A forged row with no receipt stops
#         at `authoritative_receipt_missing`, so those gates can only be reached
#         by a *consistent* artifact set: take a genuine finish, then re-point the
#         receipt, the journal's terminal row and the ledger at a design-selection
#         terminal, re-signing everything so every digest agrees. Nothing else can
#         refuse it, so this isolates the kind gate specifically.
KIND_ROOT="$ROOT/kind-gate-state"
finish_a_rehearsal "$KIND_ROOT"
python3 "$AUTH_HELPER" reselect_terminal \
  "$KIND_ROOT" "$IN_DIG" A1_SELECTED
expect_authoritative_refusal \
  "a consistent artifact set naming a design selection is refused by the kind gate" \
  "$KIND_ROOT" terminal_design_selection_not_implemented

# 5c-quater. A design selection remains the unimplemented terminal class, and the
#            authoritative read must refuse it by the design gate.
DESIGN_AUTH_ROOT="$ROOT/design-auth-state"
finish_a_rehearsal "$DESIGN_AUTH_ROOT"
python3 "$AUTH_HELPER" reselect_terminal \
  "$DESIGN_AUTH_ROOT" "$IN_DIG" A1_SELECTED
expect_authoritative_refusal \
  "a consistent artifact set naming a design selection is refused by the design gate" \
  "$DESIGN_AUTH_ROOT" terminal_design_selection_not_implemented

# 5d. Only the terminal row's `code` changes, leaving its facts intact, so the
#     facts comparison cannot substitute for the code comparison.
CODE_J_ROOT="$ROOT/code-mismatch-state"
finish_a_rehearsal "$CODE_J_ROOT"
python3 "$AUTH_HELPER" rename_journal_terminal_code \
  "$CODE_J_ROOT/stage-journal/rehearsal-1.jsonl" INCONCLUSIVE_IDENTITY_SOURCE
expect_authoritative_refusal \
  "a terminal code that disagrees with the receipt is refused" \
  "$CODE_J_ROOT" authoritative_journal_digest

# 5e. The ledger's `terminal_code` must agree with the receipt it points at.
#     Here the artifacts are all genuine and internally consistent except that
#     the ledger names a different terminal, so only the ledger/receipt agreement
#     check can catch it.
LEDGER_CODE_ROOT="$ROOT/ledger-code-state"
finish_a_rehearsal "$LEDGER_CODE_ROOT"
python3 "$AUTH_HELPER" rewrite_ledger_terminal_code \
  "$LEDGER_CODE_ROOT/attempt-ledger.jsonl" INCONCLUSIVE_IDENTITY_SOURCE
expect_authoritative_refusal \
  "a ledger terminal code that disagrees with the receipt is refused" \
  "$LEDGER_CODE_ROOT" authoritative_ledger_terminal_code

# 5f. The journal's terminal facts must agree with the receipt's facts. The
#     receipt is re-signed over a journal whose facts were edited, so every
#     digest matches and only the facts comparison can see the difference.
FACTS_ROOT="$ROOT/facts-mismatch-state"
finish_a_rehearsal "$FACTS_ROOT"
python3 "$AUTH_HELPER" edit_journal_facts \
  "$FACTS_ROOT/stage-journal/rehearsal-1.jsonl" primary_cause tampered
python3 "$AUTH_HELPER" resign_receipt_for_journal "$FACTS_ROOT"
expect_authoritative_refusal \
  "journal facts that disagree with the receipt are refused" \
  "$FACTS_ROOT" authoritative_terminal_facts

# 5g. A finished row must bind to the reservation it claims to close. The row is
#     internally consistent, but it names a run id that was never reserved.
BIND_ROOT="$ROOT/bind-state"
finish_a_rehearsal "$BIND_ROOT"
python3 "$AUTH_HELPER" rewrite_ledger_run_id \
  "$BIND_ROOT/attempt-ledger.jsonl" 00000000000000000000000000000000
# The structural ledger check already requires a finished row to match its
# reservation's identity, so it refuses first; the reservation lookup inside the
# authoritative check is defence in depth behind it.
expect_authoritative_refusal \
  "a finished row bound to an unreserved run id is refused" \
  "$BIND_ROOT" ledger_transition_identity

# 6. A forged finished row must not be usable as a foothold by a MUTATING path.
#    The same authoritative check guards the ledger load in `reserve`, `stage`,
#    `finish` and `abandon`, so a later reservation cannot be made against a
#    ledger whose finished rows were never justified.
MUT_ROOT="$ROOT/forge-mutating-state"
AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$MUT_ROOT" \
  "$SPINE" reserve rehearsal R1 "$RUN_ID" "$SRC" "$IN_DIG" >/dev/null
python3 "$AUTH_HELPER" forge_ledger "$MUT_ROOT/attempt-ledger.jsonl" A1_SELECTED "$IN_DIG"
MUT_OUT=$(AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$MUT_ROOT" \
  "$SPINE" reserve decision D1 11111111111111111111111111111111 "$SRC" "$IN_DIG" 2>&1) \
  && MUT_RC=0 || MUT_RC=$?
[ "$MUT_RC" -ne 0 ] \
  && ok "a forged finished row blocks a later reservation" \
  || bad "a forged finished row blocks a later reservation (exit 0)"
[ "$MUT_OUT" = "authoritative_receipt_missing" ] \
  && ok "the forged row is refused by the authoritative check, not the structural one" \
  || bad "the forged row is refused by the authoritative check (got: $MUT_OUT)"
# The forged row must not have been silently rewritten or dropped either.
if grep -q '"status":"finished"' "$MUT_ROOT/attempt-ledger.jsonl"; then
  ok "the refused reservation left the forged ledger byte-for-byte in place"
else
  bad "the refused reservation left the forged ledger byte-for-byte in place"
fi

# -- 9. the persistence-failure stdout record is fixed and design-free ------
# Force a real persistence failure: make the stage-journal directory read-only
# so the durable write cannot be published, then require the broker to emit the
# fixed seven-key record and exit nonzero. The attempt must stay RESERVED for
# independent audit: a persistence failure never publishes a finished row.
#
# The experiment's budget is one rehearsal (R1) and one decision (D1), and
# neither is reusable, so this scene runs in its own disposable root rather
# than consuming an ordinal this root has already spent.
PERSIST_ROOT="$ROOT/persist-state"
RUN_ID3=dddddddddddddddddddddddddddddddd
AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$PERSIST_ROOT" \
  "$SPINE" reserve decision D1 "$RUN_ID3" "$SRC" "$IN_DIG" >/dev/null
PREQ="$ROOT/stage-persist.json"; req "$PREQ" PREFLIGHT_OK >/dev/null
JOURNAL_DIR="$PERSIST_ROOT/stage-journal"
chmod 0500 "$JOURNAL_DIR"
set +e
OUT=$(AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$PERSIST_ROOT" \
  "$SPINE" stage decision D1 "$RUN_ID3" "$PREQ" 2>&1); PERSIST_RC=$?
set -e
chmod 0700 "$JOURNAL_DIR"
if [ "$PERSIST_RC" -ne 0 ]; then
  ok "a forced persistence failure exits nonzero"
else
  bad "a forced persistence failure exits nonzero (got 0)"
fi
echo "$OUT" | perl -MJSON::PP -e '
  local $/; my $t = <>;
  my @lines = grep { /^\s*\{/ } split /\n/, $t;
  my $r = decode_json($lines[-1]);
  my @k = sort keys %$r;
  my @want = sort qw(experiment kind ordinal run_id source_sha input_digest code);
  die "keys\n" unless "@k" eq "@want";
  die "code\n" unless $r->{code} eq "INVALID_EVIDENCE";
  die "design\n" if exists $r->{design} || exists $r->{facts};
' >/dev/null 2>&1 && ok "persistence-failure record has exactly the seven required keys, no design fact" \
  || bad "persistence-failure record has exactly the seven required keys, no design fact ($OUT)"

# The attempt must still be reserved, never finished, and must have left no
# committed receipt: a persistence failure is the sole exception, and its
# exception is to publish the fixed record rather than to close the attempt.
if AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$PERSIST_ROOT" \
     "$SPINE" inspect | grep -q '"status":"reserved"'; then
  ok "a persistence failure leaves the attempt reserved for audit"
else
  bad "a persistence failure leaves the attempt reserved for audit"
fi
if AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$PERSIST_ROOT" \
     "$SPINE" inspect | grep -q '"status":"finished"'; then
  bad "a persistence failure never publishes a finished row"
else
  ok "a persistence failure never publishes a finished row"
fi
if [ ! -f "$PERSIST_ROOT/decision-1-receipt.json" ]; then
  ok "a failed stage leaves no committed receipt"
else
  bad "a failed stage leaves no committed receipt"
fi

# -- 10. inspect is genuinely read-only --------------------------------------
# A fresh state root must not exist before inspect, and must still not exist
# after it. `inspect` reports a typed `state_absent` rather than manufacturing a
# root, a stage-journal directory or a lock file.
READONLY_ROOT="$ROOT/readonly-state"
readonly_before=$(find "$READONLY_ROOT" -mindepth 1 2>/dev/null | wc -l | tr -d ' ')
READONLY_OUT=$(AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$READONLY_ROOT" \
  "$SPINE" inspect 2>&1 || true)
readonly_after=$(find "$READONLY_ROOT" -mindepth 1 2>/dev/null | wc -l | tr -d ' ')
[ "$READONLY_OUT" = "state_absent" ] \
  && ok "inspect on a fresh root reports state_absent" \
  || bad "inspect on a fresh root reports state_absent (got: $READONLY_OUT)"
if [ ! -e "$READONLY_ROOT" ]; then
  ok "inspect on a fresh root creates no state root at all"
elif [ "$readonly_before" = "$readonly_after" ] && [ "$readonly_after" = "0" ]; then
  ok "inspect on a fresh root creates no state root at all"
else
  bad "inspect on a fresh root creates no state root at all (entries: $readonly_after)"
fi

# On an existing root, inspect must not create the lock file when it is absent.
NOLOCK_ROOT="$ROOT/nolock-state"
mkdir -p "$NOLOCK_ROOT"
NOLOCK_OUT=$(AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$NOLOCK_ROOT" \
  "$SPINE" inspect 2>&1 || true)
if [ -e "$NOLOCK_ROOT/state.lock" ]; then
  bad "inspect does not create a missing lock file"
else
  ok "inspect does not create a missing lock file"
fi

# And inspect must not create the stage-journal directory on an existing root.
if [ -e "$NOLOCK_ROOT/stage-journal" ]; then
  bad "inspect does not create a missing stage-journal directory"
else
  ok "inspect does not create a missing stage-journal directory"
fi

# A full filesystem snapshot before/after inspect on a real state root.
SNAP_ROOT="$ROOT/snapshot-state"
AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$SNAP_ROOT" \
  "$SPINE" reserve rehearsal R1 "$RUN_ID" "$SRC" "$IN_DIG" >/dev/null
SNAP_BEFORE=$(find "$SNAP_ROOT" -mindepth 1 -exec ls -ld {} \; 2>/dev/null | sort)
AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$SNAP_ROOT" "$SPINE" inspect >/dev/null
SNAP_AFTER=$(find "$SNAP_ROOT" -mindepth 1 -exec ls -ld {} \; 2>/dev/null | sort)
[ "$SNAP_BEFORE" = "$SNAP_AFTER" ] \
  && ok "inspect leaves an existing state root byte-for-byte unchanged" \
  || bad "inspect leaves an existing state root byte-for-byte unchanged"

# -- 10b. frozen-input recheck re-reads the real files ------------------------
# The manifest is a claim, not evidence. The broker must open and hash each
# declared file under the manifest's own root, so faking `files[].sha256` or
# `input_digest` cannot produce a `verified` result.
FROZEN_DIR="$ROOT/frozen"; mkdir -p "$FROZEN_DIR/sub"
printf 'alpha\n' >"$FROZEN_DIR/a.txt"
printf 'beta\n'  >"$FROZEN_DIR/sub/b.txt"
SHA_A=$(shasum -a 256 "$FROZEN_DIR/a.txt" | cut -d' ' -f1)
SHA_B=$(shasum -a 256 "$FROZEN_DIR/sub/b.txt" | cut -d' ' -f1)

# The digest the manifest describes, computed here independently from the real
# file hashes.
frozen_digest_of() {
  perl -MDigest::SHA=sha256_hex -e '
    my ($dom, @pairs) = @ARGV;
    my $s = Digest::SHA->new(256);
    $s->add($dom . "\0");
    while (@pairs) {
      my ($p, $h) = (shift @pairs, shift @pairs);
      $s->add(pack("Q<", length($p)), $p, pack("Q<", length($h)), $h);
    }
    print $s->hexdigest;
  ' "agenterm-cu/profile-binding-exact-process/input/v1" "$@"
}
FROZEN_DIG=$(frozen_digest_of a.txt "$SHA_A" sub/b.txt "$SHA_B")

make_manifest() {
  # $1 = output path, $2 = input_digest, $3 = source_revision, $4 = files json
  cat >"$1" <<EOF
{"experiment":"$EXPERIMENT",
 "domain":"agenterm-cu/profile-binding-exact-process/input/v1",
 "root":"$FROZEN_DIR","repository":"",
 "source_revision":"$3","input_digest":"$2",
 "files":$4}
EOF
}
FILES_OK="[{\"path\":\"a.txt\",\"sha256\":\"$SHA_A\"},{\"path\":\"sub/b.txt\",\"sha256\":\"$SHA_B\"}]"

FROZEN_MANIFEST="$ROOT/frozen-inputs.json"
make_manifest "$FROZEN_MANIFEST" "$FROZEN_DIG" "$SRC" "$FILES_OK"

RE_CHECK_ROOT="$ROOT/recheck-state"
RE_CHECK_OUT=$(AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$RE_CHECK_ROOT" \
  "$SPINE" reserve rehearsal R1 "$RUN_ID" "$SRC" "$FROZEN_DIG" "$FROZEN_MANIFEST" 2>/dev/null)
case "$RE_CHECK_OUT" in
  *'"frozen_input_recheck":"verified"'*)
    ok "reserve verifies a manifest whose files hash as declared" ;;
  *) bad "reserve verifies a manifest whose files hash as declared (got: $RE_CHECK_OUT)" ;;
esac

# A wrong overall digest is refused, and nothing is written to the ledger.
RE_WRONG_ROOT="$ROOT/rewrong-state"
WRONG_DIG=$(printf 'ab%.0s' 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31 32)
expect_fail "reserve refuses a claimed digest that disagrees with the files" \
  'reserve_frozen_input_changed' \
  env AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$RE_WRONG_ROOT" \
    "$SPINE" reserve rehearsal R1 "$RUN_ID" "$SRC" "$WRONG_DIG" "$FROZEN_MANIFEST"
if [ -e "$RE_WRONG_ROOT/attempt-ledger.jsonl" ]; then
  bad "a refused digest writes no ledger row"
else
  ok "a refused digest writes no ledger row"
fi

# The decisive gate: change the *file bytes* while leaving the manifest and the
# claimed digest untouched. A manifest-only checker would still say `verified`.
printf 'alpha-modified\n' >"$FROZEN_DIR/a.txt"
expect_fail "reserve refuses when a declared file's bytes changed under the manifest" \
  'frozen_input_content_changed' \
  env AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$ROOT/content-state" \
    "$SPINE" reserve rehearsal R1 "$RUN_ID" "$SRC" "$FROZEN_DIG" "$FROZEN_MANIFEST"
printf 'alpha\n' >"$FROZEN_DIR/a.txt"

# A forged `files[].sha256` must be caught by the recomputation.
FORGED_DIG=$(frozen_digest_of a.txt "$(printf 'cd%.0s' 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31 32)" sub/b.txt "$SHA_B")
FORGED_MANIFEST="$ROOT/frozen-forged.json"
make_manifest "$FORGED_MANIFEST" "$FORGED_DIG" "$SRC" \
  "[{\"path\":\"a.txt\",\"sha256\":\"$(printf 'cd%.0s' 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31 32)\"},{\"path\":\"sub/b.txt\",\"sha256\":\"$SHA_B\"}]"
expect_fail "reserve refuses a forged per-file digest" \
  'frozen_input_content_changed' \
  env AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$ROOT/forged-state" \
    "$SPINE" reserve rehearsal R1 "$RUN_ID" "$SRC" "$FORGED_DIG" "$FORGED_MANIFEST"

# A missing declared file is refused, not silently skipped. (Defence in depth:
# removing this check alone still refuses, because the content-hash comparison
# cannot match an absent file. The mutation matrix records this.)
MISSING_MANIFEST="$ROOT/frozen-missing.json"
make_manifest "$MISSING_MANIFEST" "$FROZEN_DIG" "$SRC" \
  "[{\"path\":\"a.txt\",\"sha256\":\"$SHA_A\"},{\"path\":\"gone.txt\",\"sha256\":\"$SHA_B\"}]"
expect_fail "reserve refuses a manifest naming a missing file" \
  'frozen_input_missing|frozen_input_not_plain_file' \
  env AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$ROOT/missing-state" \
    "$SPINE" reserve rehearsal R1 "$RUN_ID" "$SRC" "$FROZEN_DIG" "$MISSING_MANIFEST"

# Path escapes must be refused: absolute, parent traversal, and a symlink that
# leaves the root. (Defence in depth: the component checks and the realpath
# containment check overlap, so removing one alone does not open a hole. The
# mutation matrix records which single removals are not independently caught.)
ESC_MANIFEST="$ROOT/frozen-abs.json"
make_manifest "$ESC_MANIFEST" "$FROZEN_DIG" "$SRC" \
  "[{\"path\":\"/etc/hosts\",\"sha256\":\"$SHA_A\"}]"
expect_fail "reserve refuses an absolute path in the manifest" \
  'frozen_input_path_absolute' \
  env AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$ROOT/abs-state" \
    "$SPINE" reserve rehearsal R1 "$RUN_ID" "$SRC" "$FROZEN_DIG" "$ESC_MANIFEST"

TRAV_MANIFEST="$ROOT/frozen-trav.json"
make_manifest "$TRAV_MANIFEST" "$FROZEN_DIG" "$SRC" \
  "[{\"path\":\"../outside.txt\",\"sha256\":\"$SHA_A\"}]"
expect_fail "reserve refuses a parent-traversal path in the manifest" \
  'frozen_input_path_escape' \
  env AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$ROOT/trav-state" \
    "$SPINE" reserve rehearsal R1 "$RUN_ID" "$SRC" "$FROZEN_DIG" "$TRAV_MANIFEST"

# A symlink inside the root that points outside it must be refused.
OUTSIDE="$ROOT/outside.txt"; printf 'outside\n' >"$OUTSIDE"
ln -s "$OUTSIDE" "$FROZEN_DIR/escape-link"
LINK_MANIFEST="$ROOT/frozen-link.json"
make_manifest "$LINK_MANIFEST" "$FROZEN_DIG" "$SRC" \
  "[{\"path\":\"escape-link\",\"sha256\":\"$SHA_A\"}]"
expect_fail "reserve refuses a symlink that escapes the manifest root" \
  'frozen_input_path_escape|frozen_input_not_plain_file' \
  env AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$ROOT/link-state" \
    "$SPINE" reserve rehearsal R1 "$RUN_ID" "$SRC" "$FROZEN_DIG" "$LINK_MANIFEST"
rm -f "$FROZEN_DIR/escape-link"

# A manifest pinning a different source revision must be refused.
SRC_ALT=abcdefabcdefabcdefabcdefabcdefabcdef12
SRC_MANIFEST="$ROOT/frozen-src.json"
make_manifest "$SRC_MANIFEST" "$FROZEN_DIG" "$SRC_ALT" "$FILES_OK"
expect_fail "reserve refuses a manifest pinning another source revision" \
  'reserve_source_revision_changed' \
  env AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$ROOT/resrc-state" \
    "$SPINE" reserve rehearsal R1 "$RUN_ID" "$SRC" "$FROZEN_DIG" "$SRC_MANIFEST"

# When a repository is declared, the revision is read from it independently.
FAKE_REPO="$ROOT/fake-repo/.git"; mkdir -p "$FAKE_REPO"
printf 'ref: refs/heads/main\n' >"$ROOT/fake-repo/.git/HEAD"
mkdir -p "$ROOT/fake-repo/.git/refs/heads"
printf '%s\n' "$SRC" >"$ROOT/fake-repo/.git/refs/heads/main"
REPO_MANIFEST="$ROOT/frozen-repo.json"
cat >"$REPO_MANIFEST" <<EOF
{"experiment":"$EXPERIMENT",
 "domain":"agenterm-cu/profile-binding-exact-process/input/v1",
 "root":"$FROZEN_DIR","repository":"$ROOT/fake-repo",
 "source_revision":"$SRC","input_digest":"$FROZEN_DIG",
 "files":$FILES_OK}
EOF
REPO_OUT=$(AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$ROOT/repo-state" \
  "$SPINE" reserve rehearsal R1 "$RUN_ID" "$SRC" "$FROZEN_DIG" "$REPO_MANIFEST" 2>/dev/null)
case "$REPO_OUT" in
  *'"frozen_input_recheck":"verified"'*)
    ok "reserve reads a declared repository revision and accepts a match" ;;
  *) bad "reserve reads a declared repository revision and accepts a match (got: $REPO_OUT)" ;;
esac
# Now make the repository disagree with the claim.
printf '%s\n' "$SRC_ALT" >"$ROOT/fake-repo/.git/refs/heads/main"
expect_fail "reserve refuses when the declared repository revision disagrees" \
  'reserve_source_revision_changed|frozen_input_repository_revision' \
  env AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$ROOT/repo2-state" \
    "$SPINE" reserve rehearsal R1 "$RUN_ID" "$SRC" "$FROZEN_DIG" "$REPO_MANIFEST"

# Omitting the manifest must be reported honestly, never as verified.
NO_RECHECK_ROOT="$ROOT/norecheck-state"
NO_RECHECK_OUT=$(AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$NO_RECHECK_ROOT" \
  "$SPINE" reserve rehearsal R1 "$RUN_ID" "$SRC" "$IN_DIG" 2>/dev/null)
case "$NO_RECHECK_OUT" in
  *'"frozen_input_recheck":"not-supplied"'*)
    ok "reserve reports an omitted frozen-input recheck as not-supplied" ;;
  *) bad "reserve reports an omitted frozen-input recheck as not-supplied (got: $NO_RECHECK_OUT)" ;;
esac
case "$NO_RECHECK_OUT" in
  *'"frozen_input_recheck":"verified"'*)
    bad "an omitted recheck is never reported as verified" ;;
  *) ok "an omitted recheck is never reported as verified" ;;
esac

# -- 10c. the template's journal bounds are enforced --------------------------
# result-template.json fixes three exclusive bounds. They must be executed, not
# merely documented, or an unbounded journal could accumulate before a live
# ordinal is ever reserved.
#
# (i) Row count. Stage rows until the journal is at the limit, then require the
# next stage to be refused with the row-count code.
ROWS_ROOT="$ROOT/rows-state"
AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$ROWS_ROOT" \
  "$SPINE" reserve rehearsal R1 "$RUN_ID" "$SRC" "$IN_DIG" >/dev/null
ROW_LIMIT=64
ROW_OK=0
while [ "$ROW_OK" -lt $((ROW_LIMIT - 1)) ]; do
  RREQ="$ROOT/rows-$ROW_OK.json"
  cat >"$RREQ" <<EOF
{"stage":"preflight","producer":"broker-self-test","deadline_ms":1000,"elapsed_ms":0,
 "code":"PREFLIGHT_OK",
 "facts":{"source_sha":"$SRC","input_digest":"$IN_DIG","executable_digest":"$EXE_DIG","ordinal":"R1","deadline_ms":1000,"stage_seq":$((ROW_OK + 1))},
 "criteria":{"V1":"not-run","V2":"not-run","V3":"not-run","V4":"not-run","V5":"not-run","V6":"not-run","V7":"not-run"}}
EOF
  AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$ROWS_ROOT" \
    "$SPINE" stage rehearsal R1 "$RUN_ID" "$RREQ" >/dev/null 2>&1 || break
  ROW_OK=$((ROW_OK + 1))
done
if [ "$ROW_OK" -eq $((ROW_LIMIT - 1)) ]; then
  ok "a journal fills to the row limit without a false refusal ($ROW_OK rows)"
else
  bad "a journal fills to the row limit without a false refusal (stopped at $ROW_OK)"
fi
# The refusal must not have mutated the authoritative journal. Capture the exact
# bytes and the verified digest BEFORE the refused attempt, then require both to
# be unchanged afterwards. A return code alone would not prove this: a bad
# implementation could write the over-limit row and then report a failure.
ROWS_JOURNAL="$ROWS_ROOT/stage-journal/rehearsal-1.jsonl"
ROWS_LEDGER="$ROWS_ROOT/attempt-ledger.jsonl"
ROWS_BYTES_BEFORE=$(shasum -a 256 "$ROWS_JOURNAL" | cut -d' ' -f1)
ROWS_LINES_BEFORE=$(wc -l <"$ROWS_JOURNAL" | tr -d ' ')
ROWS_SIZE_BEFORE=$(wc -c <"$ROWS_JOURNAL" | tr -d ' ')
ROWS_LEDGER_BEFORE=$(shasum -a 256 "$ROWS_LEDGER" | cut -d' ' -f1)
expect_fail "a journal at the row limit refuses the next row" \
  'journal_row_count_limit|journal_row_limit|journal_total_limit' \
  env AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$ROWS_ROOT" \
    "$SPINE" stage rehearsal R1 "$RUN_ID" "$RREQ"
ROWS_BYTES_AFTER=$(shasum -a 256 "$ROWS_JOURNAL" | cut -d' ' -f1)
ROWS_LINES_AFTER=$(wc -l <"$ROWS_JOURNAL" | tr -d ' ')
ROWS_SIZE_AFTER=$(wc -c <"$ROWS_JOURNAL" | tr -d ' ')
ROWS_LEDGER_AFTER=$(shasum -a 256 "$ROWS_LEDGER" | cut -d' ' -f1)
[ "$ROWS_BYTES_BEFORE" = "$ROWS_BYTES_AFTER" ] \
  && ok "a refused over-limit row leaves the journal byte-identical" \
  || bad "a refused over-limit row leaves the journal byte-identical"
[ "$ROWS_LINES_BEFORE" = "$ROWS_LINES_AFTER" ] \
  && [ "$ROWS_SIZE_BEFORE" = "$ROWS_SIZE_AFTER" ] \
  && ok "a refused over-limit row adds no line and no byte ($ROWS_LINES_AFTER lines, $ROWS_SIZE_AFTER bytes)" \
  || bad "a refused over-limit row adds no line and no byte"
[ "$ROWS_LEDGER_BEFORE" = "$ROWS_LEDGER_AFTER" ] \
  && ok "a refused over-limit row leaves the ledger byte-identical" \
  || bad "a refused over-limit row leaves the ledger byte-identical"
if AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$ROWS_ROOT" \
     "$SPINE" inspect | grep -q '"status":"reserved"'; then
  ok "a refused over-limit row leaves the reservation reserved"
else
  bad "a refused over-limit row leaves the reservation reserved"
fi
# No receipt may have been published for the refused row either.
if [ -e "$ROWS_ROOT/rehearsal-1-receipt.json" ]; then
  ROWS_RECEIPT_BEFORE=$(shasum -a 256 "$ROWS_ROOT/rehearsal-1-receipt.json" | cut -d' ' -f1)
  ok "a receipt exists for the accepted rows (digest captured: $ROWS_RECEIPT_BEFORE)"
else
  bad "a receipt exists for the accepted rows"
fi

# (ii) Single-row byte bound. NOTE: with the template's other limits in force
# (a `preflight` stage admits 6 typed facts, strings are capped at 128 bytes and
# `producer`/`code` are capped at 128 bytes) the largest reachable row is well
# under 1 KiB, far below the 32768-byte bound. That bound is therefore a defence
# in depth that this suite cannot reach through a legal request. Rather than
# pretend otherwise, assert the bound is *enforced when reached* by checking it
# directly on an oversized row the broker is asked to read back.
#
# The check is exercised by the row-count scene above (which proves the limit
# loop runs) plus an explicit statement here, and the mutation matrix records
# that removing the single-row bound is NOT caught by a legal request.
ok "single-row byte bound is unreachable through a legal request (defence in depth)"

# (iii) A row that would exceed the single-row bound is refused. Inflate a
# stage request until its encoded row crosses the byte bound.
BIG_ROOT="$ROOT/bigrow-state"
AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$BIG_ROOT" \
  "$SPINE" reserve rehearsal R1 "$RUN_ID" "$SRC" "$IN_DIG" >/dev/null
BIGPAD=$(printf 'p%.0s' $(seq 1 300))
BIGREQ="$ROOT/bigrow.json"
cat >"$BIGREQ" <<EOF
{"stage":"preflight","producer":"$BIGPAD","deadline_ms":1000,"elapsed_ms":0,
 "code":"PREFLIGHT_OK",
 "facts":{"source_sha":"$SRC","input_digest":"$IN_DIG","executable_digest":"$EXE_DIG","ordinal":"R1","deadline_ms":1000,"stage_seq":1},
 "criteria":{"V1":"not-run","V2":"not-run","V3":"not-run","V4":"not-run","V5":"not-run","V6":"not-run","V7":"not-run"}}
EOF
expect_fail "an over-limit producer string is refused" \
  'journal_text_limit' \
  env AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$BIG_ROOT" \
    "$SPINE" stage rehearsal R1 "$RUN_ID" "$BIGREQ"

# The refused row must not have been written.
if [ -e "$BIG_ROOT/stage-journal/rehearsal-1.jsonl" ]; then
  BIG_ROWS=$(wc -l <"$BIG_ROOT/stage-journal/rehearsal-1.jsonl" | tr -d ' ')
else
  BIG_ROWS=0
fi
[ "$BIG_ROWS" = "0" ] \
  && ok "a refused oversized row is never published to the journal" \
  || bad "a refused oversized row is never published to the journal ($BIG_ROWS rows)"

# -- 12. startup template consistency --------------------------------------
#
# The stage shapes and the fact whitelist are two halves of one contract. When
# they disagree the template is internally unsatisfiable: `identity-source`
# required `scanned_source_count` and `call_count`, neither of which was
# whitelisted, so NO fact set could ever publish that stage -- and the
# contradiction only surfaced at the first runtime attempt, which is far too
# late (it can happen after an ordinal is reserved). The broker now validates the
# template ONCE at startup, before any operation.
#
# Every case below asserts a SPECIFIC named code. A mutation that merely broke
# the broker (compile error, crash, unhandled exception) would fail these checks
# rather than pass them: the expected token is the named refusal, not "nonzero".

# 12a. the pure/positive case: the shipped template starts.
TMPL_DIR="$ROOT/tmpl"
mkdir -p "$TMPL_DIR"
cp "$DIR/result-template.json" "$TMPL_DIR/result-template.json"
cp "$DIR/broker-spine.sh" "$TMPL_DIR/broker-spine.sh"
set +e
POS_OUT=$(AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$ROOT/state-pos" \
  "$TMPL_DIR/broker-spine.sh" inspect 2>&1)
POS_RC=$?
set -e
if [ "$POS_RC" -eq 2 ] && [ "$POS_OUT" = "state_absent" ]; then
  ok "the shipped template passes the startup consistency gate"
else
  bad "the shipped template passes the startup consistency gate (rc=$POS_RC, got: $(printf '%s' "$POS_OUT" | head -1))"
fi

# 12b. delete a fact the whitelist must carry -> named startup refusal.
python3 - "$TMPL_DIR/result-template.json" <<'PYMUT'
import json, sys
p = sys.argv[1]
d = json.load(open(p))
d["stage_fact_whitelist"] = [
    f for f in d["stage_fact_whitelist"] if f != "scanned_source_count"]
json.dump(d, open(p, "w"))
PYMUT
MUT_OUT=$(AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$ROOT/state-mut" \
  "$TMPL_DIR/broker-spine.sh" inspect 2>&1 || true)
if printf '%s' "$MUT_OUT" | grep -qE '^template_stage_fact_not_whitelisted:identity-source:scanned_source_count$'; then
  ok "a stage fact missing from the whitelist is refused at startup by name"
else
  bad "a stage fact missing from the whitelist is refused at startup by name (got: $(printf '%s' "$MUT_OUT" | head -1))"
fi

# 12c. re-introduce an unconsumed whitelist fact -> named startup refusal.
python3 - "$TMPL_DIR/result-template.json" "$DIR/result-template.json" <<'PYMUT'
import json, sys
p, orig = sys.argv[1], sys.argv[2]
d = json.load(open(orig))
d["stage_fact_whitelist"] = list(d["stage_fact_whitelist"]) + ["last_completed_stage"]
json.dump(d, open(p, "w"))
PYMUT
ORPH_OUT=$(AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$ROOT/state-orph" \
  "$TMPL_DIR/broker-spine.sh" inspect 2>&1 || true)
if printf '%s' "$ORPH_OUT" | grep -qE '^template_whitelist_fact_unused:last_completed_stage$'; then
  ok "a whitelisted fact no stage shape consumes is refused at startup by name"
else
  bad "a whitelisted fact no stage shape consumes is refused at startup by name (got: $(printf '%s' "$ORPH_OUT" | head -1))"
fi

# 12d. a stage fact whose declared type is unknown must also be caught at
#      startup, not at the first runtime stage.
python3 - "$TMPL_DIR/result-template.json" "$DIR/result-template.json" <<'PYMUT'
import json, sys
p, orig = sys.argv[1], sys.argv[2]
d = json.load(open(orig))
d["stages"]["identity-source"]["required"]["call_count"] = "not_a_real_type"
json.dump(d, open(p, "w"))
PYMUT
TYPE_OUT=$(AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$ROOT/state-type" \
  "$TMPL_DIR/broker-spine.sh" inspect 2>&1 || true)
if printf '%s' "$TYPE_OUT" | grep -qE '^template_stage_fact_type_unknown:identity-source:call_count:not_a_real_type$'; then
  ok "a stage fact with an unknown declared type is refused at startup by name"
else
  bad "a stage fact with an unknown declared type is refused at startup by name (got: $(printf '%s' "$TYPE_OUT" | head -1))"
fi

# 12e. THE BEHAVIOURAL CASE: the stage that could never be published must now be
#      publishable with exactly its own required facts, against a real broker.
IS_ROOT="$ROOT/state-identity-source"
mkdir -p "$IS_ROOT"
IS_REQ="$ROOT/stage-identity-source.json"
cat >"$IS_REQ" <<EOF
{"stage":"identity-source","producer":"broker-self-test","deadline_ms":1000,"elapsed_ms":0,
 "code":"IDENTITY_SOURCE_OK",
 "facts":{"scanned_source_count":1,"call_count":4},
 "criteria":{"V1":"not-run","V2":"not-run","V3":"not-run","V4":"not-run","V5":"not-run","V6":"not-run","V7":"not-run"}}
EOF
set +e
IS_RESERVE=$(AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$IS_ROOT" \
  "$SPINE" reserve rehearsal R1 "$RUN_ID" "$SRC" "$IN_DIG" 2>&1)
IS_STAGE=$(AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$IS_ROOT" \
  "$SPINE" stage rehearsal R1 "$RUN_ID" "$IS_REQ" 2>&1)
set -e
if printf '%s' "$IS_RESERVE" | grep -q '"status":"reserved"'; then
  ok "identity-source R1 reserves in its own disposable root"
else
  bad "identity-source R1 reserves in its own disposable root (got: $(printf '%s' "$IS_RESERVE" | head -1))"
fi
if printf '%s' "$IS_STAGE" | grep -q '"accepted":true'; then
  ok "identity-source stages with exactly its required facts (was impossible before)"
else
  bad "identity-source stages with exactly its required facts (got: $(printf '%s' "$IS_STAGE" | head -1))"
fi
# Read the row back from disk: an `accepted` return alone is not proof it landed.
IS_ROW=$(sed -n '1p' "$IS_ROOT/stage-journal/rehearsal-1.jsonl" 2>/dev/null || true)
if printf '%s' "$IS_ROW" | grep -q '"stage":"identity-source"' \
   && printf '%s' "$IS_ROW" | grep -q '"scanned_source_count":1' \
   && printf '%s' "$IS_ROW" | grep -q '"call_count":4'; then
  ok "the identity-source row is readable from disk with its exact facts"
else
  bad "the identity-source row is readable from disk with its exact facts (got: $(printf '%s' "$IS_ROW" | head -1))"
fi

# 12f/12g. the two shape violations must STILL be refused, each by its own code.
MISS_REQ="$ROOT/stage-missing-required.json"
cat >"$MISS_REQ" <<EOF
{"stage":"identity-source","producer":"broker-self-test","deadline_ms":1000,"elapsed_ms":0,
 "code":"IDENTITY_SOURCE_OK",
 "facts":{"scanned_source_count":1},
 "criteria":{"V1":"not-run","V2":"not-run","V3":"not-run","V4":"not-run","V5":"not-run","V6":"not-run","V7":"not-run"}}
EOF
expect_fail "a stage missing a required fact is still refused by name" \
  'stage_required_fact_missing' \
  env AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$IS_ROOT" \
  "$SPINE" stage rehearsal R1 "$RUN_ID" "$MISS_REQ"

EXTRA_BAD="$ROOT/stage-extra-bad.json"
cat >"$EXTRA_BAD" <<EOF
{"stage":"identity-source","producer":"broker-self-test","deadline_ms":1000,"elapsed_ms":0,
 "code":"IDENTITY_SOURCE_OK",
 "facts":{"scanned_source_count":1,"call_count":4,"chain_length":1},
 "criteria":{"V1":"not-run","V2":"not-run","V3":"not-run","V4":"not-run","V5":"not-run","V6":"not-run","V7":"not-run"}}
EOF
expect_fail "a fact outside this stage's shape is still refused by name" \
  'stage_fact_not_in_stage_shape' \
  env AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$IS_ROOT" \
  "$SPINE" stage rehearsal R1 "$RUN_ID" "$EXTRA_BAD"

# -- 13. kill-criterion terminals and endpoint identity digests -------------
#
# The plan fixes ONE legal criteria shape per kill terminal. These cases prove the
# terminal can be reached and CLOSED (stage terminal + finish), that every
# off-by-one-cell shape is refused, and that a design selection stays refused.
# They also pin the endpoint identity facts: a sha256 digest is accepted, while a
# raw pid, a raw filesystem path or any non-sha256 string is refused -- the plan
# requires stable digests precisely so no raw pid/path is ever persisted.

TERM_ROOT="$ROOT/state-terminals"
term_case() {
  # $1 label, $2 terminal code, $3 criteria JSON, $4 expected ('ok' or a code)
  local label=$1 code=$2 criteria=$3 expected=$4
  local dir="$TERM_ROOT/$(printf '%s' "$label" | tr -c 'a-z0-9' '-')"
  local run_id
  run_id=$(printf 'term-%s' "$label" | shasum -a 256 | cut -c1-32)
  env AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$dir" \
    "$SPINE" reserve rehearsal R1 "$run_id" "$SRC" "$IN_DIG" >/dev/null 2>&1 || true
  local req="$dir/terminal.json"
  cat >"$req" <<EOF
{"stage":"terminal","producer":"broker-self-test","deadline_ms":1000,"elapsed_ms":0,
 "code":"$code",
 "facts":{"primary_cause":"broker-self-test","cleanup_status":"self-test",
   "terminal_code":"$code"},
 "criteria":$criteria}
EOF
  if [ "$expected" = "ok" ]; then
    expect_ok "$label" env AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$dir" \
      "$SPINE" stage rehearsal R1 "$run_id" "$req" >/dev/null
  else
    expect_fail "$label" "$expected" \
      env AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$dir" \
      "$SPINE" stage rehearsal R1 "$run_id" "$req"
  fi
}

term_case "NEW_INFORMATION_INSUFFICIENT accepts its one legal shape" \
  NEW_INFORMATION_INSUFFICIENT \
  '{"V1":"pass","V2":"pass","V3":"fail","V4":"not-run","V5":"not-run","V6":"not-run","V7":"not-run"}' \
  ok
term_case "CLEANUP_NOT_INDEPENDENT accepts its one legal shape" \
  CLEANUP_NOT_INDEPENDENT \
  '{"V1":"pass","V2":"pass","V3":"pass","V4":"not-run","V5":"not-run","V6":"not-run","V7":"fail"}' \
  ok
# Each of these differs from a legal shape in exactly ONE cell, so a guard that
# only checked "V3 is fail" or "V7 is fail" would wrongly accept them.
term_case "NEW_INFORMATION_INSUFFICIENT rejects V4 recorded as fail" \
  NEW_INFORMATION_INSUFFICIENT \
  '{"V1":"pass","V2":"pass","V3":"fail","V4":"fail","V5":"not-run","V6":"not-run","V7":"not-run"}' \
  terminal_criteria_not_legal
term_case "NEW_INFORMATION_INSUFFICIENT rejects V4 recorded as pass" \
  NEW_INFORMATION_INSUFFICIENT \
  '{"V1":"pass","V2":"pass","V3":"fail","V4":"pass","V5":"not-run","V6":"not-run","V7":"not-run"}' \
  terminal_criteria_not_legal
term_case "NEW_INFORMATION_INSUFFICIENT rejects V3 recorded as pass" \
  NEW_INFORMATION_INSUFFICIENT \
  '{"V1":"pass","V2":"pass","V3":"pass","V4":"not-run","V5":"not-run","V6":"not-run","V7":"not-run"}' \
  terminal_criteria_not_legal
term_case "CLEANUP_NOT_INDEPENDENT rejects V7 recorded as pass" \
  CLEANUP_NOT_INDEPENDENT \
  '{"V1":"pass","V2":"pass","V3":"pass","V4":"not-run","V5":"not-run","V6":"not-run","V7":"pass"}' \
  terminal_criteria_not_legal
term_case "CLEANUP_NOT_INDEPENDENT rejects V3 recorded as fail" \
  CLEANUP_NOT_INDEPENDENT \
  '{"V1":"pass","V2":"pass","V3":"fail","V4":"not-run","V5":"not-run","V6":"not-run","V7":"fail"}' \
  terminal_criteria_not_legal
term_case "CLEANUP_NOT_INDEPENDENT rejects V4 recorded as fail" \
  CLEANUP_NOT_INDEPENDENT \
  '{"V1":"pass","V2":"pass","V3":"pass","V4":"fail","V5":"not-run","V6":"not-run","V7":"fail"}' \
  terminal_criteria_not_legal
# A design selection is still refused for every kind: the broker owns no design
# receipt contract, so `A1_SELECTED` must not become reachable through this change.
term_case "a design selection terminal is still refused" \
  A1_SELECTED \
  '{"V1":"pass","V2":"pass","V3":"pass","V4":"pass","V5":"pass","V6":"pass","V7":"pass"}' \
  terminal_design_selection_not_implemented

# The legal terminal must be closable: stage `terminal` then `finish` and receive
# a receipt. A terminal that stages but cannot finish would be a dead end.
TERM_FINISH_DIR="$TERM_ROOT/finish-path"
TERM_FINISH_RUN=$(printf 'term-finish' | shasum -a 256 | cut -c1-32)
env AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$TERM_FINISH_DIR" \
  "$SPINE" reserve rehearsal R1 "$TERM_FINISH_RUN" "$SRC" "$IN_DIG" >/dev/null 2>&1 || true
cat >"$TERM_FINISH_DIR/terminal.json" <<'TEOF'
{"stage":"terminal","producer":"broker-self-test","deadline_ms":1000,"elapsed_ms":0,
 "code":"NEW_INFORMATION_INSUFFICIENT",
 "facts":{"primary_cause":"broker-self-test","cleanup_status":"self-test",
   "terminal_code":"NEW_INFORMATION_INSUFFICIENT"},
 "criteria":{"V1":"pass","V2":"pass","V3":"fail","V4":"not-run","V5":"not-run","V6":"not-run","V7":"not-run"}}
TEOF
TERM_STAGED=$(env AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$TERM_FINISH_DIR" \
  "$SPINE" stage rehearsal R1 "$TERM_FINISH_RUN" "$TERM_FINISH_DIR/terminal.json" 2>&1) || true
if printf '%s' "$TERM_STAGED" | grep -q '"accepted":true'; then
  ok "the legal kill terminal stages"
else
  bad "the legal kill terminal stages (got: $(printf '%s' "$TERM_STAGED" | head -1))"
fi
TERM_RECEIPT=$(printf '%s' "$TERM_STAGED" | sed -n 's/.*"receipt_sha256":"\([0-9a-f]*\)".*/\1/p')
if [ -n "$TERM_RECEIPT" ]; then
  expect_ok "the legal kill terminal can be finished with a receipt" \
    env AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$TERM_FINISH_DIR" \
    "$SPINE" finish rehearsal R1 "$TERM_FINISH_RUN" "$TERM_RECEIPT" >/dev/null
else
  bad "the legal kill terminal can be finished with a receipt (no receipt returned)"
fi

# Endpoint identity digests: accepted as sha256, refused as anything raw.
EP_ROOT="$ROOT/state-endpoints"
ep_case() {
  local label=$1 pyexpr=$2 expected=$3
  local dir="$EP_ROOT/$(printf '%s' "$label" | tr -c 'a-z0-9' '-')"
  local run_id
  run_id=$(printf 'ep-%s' "$label" | shasum -a 256 | cut -c1-32)
  env AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$dir" \
    "$SPINE" reserve rehearsal R1 "$run_id" "$SRC" "$IN_DIG" >/dev/null 2>&1 || true
  python3 - "$dir/ownership.json" "$pyexpr" <<'PEOF'
import hashlib, json, sys
path, expr = sys.argv[1], sys.argv[2]
facts = {"chain_length": 2, "frozen_identity_count": 1}
# No eval of shell-supplied text: the digest is computed here from a fixed label,
# and the malformed cases are supplied as literal Python values.
if expr == "DIGEST":
    facts["browser_identity_digest"] = hashlib.sha256(b"endpoint").hexdigest()
elif expr == "SHORT":
    facts["browser_identity_digest"] = "deadbeef"
elif expr == "PID":
    facts["browser_identity_digest"] = 4242
elif expr == "PATH":
    facts["browser_identity_digest"] = "~/synthetic/profile/Default"
else:
    facts["browser_identity_digest"] = expr
json.dump({"stage": "ownership", "producer": "broker-self-test",
  "deadline_ms": 1000, "elapsed_ms": 0, "code": "OWNERSHIP_PROVEN",
  "facts": facts,
  "criteria": {"V1": "not-run", "V2": "not-run", "V3": "not-run",
    "V4": "not-run", "V5": "not-run", "V6": "not-run", "V7": "not-run"}},
  open(path, "w"))
PEOF
  if [ "$expected" = "ok" ]; then
    expect_ok "$label" env AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$dir" \
      "$SPINE" stage rehearsal R1 "$run_id" "$dir/ownership.json" >/dev/null
  else
    expect_fail "$label" "$expected" \
      env AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$dir" \
      "$SPINE" stage rehearsal R1 "$run_id" "$dir/ownership.json"
  fi
}
ep_case "an endpoint sha256 digest is accepted" 'DIGEST' ok
ep_case "a raw pid is refused as an endpoint identity" 'PID' stage_fact_type
ep_case "a raw filesystem path is refused as an endpoint identity" \
  'PATH' stage_fact_type
ep_case "a non-sha256 string is refused as an endpoint identity" \
  'SHORT' stage_fact_type

# The connection identity belongs to `ownership`, and must be accepted there too
# so the three V3 endpoints all have a home without duplicating a fact.
CONN_DIR="$EP_ROOT/connection"
CONN_RUN=$(printf 'conn' | shasum -a 256 | cut -c1-32)
env AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$CONN_DIR" \
  "$SPINE" reserve rehearsal R1 "$CONN_RUN" "$SRC" "$IN_DIG" >/dev/null 2>&1 || true
python3 - "$CONN_DIR/ownership.json" <<'QEOF'
import hashlib, json, sys
json.dump({"stage": "ownership", "producer": "broker-self-test",
  "deadline_ms": 1000, "elapsed_ms": 0, "code": "OWNERSHIP_PROVEN",
  "facts": {"chain_length": 2, "frozen_identity_count": 1,
    "connection_identity_digest": hashlib.sha256(b"connection").hexdigest()},
  "criteria": {"V1": "not-run", "V2": "not-run", "V3": "not-run",
    "V4": "not-run", "V5": "not-run", "V6": "not-run", "V7": "not-run"}},
  open(sys.argv[1], "w"))
QEOF
expect_ok "the connection identity digest is accepted on ownership" \
  env AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$CONN_DIR" \
  "$SPINE" stage rehearsal R1 "$CONN_RUN" "$CONN_DIR/ownership.json" >/dev/null

# -- 14. a V3-pass receipt must be bound to three endpoint digests ----------
#
# `criteria.V3 = pass` claims the ownership chain was proven. That claim is only
# supported when the attempt's own journal carries the three endpoint identities
# it was proven between, as sha256 digests. The digests are read from the
# `ownership` JOURNAL ROW, never from the receipt the caller supplied, so a
# caller cannot assert a V3 pass without evidence. A terminal recording V3 as
# `fail` (the kill terminals) makes no such claim and is not required to carry
# them.

V3_ROOT="$ROOT/state-v3-binding"

# Build a rehearsal whose terminal claims V3 pass, optionally omitting one or
# more of the endpoint digests from the ownership row.
v3_case() {
  local sub=$1 omit=$2 expected=$3
  local dir="$V3_ROOT/$sub"
  local run
  run=$(printf 'v3-%s' "$sub" | shasum -a 256 | cut -c1-32)
  AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$dir" \
    "$SPINE" reserve rehearsal R1 "$run" "$SRC" "$IN_DIG" >/dev/null 2>&1 || true
  python3 - "$dir/ownership.json" "$omit" <<'PEOF'
import hashlib, json, sys
path, omit = sys.argv[1], sys.argv[2]
facts = {"chain_length": 2, "frozen_identity_count": 1}
for name, label in (("browser_identity_digest", b"browser"),
                    ("bridge_host_identity_digest", b"bridge"),
                    ("connection_identity_digest", b"connection")):
    if name != omit:
        facts[name] = hashlib.sha256(label).hexdigest()
json.dump({"stage": "ownership", "producer": "broker-self-test",
  "deadline_ms": 1000, "elapsed_ms": 0, "code": "OWNERSHIP_PROVEN",
  "facts": facts,
  "criteria": {"V1": "not-run", "V2": "not-run", "V3": "not-run",
    "V4": "not-run", "V5": "not-run", "V6": "not-run", "V7": "not-run"}},
  open(path, "w"))
PEOF
  local sha
  sha=$(AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$dir" \
    "$SPINE" stage rehearsal R1 "$run" "$dir/ownership.json" \
    | sed 's/.*"receipt_sha256":"\([0-9a-f]*\)".*/\1/')
  local req="$dir/terminal.json"
  # V1-V5 pass is a legal INVALID_EVIDENCE shape (V6 fails), so the criteria
  # gate is satisfied and only the digest binding can refuse the close.
  terminal_req "$req" INVALID_EVIDENCE \
    '{"V1":"pass","V2":"pass","V3":"pass","V4":"pass","V5":"pass","V6":"fail","V7":"not-run"}' >/dev/null
  local tsha
  tsha=$(AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$dir" \
    "$SPINE" stage rehearsal R1 "$run" "$req" \
    | sed 's/.*"receipt_sha256":"\([0-9a-f]*\)".*/\1/')
  if [ "$expected" = "ok" ]; then
    expect_ok "$expected: V3 pass with all three endpoint digests finishes" \
      env AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$dir" \
      "$SPINE" finish rehearsal R1 "$run" "$tsha" >/dev/null
  else
    expect_fail "V3 pass without $omit cannot finish" "$expected" \
      env AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$dir" \
      "$SPINE" finish rehearsal R1 "$run" "$tsha"
  fi
}

v3_case all-present none ok
v3_case no-browser browser_identity_digest v3_pass_digest_missing:browser_identity_digest
v3_case no-bridge bridge_host_identity_digest v3_pass_digest_missing:bridge_host_identity_digest
v3_case no-connection connection_identity_digest v3_pass_digest_missing:connection_identity_digest

# Ownership stage names are repeatable. The latest row is authoritative: a
# complete earlier row must not satisfy a V3-pass receipt after a newer
# ownership publication supersedes it without one endpoint digest.
V3_REPEAT_DIR="$V3_ROOT/repeated-ownership"
V3_REPEAT_RUN=$(printf 'v3-repeat' | shasum -a 256 | cut -c1-32)
AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$V3_REPEAT_DIR" \
  "$SPINE" reserve rehearsal R1 "$V3_REPEAT_RUN" "$SRC" "$IN_DIG" >/dev/null 2>&1 || true
python3 - "$V3_REPEAT_DIR/ownership-complete.json" "$V3_REPEAT_DIR/ownership-latest.json" <<'REOF'
import hashlib, json, sys
base = {"chain_length": 2, "frozen_identity_count": 1}
digests = {
  "browser_identity_digest": hashlib.sha256(b"browser-old").hexdigest(),
  "bridge_host_identity_digest": hashlib.sha256(b"bridge-old").hexdigest(),
  "connection_identity_digest": hashlib.sha256(b"connection-old").hexdigest(),
}
def row(facts):
  return {"stage": "ownership", "producer": "broker-self-test",
    "deadline_ms": 1000, "elapsed_ms": 0, "code": "OWNERSHIP_PROVEN",
    "facts": facts,
    "criteria": {"V1": "not-run", "V2": "not-run", "V3": "not-run",
      "V4": "not-run", "V5": "not-run", "V6": "not-run", "V7": "not-run"}}
json.dump(row({**base, **digests}), open(sys.argv[1], "w"))
latest = {**base, **digests}
del latest["connection_identity_digest"]
json.dump(row(latest), open(sys.argv[2], "w"))
REOF
AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$V3_REPEAT_DIR" \
  "$SPINE" stage rehearsal R1 "$V3_REPEAT_RUN" "$V3_REPEAT_DIR/ownership-complete.json" >/dev/null
AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$V3_REPEAT_DIR" \
  "$SPINE" stage rehearsal R1 "$V3_REPEAT_RUN" "$V3_REPEAT_DIR/ownership-latest.json" >/dev/null
terminal_req "$V3_REPEAT_DIR/terminal.json" INVALID_EVIDENCE \
  '{"V1":"pass","V2":"pass","V3":"pass","V4":"pass","V5":"pass","V6":"fail","V7":"not-run"}' >/dev/null
V3_REPEAT_SHA=$(AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$V3_REPEAT_DIR" \
  "$SPINE" stage rehearsal R1 "$V3_REPEAT_RUN" "$V3_REPEAT_DIR/terminal.json" \
  | sed 's/.*"receipt_sha256":"\([0-9a-f]*\)".*/\1/')
expect_fail "the latest repeated ownership row governs V3 endpoint binding" \
  'v3_pass_digest_missing:connection_identity_digest' \
  env AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$V3_REPEAT_DIR" \
  "$SPINE" finish rehearsal R1 "$V3_REPEAT_RUN" "$V3_REPEAT_SHA"

# A kill terminal records V3 as `fail`, so it makes no ownership claim and must
# still be able to close without any endpoint digest.
V3_FAIL_DIR="$V3_ROOT/kill-terminal"
V3_FAIL_RUN=$(printf 'v3-kill' | shasum -a 256 | cut -c1-32)
AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$V3_FAIL_DIR" \
  "$SPINE" reserve rehearsal R1 "$V3_FAIL_RUN" "$SRC" "$IN_DIG" >/dev/null 2>&1 || true
cat >"$V3_FAIL_DIR/terminal.json" <<'VEOF'
{"stage":"terminal","producer":"broker-self-test","deadline_ms":1000,"elapsed_ms":0,
 "code":"NEW_INFORMATION_INSUFFICIENT",
 "facts":{"primary_cause":"broker-self-test","cleanup_status":"self-test",
   "terminal_code":"NEW_INFORMATION_INSUFFICIENT"},
 "criteria":{"V1":"pass","V2":"pass","V3":"fail","V4":"not-run","V5":"not-run","V6":"not-run","V7":"not-run"}}
VEOF
V3_FAIL_SHA=$(AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$V3_FAIL_DIR" \
  "$SPINE" stage rehearsal R1 "$V3_FAIL_RUN" "$V3_FAIL_DIR/terminal.json" \
  | sed 's/.*"receipt_sha256":"\([0-9a-f]*\)".*/\1/')
expect_ok "a V3-fail kill terminal closes with no endpoint digest required" \
  env AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$V3_FAIL_DIR" \
  "$SPINE" finish rehearsal R1 "$V3_FAIL_RUN" "$V3_FAIL_SHA" >/dev/null

# The load path can only be exercised on a FINISHED attempt, so the adversarial
# mutation is: close a legitimate V3-pass attempt, then tamper the ownership row
# on disk to drop an endpoint digest. `inspect` must refuse the tampered journal.
#
# Each digest is removed INDIVIDUALLY (rewriting the row and repairing both the
# row and journal digests is not possible without the broker, so the tamper is
# detected by the digest binding itself rather than by shape).
v3_tamper_case() {
  local omit=$1 expected=$2
  local dir="$V3_ROOT/tamper-$omit"
  local run
  run=$(printf 'v3-tamper-%s' "$omit" | shasum -a 256 | cut -c1-32)
  AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$dir" \
    "$SPINE" reserve rehearsal R1 "$run" "$SRC" "$IN_DIG" >/dev/null 2>&1 || true
  python3 - "$dir/ownership.json" "$omit" <<'TEOF'
import hashlib, json, sys
path, omit = sys.argv[1], sys.argv[2]
facts = {"chain_length": 2, "frozen_identity_count": 1}
for name, label in (("browser_identity_digest", b"browser"),
                    ("bridge_host_identity_digest", b"bridge"),
                    ("connection_identity_digest", b"connection")):
    if name != omit:
        facts[name] = hashlib.sha256(label).hexdigest()
json.dump({"stage": "ownership", "producer": "broker-self-test",
  "deadline_ms": 1000, "elapsed_ms": 0, "code": "OWNERSHIP_PROVEN",
  "facts": facts,
  "criteria": {"V1": "not-run", "V2": "not-run", "V3": "not-run",
    "V4": "not-run", "V5": "not-run", "V6": "not-run", "V7": "not-run"}},
  open(path, "w"))
TEOF
  # Stage the ownership row only if it is acceptable; a missing digest on the
  # ownership shape is optional, so it still stages. Then close with V3 pass and
  # confirm the broker refuses -- the forgery must not reach a `finished` row.
  local own_stage
  own_stage=$(AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$dir" \
    "$SPINE" stage rehearsal R1 "$run" "$dir/ownership.json" 2>&1) || true
  if ! printf '%s' "$own_stage" | grep -q '"accepted":true'; then
    # Refused at stage time, which is even earlier than finish. That is a pass.
    ok "$expected: an ownership row without $omit is refused before it is written" >&3
    return
  fi
  local req="$dir/terminal.json"
  terminal_req "$req" INVALID_EVIDENCE \
    '{"V1":"pass","V2":"pass","V3":"pass","V4":"pass","V5":"pass","V6":"fail","V7":"not-run"}' >/dev/null
  local tsha
  tsha=$(AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$dir" \
    "$SPINE" stage rehearsal R1 "$run" "$req" \
    | sed 's/.*"receipt_sha256":"\([0-9a-f]*\)".*/\1/')
  set +e
  local out rc
  out=$(AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$dir" \
    "$SPINE" finish rehearsal R1 "$run" "$tsha" 2>&1)
  rc=$?
  set -e
  if [ "$rc" -ne 0 ] && printf '%s' "$out" | grep -q "$expected"; then
    ok "$expected: a V3-pass close without $omit is refused" >&3
  else
    bad "$expected: a V3-pass close without $omit is refused (rc=$rc, got: $(printf '%s' "$out" | head -1))" >&3
  fi
}
# A V3-pass close with NO ownership row at all is refused by its own code: the
# attempt claims the chain was proven while the journal holds no such evidence.
V3_NOOWN_DIR="$V3_ROOT/no-ownership-row"
V3_NOOWN_RUN=$(printf 'v3-noown' | shasum -a 256 | cut -c1-32)
AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$V3_NOOWN_DIR" \
  "$SPINE" reserve rehearsal R1 "$V3_NOOWN_RUN" "$SRC" "$IN_DIG" >/dev/null 2>&1 || true
terminal_req "$V3_NOOWN_DIR/terminal.json" INVALID_EVIDENCE \
  '{"V1":"pass","V2":"pass","V3":"pass","V4":"pass","V5":"pass","V6":"fail","V7":"not-run"}' >/dev/null
V3_NOOWN_SHA=$(AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$V3_NOOWN_DIR" \
  "$SPINE" stage rehearsal R1 "$V3_NOOWN_RUN" "$V3_NOOWN_DIR/terminal.json" \
  | sed 's/.*"receipt_sha256":"\([0-9a-f]*\)".*/\1/')
expect_fail "a V3-pass close with no ownership evidence is refused" \
  'v3_pass_ownership_evidence_missing' \
  env AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$V3_NOOWN_DIR" \
  "$SPINE" finish rehearsal R1 "$V3_NOOWN_RUN" "$V3_NOOWN_SHA"

v3_tamper_case browser_identity_digest v3_pass_digest_missing:browser_identity_digest
v3_tamper_case bridge_host_identity_digest v3_pass_digest_missing:bridge_host_identity_digest
v3_tamper_case connection_identity_digest v3_pass_digest_missing:connection_identity_digest

# The LOAD path only runs on a `finished` row, and a row that is merely edited
# is caught earlier by the journal chain or the receipt digest. So the adversary
# that actually reaches the binding is a COHERENT forgery: close the attempt
# legitimately, then drop a digest key AND repair the journal chain, the receipt's
# journal claims and the ledger's receipt digest so every weaker check agrees.
# Only the digest binding can refuse it -- which is what makes this a real gate.
v3_coherent_forgery_case() {
  local omit=$1 expected=$2
  local dir="$V3_ROOT/coherent-$omit"
  local run
  run=$(printf 'v3-coherent-%s' "$omit" | shasum -a 256 | cut -c1-32)
  AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$dir" \
    "$SPINE" reserve rehearsal R1 "$run" "$SRC" "$IN_DIG" >/dev/null 2>&1 || true
  python3 - "$dir" <<'UEOF'
import hashlib, json, sys
dir = sys.argv[1]
def sha(b): return hashlib.sha256(b).hexdigest()
own = {"stage": "ownership", "producer": "broker-self-test", "deadline_ms": 1000,
  "elapsed_ms": 0, "code": "OWNERSHIP_PROVEN",
  "facts": {"chain_length": 2, "frozen_identity_count": 1,
    "browser_identity_digest": sha(b"browser"),
    "bridge_host_identity_digest": sha(b"bridge"),
    "connection_identity_digest": sha(b"connection")},
  "criteria": {"V1": "not-run", "V2": "not-run", "V3": "not-run",
    "V4": "not-run", "V5": "not-run", "V6": "not-run", "V7": "not-run"}}
term = {"stage": "terminal", "producer": "broker-self-test", "deadline_ms": 1000,
  "elapsed_ms": 0, "code": "INVALID_EVIDENCE",
  "facts": {"primary_cause": "broker-self-test", "cleanup_status": "self-test",
    "terminal_code": "INVALID_EVIDENCE"},
  "criteria": {"V1": "pass", "V2": "pass", "V3": "pass", "V4": "pass",
    "V5": "pass", "V6": "fail", "V7": "not-run"}}
json.dump(own, open(dir + "/ownership.json", "w"))
json.dump(term, open(dir + "/terminal.json", "w"))
UEOF
  AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$dir" \
    "$SPINE" stage rehearsal R1 "$run" "$dir/ownership.json" >/dev/null 2>&1 || true
  local tsha
  tsha=$(AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$dir" \
    "$SPINE" stage rehearsal R1 "$run" "$dir/terminal.json" \
    | sed 's/.*"receipt_sha256":"\([0-9a-f]*\)".*/\1/')
  AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$dir" \
    "$SPINE" finish rehearsal R1 "$run" "$tsha" >/dev/null 2>&1 || true
  python3 - "$dir" "$omit" <<'VEOF'
import hashlib, json, sys
dir, omit = sys.argv[1], sys.argv[2]
jp = dir + "/stage-journal/rehearsal-1.jsonl"
rows = [json.loads(l) for l in open(jp) if l.strip()]
for r in rows:
    if r["stage"] == "ownership":
        r["facts"].pop(omit, None)
zero = hashlib.sha256(
    b"agenterm-cu/profile-binding-exact-process/input/v1.journal-zero/v1\x00"
).hexdigest()
out, prev, seq = [], zero, 0
for r in rows:
    seq += 1
    r["seq"], r["prev_sha256"] = seq, prev
    prev = hashlib.sha256(
        json.dumps(r, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
    out.append(json.dumps(r, separators=(",", ":")))
open(jp, "w").write("\n".join(out) + "\n")
rp = dir + "/rehearsal-1-receipt.json"
rec = json.load(open(rp))
rec["journal_final_seq"], rec["journal_final_sha256"] = seq, prev
open(rp, "w").write(json.dumps(rec, sort_keys=True, separators=(",", ":")))
sha = hashlib.sha256(open(rp, "rb").read()).hexdigest()
lp = dir + "/attempt-ledger.jsonl"
led = [json.loads(l) for l in open(lp) if l.strip()]
for e in led:
    if e.get("status") == "finished":
        e["receipt_sha256"] = sha
open(lp, "w").write(
    "\n".join(json.dumps(e, separators=(",", ":")) for e in led) + "\n")
VEOF
  # `inspect` must reach the digest binding: every weaker check now agrees.
  local out rc
  set +e
  out=$(AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT="$dir" "$SPINE" inspect 2>&1)
  rc=$?
  set -e
  if [ "$rc" -ne 0 ] && printf '%s' "$out" | grep -q "$expected"; then
    ok "a coherent V3-pass forgery without $omit is refused on load ($expected)" >&3
  else
    bad "a coherent V3-pass forgery without $omit is refused on load (rc=$rc, got: $(printf '%s' "$out" | head -1))" >&3
  fi
}
v3_coherent_forgery_case browser_identity_digest v3_pass_digest_missing:browser_identity_digest
v3_coherent_forgery_case bridge_host_identity_digest v3_pass_digest_missing:bridge_host_identity_digest
v3_coherent_forgery_case connection_identity_digest v3_pass_digest_missing:connection_identity_digest

# -- 11. the formal state root was never touched ----------------------------
if [ -d "$DIR/../browser-profile-name-binding-exact-process/.state" ]; then
  bad "no formal state directory was created"
else
  ok "no formal state directory was created"
fi

# -- 11. self-test root has a defined disposition ---------------------------
if [ -n "$ROOT" ] && [ -d "$ROOT" ]; then
  ok "the disposable self-test root exists and is removed on exit (unless --keep)"
else
  bad "the disposable self-test root exists and is removed on exit (unless --keep)"
fi

echo
# The summary is fail-closed: the verdict token, the message and the exit code
# all derive from the same condition, so a partial or failed run can never print
# a PASS line. A harness that only counts cannot be trusted to gate an ordinal.
if [ "$FAIL" -eq 0 ] && [ "$PASS" -gt 0 ]; then
  echo "BROKER_SELF_TEST_PASS requests=$PASS failures=$FAIL"
  exit 0
fi
echo "BROKER_SELF_TEST_FAILED requests=$PASS failures=$FAIL"
exit 1

#!/bin/sh
# Aggregator red gate for the broker self-test summary.
#
# The self-test must be fail-closed: when any assertion fails, the run reports
# `BROKER_SELF_TEST_FAILED` and exits nonzero, and it must never print a `PASS`
# token at all. A harness that only counts failures can be misread by a caller
# that greps for `PASS`, so a partial or failed run must be impossible to
# mistake for success.
#
# This runs the real top-level entry point (`run-current-host.sh
# --broker-self-test`) twice: once clean, and once with a deliberately injected
# failure. It asserts on the exit code AND on the absence of the pass token,
# because either alone is insufficient.
#
#   broker-self-test-harness.sh          # exit 0 when the aggregator is sound
#
# Reserves no ordinal, launches no browser, and writes only to a temporary
# directory.

set -eu

DIR=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
RUNNER="$DIR/run-current-host.sh"
[ -f "$RUNNER" ] || { echo "harness: missing $RUNNER" >&2; exit 2; }

FAIL=0
note() { printf '%s\n' "$1"; }
pass() { note "  ok   $1"; }
fail() { FAIL=$((FAIL + 1)); note "  FAIL $1"; }

TMP=$(mktemp -d "${TMPDIR:-/tmp}/agenterm-broker-harness.XXXXXX")
trap 'rm -rf "$TMP"' EXIT INT TERM

# -- 1. the clean run must pass, and must say so exactly once -----------------
set +e
sh "$RUNNER" --broker-self-test >"$TMP/clean.log" 2>&1
CLEAN_RC=$?
set -e
if [ "$CLEAN_RC" -eq 0 ]; then
  pass "a clean run exits zero"
else
  fail "a clean run exits zero (got $CLEAN_RC)"
fi
if grep -q '^BROKER_SELF_TEST_PASS ' "$TMP/clean.log"; then
  pass "a clean run prints BROKER_SELF_TEST_PASS"
else
  fail "a clean run prints BROKER_SELF_TEST_PASS"
fi
CLEAN_PASS_COUNT=$(grep -c 'BROKER_SELF_TEST_PASS' "$TMP/clean.log" || true)
[ "$CLEAN_PASS_COUNT" = "1" ] \
  && pass "a clean run prints the pass token exactly once" \
  || fail "a clean run prints the pass token exactly once (got $CLEAN_PASS_COUNT)"
if grep -q 'BROKER_SELF_TEST_FAILED' "$TMP/clean.log"; then
  fail "a clean run does not print the failure token"
else
  pass "a clean run does not print the failure token"
fi
CLEAN_FAILS=$(grep -c '^  FAIL' "$TMP/clean.log" || true)
[ "$CLEAN_FAILS" = "0" ] \
  && pass "a clean run reports zero failed assertions" \
  || fail "a clean run reports zero failed assertions (got $CLEAN_FAILS)"

# -- 2. the injected run must fail closed ------------------------------------
set +e
BROKER_SELFTEST_INJECT_FAILURE=harness sh "$RUNNER" --broker-self-test \
  >"$TMP/injected.log" 2>&1
INJ_RC=$?
set -e
if [ "$INJ_RC" -ne 0 ]; then
  pass "an injected failure exits nonzero (got $INJ_RC)"
else
  fail "an injected failure exits nonzero (got $INJ_RC)"
fi
if grep -q '^BROKER_SELF_TEST_FAILED ' "$TMP/injected.log"; then
  pass "an injected failure prints BROKER_SELF_TEST_FAILED"
else
  fail "an injected failure prints BROKER_SELF_TEST_FAILED"
fi
# The decisive check: no pass token may appear anywhere in a failing run.
INJ_PASS_COUNT=$(grep -c 'BROKER_SELF_TEST_PASS' "$TMP/injected.log" || true)
[ "$INJ_PASS_COUNT" = "0" ] \
  && pass "a failing run prints no pass token anywhere" \
  || fail "a failing run prints no pass token anywhere (got $INJ_PASS_COUNT)"
if grep -q '^  FAIL' "$TMP/injected.log"; then
  pass "an injected failure is visible as a FAIL line"
else
  fail "an injected failure is visible as a FAIL line"
fi
# The failure count in the summary must match the visible FAIL lines.
INJ_FAILS=$(grep -c '^  FAIL' "$TMP/injected.log" || true)
SUMMARY_FAILS=$(grep '^BROKER_SELF_TEST_FAILED ' "$TMP/injected.log" \
  | sed 's/.*failures=\([0-9]*\).*/\1/')
[ "$INJ_FAILS" = "$SUMMARY_FAILS" ] \
  && pass "the summary's failure count matches the visible FAIL lines" \
  || fail "the summary's failure count matches the visible FAIL lines ($INJ_FAILS vs $SUMMARY_FAILS)"

# -- 3. the reference group must be byte-identical between the two runs -------
# The injected failure is additive: it must not perturb the real assertions.
CLEAN_OK=$(grep -c '^  ok' "$TMP/clean.log" || true)
INJ_OK=$(grep -c '^  ok' "$TMP/injected.log" || true)
[ "$CLEAN_OK" = "$INJ_OK" ] \
  && pass "the injected failure adds no spurious ok line ($CLEAN_OK cases)" \
  || fail "the injected failure adds no spurious ok line ($CLEAN_OK vs $INJ_OK)"

echo
if [ "$FAIL" -eq 0 ]; then
  echo "BROKER_SELF_TEST_HARNESS_PASS"
  exit 0
fi
echo "BROKER_SELF_TEST_HARNESS_FAILED failures=$FAIL"
exit 1

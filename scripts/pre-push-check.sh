#!/bin/bash
# The checks the Candidate's Windows quality gate runs first, run here instead.
#
# That gate takes ninety minutes to reach its own verdict and stops at the first
# failing check, so a formatting slip and a lint slip are two separate rounds of
# it. Both are minutes on this Mac. Run this before pushing anything the gate
# will judge.
#
# It deliberately runs the lint against the WINDOWS target in the RELEASE
# profile, because that is the configuration the gate uses: a host-target debug
# Clippy said nothing about two `let ... else` clauses that became irrefutable
# when an enum lost a variant, and the gate rejected the lib tests for it.
set -uo pipefail
R="$(cd "$(dirname "$0")/.." && pwd)"
cd "$R"
# See docs/macos-local-build.md: the MSVC CRT/SDK fetch does not complete on the
# direct route from this network.
P="${AGENTERM_BUILD_PROXY-http://127.0.0.1:8888}"
if [ -n "$P" ]; then export HTTPS_PROXY="$P" HTTP_PROXY="$P" ALL_PROXY="$P"; fi
TARGET="${AGENTERM_PRE_PUSH_TARGET:-x86_64-pc-windows-msvc}"

failed=0
step() {
  local name="$1"; shift
  if "$@" >"${TMPDIR:-/tmp}/pre-push-$name.log" 2>&1; then
    echo "  ok    $name"
  else
    echo "  FAIL  $name -- ${TMPDIR:-/tmp}/pre-push-$name.log"
    failed=1
  fi
}

echo "pre-push checks ($TARGET):"
step fmt cargo fmt --check
step clippy cargo xwin clippy --locked --profile release-fast \
  --target "$TARGET" --all-targets -- -D warnings
# A shared crate is also judged under MiniCon's actual consumer feature
# unions. Cross-target checks compile the module; native tests execute it.
step minicon-consumer python3 ./scripts/minicon-consumer-matrix.py --all --test-native
# On a non-Windows host `check.qjs` drops to its quick lane, which is a real
# subset of the Candidate's gate lane -- PRD capability alignment, the native
# public catalog clients, host Clippy and the library tests -- for about 90
# seconds. It is the widest local net available and it costs almost nothing.
step quick-lane env AGENTERM_BOOTSTRAP_TASK=check ./scripts/bootstrap.sh
step policy sh -c 'cargo test --locked --test release_workflow_policy --test candidate_local_receipt && python3 ./scripts/verify-local-candidate-test.py'
step redact ./scripts/doc-redact-check.sh

if [ "$failed" -ne 0 ]; then
  echo "pre-push: NOT ready to push"
  exit 1
fi
echo "pre-push: clean"

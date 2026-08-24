#!/usr/bin/env bash
# Produce all four builds and print every criterion in section 3 of
# plan/design-value-representation-experiment.md.
#
# Prerequisite: the tinyvm checkout is a sibling of this repository
# (../../../tinyvm relative to this directory). Cargo 1.97.0.
#
#   ./measure.sh                 # print the report
#   ./measure.sh > raw.md        # capture it
set -euo pipefail
cd "$(dirname "$0")"

echo "## Measurement conditions"
echo
echo "- date: $(date -u '+%Y-%m-%d %H:%M:%SZ')"
echo "- host: $(uname -sm)"
echo "- rustc: $(rustc --version)"
echo "- cargo: $(cargo --version)"
echo "- tinyvm: path dependency ../../../tinyvm/crates/tinyvm @ $(git -C ../../../tinyvm rev-parse --short HEAD 2>/dev/null || echo 'unknown')"
echo "- agenterm: $(git rev-parse --short HEAD)"
echo

echo "## Lines of code (non-blank, non-comment)"
echo
echo 'Command: `grep -cvE "^[[:space:]]*(//|/\*|\*|$)" <file>`'
echo
echo "| file | role | LOC |"
echo "|---|---|---|"
loc() { grep -cvE "^[[:space:]]*(//|/\*|\*|$)" "$1"; }
shared=0
for f in src/lib.rs src/lex.rs src/ast.rs src/parse.rs src/ir.rs src/encode.rs src/emit.rs src/runtime.rs src/repr.rs src/harness.rs src/bin/measure.rs; do
  n=$(loc "$f"); shared=$((shared + n))
  echo "| $f | shared | $n |"
done
p=$(loc src/repr_pair.rs)
n=$(loc src/repr_nanbox.rs)
echo "| src/repr_pair.rs | V1 only | $p |"
echo "| src/repr_nanbox.rs | V2 only | $n |"
echo
echo "Shared total: $shared. V1 representation layer: $p. V2 representation layer: $n."
echo "Both implement every method of the same \`Repr\` trait, so the capability sets are equal."
echo

echo "## Independent validator cross-check"
echo
echo 'Command: `cargo test`'
echo '```'
cargo test 2>&1 | grep -E '^test [a-z]|^test result' || true
echo '```'
echo

cargo run -q --bin measure

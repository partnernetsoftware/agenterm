#!/bin/sh
# Cross-process practice court: ephemeral / resident / crash reclaim.
set -eu
cd "$(dirname "$0")"
cargo build --release --bin shmbox-practice
exec ./target/release/shmbox-practice

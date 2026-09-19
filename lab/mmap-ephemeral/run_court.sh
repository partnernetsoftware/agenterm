#!/bin/sh
# One court: IPC slot hygiene + RPC call/serve. Release, one thread.
set -eu
cd "$(dirname "$0")"
exec cargo test --release --lib court -- --test-threads=1

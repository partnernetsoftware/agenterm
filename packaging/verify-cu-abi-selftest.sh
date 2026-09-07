#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TMP="$(mktemp -d "${TMPDIR:-/tmp}/agenterm-cu-abi-selftest.XXXXXXXX")"
trap 'rm -rf "$TMP"' EXIT HUP INT TERM

printf 'fixture ABI\n' >"$TMP/libagenterm.dylib"
write_cu() {
  local major="$1" minor="$2" required_major="$3" required_minor="$4"
  cat >"$TMP/agenterm-cu" <<EOF
#!/bin/sh
test -f "\${AGENTERM_ABI_LIB:?}" || exit 1
cat <<'JSON'
{"ok":true,"data":{"checks":{"abi":{"status":"available","detail":{"major":$major,"minor":$minor,"required_major":$required_major,"required_minor":$required_minor,"required_symbols":61}}}}}
JSON
EOF
  chmod 0755 "$TMP/agenterm-cu"
}

write_cu 1 28 1 28
"$ROOT/packaging/verify-cu-abi.sh" "$TMP/agenterm-cu" "$TMP/libagenterm.dylib" >/dev/null

write_cu 1 27 1 28
if "$ROOT/packaging/verify-cu-abi.sh" "$TMP/agenterm-cu" "$TMP/libagenterm.dylib" >/dev/null 2>&1; then
  echo "older ABI was accepted" >&2
  exit 1
fi

write_cu 1 29 1 28
if "$ROOT/packaging/verify-cu-abi.sh" "$TMP/agenterm-cu" "$TMP/libagenterm.dylib" >/dev/null 2>&1; then
  echo "non-matching ABI was accepted" >&2
  exit 1
fi

write_cu 2 28 1 28
if "$ROOT/packaging/verify-cu-abi.sh" "$TMP/agenterm-cu" "$TMP/libagenterm.dylib" >/dev/null 2>&1; then
  echo "wrong ABI major was accepted" >&2
  exit 1
fi

rm "$TMP/libagenterm.dylib"
if "$ROOT/packaging/verify-cu-abi.sh" "$TMP/agenterm-cu" "$TMP/libagenterm.dylib" >/dev/null 2>&1; then
  echo "missing ABI library was accepted" >&2
  exit 1
fi

echo "PASS: CU package ABI 1.28 structured readiness gate"

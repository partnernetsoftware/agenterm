#!/usr/bin/env bash
set -uo pipefail

if [[ $# -eq 0 ]]; then
  echo "usage: inspect-authenticode.sh FILE [FILE ...]" >&2
  exit 64
fi
if ! command -v osslsigncode >/dev/null 2>&1; then
  echo "inspect-authenticode: osslsigncode is required" >&2
  exit 69
fi

failed=0
for file in "$@"; do
  if [[ ! -f "$file" ]]; then
    echo "inspect-authenticode: not a file: $file" >&2
    failed=1
    continue
  fi
  echo "== $file =="
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$file"
  else
    sha256sum "$file"
  fi
  wc -c "$file"
  if ! osslsigncode verify -in "$file"; then
    failed=1
  fi
done

if [[ $failed -ne 0 ]]; then
  echo "inspect-authenticode: one or more files are unsigned or did not verify" >&2
  echo "Windows Get-AuthenticodeSignature is authoritative for the Windows trust result." >&2
  exit 2
fi

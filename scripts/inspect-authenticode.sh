#!/usr/bin/env bash
# Canonical contract: pns-authenticode-inspector/v3
set -uo pipefail

usage() {
  echo "usage: inspect-authenticode.sh [--ca-file FILE] [--] FILE [FILE ...]" >&2
}

ca_file=
while [[ $# -gt 0 ]]; do
  case "$1" in
    --ca-file)
      if [[ $# -lt 2 || -n "$ca_file" ]]; then
        usage
        exit 64
      fi
      ca_file=$2
      shift 2
      ;;
    --)
      shift
      break
      ;;
    -*)
      usage
      exit 64
      ;;
    *) break ;;
  esac
done
if [[ $# -eq 0 ]]; then
  usage
  exit 64
fi
if ! command -v osslsigncode >/dev/null 2>&1; then
  echo "inspect-authenticode: osslsigncode is required" >&2
  exit 69
fi
verify_args=()
if [[ -n "$ca_file" ]]; then
  if [[ ! -f "$ca_file" ]]; then
    echo "inspect-authenticode: CA bundle is not a file" >&2
    exit 66
  fi
  verify_args=(-CAfile "$ca_file" -TSA-CAfile "$ca_file")
  echo "inspect-authenticode: using caller-supplied CA bundle for portable verification"
fi

failed=0
probe_root=$(mktemp -d "${TMPDIR:-/tmp}/inspect-authenticode.XXXXXX")
trap 'rm -rf -- "$probe_root"' EXIT
index=0
for file in "$@"; do
  index=$((index + 1))
  if [[ ! -f "$file" ]]; then
    echo "inspect-authenticode: not a file: $file" >&2
    if [[ $failed -lt 2 ]]; then failed=2; fi
    continue
  fi
  echo "== $file =="
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$file"
  else
    sha256sum "$file"
  fi
  wc -c "$file"
  if ! osslsigncode verify "${verify_args[@]}" -in "$file"; then
    if osslsigncode extract-signature -in "$file" \
      -out "$probe_root/signature-$index.p7b" >/dev/null 2>&1; then
      echo "inspect-authenticode: embedded signature exists, but portable verification failed: $file" >&2
      echo "inspect-authenticode: Windows Get-AuthenticodeSignature is authoritative for the trust result." >&2
      if [[ $failed -lt 3 ]]; then failed=3; fi
    else
      echo "inspect-authenticode: no extractable embedded Authenticode signature: $file" >&2
      if [[ $failed -lt 2 ]]; then failed=2; fi
    fi
  fi
done

if [[ $failed -ne 0 ]]; then
  exit "$failed"
fi

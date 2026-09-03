#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: fetch-microsoft-trust-bundle.sh OUTPUT.pem" >&2
  exit 64
fi
for command_name in curl openssl; do
  if ! command -v "$command_name" >/dev/null 2>&1; then
    echo "fetch-microsoft-trust-bundle: required command is unavailable: $command_name" >&2
    exit 69
  fi
done

output=$1
if [[ -e "$output" ]]; then
  echo "fetch-microsoft-trust-bundle: refusing to overwrite output" >&2
  exit 73
fi
output_parent=$(dirname -- "$output")
if [[ ! -d "$output_parent" ]]; then
  echo "fetch-microsoft-trust-bundle: output parent is not a directory" >&2
  exit 66
fi

root_url='https://www.microsoft.com/pkiops/certs/microsoft%20identity%20verification%20root%20certificate%20authority%202020.crt'
root_sha256='5367f20c7ade0e2bca790915056d086b720c33c1fa2a2661acf787e3292e1270'
tsa_url='https://www.microsoft.com/pkiops/certs/Microsoft%20Public%20RSA%20Timestamping%20CA%202020.crt'
tsa_sha256='36e731cfa9bfd69dafb643809f6dec500902f7197daeaad86ea0159a2268a2b8'

scratch=$(mktemp -d "${TMPDIR:-/tmp}/microsoft-artifact-signing-trust.XXXXXX")
staged=$(mktemp "$output_parent/.microsoft-artifact-signing-trust.XXXXXX")
cleanup() {
  rm -rf -- "$scratch"
  if [[ -n "${staged:-}" && -f "$staged" ]]; then
    rm -f -- "$staged"
  fi
}
trap cleanup EXIT

curl --fail --silent --show-error --proto '=https' --tlsv1.2 \
  "$root_url" -o "$scratch/root.crt"
curl --fail --silent --show-error --proto '=https' --tlsv1.2 \
  "$tsa_url" -o "$scratch/tsa.crt"

digest() {
  openssl dgst -sha256 "$1" | awk '{print $NF}'
}
if [[ "$(digest "$scratch/root.crt")" != "$root_sha256" ]]; then
  echo "fetch-microsoft-trust-bundle: root certificate digest mismatch" >&2
  exit 65
fi
if [[ "$(digest "$scratch/tsa.crt")" != "$tsa_sha256" ]]; then
  echo "fetch-microsoft-trust-bundle: timestamp CA digest mismatch" >&2
  exit 65
fi

openssl x509 -inform DER -in "$scratch/root.crt" -out "$scratch/root.pem"
openssl x509 -inform DER -in "$scratch/tsa.crt" -out "$scratch/tsa.pem"
{
  cat "$scratch/root.pem"
  cat "$scratch/tsa.pem"
} >"$staged"
chmod 0644 "$staged"
mv -- "$staged" "$output"
staged=
echo "READY microsoft-artifact-signing-trust-bundle"
echo "READY_CHECK root_sha256=$root_sha256"
echo "READY_CHECK timestamp_ca_sha256=$tsa_sha256"

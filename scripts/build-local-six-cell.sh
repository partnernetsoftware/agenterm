#!/usr/bin/env bash
set -euo pipefail

# Build all release client binaries and the thin Chassis loader locally from
# one exact main revision. Hosted runners only consume these bytes for tests.
#
# Usage: scripts/build-local-six-cell.sh [TARGET_LANE]

if [[ "${GITHUB_ACTIONS:-}" == "true" ]]; then
  echo "local six-cell build refuses to run inside GitHub Actions" >&2
  exit 2
fi

lane="${1:-local-six-cell}"
if [[ ! "$lane" =~ ^[A-Za-z0-9][A-Za-z0-9._-]*$ ]]; then
  echo "invalid lane name" >&2
  exit 2
fi

repo="$(git rev-parse --show-toplevel)"
cd "$repo"
source_sha="$(git rev-parse HEAD)"
if [[ "$(git branch --show-current)" != main ]]; then
  echo "six-cell release build requires main" >&2
  exit 2
fi
if [[ -n "$(git status --porcelain=v1 --untracked-files=normal)" ]]; then
  echo "six-cell release build requires a clean worktree" >&2
  exit 2
fi
remote_main="$(git ls-remote origin refs/heads/main | awk 'NR == 1 { print $1 }')"
if [[ "$remote_main" != "$source_sha" ]]; then
  echo "six-cell release build requires exact origin/main" >&2
  exit 2
fi

for tool in cargo cargo-xwin cargo-zigbuild zig; do
  command -v "$tool" >/dev/null 2>&1 || {
    echo "required local cross-build tool is missing: $tool" >&2
    exit 2
  }
done
rustup target list --installed | rg -q '^aarch64-apple-darwin$'
rustup target list --installed | rg -q '^x86_64-apple-darwin$'
rustup target list --installed | rg -q '^aarch64-unknown-linux-gnu$'
rustup target list --installed | rg -q '^x86_64-unknown-linux-gnu$'
rustup target list --installed | rg -q '^aarch64-pc-windows-msvc$'
rustup target list --installed | rg -q '^x86_64-pc-windows-msvc$'

export AGENTERM_BUILD_DIST_DIR="dist/$lane"
export CARGO_TARGET_DIR="target/$lane"
if [[ -L "$AGENTERM_BUILD_DIST_DIR" || -L "$CARGO_TARGET_DIR" ]]; then
  echo "build lane cannot be a symlink" >&2
  exit 2
fi

# One Cargo task owns the six client cells and their common target tree.
AGENTERM_BOOTSTRAP_TASK=client-build-all \
  ./scripts/bootstrap.sh release

# The product packages include the L1 loader as part of the same local build
# lane. Linux uses Zig's pinned glibc floor; Windows uses cargo-xwin's MSVC
# toolchain. No hosted runner invokes Cargo for these targets.
cargo build --locked --release --target aarch64-apple-darwin \
  -p agenterm-chassis --features loader --bin agenterm-chassis-loader
cargo build --locked --release --target x86_64-apple-darwin \
  -p agenterm-chassis --features loader --bin agenterm-chassis-loader
cargo zigbuild --locked --release --target aarch64-unknown-linux-gnu.2.28 \
  -p agenterm-chassis --features loader --bin agenterm-chassis-loader
cargo zigbuild --locked --release --target x86_64-unknown-linux-gnu.2.28 \
  -p agenterm-chassis --features loader --bin agenterm-chassis-loader
cargo xwin build --locked --release --target aarch64-pc-windows-msvc \
  -p agenterm-chassis --features loader --bin agenterm-chassis-loader
cargo xwin build --locked --release --target x86_64-pc-windows-msvc \
  -p agenterm-chassis --features loader --bin agenterm-chassis-loader

version="$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n1)"
export AGENTERM_PACKAGE_DIST="dist/$lane"
mkdir -p "$AGENTERM_PACKAGE_DIST"

# Produce the SBOM and every archive that can be correctly assembled on the
# Mac host. Linux archives require their native library closure and are built
# later on the matching Linux courts from these exact cross-compiled bytes.
AGENTERM_BOOTSTRAP_TASK=supply-chain \
  ./scripts/bootstrap.sh . "$AGENTERM_PACKAGE_DIST/agenterm-$version-sbom.spdx.json"

for cell in \
  windows-x86_64 \
  windows-aarch64 \
  macos-aarch64 \
  macos-x86_64; do
  os="${cell%-*}"
  arch="${cell##*-}"
  target=""
  case "$cell" in
    windows-x86_64) target=x86_64-pc-windows-msvc ;;
    windows-aarch64) target=aarch64-pc-windows-msvc ;;
    macos-aarch64) target=aarch64-apple-darwin ;;
    macos-x86_64) target=x86_64-apple-darwin ;;
  esac
  mode=()
  if [[ "$os" == macos ]]; then
    mode=(--unsigned-preview)
  fi
  AGENTERM_BOOTSTRAP_TASK=package-client-release \
    ./scripts/bootstrap.sh "$version" "$os" "$arch" \
      "target/$lane/$target/release" "${mode[@]}"
done

out="target/$lane/candidate-input"
mkdir -p "$out"
cp "$AGENTERM_PACKAGE_DIST/agenterm-$version-sbom.spdx.json" "$out/"
python3 scripts/chassis-stage-l1-loader.py \
  --loader "target/$lane/aarch64-apple-darwin/release/agenterm-chassis-loader" \
  --cell osx-aarch64 --version "$version" --source-sha "$source_sha" --out "$out"
python3 scripts/chassis-stage-l1-loader.py \
  --loader "target/$lane/x86_64-apple-darwin/release/agenterm-chassis-loader" \
  --cell osx-x86_64 --version "$version" --source-sha "$source_sha" --out "$out"
python3 scripts/chassis-stage-l1-loader.py \
  --loader "target/$lane/aarch64-unknown-linux-gnu/release/agenterm-chassis-loader" \
  --cell lnx-aarch64 --version "$version" --source-sha "$source_sha" --out "$out"
python3 scripts/chassis-stage-l1-loader.py \
  --loader "target/$lane/x86_64-unknown-linux-gnu/release/agenterm-chassis-loader" \
  --cell lnx-x86_64 --version "$version" --source-sha "$source_sha" --out "$out"
python3 scripts/chassis-stage-l1-loader.py \
  --loader "target/$lane/aarch64-pc-windows-msvc/release/agenterm-chassis-loader.exe" \
  --cell win-aarch64 --version "$version" --source-sha "$source_sha" --out "$out"
python3 scripts/chassis-stage-l1-loader.py \
  --loader "target/$lane/x86_64-pc-windows-msvc/release/agenterm-chassis-loader.exe" \
  --cell win-x86_64 --version "$version" --source-sha "$source_sha" --out "$out"

stage_part() {
  local platform="$1" cell="$2" archive_name="$3"
  local part="$out/candidate-part-$platform"
  mkdir -p "$part/chassis-l1"
  cp "$AGENTERM_PACKAGE_DIST/$archive_name" \
    "$AGENTERM_PACKAGE_DIST/$archive_name.sha256" \
    "$AGENTERM_PACKAGE_DIST/$archive_name.provenance.json" "$part/"
  cp -R "$out/chassis-l1/$cell" "$part/chassis-l1/"
}

stage_part windows-x86_64 win-x86_64 \
  "agenterm-$version-windows-x86_64.zip"
stage_part windows-aarch64 win-aarch64 \
  "agenterm-$version-windows-aarch64.zip"
stage_part macos-aarch64 osx-aarch64 \
  "agenterm-$version-macos-aarch64-unsigned-preview.zip"
stage_part macos-x86_64 osx-x86_64 \
  "agenterm-$version-macos-x86_64-unsigned-preview.zip"
cp "$out/agenterm-$version-sbom.spdx.json" \
  "$out/candidate-part-windows-x86_64/"
if [[ -f "$AGENTERM_PACKAGE_DIST/MACOS_UNSIGNED_PREVIEW_README.md" ]]; then
  cp "$AGENTERM_PACKAGE_DIST/MACOS_UNSIGNED_PREVIEW_README.md" \
    "$AGENTERM_PACKAGE_DIST/MACOS_UNSIGNED_PREVIEW_README.zh-Hant.md" \
    "$out/candidate-part-macos-aarch64/"
fi

python3 scripts/chassis-candidate-pack.py \
  --candidate-input "$out" \
  --version "$version" \
  --source-sha "$source_sha" \
  --out "$AGENTERM_PACKAGE_DIST/agenterm-$version-chassis-product.tgz" \
  > "$AGENTERM_PACKAGE_DIST/chassis-pack.json"
cp "$AGENTERM_PACKAGE_DIST/agenterm-$version-chassis-product.tgz" \
  "$AGENTERM_PACKAGE_DIST/agenterm-$version-chassis-product.tgz.sha256" \
  "$AGENTERM_PACKAGE_DIST/agenterm-$version-chassis-product.tgz.provenance.json" \
  "$out/"

python3 - "$source_sha" "$version" "$lane" <<'PY'
import hashlib
import json
import pathlib
import subprocess
import sys

source_sha, version, lane = sys.argv[1:]
root = pathlib.Path("target") / lane
build_summary = pathlib.Path("target/qualification/six-cell/summary.json")
if not build_summary.is_file():
    raise SystemExit("six-cell client build summary is missing")
summary = json.loads(build_summary.read_text(encoding="utf-8"))
expected_targets = {
    "aarch64-apple-darwin", "x86_64-apple-darwin",
    "aarch64-unknown-linux-gnu", "x86_64-unknown-linux-gnu",
    "aarch64-pc-windows-msvc", "x86_64-pc-windows-msvc",
}
if summary.get("profile") != "release" or {
    item.get("target") for item in summary.get("cells", [])
} != expected_targets or len(summary.get("cells", [])) != 6:
    raise SystemExit("client build summary does not contain the six release cells")
for item in summary["cells"]:
    if item.get("state") != "PASS":
        raise SystemExit(f"client build failed: {item.get('target')}")
    for artifact in item.get("artifacts", []):
        path = pathlib.Path(artifact["path"])
        if (not path.is_file() or path.is_symlink()
                or hashlib.sha256(path.read_bytes()).hexdigest() != artifact.get("sha256")):
            raise SystemExit(f"client build artifact changed: {item.get('target')}")

cell_targets = {
    "osx-aarch64": "aarch64-apple-darwin",
    "osx-x86_64": "x86_64-apple-darwin",
    "lnx-aarch64": "aarch64-unknown-linux-gnu",
    "lnx-x86_64": "x86_64-unknown-linux-gnu",
    "win-aarch64": "aarch64-pc-windows-msvc",
    "win-x86_64": "x86_64-pc-windows-msvc",
}
loaders = {}
for cell, target in cell_targets.items():
    suffix = ".exe" if target.endswith("msvc") else ""
    path = pathlib.Path("target") / lane / target / "release" / f"agenterm-chassis-loader{suffix}"
    if not path.is_file() or path.stat().st_size == 0:
        raise SystemExit(f"Chassis loader missing: {cell}")
    descriptor = json.loads(
        (root / "candidate-input/chassis-l1" / cell / "loader.json").read_text(encoding="utf-8")
    )
    digest = hashlib.sha256(path.read_bytes()).hexdigest()
    if (descriptor.get("cell") != cell or descriptor.get("version") != version
            or descriptor.get("source_sha") != source_sha
            or descriptor.get("bytes") != path.stat().st_size
            or descriptor.get("sha256") != digest):
        raise SystemExit(f"Chassis loader descriptor mismatch: {cell}")
    loaders[cell] = {"bytes": path.stat().st_size, "sha256": digest}

manifest = {
    "schema": 1,
    "kind": "agenterm-local-six-cell-build",
    "source_sha": source_sha,
    "version": version,
    "profile": "release",
    "client_summary_sha256": hashlib.sha256(build_summary.read_bytes()).hexdigest(),
    "loaders": loaders,
    "host_packaged_cells": [
        "windows-x86_64", "windows-aarch64", "macos-aarch64", "macos-x86_64"
    ],
    "linux_packaging": "requires native Linux court; no compile",
}
(root / "candidate-input/build-manifest.json").write_text(
    json.dumps(manifest, sort_keys=True, indent=2) + "\n", encoding="utf-8"
)
print(f"LOCAL SIX-CELL BUILD PASS {source_sha} {version}")
print(f"candidate input: {root / 'candidate-input'}")
PY

after_sha="$(git rev-parse HEAD)"
if [[ "$after_sha" != "$source_sha" || -n "$(git status --porcelain=v1 --untracked-files=normal)" ]]; then
  echo "source changed during local six-cell build" >&2
  exit 2
fi

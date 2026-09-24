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
export AGENTERM_CANDIDATE_SOURCE_SHA="$source_sha"
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
total_started=$SECONDS
if [[ -L "$AGENTERM_BUILD_DIST_DIR" || -L "$CARGO_TARGET_DIR" ]]; then
  echo "build lane cannot be a symlink" >&2
  exit 2
fi

mkdir -p "$CARGO_TARGET_DIR" "$AGENTERM_BUILD_DIST_DIR"
# The quick gate owns Cargo cleanup and may remove files below CARGO_TARGET_DIR.
# Keep the gate transcript in the lane's dist directory so its evidence survives.
pre_push_log="$AGENTERM_BUILD_DIST_DIR/pre-push-check.log"
pre_push_started=$SECONDS
set +e
scripts/pre-push-check.sh >"$pre_push_log" 2>&1
pre_push_status=$?
set -e
cat "$pre_push_log"
pre_push_seconds=$((SECONDS - pre_push_started))
if [[ "$pre_push_status" -ne 0 ]]; then
  exit "$pre_push_status"
fi
pre_push_sha256="$(shasum -a 256 "$pre_push_log" | awk '{print $1}')"

# One Cargo task owns the six client cells and their common target tree.
# The summary path is shared by the six-cell task rather than nested below
# CARGO_TARGET_DIR. Remove only that generated summary so an interrupted older
# run cannot stand in for this exact-source build.
summary_path="target/qualification/six-cell/summary.json"
if [[ -L "$summary_path" ]]; then
  echo "six-cell summary cannot be a symlink" >&2
  exit 2
fi
rm -f "$summary_path"
client_build_started=$SECONDS
AGENTERM_BOOTSTRAP_TASK=client-build-all \
  ./scripts/bootstrap.sh release
client_build_seconds=$((SECONDS - client_build_started))

# The product packages include the L1 loader as part of the same local build
# lane. Linux uses Zig's pinned glibc floor; Windows uses cargo-xwin's MSVC
# toolchain. No hosted runner invokes Cargo for these targets.
chassis_build_started=$SECONDS
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
chassis_build_seconds=$((SECONDS - chassis_build_started))

version="$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n1)"
export AGENTERM_PACKAGE_DIST="dist/$lane"
mkdir -p "$AGENTERM_PACKAGE_DIST"
package_started=$SECONDS

# Produce the SBOM and every archive that can be correctly assembled on the
# Mac host. Linux archives require their native library closure and are built
# later on the matching Linux courts from these exact cross-compiled bytes.
AGENTERM_BOOTSTRAP_TASK=supply-chain \
  ./scripts/bootstrap.sh . "$AGENTERM_PACKAGE_DIST/agenterm-$version-sbom.spdx.json"
export AGENTERM_PACKAGE_SBOM="$PWD/$AGENTERM_PACKAGE_DIST/agenterm-$version-sbom.spdx.json"

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
  if [[ "$os" == macos ]]; then
    AGENTERM_BOOTSTRAP_TASK=package-client-release \
      ./scripts/bootstrap.sh "$version" "$os" "$arch" \
        "target/$lane/$target/release" --unsigned-preview
  else
    AGENTERM_BOOTSTRAP_TASK=package-client-release \
      ./scripts/bootstrap.sh "$version" "$os" "$arch" \
        "target/$lane/$target/release"
  fi
done

out="target/$lane/candidate-input"
if [[ -L "$out" ]]; then
  echo "Candidate staging input cannot be a symlink" >&2
  exit 2
fi
rm -rf "$out"
mkdir -p "$out"
cp "$AGENTERM_PACKAGE_DIST/agenterm-$version-sbom.spdx.json" "$out/"
cp "$pre_push_log" "$out/pre-push-check.log"
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
package_seconds=$((SECONDS - package_started))
total_seconds=$((SECONDS - total_started))

python3 - "$source_sha" "$version" "$lane" "$pre_push_sha256" \
  "$pre_push_seconds" "$client_build_seconds" "$chassis_build_seconds" \
  "$package_seconds" "$total_seconds" <<'PY'
import hashlib
import json
import pathlib
import shutil
import tempfile
import sys
import tarfile

(
    source_sha, version, lane, pre_push_sha256, pre_push_seconds,
    client_build_seconds, chassis_build_seconds, package_seconds, total_seconds,
) = sys.argv[1:]
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
platform_targets = {
    "macos-aarch64": "aarch64-apple-darwin",
    "macos-x86_64": "x86_64-apple-darwin",
    "linux-aarch64": "aarch64-unknown-linux-gnu",
    "linux-x86_64": "x86_64-unknown-linux-gnu",
    "windows-aarch64": "aarch64-pc-windows-msvc",
    "windows-x86_64": "x86_64-pc-windows-msvc",
}
target_platforms = {target: platform for platform, target in platform_targets.items()}
cells = []
candidate_input = root / "candidate-input"
for item in summary["cells"]:
    target = item["target"]
    platform_id = target_platforms[target]
    artifacts = []
    build_artifacts = []
    for artifact in item["artifacts"]:
        path = pathlib.Path(artifact["path"])
        build_artifacts.append({
            "name": path.name,
            "bytes": path.stat().st_size,
            "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
        })
        if platform_id.startswith("linux-"):
            raw = candidate_input / "raw-linux" / platform_id
            raw.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(path, raw / path.name)
            artifacts.append(build_artifacts[-1])
    if not platform_id.startswith("linux-"):
        part = candidate_input / f"candidate-part-{platform_id}"
        packages = sorted(part.glob(f"agenterm-{version}-{platform_id.split('-', 1)[0]}-*.zip"))
        if len(packages) != 1:
            raise SystemExit(f"expected one locally packaged archive for {platform_id}")
        package = packages[0]
        artifacts.append({
            "name": package.name,
            "bytes": package.stat().st_size,
            "sha256": hashlib.sha256(package.read_bytes()).hexdigest(),
        })
    cells.append({
        "platform_id": platform_id,
        "target": target,
        "state": "PASS",
        "artifacts": artifacts,
        "build_artifacts": build_artifacts,
    })

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

loader_records = [
    {"cell": cell, **identity} for cell, identity in loaders.items()
]
package_records = []
for part in sorted(candidate_input.glob("candidate-part-*")):
    for archive in sorted(part.glob("agenterm-*.zip")):
        package_records.append({
            "platform_id": part.name.removeprefix("candidate-part-"),
            "name": archive.name,
            "bytes": archive.stat().st_size,
            "sha256": hashlib.sha256(archive.read_bytes()).hexdigest(),
        })

manifest = {
    "schema_version": 1,
    "kind": "agenterm-local-six-cell-build",
    "source_sha": source_sha,
    "version": version,
    "profile": "release",
    "client_summary_sha256": hashlib.sha256(build_summary.read_bytes()).hexdigest(),
    "pre_push_check": {
        "status": "passed",
        "name": "pre-push-check.log",
        "size": (candidate_input / "pre-push-check.log").stat().st_size,
        "sha256": pre_push_sha256,
    },
    "durations_seconds": {
        "pre_push": int(pre_push_seconds),
        "client_build_six_cells": int(client_build_seconds),
        "chassis_loader_six_cells": int(chassis_build_seconds),
        "packaging_and_bundle": int(package_seconds),
        "total": int(total_seconds),
    },
    "source_inputs": {
        "cargo_lock_sha256": hashlib.sha256(pathlib.Path("Cargo.lock").read_bytes()).hexdigest(),
        "artifact_manifest_sha256": hashlib.sha256(pathlib.Path("scripts/artifacts.json").read_bytes()).hexdigest(),
        "release_policy_sha256": hashlib.sha256(pathlib.Path("release-policy.json").read_bytes()).hexdigest(),
        "gate_manifest_sha256": hashlib.sha256(pathlib.Path("scripts/qualification-gates.json").read_bytes()).hexdigest(),
        "sbom_sha256": hashlib.sha256((candidate_input / f"agenterm-{version}-sbom.spdx.json").read_bytes()).hexdigest(),
    },
    "cells": cells,
    "chassis_loaders": loader_records,
    "host_packages": package_records,
    "host_packaged_cells": [
        "windows-x86_64", "windows-aarch64", "macos-aarch64", "macos-x86_64"
    ],
    "linux_packaging": "requires native Linux court; no compile",
}
build_manifest_path = candidate_input / "local-build-manifest.json"
build_manifest_path.write_text(
    json.dumps(manifest, sort_keys=True, indent=2) + "\n", encoding="utf-8"
)
qualification = {
        "schema_version": 2,
    "product": "AgenTerm",
    "profile": "prebuilt-six-cell-execute-only",
    "release": True,
    "stress_included": False,
    "source_sha": source_sha,
    "version": version,
    "release_policy_sha256": manifest["source_inputs"]["release_policy_sha256"],
    "pre_push_check": manifest["pre_push_check"],
    "local_build_manifest": {
        "name": build_manifest_path.name,
        "size": build_manifest_path.stat().st_size,
        "sha256": hashlib.sha256(build_manifest_path.read_bytes()).hexdigest(),
    },
}
(candidate_input / "qualification-receipt.json").write_text(
    json.dumps(qualification, sort_keys=True, indent=2) + "\n", encoding="utf-8"
)

bundle_name = f"agenterm-{version}-local-candidate-input.tar.gz"
bundle_path = pathlib.Path("dist") / lane / bundle_name
with tarfile.open(bundle_path, "w:gz") as bundle:
    for path in sorted(candidate_input.rglob("*")):
        if path.is_symlink() or (not path.is_file() and not path.is_dir()):
            raise SystemExit(f"unsafe Candidate input entry: {path.name}")
        if path.is_file():
            bundle.add(path, arcname=path.relative_to(root))
bundle_hash = hashlib.sha256(bundle_path.read_bytes()).hexdigest()
(bundle_path.with_name(bundle_path.name + ".sha256")).write_text(
    f"{bundle_hash}  {bundle_path.name}\n", encoding="utf-8"
)
verify_out = pathlib.Path(tempfile.mkdtemp(prefix="local-bundle-check-", dir=root))
try:
    import subprocess
    subprocess.run(
        [
            sys.executable,
            "scripts/verify-local-candidate.py",
            "--archive", str(bundle_path),
            "--checksum", str(bundle_path.with_name(bundle_path.name + ".sha256")),
            "--out", str(verify_out),
            "--source-sha", source_sha,
            "--version", version,
        ],
        check=True,
    )
finally:
    shutil.rmtree(verify_out)
print(f"LOCAL SIX-CELL BUILD PASS {source_sha} {version}")
print(
    "LOCAL SIX-CELL TIMINGS "
    f"pre_push={pre_push_seconds}s client_build={client_build_seconds}s "
    f"chassis_loaders={chassis_build_seconds}s package={package_seconds}s "
    f"total={total_seconds}s"
)
print(f"candidate input: {root / 'candidate-input'}")
print(f"draft staging bundle: {bundle_path}")
PY

after_sha="$(git rev-parse HEAD)"
if [[ "$after_sha" != "$source_sha" || -n "$(git status --porcelain=v1 --untracked-files=normal)" ]]; then
  echo "source changed during local six-cell build" >&2
  exit 2
fi

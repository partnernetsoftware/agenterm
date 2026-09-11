# singleton-safety build manifest

Generator and validator for the four-member singleton-safety build manifest
described by [`../build-manifest-contract.json`](../build-manifest-contract.json).

## Purpose

From repository sources plus the four final artifacts under
`target/browser-profile-open-singleton-safety-fixtures/`, produce
`target/browser-profile-open-singleton-safety-fixtures/build-manifest.json` as
**one RFC 8785 canonical JSON object** satisfying every rule in the contract, and
a validator that re-derives it and compares byte-for-byte. Both reject
missing/extra keys and hash mismatches.

## Scope and non-goals

- No runner/court, no `result-template.json` change, no touch to the frozen
  contracts or existing fixture sources.
- The generator never compiles: it records the compiler probe and hashes the
  artifacts that already exist. When an artifact is missing it fails closed by
  name.
- No GUI/browser/court execution and no network.

## Authority boundary

- `../build-manifest-contract.json` is the read-only authority for every key,
  order, value domain, and cross-member rule.
- The scripts here own generation and validation only. The L0 composite digest
  is **not** stored in the generated manifest (contract `composite.output_rule`),
  so nothing depends on itself.

## Why the scripts live under `scripts/qjs/`

`AGENTS.md` ("Script engines") requires repository scripts to be `.qjs` under
`scripts/qjs/`. This directory keeps this README; the runnable scripts, the
shared library, and the synthetic self-test live where the engine and the task
runner expect them:

| artifact | path |
|----------|------|
| canonical JSON lib (RFC 8785) | `scripts/qjs/lib/canonical_json.qjs` |
| contract model + generate + validate | `scripts/qjs/lib/build_manifest.qjs` |
| generate entry | `scripts/qjs/build-manifest.qjs` |
| validate entry | `scripts/qjs/validate-build-manifest.qjs` |
| synthetic self-test | `scripts/qjs/build-manifest-selftest.qjs` |

## Contract coverage (one line per rule family)

- **exact keys** on every object (manifest, member, contract ref, source,
  compiler, artifact, plist, codesign, composite) → `validate_object` /
  `exact_keys`.
- **member order** `independent-process-helper`, `independent-foreground-helper`,
  `primary-focus-fixture`, `secondary-focus-fixture` → `MEMBER_ORDER`.
- **hashes** recomputed from exact bytes: contract, sources, artifacts, plists →
  `sha256_or_fail`.
- **compiler** family/version/argv normalized, `xcrun` first and tool second; the
  normalized `version` line carries no absolute path and no timestamp → `compiler_of`.
- **codesign** mode/status/scope with mode/status consistency → `codesign`.
- **composite metadata** records domain/order/formula and **no computed digest**
  → `member_of`.
- **RFC 8785** single object, no trailing LF → `canonical_bytes` (custom
  canonicalizer; the engine's `JSON.stringify` preserves insertion order).
- **no absolute home path** anywhere → `is_valid_logical_path` + self-test.
- **cross-member** equal fixture artifact/compiler/contract/source hashes and
  differing plist source hashes / bundle ids → `validate_object` tail.

## Usage

Generate (needs the four final artifacts under
`target/browser-profile-open-singleton-safety-fixtures/`):

```
./dist/agenterm cli script task run build-manifest --manifest agenterm.tasks.json -- <repo> aarch64 <signing.json> target/browser-profile-open-singleton-safety-fixtures/build-manifest.json
```

Validate (re-derives and compares):

```
./dist/agenterm cli script task run validate-build-manifest --manifest agenterm.tasks.json -- <repo> aarch64 <signing.json> target/browser-profile-open-singleton-safety-fixtures/build-manifest.json
```

`<signing.json>` is a map `logical_id -> {mode, status}` with `mode` in
`none | linker-ad-hoc | explicit-ad-hoc` and `status` `unsigned | valid`
(`none` requires `unsigned`; ad-hoc modes require `valid`).

## Self-test

```
./dist/agenterm cli script task run build-manifest-selftest --manifest agenterm.tasks.json
```

Builds a throwaway tree under
`target/singleton-safety-build-manifest-selftest/`, generates, re-derives, and
asserts every negative the contract names (missing/extra key, member reorder,
helper plist not null, codesign contradiction, cross-member hash equality and
inequality, source hash mismatch, missing artifact, logical-path domain).
No GUI/browser/court; the only host work is `xcrun --version` and SHA-256.

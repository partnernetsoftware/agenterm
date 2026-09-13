# Fixture provenance

These bytes are copied from `research/browser-profile-name-binding-v2/fixtures/`.

| File | Source | Note |
|---|---|---|
| `local-state.json` | v2 `fixtures/local-state.json` | Synthetic Chromium Local State shape (`profile.last_used`, `profiles_order`, `info_cache`) |
| `preferences.json` | v2 `fixtures/preferences.json` | Synthetic Chromium Preferences shape |

Reuse is by **copying with provenance** (spec §5). Nothing here imports mutable
result state or an attempt ledger from either exhausted experiment.

The fixtures describe a **synthetic** Profile whose display name is `Work`.
They are not a real user Profile and must never be written into a real HOME.

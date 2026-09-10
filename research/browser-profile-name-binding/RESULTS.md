# Browser Profile name-binding results

## Attempt 1 · dependency inconclusive

- Source SHA: `c576db84b7035b7cc353346ae936a49b53cf94ce`.
- Input digest: `87321a57f80afd25a89e906211afd4a7c52211e6a7adea76021ec398643debbc`.
- Attempt: 1 of 2.
- Expected extension build id: `c359ed705ab34163a91f43a6fe060b746370ab2f4a2a781736bd9d6ee947f3b2`.
- Executed `agenterm` SHA-256: `41399ce801f1a5c8b018554cd6e96d4b81d842b8a68eabe30a12a7dc85989b90`.
- Executed `agenterm-cu` SHA-256: `0742499b54f7f2f07769d651a0bf9b305ecd2090762479333ec20c8c490dddbf`.
- Executed Chromium-family binary SHA-256: `5f3d43277ede0c2804c099e71f712d033a3351216f49ac34d9482cab00ec9d0c`.
- Court result: `INCONCLUSIVE_DEPENDENCY` with
  `selector_observation_no_connection`; cleanup was not proven by the frozen
  failure receipt.

The selected binary was the installed stable Google Chrome application. Its
process started, but no bridge connection appeared before the bounded deadline,
so G1 failed before G2, G4a, A0, A1 or G7 could be measured. This result makes
no profile-binding decision. It registered no evidence and changed no product,
PRD, ledger or compatibility state.

The one permitted fixture repair changes the second attempt to the installed
Brave Origin application, avoiding the stable-Chrome command-line unpacked
extension path. It also makes the court prove an empty 0/0 connection inventory
after every failed browser start. These changes do not alter any criterion or
selection rule. Attempt 2 must use a new frozen, pushed source and is terminal
for validity failures.

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

## Attempt 2 · fixture exhausted without a design verdict

- Source SHA: `6e675b05549cae3e69c94db16080c98a5720af4f`.
- Input digest: `bc816430bd53f079571128949c859486e944a82dccac8b707e3a364167956eb2`.
- Attempt: 2 of 2.
- Expected extension build id: `c359ed705ab34163a91f43a6fe060b746370ab2f4a2a781736bd9d6ee947f3b2`.
- Executed `agenterm` SHA-256: `41399ce801f1a5c8b018554cd6e96d4b81d842b8a68eabe30a12a7dc85989b90`.
- Executed `agenterm-cu` SHA-256: `0742499b54f7f2f07769d651a0bf9b305ecd2090762479333ec20c8c490dddbf`.
- Executed Brave Origin binary SHA-256: `c70018c3e1a8c07dd1bbf5ceb4787d085ed9bb2439412c76eff52490fc20c95d`.
- Court result: `INCONCLUSIVE_FIXTURE_EXHAUSTED` with
  `profile_binding_connection_cleanup_unverified`; cleanup was not proven,
  `registers_evidence=false`, and `changes_product_state=false`.

The named failure can occur only after a live connection, strict status,
single-row inventory, exact-prefix control, G2 computation, G4a inventory and
model adjudication have executed, followed by browser kill and wait. Those
intermediate values were not serialized into the frozen failure receipt. They
are therefore a control-flow reconstruction only, are not authoritative
measurements, and cannot select or reject A0, A1 or B. Separately, the pure G7
seven-operation model and fault matrix pass their static self-test at this
source; that fact is reproducible without a browser but is not an implementation
authorization or a design verdict.

Post-stop inventory did not reach the required visited=0 and returned=0 state
within either bounded cleanup window. This is a fixture/host-lifecycle validity
failure, not evidence that Profile name binding is impossible or that any
candidate design passed or failed. Attempts 1 and 2 have exhausted the frozen
research budget. A third run is forbidden. The compatibility TODO, ledger gap,
evidence set and product implementation remain unchanged.

Any future investigation requires a new precommitment. Before spending another
run, it must prove that the browser child being stopped owns the bridge
connection process identity, preserve a bounded failure stage in the receipt,
and define cleanup independently of provisional selector results.

The tracked `budget-exhausted.json` marker makes this terminal disposition
executable: both the runner and the browser path of the court reject any later
attempt before fixture creation or process launch.

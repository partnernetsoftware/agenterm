# MCP persisted-session experiment results

Attempt 1 was executed against frozen commit
`f7046fa2d6330b3d7e2e1ff4e72253d6cd5787bd` and exited zero. Its
invocation lane was subsequently lost; the receipt, metadata and attempt ledger
are unrecoverable, and the deletion source could not be identified. Under the
frozen README, this run is void and its budget is consumed, not unused.

The run's frozen content manifest was
`9515c7d123e8100317d5a09c4468dc12043d98cf3cc3602411aedb43d4c0dc43`;
the authoritative bytes are the eight research files in commit
`f7046fa2d6330b3d7e2e1ff4e72253d6cd5787bd`. This result write-back postdates
that manifest and does not alter the run's frozen input identity.

No alternative is selected, no evidence is registered, and no attempt-2 repair
chain exists. Because the terminal observation was seen before the artifacts
were lost, this precommitment cannot be rerun as written. Any further work
requires a new precommitment that records this history.

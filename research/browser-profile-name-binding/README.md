# Browser Profile name-binding experiment

This directory implements the frozen experiment in
`plan/design-browser-profile-name-binding-experiment.md`. It is a research
court, not a provider, qualification gate or compatibility route.

The court uses one invocation-owned synthetic HOME. Candidate A is the tracked
Local State and Preferences fixture under the only candidate browser root. Live
Profile B is launched outside every candidate root. The browser court records
only fixed fixture labels, cardinalities, boolean edge decisions and public
high-entropy digests; it never prints a path, PID, title, URL or full Profile
instance id.

The input digest binds the exact tracked specification, research programs,
fixtures and embedded extension source assets at the reachable source commit.
Separate SHA-256 fields identify the actual two AgenTerm executables and
Chromium binary used by the run. The loaded extension status must equal the
build id independently recomputed from the frozen embedded assets; executable
digests identify the bytes used but do not claim that those binaries were built
from the source commit.

Only the primary agent may run the browser court. Before a run, commit and push
every research input, obtain an independent read-only review, and set the exact
browser and current product binaries:

```sh
AGENTERM_EXE=target/debug/agenterm \
AGENTERM_CU_EXE=target/debug/agenterm-cu \
AGENTERM_CU_BROWSER_EXE=~/path/to/chromium \
ACU075_BINDING_ATTEMPT=1 \
research/browser-profile-name-binding/run-current-host.sh
```

One fixture-only repair is allowed. The second attempt is terminal. A run does
not change product state: first record its trace in `RESULTS.md` and the design,
then review the selected branch before implementing anything. Because
`RESULTS.md` is itself a digest input, any second attempt requires a new frozen,
pushed commit containing the first result and the reviewed fixture repair.

Attempt 1 used installed stable Google Chrome and ended before G1 with
`selector_observation_no_connection`. The reviewed attempt-2 fixture uses the
installed Brave Origin application; this changes only the browser fixture, not
the criteria. Attempt 2 reached post-stop cleanup but could not prove an empty
connection inventory, so G1 validity failed and the terminal result is
`INCONCLUSIVE_FIXTURE_EXHAUSTED`. Both attempts are consumed, no A0/A1/B design
was selected, and a third run is forbidden. The compatibility TODO, ledger and
evidence state remain unchanged. Any continuation requires a new
precommitment that proves the stopped browser child owns the bridge connection
process identity and preserves a bounded failure stage in the receipt.

`budget-exhausted.json` is the machine-readable terminal marker. The runner
and the browser path of the court reject every further attempt with
`profile_binding_research_budget_exhausted` before creating a fixture or
starting a process. A future precommitment must replace this frozen court rather
than relabel an additional run as attempt 1 or 2.

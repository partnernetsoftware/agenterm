# qjswasm harness journal result

This court compares the frozen pre-change read/concatenate/rewrite algorithm
with the production single-serialization `fs.append` path. Both variants write
33 equivalent bounded records, fold the JSONL journal, parse the final 33-row
array, and remove their owned run directory.

Run from repository root with the current debug product binary:

```sh
./target/debug/agenterm cli script task run harness-journal-cost-legacy --manifest agenterm.tasks.json --json
./target/debug/agenterm cli script task run harness-journal-cost-append --manifest agenterm.tasks.json --json
```

## Result (2026-09-04)

| variant | steps | host ops | host bytes | heap pages | duration run 1 / 2 |
|---|---:|---:|---:|---:|---:|
| legacy whole-journal rewrite | 7,425,588 | 325 | 679,542 | 21 | 418 / 423 ms |
| serialize once + `fs.append` | 5,447,214 | 194 | 106,134 | 7 | 317 / 318 ms |

The runtime counters were identical across both repetitions. The accepted path
reduces steps by 26.6%, host operations by 40.3%, host bytes by 84.4%, and heap
pages by 66.7%. Both variants produced the same observable 33-record JSON
shape. The production harness keeps fail-closed crash behavior: an incomplete
last JSONL record makes finalization fail instead of publishing partial JSON.

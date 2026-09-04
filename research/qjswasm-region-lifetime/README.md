# qjswasm region-lifetime evidence

This directory holds the measured evidence for
`plan/design-qjswasm-region-lifetime-experiment.md`.

The diagnostic path is opt-in:

```text
AGENTERM_QJS_ALLOCATION_PROBE=1
```

It adds `heap_start_bytes`, `heap_bytes`, `json_parse_bytes`, and
`json_stringify_bytes` to the Script cost envelope. A normal run omits all four
fields. `heap_bytes - heap_start_bytes` is run allocation; the JSON counters
are gross operation-family allocation. Neither number is automatically
reclaimable memory. Reclaimability additionally needs a closed root/alias proof
and a dead suffix at the proposed operation boundary.

The report never treats one-shot slot destruction as a successful region
recovery: every `script run` already destroys its slot after the call. L0 asks
whether JSON parse, JSON stringify, or host-reply projection has a dead suffix
that can be reclaimed *inside* a still-running script.

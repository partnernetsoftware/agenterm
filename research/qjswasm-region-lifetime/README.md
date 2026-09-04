# qjswasm region-lifetime evidence

This directory holds the measured evidence for
`plan/design-qjswasm-region-lifetime-experiment.md`.

The diagnostic path is opt-in:

```text
AGENTERM_QJS_ALLOCATION_PROBE=1
```

It adds `heap_start_bytes` and `heap_bytes` to the Script cost envelope. A
normal run omits both fields. `heap_bytes - heap_start_bytes` is run allocation;
it is not automatically reclaimable memory. Reclaimability additionally needs
a closed root/alias proof at the proposed operation boundary.

The report never treats one-shot slot destruction as a successful region
recovery: every `script run` already destroys its slot after the call. L0 asks
whether JSON parse, JSON stringify, or host-reply projection has a dead suffix
that can be reclaimed *inside* a still-running script.

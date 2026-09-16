# Local LLM gateway (`agenterm-llm-gateway.exe`)

Parent: [AgenTerm product tree](../PRD.md#product-tree)

Legend: `[x]` shipped, `[~]` partial, `[ ]` planned.

This module records safety gates for an unassigned product hypothesis. It does
not commit the gateway executable or a release version.

Product design for **Web session providers** (Playwright/Camoufox → OpenAI-compatible)
and **user BYOK** routing lives in
[`plan/design-llm-bridge-web-to-api.md`](../plan/design-llm-bridge-web-to-api.md).
The product hypothesis retains a **Native Shell + independently versioned Logic
Pack** split so frequent adapter changes need not force a shell release. The
former Rhai-specific implementation draft is
[`archived`](../plan/archive/design-llm-gateway-rhai-logic-pack.md): Rhai left
this repository, so no backend, file extension or embedded engine is selected
until implementation is assigned and the current Script Runtime contract passes
its owning gates. Those adapters must terminate at this gateway, not bypass it.

- Dependency and isolation
  - [ ] implementation begins only after Observable Fleet, the stable
    unrestricted Script Runtime contract, MCP typed tools, credential
    isolation, and audit contracts pass their gates; the gateway cannot turn a
    Script profile or capability declaration into an authorization boundary
  - [ ] run as an optional loopback-authenticated sidecar, separate from the
    lightweight specialized-model worker and from GUI startup
  - [ ] keep provider credentials in an OS credential store, outside
    workspaces, tab environments, scripts, and child-process inheritance
  - [ ] prompt and response bodies are not logged by default; PTY content
    requires explicit scoped authorization, redaction, and bounded lifetime
- Governed forwarding
  - [ ] support provider and local endpoints through destination allowlists,
    policy routing, per-workspace/tab/agent quotas, token and monetary
    budgets, deadlines, retry/idempotency, circuit breaking, health checks,
    streaming cancellation, and policy-controlled fallback
  - [ ] audit provider/model route, latency, actual or estimated token use,
    versioned price basis, cost, policy decision, and denial reason without
    recording credentials or content secrets
  - [ ] prefer provider-reported usage to estimates and reconcile retries so
    cancellation or duplicate attempts cannot silently hide cost
  - [ ] LLM text is never the sole proof of a successful fleet operation;
    MCP tools verify typed post-state through the AgenTerm control plane

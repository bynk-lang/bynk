# 0518 — `bynk-wasm`'s editor entry points take an optional platform name

- **Status:** Accepted (v0.359.1)

**Context.** The three editor entry points were hard-wired to `Platform::Browser`, the playground's platform. Under it, a unit that consumes `bynk.cloudflare` (KV, for instance) draws `bynk.target.vendor_required`. On the desktop, `bynk-lsp` analyses a project for `Platform::default()` (Cloudflare), where the same file checks clean, so the web extension would have shown false errors on ordinary Workers code.

**Decision.** Each of the three exports takes a trailing `platform: Option<String>`, which wasm-bindgen exposes as an optional `platform?: string | null`. `platform_named` maps `"cloudflare"` and `"node"` to their platforms, and anything else, or nothing, to `browser`. The playground's calls are unchanged. The web worker passes `"cloudflare"`. `bynk_compile` keeps its single argument.

**Consequences.** The playground and the web extension share one wasm surface, each analysing for the platform its users target. A project-level platform setting, if one is added, can flow through the same argument.

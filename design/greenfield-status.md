<!-- GENERATED FILE — do not edit by hand.
     Source: cargo xtask greenfield-status (xtask/src/greenfield_status.rs).
     Regenerate with: cargo xtask greenfield-status --apply -->

# Greenfield status

Track slice T0.0 (#999); `ts_writes`/`ts_any` added by P7.0 (#1296); `verbatim_origins`/`verbatim_sites` added by P7.5 (#1307); `incremental_query_types`/`keystroke_latency` added by P8.0 (#1510); `unconsumed_ir_items` added by Slice D3 of the IR cutover (#1542); `diagnostic_coverage` added by #1662. Sixteen probes are gated — a disagreement between this file and a fresh run fails `greenfield_status_table_is_current` (`xtask/tests/greenfield_status.rs`). Five are trend probes, reported only.

| Probe | Gated | Reads |
|---|---|---|
| `workspace_lints` | yes | present — wildcard_enum_match_arm = "warn" |
| `fs_below_driver` | yes | 0 files (bynk-emit=0, bynk-ide=0, bynk-fmt=0) — 0 named floor, 0 residual total |
| `options_sources` | yes | present |
| `hoist_sinks` | yes | 0 |
| `span_keyed_maps` | yes | 4 |
| `emit_diagnostics` | yes | bynk-emit=14/19, bynk-check=524/570 (true/naive) |
| `ide_emit_edge` | yes | absent |
| `ast_importers` | yes | 5 |
| `emit_abi_shapes` | yes | 29 (bynk-browser.ts:FetchOptions, bynk-browser.ts:LogLevel, bynk-cloudflare.ts:FetchOptions, bynk-cloudflare.ts:LogLevel, bynk-cloudflare.ts:negotiateLocale, bynk-node.ts:FetchOptions, bynk-node.ts:LogLevel, cloudflare.binding.ts:Analytics, cloudflare.binding.ts:DataPoint, cloudflare.binding.ts:EdgeCache, cloudflare.binding.ts:KvEntry, cloudflare.binding.ts:KvExpiration, cloudflare.binding.ts:KvGet, cloudflare.binding.ts:KvKey, cloudflare.binding.ts:KvList, cloudflare.binding.ts:KvListing, cloudflare.binding.ts:KvPut, cloudflare.binding.ts:Queue, cloudflare.binding.ts:QueueSend, cloudflare.binding.ts:R2, cloudflare.binding.ts:R2Listing, cloudflare.binding.ts:R2Object, cloudflare.binding.ts:R2Put, cloudflare.binding.ts:RateLimit, cloudflare.binding.ts:AnalyticsDataset, cloudflare.binding.ts:QueueProducer, cloudflare.binding.ts:R2Bucket, cloudflare.binding.ts:R2ObjectMeta, cloudflare.binding.ts:RateLimiter) |
| `ts_writes` | yes | 996 |
| `ts_any` | yes | 28 |
| `verbatim_origins` | yes | 2 |
| `verbatim_sites` | yes | 13 |
| `incremental_query_types` | yes | unit_signature present; shared_cache migrated; stability_test present; definition/project levels absent (deleted by #1537) |
| `unconsumed_ir_items` | yes | 0 |
| `diagnostic_coverage` | yes | unasserted=7 (asserted 634/641) |
| `wildcard_arms` | no (trend) | 356 |
| `keep_in_sync` | no (trend) | 202 |
| `test_density` | no (trend) | bynk=16.0%, bynk-check=14.2%, bynk-driver=24.3%, bynk-emit=12.9%, bynk-fmt=17.1%, bynk-grammar=33.2%, bynk-ide=41.8%, bynk-ir=0.0%, bynk-lower=66.1%, bynk-lsp=38.4%, bynk-project=36.6%, bynk-render=45.2%, bynk-std=49.7%, bynk-strip=55.9%, bynk-syntax=15.3%, bynk-testkit=0.0%, bynk-ts=53.7%, bynk-wasm=45.6%, bynkc=0.0%, xtask=40.0% |
| `fixture_kinds` | no (trend) | contains=3, absent=2, diagnostics=14, error=690, warnings=7, run=125 |
| `keystroke_latency` | no (trend) | not measured — no scheduler exists yet (R3.15, deferred whole this phase) |

## Rules closed

See [`design/greenfield-status-rules.md`](greenfield-status-rules.md) for rule ids closed so far (written by `cargo xtask stamp --apply` at merge; may not exist yet if no increment has cited `closes_rule`).

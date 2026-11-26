<!-- GENERATED FILE — do not edit by hand.
     Source: cargo xtask greenfield-status (xtask/src/greenfield_status.rs).
     Regenerate with: cargo xtask greenfield-status --apply -->

# Greenfield status

Track slice T0.0 (#999); `ts_writes`/`ts_any` added by P7.0 (#1296); `verbatim_origins`/`verbatim_sites` added by P7.5 (#1307); `incremental_query_types`/`keystroke_latency` added by P8.0 (#1510); `unconsumed_ir_items` added by Slice D3 of the IR cutover (#1542); `diagnostic_coverage` added by #1662; `model_readers_beyond_emit` added by crate-hygiene B1 (#2939); `unused_deps` added by crate-hygiene A4 (#2976); `explained_codes` added by #3163; `ts_raw_sites`/`ts_text_idents` added by #3681; `ts_raw_sites` split into `ts_raw_sites_emit`/`ts_raw_sites_testgen` and `emit_thread_locals` added by 1A.19; `emit_silent_fallbacks` added by 1A.34; `emit_print_calls` added by 1A.35b (`lowering_text_bridge` and `stmt_text_lines`, added by #3694 and #3711, were retired at 0 by 1A.35b); `import_prediction_fns` added by 1A.36a; `deploy_kind_accessors` added by 1D.1a; `env_protocol_literals` added by 1D.5a; `tool_spawn_sites` added by 1D.5b; `deploy_ledger_writes` added by 1D.2a; `testgen_print_calls` added by 1B.2a and retired at 0 by 1B.5e, which also returned `emit_print_calls`, `import_prediction_fns` and `emit_silent_fallbacks` to the whole of `bynk-emit/src`; `node_builder_defs` added by 1B.4a; `binding_kind_homes` added by 1C.1b; `wrangler_key_literals` added by 1C.2. Thirty-three probes are gated — a disagreement between this file and a fresh run fails `greenfield_status_table_is_current` (`xtask/tests/greenfield_status.rs`). Five are trend probes, reported only.

| Probe | Gated | Reads |
|---|---|---|
| `workspace_lints` | yes | present — wildcard_enum_match_arm = "warn"; opted in: bynk-emit, bynk-model, bynk-ts |
| `fs_below_driver` | yes | 0 files (bynk-emit=0, bynk-ide=0, bynk-fmt=0) — 0 named floor, 0 residual total |
| `options_sources` | yes | present |
| `hoist_sinks` | yes | 0 |
| `span_keyed_maps` | yes | 6 |
| `emit_diagnostics` | yes | bynk-emit=1/7, bynk-driver=15/19, bynk-check=841/926 (true/naive) |
| `ide_emit_edge` | yes | absent |
| `ast_importers` | yes | 22 |
| `emit_abi_shapes` | yes | 118 (bynk-browser.ts:Secret, bynk-browser.ts:Claim, bynk-browser.ts:withTraceField, bynk-cloudflare.ts:Claim, bynk-cloudflare.ts:Secret, bynk-cloudflare.ts:negotiateLocale, bynk-cloudflare.ts:byteStreamBody, bynk-cloudflare.ts:claimStream, bynk-cloudflare.ts:withTraceField, bynk-node.ts:Claim, bynk-node.ts:Secret, bynk-node.ts:byteStreamBody, bynk-node.ts:claimStream, bynk-node.ts:withTraceField, cloudflare.binding.ts:Ai, cloudflare.binding.ts:AiError, cloudflare.binding.ts:AiSearch, cloudflare.binding.ts:SearchAnswer, cloudflare.binding.ts:Analytics, cloudflare.binding.ts:AnalyticsSql, cloudflare.binding.ts:AnalyticsSqlError, cloudflare.binding.ts:AnalyticsSqlResult, cloudflare.binding.ts:Condition, cloudflare.binding.ts:Container, cloudflare.binding.ts:ContainerError, cloudflare.binding.ts:D1, cloudflare.binding.ts:DataPoint, cloudflare.binding.ts:EdgeCache, cloudflare.binding.ts:Email, cloudflare.binding.ts:EmailError, cloudflare.binding.ts:EmailMessage, cloudflare.binding.ts:FilterOp, cloudflare.binding.ts:ChatMessage, cloudflare.binding.ts:GenerateRequest, cloudflare.binding.ts:Generation, cloudflare.binding.ts:ImageFit, cloudflare.binding.ts:ImageFormat, cloudflare.binding.ts:ImageInfo, cloudflare.binding.ts:ImageResult, cloudflare.binding.ts:ImageStream, cloudflare.binding.ts:ImagesError, cloudflare.binding.ts:Images, cloudflare.binding.ts:IndexInfo, cloudflare.binding.ts:KvEntry, cloudflare.binding.ts:KvExpiration, cloudflare.binding.ts:KvGet, cloudflare.binding.ts:KvKey, cloudflare.binding.ts:KvList, cloudflare.binding.ts:KvListing, cloudflare.binding.ts:KvPut, cloudflare.binding.ts:MetaValue, cloudflare.binding.ts:MtlsCertificate, cloudflare.binding.ts:Mutation, cloudflare.binding.ts:MySql, cloudflare.binding.ts:MySqlError, cloudflare.binding.ts:MySqlRun, cloudflare.binding.ts:MySqlStatement, cloudflare.binding.ts:MySqlValue, cloudflare.binding.ts:OutputOptions, cloudflare.binding.ts:Overlay, cloudflare.binding.ts:PgError, cloudflare.binding.ts:PgRun, cloudflare.binding.ts:PgStatement, cloudflare.binding.ts:PgValue, cloudflare.binding.ts:Postgres, cloudflare.binding.ts:QueryOptions, cloudflare.binding.ts:Queue, cloudflare.binding.ts:QueueSend, cloudflare.binding.ts:R2, cloudflare.binding.ts:R2Body, cloudflare.binding.ts:R2GetIf, cloudflare.binding.ts:R2List, cloudflare.binding.ts:R2Listing, cloudflare.binding.ts:R2Object, cloudflare.binding.ts:R2Part, cloudflare.binding.ts:R2Partial, cloudflare.binding.ts:R2Put, cloudflare.binding.ts:R2Range, cloudflare.binding.ts:R2Read, cloudflare.binding.ts:R2Upload, cloudflare.binding.ts:R2UploadOptions, cloudflare.binding.ts:RateLimit, cloudflare.binding.ts:SearchError, cloudflare.binding.ts:SearchHit, cloudflare.binding.ts:SearchOptions, cloudflare.binding.ts:SqlError, cloudflare.binding.ts:SqlRun, cloudflare.binding.ts:SqlStatement, cloudflare.binding.ts:SqlValue, cloudflare.binding.ts:StoredSecret, cloudflare.binding.ts:TransformOptions, cloudflare.binding.ts:Vector, cloudflare.binding.ts:VectorError, cloudflare.binding.ts:VectorMatch, cloudflare.binding.ts:Vectorize, cloudflare.binding.ts:Version, cloudflare.binding.ts:WorkerVersion, cloudflare.binding.ts:claimStream, cloudflare.binding.ts:EDGE_TO_PREFIX, cloudflare.binding.ts:edgeKeyOf, cloudflare.binding.ts:signQueueMessage, cloudflare.binding.ts:AnalyticsDataset, cloudflare.binding.ts:D1Database, cloudflare.binding.ts:D1PreparedStatement, cloudflare.binding.ts:D1Result, cloudflare.binding.ts:QueueProducer, cloudflare.binding.ts:R2Bucket, cloudflare.binding.ts:R2MultipartUpload, cloudflare.binding.ts:R2ObjectMeta, cloudflare.binding.ts:RateLimiter, cloudflare.binding.ts:SecretsStoreSecret, cloudflare.binding.ts:SendEmail, cloudflare.binding.ts:VectorizeIndex, cloudflare.binding.ts:WorkerAiBinding, cloudflare.binding.ts:WorkerImagesBinding, cloudflare.binding.ts:AnalyticsSqlBinding, cloudflare.binding.ts:AiSearchInstance, cloudflare.binding.ts:WorkerVersionMetadata) |
| `ts_writes` | yes | 486 |
| `ts_any` | yes | 23 |
| `verbatim_origins` | yes | 3 |
| `verbatim_sites` | yes | 9 |
| `ts_raw_sites_emit` | yes | 4 |
| `ts_raw_sites_testgen` | yes | 2 |
| `ts_text_idents` | yes | 0 |
| `emit_thread_locals` | yes | 0 |
| `emit_silent_fallbacks` | yes | 0 |
| `emit_print_calls` | yes | 0 |
| `import_prediction_fns` | yes | 0 |
| `node_builder_defs` | yes | 0 |
| `deploy_kind_accessors` | yes | 0 |
| `env_protocol_literals` | yes | 2 |
| `tool_spawn_sites` | yes | 10 (process control 8, self re-execution 2, other 0) |
| `deploy_ledger_writes` | yes | 1 |
| `binding_kind_homes` | yes | 1 |
| `wrangler_key_literals` | yes | 0 |
| `cloudflare_client_sites` | yes | 0 |
| `incremental_query_types` | yes | unit_signature present; shared_cache migrated; stability_test present; definition/project levels absent (deleted by #1537) |
| `unconsumed_ir_items` | yes | 0 |
| `diagnostic_coverage` | yes | unasserted=37 (asserted 968/1005) |
| `model_readers_beyond_emit` | yes | 7 (bynk-model::EntryHandler, bynk-model::EntryParam, bynk-model::EntryShape, bynk-model::IrHandlerKind, bynk-model::IrHttpMethod, bynk-model::entry_handlers, bynk-model::lower_handler_kind_ir) |
| `unused_deps` | yes | 0 |
| `explained_codes` | yes | explained=896 (of 1004) |

## Trend probes (not gated)

| Probe | Gated | Reads |
|---|---|---|
| `wildcard_arms` | no (trend) | 747 |
| `keep_in_sync` | no (trend) | 222 |
| `test_density` | no (trend) | bynk=35.9%, bynk-check=14.5%, bynk-driver=31.5%, bynk-emit=16.6%, bynk-fmt=21.2%, bynk-ide=41.4%, bynk-lsp=40.9%, bynk-model=52.6%, bynk-project=25.8%, bynk-registry-core=16.8%, bynk-std=43.2%, bynk-strip=44.5%, bynk-syntax=14.1%, bynk-testkit=0.0%, bynk-tests=0.0%, bynk-ts=54.1%, bynk-wasm=52.5%, xtask=37.5% |
| `fixture_kinds` | no (trend) | contains=5, absent=2, diagnostics=17, error=1492, warnings=24, run=377 |
| `keystroke_latency` | no (trend) | edit-to-diagnostics median: 10 contexts 5.8 ms, 50 contexts 20 ms, 200 contexts 90.5 ms, 500 contexts 262.4 ms; debounce excluded; budget p95 < 100 ms at 50 contexts: met (24.4 ms); scheduler yardstick p95 > 250 ms at 200 contexts: not reached (independent 103.1 ms, linked 220.3 ms) (v0.537.0, design/benchmarks/v0.537.0.json) |
| `module_lines` | no (trend) | ≥2000: 29 (≥3000: 25, ≥5000: 11); largest: bynk-syntax/src/diagnostics.rs 10949; back half: bynk-emit/src/emitter/emit.rs 10717, bynk-emit/src/emitter/lower.rs 4884, bynk-driver/src/project.rs 4312; back half ≥3000: 8; ceilings: 10 |

## Rules closed

See [`design/greenfield-status-rules.md`](greenfield-status-rules.md) for rule ids closed so far (written by `cargo xtask stamp --apply` at merge; may not exist yet if no increment has cited `closes_rule`).

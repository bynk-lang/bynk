<!-- GENERATED, APPEND-ONLY — written by `cargo xtask stamp --apply` (xtask/src/stamp.rs) at merge, one row per growth probe an increment raises with its `raises` frontmatter (#3679).
     `from` is the probe's committed leading count before the stamp and `to` the live count the stamp commits; a PR that moves no growth probe upward writes nothing here. Do not hand-edit. -->

# Growth-probe raises

| Probe | From | To | Version | PR | Reason |
|---|---|---|---|---|---|
| ts_writes | 486 | 488 | v0.806.0 | #4519 | the test double of `Ai` carries one text write, the `generateJson` member that `Ai.generateValue[T]` is lowered over, which is not an operation of `Ai` and so cannot be a stub clause (#4099) |
| ts_writes | 486 | 488 | v0.800.0 | #4508 | `object_key` quotes a wire name that is not an identifier as a TypeScript string literal, with one `format!` for a control character's escape; the name is data from the declaration, not authored TypeScript text |
| ts_writes | 481 | 486 | v0.792.0 | #4503 | `checked_callees` in bynk-driver builds a refusal message with `format!`, an error-message line the text-level scanner counts as a TypeScript write (the same one-site over-count the probe documents for struct-field labels); it writes no TypeScript (#4105) |
| ts_writes | 481 | 486 | v0.791.0 | #4481 | the stored-record rename statement names its `typeof <src>` cast type with `format!`, as the neighbouring emitted names are; no TypeScript text is written |
| ts_writes | 481 | 486 | v0.785.0 | #4464 | the `writeValue` lowering builds its data point as a TypeScript node and one text write of the `Cap.writeValue` trace-step target, the first typed-point lowering over `write` (#4105) |
| emit_abi_shapes | 115 | 118 | v0.776.0 | #4472 | the platform bindings now import `Secret` from the bynk module to wrap what they read, three more binding-to-emitter name couplings (#4111) |
| ts_writes | 481 | 486 | v0.776.0 | #4472 | the Secret runtime wrapper's constructors (`__bynkSecret(value) as Secret`) are two more emitted casts, in `Secret.of` and `unsafe`; there is no typed node for a branded wrapper yet (#4111) |
| ts_writes | 481 | 486 | v0.771.0 | #4452 | two new codec identifiers (`__<field>$ms`, and the map decoder's self-call) are built with `format!`, as the neighbouring codec names are; no TypeScript text is written |
| emit_abi_shapes | 109 | 115 | v0.764.0 | #4435 | the `bynk.cloudflare` binding's `WorkersQueue` imports three more runtime names (`signQueueMessage`, `edgeKeyOf`, `EDGE_TO_PREFIX`) to sign a Bynk producer's messages under the call edge's key |
| emit_abi_shapes | 109 | 115 | v0.760.0 | #4404 | the three Logger bindings import `withTraceField` from the runtime to put the request's trace on a structured record, and each is one more name in the bindings' runtime surface |
| ts_any | 21 | 23 | v0.759.0 | #4422 | the identity route's verify result and run step carry the actor's principal, whose concrete type (the handler's own actor-deps shape) cannot be spelt at that seam; two `any` sites, the same shape the event wrapper keeps (#3926) |
| ts_writes | 475 | 481 | v0.759.0 | #4422 | the verify step's return type and the entry's refusal guards are spelt as TS text, the same shape the neighbouring actor wrappers keep (#3926) |
| ts_writes | 475 | 481 | v0.755.0 | #4432 | the generated client gains an EventSource function per streaming GET route (#3918) |
| ts_writes | 475 | 481 | v0.751.0 | #4425 | the queue entry builds a sum consumer's resolved actor (`const __who = { tag: … }`) from the verified signer, one more generated-TypeScript write, as the call entry does for its sum |
| ts_writes | 475 | 481 | v0.749.1 | #4421 | the emitter spells a qualified type's codec helper names (`__serialise_lib_x_Status`) with a `format!` in `codec_names`, as the other codec names are spelt (#4403) |
| ts_writes | 473 | 475 | v0.749.0 | #4420 | Streaming gains an optional heartbeat interval and the SSE body writes comment lines while idle (#3918) |
| ts_writes | 473 | 475 | v0.745.0 | #4392 | the SSE framer in the runtime's http.ts now frames a structured event (#3918) |
| emit_abi_shapes | 104 | 109 | v0.742.0 | #4406 | the `WorkersR2` binding names five more types (`R2GetIf`, `R2List`, `R2Partial`, `R2Range`, `R2Read`) that the new operations take and return (#4049) |
| ts_writes | 473 | 475 | v0.730.0 | #4376 | two more HttpResult statuses in the runtime's status tables (#3917) |
| ts_writes | 473 | 475 | v0.727.0 | #4367 | the qualifier's namespace text, as for ts_raw_sites_emit (#4045) |
| ts_writes | 471 | 473 | v0.692.0 |  | the security policy literal carries two more typed string fields, the HTML CSP and referrer policy (#3923) |
| ts_writes | 470 | 471 | v0.674.0 | #4249 | the typed headers decoder is built as typed nodes in the entry emitter (#3911) |
| ts_writes | 467 | 470 | v0.670.0 | #4241 | the typed path-parameter decoder and the enum segment of the system driver URL are built as typed nodes in the entry and driver emitters (#3910) |
| ts_writes | 462 | 467 | v0.662.1 |  | the route's `applyChallenge` wrap and the upgrade's challenge array are built as typed nodes in the entry emitter (#3914) |
| ts_writes | 458 | 464 | v0.640.70 |  | 6 `format!` lines of hand-laid JSON (the group inspect listing and agents manifest, which `serde_json::to_string_pretty` wrote before the code moved into `bynk-emit`); they write JSON, not TypeScript |
| ts_writes | 514 | 517 | v0.640.60 | #4070 | the block renderer's three `format!` calls moved from the CLI (`bynk/src/deploy/config.rs`) into `bynk-emit` (1C.2); they write TOML, not TypeScript, and are not new |
| testgen_print_calls | 43 | 44 | v0.640.42 | #4055 | moved from production emission (1B.2b), not new |
| ts_raw_sites_emit | 4 | 6 | v0.640.38 | #4030 | reclassified from ts_raw_sites_testgen: the production commons barrel (sum unchanged) |
| verbatim_sites | 13 | 14 | v0.640.1 |  | the `Form.decode`/`Form.encode` field-spec table is `workers_entry` source text for the router's own array literal, so it is one foreign-text carrier (`VerbatimOrigin::FormFieldSpecs`, built at the single site `form_field_specs`) (#3736) |
| ts_any | 39 | 40 | v0.639.30 | #3764 | the list kernel's two `any` fallbacks (a checked `traverseTry` shape and a `sequenceAll`/`sequenceTry` shape that are absent) were the text `any`, which the probe does not see; they are now one `TsType::named("any")` in `lower_list.rs` (`any_ty`), kept at the same points as #3719 Decision D requires, and fall when the `1A.fallback` slice removes them |
| ts_any | 38 | 39 | v0.639.28 | #3762 | the node form of the test-scaffold refined brand (`unchecked_construct_test_node`, `TsType::named("any")`) sits beside the text `unchecked_construct_test`, which the driver's `property_generators.rs` still calls; the cast itself is unchanged (P7.2) and the count falls when those callers convert |

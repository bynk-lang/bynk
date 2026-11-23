---
level: patch
changelog: Internal restructuring: the last predicate, call and cast text in the emitter is built as nodes; no behaviour changes
probes: ts_writes
---
refactor: the remaining predicate, call and cast text becomes nodes; the audit's class list is deleted (1A.40c)

Closes #3996. Part of #3676.

No behaviour changes; generated output and source maps are byte-identical. The last classes of text `Ident` that the identifier audit listed are built as nodes: the `via schema(N)` guard (`schema_version_test`) and the event-pattern guard (`event_pattern_guard_ir`), the agent's `(this as unknown as Record<..>)[methodName]` and `this.__env` casts (`record_cast`), the plain-JSON test, the `on rejected` arrow, and the harness's two `(detached ..)` conditionals. The claim predicate moves from `bynk-check` (`claim_predicate_to_js`, deleted) to `claim_predicate_node` in `bynk-emit`, so the 403 guard is a tree. The codec parameters `path: string = "$"` and `depth: number = 0` are typed `TsParam`s with defaults. `TsType::Fn` gains a `rest` flag so the agent method table prints `(...bynkArgs: unknown[]) => unknown`.

The audit's `TEXT_CLASSES` mechanism is deleted: `ident_audit` fails on any text `Ident`, and every audited module has none. The test harness's `tests/*.test.ts` modules stay outside the audit until 1B.5.

Carried from #4069: the `hist_member` nit (the dotted `s.{path}` text in the state encoder), agreement tests for the event and generic-parameters paths of the wire-default validator and tree, and the probe-narrowing wording of `node_builder_defs` (zero-parameter helpers such as `plain_json_test` and `deserialise_path_param` are constants).

Not in this slice (Decision F): `expr_contains_await`'s text fallback and the printer's `debug_assert!(is_identifier_path(name))` go to 1B.5e; #4054's done-when carries them.

`ts_writes` falls by 14 (570 to 556); no other gated probe moves.

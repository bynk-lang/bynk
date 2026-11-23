---
level: patch
changelog: Internal restructuring: the test harness builds a stub class and the platform capability doubles of a generated test module as typed nodes instead of text, so a stub class's methods are separated by a blank line; no behaviour changes
probes: ts_raw_sites_testgen, testgen_print_calls, ts_writes
---
refactor: the stub classes are nodes (1B.5c)

Part of #4054. Part of #3676.

A stub class (`__Stub_<Cap>`) is a `TsDecl::Class` node: its per-call cursor fields, its constructor, `__applies`, and one method per stubbed operation. Each method's body is statements. `emit_stub_rhs` and `lower_stub_value_block` return statements, so the `returns each` switch's case bodies, the `after` closure, the argument-pattern closures, the suite declarations, the late codecs and the fixture aliases are nodes, and `reindent_by` and the text `suite_late_codecs` are deleted. `scopes_test` builds the case and suite test as an expression. `bynk_ts` gains one thing: a class field may carry an initialiser and omit its type (`TsClassField::inferred`), for `__seq_0 = 0;`.

The platform capability doubles (`platform_double`, `env_secrets_double`) are `TsDecl::Function` nodes. Each body stays one authored text statement that declares the runtime and sibling names it spells (`double_uses`), as #4054 Decision A rules: it is test-double code written to be read as TypeScript. The two doubles with parameters (`RateLimit`, `Vectorize`) have them as typed parameters with defaults.

The text paths this leaves dead are deleted: `extend_printed_at`, `lower_block_to_async_body` and `emit_block_as_function_body_with_return`. `LowerCtx`'s source-map recording and `lower_stmt::render` stay for the tests that compare the node form against the text it replaced (`#[cfg(test)]`).

Goldens: see the PR.

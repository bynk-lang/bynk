---
level: patch
probes: ts_writes, stmt_text_lines
changelog: The code generator builds `match` in tail and value position as switch, if-chain and arrow nodes where it can, with tail-position output and source maps unchanged
---
refactor(emit): `match` lowers to nodes, tail and value position (1A.13)

`TsSwitchCase` gains `labels: Vec<TsExpr>` (empty is `default`; shared labels print stacked, the brace on the last) and an optional `span` that `print_stmt_with_spans` records at the first label line. The tail `switch`, the tail `if` chain and the `match` IIFE (an `Arrow` over `__d`, `Await` when async) are built as nodes when the discriminant, every pattern test, every binding and every arm body are nodes; otherwise today's text is emitted byte for byte. `pattern_match_tests` returns nodes, `emit_pattern_bindings` returns `Const` nodes and `literal_case_label` has a node twin; their text forms stay for `is` bindings (1A.14). Tail-position source maps are identical (the discriminant mark goes to the first statement, each arm mark to its `TsSwitchCase.span` or `If`); the IIFE is unmapped, as 1A.12's.

Also folds in the 1A.12 review's printer depth fixes (`Spread` entries and multi-line array items now render a multi-line arrow at the right depth). The compact-statements branch of `render_block_body` has the same fix but is unobservable (a debug assertion forbids multi-line statements there), so it has no test.

Layout only changes in the IIFE and in arrows still inside a text parent: one positive fixture, `2712_record_pattern_lambda_params`, changes whitespace. `209_option_result_kernel` and `1800_state_write_in_match_guard` are unchanged because their text parents (a kernel call, a state-write guard) are converted by later slices.

Gated probes: `ts_writes` (1477 to 1460) and `stmt_text_lines` (59 to 40) fall as the `match` statement producers build nodes. `lowering_text_bridge` stays at 111: the text fallbacks keep their construction sites, and the new pattern-test fallbacks leave through one `From<Built> for LoweredExpr` exit instead of new sites.

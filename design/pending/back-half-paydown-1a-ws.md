---
level: patch
changelog: The closure a service or agent handler body runs in, the contract-guard closure and the subscriber and schema prologue guards are typed nodes instead of text lines; the generated code is unchanged apart from indentation, the body of an events-emitting service handler now sitting one step deeper inside its closure
probes: stmt_text_lines, ts_writes
---
refactor(emit): the handler wrapper lines become nodes (1A.ws, prerequisite to #3736)

Part of #3736. Part of #3676.

`result_closure_open`/`result_closure_close`, the contract-guard closure and the two `if (!(guard)) return undefined;` prologue guards were `Item::line` text, because the wrapped body sat at the open line's indentation. They are now a `const` over an (awaited) arrow IIFE whose statement body is the wrapped body, and an `if` with a `return`. The `Item::line` helper is deleted. `Item::Line` stays for 1A.35b.

---
level: patch
changelog: Extract-function re-checks its edit before offering it, refuses a selection containing `?`, and says why a capability-using selection can't be extracted
---

## ADR: lsp-extract-function-stays-capability-free
title: Extracted functions stay capability-free
summary: Extract-function never lifts a capability-using selection (a `fn` has no `given`) and says why; it never lifts a `?`; it re-checks its edit lazily

**Context.** Issue #2100's last slice asked extract-function to carry the
selection's required `given` capabilities into the new function. The
extracted code becomes a top-level `fn`, and a `fn` has no `given` clause
(`FnDecl` has none, and the grammar allows none). So carrying the capabilities
would need either a language change (`given` on `fn`) or a different
extraction target. Extract-function (tracks #800/#813) already refused any
selection with a recorded capability requirement, but it did so silently.

**Decision.** Extracted functions stay capability-free (the user's decision on
#2100). A selection that uses a capability is refused as before, and the
refusal now carries a reason naming the capabilities and saying a `fn` has no
`given` clause to carry them. A client that advertises
`codeAction.disabledSupport` gets a disabled "Extract function" action
showing it. A selection containing a `?` outside a lambda is refused the same
way: lifted into a new `fn`, the `?` would return from that `fn` instead of
the one it was written in.

Like the other lightbulb actions (ADR `lsp-lightbulb-actions-resolve-lazily`),
extract-function is now listed without an edit. `codeAction/resolve`
re-plans it against the committed round and offers it only if the edited
project re-checks clean (DECISION B).

**Consequences.** No language change. Capability-using code is extracted by
hand into a handler, provider or agent, which can declare `given`. Extract-
function can no longer offer an edit that fails to type-check, at the cost of
a project analysis when it is picked. A client without disabled-action
support sees no extract-function action on a refused selection, as before.

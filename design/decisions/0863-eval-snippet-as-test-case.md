# 0863 — A snippet is evaluated as a synthetic test case that reports its value

- **Status:** Accepted (v0.629.3)

**Context.** A notebook cell, or any tool that wants to try an expression against a project, needs a value back. Nothing ran Bynk code outside `bynk test` and `bynk dev`, and a doc example marked `bynk test` only passes or fails (#3585). [ADR 0638](0638-refined-explorer-evaluation.md) considered a `bynk eval` driver command for the refined-type explorer and rejected it, because a hover is an implicit request and must not build and execute the project.

**Decision.** A snippet is the body of one `case` in a synthetic `suite` that targets a chosen `commons` or `context`, built in memory beside that unit's file and run through `bynk test`'s own build and runner, as a doc example is. The case's final expression is not discarded: it is encoded as the wire shape `Json.encode` gives it, and written with its type as one `eval-value` event on the runner's NDJSON stream, which the document reports as `eval`.

1. *Same scope as a suite.* The snippet sees the unit's declarations and the `stub`s it writes. Its leading `consumes` and `stub` lines are the suite's own; a snippet `consumes` what it uses from other units, as a suite file does (ADR 0849).
2. *Stateless.* Each evaluation is independent. A client that wants an earlier cell's `let`s sends them again ahead of the snippet.
3. *No real I/O.* The test runtime refuses a platform capability that reaches outside (`Fetch`, a database, a secret store) when it has no `stub`, and the evaluation reports the refusal; `Clock`, `Random` and `Logger` run as the in-memory doubles every test gets. Only the unit's own capabilities are in scope.
4. *Bounded.* A run is killed after a time limit and reported as timed out.
5. *Explicit and local, not implicit.* An evaluation is a request to run code the user wrote in their own workspace, at the trust level of `bynk test`. ADR 0638 is not overturned: the refined-type explorer keeps evaluating compiler-generated predicates in the editor, because a hover must not build and run a project, while a notebook cell is an explicit instruction to do so.

**Consequences.** No grammar, checker or runtime-library change. The one lowering change is that a case named `bynk eval` reports its tail. The command line, JSON result and time-limit flag follow. The build is `bynk test`'s, but the project's own suites are left out of it by default, so a suite that does not compile cannot block an evaluation; `--suite` builds the target unit's own suite in and puts the snippet inside it, so the suite's `stub`s and helper `fn`s apply, and its errors are then reported. The name `bynk eval` is reserved for the synthetic case: a user-written case of that name would print one extra event line and add `eval` to the JSON document.

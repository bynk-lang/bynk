# 0531 — Against a real runtime, a failing `expect` is retried by a static replay rule, up to `--settle`

- **Status:** Accepted (v0.370)

**Context.** End-to-end testing track §3.6 (#2138, DECISION C). In process, ADR 0478 and 0467 settle every send and the fan-out before the next line runs. A real runtime doesn't, so a correct case can see a write a moment late, which is why `bynk-lang/shop` polls its ledger.

**Decision.**

- **No new syntax.** ADR 0155 D2 rejected `eventually`/`always` keywords, and this adds none. It is a runner policy: `--settle <Duration>`, default `5s` against a real runtime. A remote build alone emits the retry, so in process nothing changes.
- **The rule is static.** The emitter classifies each top-level statement of a remote case. An address whose method is safe per RFC 9110 (`GET`, `HEAD`, `OPTIONS`, or an asset fetch) is *safe*; any other address (`POST` and friends, `schedule`, `message`) is *unsafe*. The classification fails closed. An effectful statement (`<-`, `do`, `~>`, `:=`, `advance`) is safe only when its value *is* a safe address and holds no other address. Anything else is unsafe, including an `if`, a `match` or a helper call around an address. Only a pure `let` and an `expect` are replayed with the reads, and either one is unsafe if it holds an unsafe address (an `expect … faults` on a `POST`). The bindings an attempt returns come from the AST (`let` names, as emitted). The segment's text moves in unchanged, so a template literal keeps its contents. The statements from the last unsafe address (or the case's start) through the last `expect` before the next unsafe one form one segment, if a safe address precedes that `expect`. Each such segment is re-run as a unit until every `expect` in it holds or the deadline passes. Its bindings are returned out of the attempt, so the statements after it are unchanged. An unsafe address is never replayed, and an `expect` with no safe address before it isn't retried.
- **Reporting.** Each case's retries are counted and reported (`retries` under `--format json`), so an ordering bug shows up as a retry count rather than hiding behind a pass.

**Alternatives.** An explicit `eventually within … { }` block was rejected: it puts latency into the case body and re-opens ADR 0155's question. Settling through a test-only drain endpoint was rejected, because it needs a new endpoint in what is deployed. Prior art: Playwright's auto-retrying assertions.

**Consequences.** A remote build's case bodies carry no source map, because the rewrite moves the text. Remote runs aren't stepped through in an editor.

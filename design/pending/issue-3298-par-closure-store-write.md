---
level: minor
probes: emit_diagnostics, diagnostic_coverage, explained_codes
changelog: A closure passed to `parTraverse`, `parTraverseAll` or `parTraverseTry` may not write a `store` (`bynk.par.store_write_in_closure`), extending the `par` block's rule that store writes stay sequential
breaking: `parTraverse`, `parTraverseAll` and `parTraverseTry` closures that write an agent's `store` (`put`, `remove`, `update`, `upsert`, `add`, `append`, or `:=`) are now refused with `bynk.par.store_write_in_closure`: run the loop with `forEach` (or `traverseAll`/`traverseTry`), or do the writes after the parallel part.
---

## ADR: par-closure-store-write
title: A `parTraverse`-family closure may not write a store
summary: Track Q4's rule (store writes stay sequential) extends from `par { }` to the closures of `parTraverse`, `parTraverseAll` and `parTraverseTry`

**Context.** A `par { }` block refuses a binding that is a store write (track Q4, `bynk.par.invalid_statement`), but the closure given to `parTraverse`, `parTraverseAll` or `parTraverseTry` was not checked. A closure that read a store field and then wrote it (`get` then `put`) lost updates across branches, because every `<-` is an await point and the branches interleave between the two calls: three branches incrementing one key left it at 1. A single-call `update` or `upsert` stayed atomic per key. Proposed in #3298, which refers to #3297. Related: ADR 0109 (the handler is the atomic unit; writes stage in memory), ADR 0171 and ADR 0679 (the `parTraverse` family and its settle-all semantics).

**Decision.**

- **A. The rule.** A store write in a closure passed to `parTraverse`, `parTraverseAll` or `parTraverseTry`, on a `List` or a `Query` receiver, is the error `bynk.par.store_write_in_closure`. The sequential `forEach`, `traverseAll` and `traverseTry` are unchanged, and so is `par { }`.
- **B. What counts as a write.** The operations Q4 refuses: `put`, `remove`, `update`, `upsert`, `add`, `append`, and `:=`. `update` and `upsert` are included, so the closures and `par { }` stay equal. Allowing them later is a loosening, made for both together, and breaks nothing. Reads, a store as the combinator's receiver, and calls to other agents are allowed.
- **C. Reach.** The whole lambda, nested lambdas included, plus a lambda bound by `let` to a name and passed by name. A store is reachable only from a handler body (a free `fn` cannot name one), so no inter-procedural analysis is needed.
- **D. Combinators.** The three `parTraverse` operations only. No `parMap`, `parSequence` or `parZip` exists.
- **E. Diagnostic.** A new error code with a `bynk explain` entry. Its notes point to `forEach` (where `update` and `upsert` stay atomic per key) and to collecting with `parTraverseAll` and writing afterwards.
- **F. Alternatives rejected.** Refusing only a closure that reads and writes one field (inconsistent with Q4, and blind to a stale read taken outside the closure); a warning (the failure is silent data loss, and `par { }` already refuses); runtime serialisation (store operations are already atomic, and the lost update is the await between two of them).

**Consequences.** A program that wrote a store inside such a closure stops compiling; no example, fixture, test or Book snippet in the repository did. The rewrite is `forEach`. On main it gives up no concurrency, because a store write is a synchronous in-memory stage; the diagnostic and the Book do not promise that, since it should be re-checked once the storage-backed handles land (#3195 part 2b). A lambda `let`-bound to a name is recorded by name, not by scope, and an alias (`let g = f`) is followed; rebinding a writing lambda's name in an inner block can therefore hide or add a refusal, which scoping the record would fix.

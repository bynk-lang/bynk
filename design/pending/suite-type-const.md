---
level: minor
changelog: A suite declares its own `type` and `const` items, private to the suite and never deployed
probes: diagnostic_coverage, emit_diagnostics, ts_writes
---

## ADR: suite-local-type-and-const
title: A suite declares its own private, test-only `type` and `const` items
summary: Suite-local types and constants, scoped like suite helper fns, with codecs emitted into the runner

**Context.** A suite body admitted `uses`, `stub`, `env`, `fixture`, `fn`, `case` and `property`, but not `type` or `const` (#3066). A test that wanted to read a count back through `select[T]` had to add a test-only row type to production code, or compare raw JSON; fixture constants were repeated or wrapped in zero-argument `fn`s. ADR 0718 gave suites private helper `fn`s on the same argument: test-only items must not widen the deployable.

**Decision.**

- **A. Suite-local scope.** A suite (fragment or block form) may declare `type` and `const` items. They are visible to that suite's cases, properties, helper `fn`s and each other, in any order; invisible to the target, to every other suite (even of the same target) and to the deployable. Another suite may declare the same name. Sharing across suites (a test-only commons) stays open.
- **B. No shadowing.** A suite `type` or `const` named like anything already bound in the case scope (a function, type, constant, embed, asset, service or agent of the target or a used commons, or a sum variant) is `bynk.suite.type_name_clash` / `bynk.suite.const_name_clash`; a name repeated within the suite is `bynk.suite.duplicate_type` / `bynk.suite.duplicate_const` (a `type` and a `const` of one name count as a duplicate: the value namespace is single). A suite helper `fn` that reuses a suite item's name is the existing `bynk.suite.fn_name_clash`.
- **C. Checked as a unit's own are.** The declarations go through the same checks a context's do: name resolution (`bynk.resolve.unknown_type`), `check_type_decl`, record field defaults, and for a constant its upper-camel name, the closed static-initialiser set, and cycles. A suite type must be non-generic (`bynk.suite.type_generic`) and may not carry methods (a method `fn` stays `bynk.suite.fn_method`).
- **D. Emission into the runner, not the module.** Each test runner declares its suite's types, their JSON codecs and its constants as local declarations ahead of the helpers, with `export` dropped. A name therefore cannot collide with another suite's in the same test module, and the deployable's emitted output is byte-identical with or without suite items. The codecs cover the type and its `List`/`Option` wrappers, the shapes `select[T]`, `firstAs[T]`, `Json.decode[T]` and `Json.encode` reach; a codec root that names a suite type is filtered out of the module-level closure, which cannot name a type declared inside a runner.
- **E. Where they apply.** Unit, integration and system cases, properties and helper `fn`s. A suite-scoped `stub` value cannot name a suite `type` or `const` yet (its values are emitted outside the runner); this is a follow-up, not a restriction on cases.

**Consequences.** A test reads rows back through `select[T]` into a test-only record, shares fixture values as constants, and, with a `const` accepted as SQL (#3065), shares its SQL. Each runner repeats its suite's types and codecs in the generated test output, the same trade ADR 0718 made for helpers. A suite type used under a generic other than `List`/`Option` at a JSON boundary fails `tsc` on the generated runner rather than producing a diagnostic; widening the local codec roots is a follow-up if it is wanted. The parser, formatter, tree-sitter grammar and editor index admit `type` and `const` as suite members.

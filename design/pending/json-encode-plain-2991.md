---
level: minor
changelog: "`Json.encodePlain(v)` writes `Option` as `v` / `null`, the shape a `json { options: plain }` route writes (#2991)"
probes: emit_diagnostics, ts_writes, diagnostic_coverage
---

## ADR: json-encode-plain
title: `Json.encodePlain` writes the plain JSON form of `Option`
summary: A separate intrinsic, `Option` only, reusing the plain-route ambiguity rule, concrete types only

**Context.** ADR 0723 gave a service's HTTP response bodies a plain JSON form (`Some(v)` as `v`, `None` as `null`) and left `Json.encode` tagged. A Bynk program still could not write the JSON a non-Bynk API sends or expects: tests that stub an external API at the wire, and request bodies sent with `Fetch.send`. Proposed in #2991.

**Decision.**

- **A, spelling.** A separate intrinsic, `Json.encodePlain(v)`, with `Json.encode`'s signature. Not a second argument (no mode value exists in expression position) and not an options record (a type and a literal for one two-valued choice).
- **B, scope.** `Option` only, exactly ADR 0723's meaning of plain. Sums, `Result` and `Map` keep their tagged shapes, so a type has one plain text under `encodePlain` and under a plain route.
- **C, injectivity.** `bynk.json.nested_option_plain` applies to the argument's type (an `Option` whose payload can itself encode as `null`), by the same rule and function as a plain route's response type.
- **D, generics.** A type-parameter-bearing argument is refused (`bynk.json.encode_plain_generic`): the generic codec witness always writes tagged. Making the witness read the flag is a compatible follow-up that lifts the refusal.
- **E, capability writes.** `Queue.sendValue*`, `Events` and agent state stay tagged (their consumers are Bynk). An outbound HTTP body is the text the author builds for `Fetch.send`, so it uses `Json.encodePlain`.
- **F, decoding.** Unchanged; the lenient decoder (ADR 0450) reads both forms.
- **G, snapshots.** `expect … matches snapshot` stays tagged.

**Mechanism.** One codec with the mode flag of ADR 0723. A call lowers to the monomorphic serialiser inside an inline arrow that raises `globalThis.__bynkPlainJson` and restores it in a `finally`, so no module needs a new runtime import. The project's `Option` encoders carry the (inert unless raised) plain branch when a route is plain or when any source file mentions `encodePlain` (a textual over-approximation computed once per build, in the driver); test suites get the same switch. A project with neither is byte-for-byte unchanged.

**Consequences.** `Json.decode(Json.encodePlain(v)) == Ok(v)` for every `v` the checker admits. Hover and completion list the new member; semantic tokens are unchanged (an ordinary member access). An integration suite does not yet switch its own encoders on, because that module emitter does not draw its own codec closure (the same standing limit as `Json.encode` there).

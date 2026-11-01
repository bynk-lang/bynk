---
level: patch
probes: emit_diagnostics, diagnostic_coverage, ts_writes
changelog: "A named constant is emitted as an exported, hoisted TypeScript function that caches its value, and each reference is a call, so constants in sibling files that import each other (through a constant, a function or a type) no longer risk reading a constant before it is initialised; `bynk.resolve.const_file_cycle` is retired ([#2617](https://github.com/bynk-lang/bynk/issues/2617))"
---

## ADR: const-hoisted-functions
title: Named constants emit as hoisted, cached functions
summary: Supersedes ADR 0678's emission as an exported `const` and its refusal of file cycles through constants: each constant is a hoisted function that caches its value, every reference is a call, and module evaluation order no longer matters

**Context.** ADR 0678 emitted each named constant as `export const Name: T =
<init>;`, in dependency order within a file, and refused a cycle of files
through constant-to-constant references as `bynk.resolve.const_file_cycle`.
Review of #2661 found that the refusal was incomplete. Each file of a unit
emits as a module that imports what it names from its siblings, and those
imports can form a cycle through a function or a type import as well as
through a constant. An ES module cycle can then evaluate one module's
initialiser before a sibling's `const` is initialised, a `ReferenceError` at
load, which the constant-only file graph did not see.

**Decision.**

- **A. A constant is a hoisted, cached function.** Each constant emits as:

  ```ts
  var __const_MaxRetries: { v: number } | undefined;
  export function MaxRetries(): number { return (__const_MaxRetries ??= { v: 3 }).v; }
  ```

  A function declaration is hoisted, and a `var` reads as `undefined` rather
  than throwing before its statement runs (a `let` would itself be in the
  temporal dead zone), so a constant can be read before its module has
  evaluated. The initialiser runs once, on the first call; the `{ v }` box
  keeps a falsy or `null` value cached. The doc comment sits on the function,
  which is what an importer names.
- **B. Every reference is a call.** The checker records a bare name that
  resolves to a constant (and not to a local that shadows it) as
  `Callee::Const`, and the emitter lowers it to `Name()`. An `is` receiver
  rooted at a constant is lifted to a temporary.
- **C. Declaration order.** A module's constants follow its types and precede
  its functions, in declaration order; the dependency ordering of ADR 0678 is
  no longer needed.
- **D. `bynk.resolve.const_file_cycle` is retired.** Constants in sibling
  files may name each other across files, and the files may import each other
  through functions and types too. A genuine cycle among constants is still
  `bynk.resolve.const_cycle`.
  - **Rejected:** a reachability check over the whole sibling import graph. It
    would refuse ordinary programs whose files call each other, and would be
    harder to explain than a rule that removes the hazard.
  - **Rejected:** restricting an initialiser to constants of its own file. It
    removes a useful form (a shared base value in one file, derived values in
    another) to work around an emission detail.

**Consequences.** Module evaluation order no longer constrains where a
constant may be declared or read. A bundler can no longer inline a constant's
value, and each read is a call that returns the cached value. Output for
programs without constants is unchanged; the 2617 constant fixtures re-bless,
and the former negative fixture `2617_const_file_cycle` is now a positive,
behavioural one covering cross-file references, functions calling across the
cycle and falsy cached values.

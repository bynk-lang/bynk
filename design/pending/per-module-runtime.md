---
level: patch
changelog: Decision (#2426) — the runtime ships as per-module files, and a project emits only the modules its graph reaches; no helper moves into `bynk-std`
---

## ADR: per-module-runtime
title: The runtime ships as per-module files and a project emits only the modules it reaches
summary: Direction 1 for the compiler-ABI runtime (separate `__bynk_runtime/<module>.{ts,js}`, direct imports, reachable-closure emission); Direction 2 rejected — the pure helpers are compiler-ABI by use, not standard-library surface; `wire-client` and the coverage rule are unchanged in kind

**Context.** Every compiled project emits all of `runtime.js` (about 3,300
lines of bundled TypeScript; 2,594 of JavaScript in the playground) beside a
program that may import none of it (#2426). `bynk-emit/runtime/README.md` makes
this a contract: one flat file, "not a tree-shaking bundler", so emitter and
runtime stay lockstep and `wire-client` can be concatenated from the same
modules. The cost lands in the playground (a user's code is buried in support
code), in `bynkc --emit js` size, and in Worker parse and cold-start time.

Two facts shape the choice. First, the `@bynk/runtime` modules are already
nearly independent: the only edges are `bytes`→`result`, `equality`→`bytes`,
`boundary`→`result`/`errors`, `http`→`boundary`/`result` (types only),
`agent`→`storage`/`boundary`/`result`, `auth`→`result`, `connection`→`result`/
`storage`. The flat file is a packaging choice, not a coupling. Second, the
emitter already knows which runtime names a module uses: `RuntimeUse` records
the conditional groups (`Bytes`, `__bynkEq`, ICU, the `Int` traps) from the
producers, and every module imports through the `__` aliases (#1653) in one
import line (`runtime_import_for`). The needed set is therefore a fact emission
already produces, not something to recover by scanning text.

The runtime has two kinds of content, and the issue's two directions address
different ones:

- **Compiler ABI** — `Result`/`Option`, the validation and boundary error
  records, the boundary protocol, the storage seam, the agent lifecycle,
  `HttpResult`, the auth and webhook seams, `Connection`s. Generated code calls
  these because the compiler wrote the call; a program never names them.
- **Pure helpers** — `__bynkBytes*` codecs, `__bynkEq`, `__bynkIntDiv`,
  `__bynkToInt`, the ICU formatters. Small, but the compiler still emits the
  call (`==`, `/`, a `Bytes` kernel op, a message `render`), not the user.

**Decision.**

- **A. Direction 1 for the whole runtime: separate files, direct imports.**
  The `@bynk/runtime` modules are emitted as `__bynk_runtime/<module>.{ts,js}` (the extension follows the target's `ImportExt`, as `runtime.ts`/`runtime.js` does today) (one file per
  existing module — `result`, `bytes`, `equality`, `int`, `errors`, `storage`,
  `boundary`, `http`, `queue`, `agent`, `auth`, `connection`, `messages`,
  `locale`, `wasm`), each importing its siblings by relative path. Generated
  code imports each helper from the module that defines it, so the import line
  of an emitted file becomes one line per module it uses. The module is the unit
  of inclusion, so no bundler or dead-code analysis is added; a module is in or
  out. `aliases.ts` stops being a re-export barrel and becomes the alias
  declarations of each module's own file (the `__` names are still the only
  names emitted code imports; #1653's collision argument is unchanged).
  The directory is `__bynk_runtime/`, at the root of the output: Bynk
  identifiers cannot start with `_` (#1653), and a user file mirrors its source
  path, so a name a user cannot write cannot collide with `runtime/result.bynk`
  or any user directory. The coverage filter excludes only that first path
  component, so user code is never dropped from coverage. Rejected: a plain
  `runtime/` directory, which a user path can overwrite silently; a runtime barrel that re-exports everything — it keeps the whole
  graph reachable, which is the problem.
- **B. A project emits exactly the reachable closure.** The emitter collects,
  across every file it emits, the runtime modules named in their import lines,
  closes that set over the modules' own imports, and emits those and no others.
  Type-only edges count, for every target: `http` reaching `boundary` and
  `result` by `import type` still emits them, because `tsc` over the emitted
  `.ts` needs the files and one rule keeps the output deterministic.
  A unit with no import of a module never causes it to be emitted. The set is a
  function of the emitted code, so builds stay deterministic and same-binary:
  there is still no version skew, because the files come from the same compiler
  that wrote the imports. The Worker, Node and browser targets share the rule.
  The embedded sources become a table of `(module name, source)` rather than
  one string; `bundle.mjs` writes that table (still a committed build output,
  still drift-guarded, `cargo build` still Node-free).
- **C. Direction 2 is rejected for these helpers.** The pure helpers are
  compiler ABI by *use*: lowering emits the calls, and the boundary codecs
  already need the `Bytes` helpers whether or not a program imports a `Bytes`
  binding, so a std copy would be a second home beside a runtime one that cannot
  go. `bynk-std` is sources and bindings as data, reached when a program `uses`
  or `consumes` a unit; its bindings exist per platform (`bynk-browser.ts`,
  `-cloudflare.ts`, `-node.ts`), so moving a helper there triples it or adds a
  fourth shared file — a per-module runtime file is that fourth file, in the
  place the compiler already owns. The line: **the runtime is what generated
  code needs because the compiler wrote the call; `bynk-std` is what a program
  names.** Where the standard-library track (K-series, e.g. #2016) wants a
  *user-callable* form of a codec (`Bytes.toBase64`), that is a std binding that
  imports `__bynk_runtime/bytes.js` — the surface lives in std, the implementation stays
  one definition in the runtime. The `Result`/`Option`, boundary, storage, agent
  and connection parts never move.
- **D. `bynkc test --coverage` does not constrain where a helper lives, only
  that it not be inlined.** Coverage attributes only `.bynk` source; a helper is
  in the runtime so a never-taken fault branch is not reported on a `.bynk` line
  (`__bynkIntDiv`). Per-module files keep that property, provided the measurable
  filter excludes the whole `__bynk_runtime/` tree. Today it excludes one path,
  `runtime.js` (`is_measurable_emitted`); it must become a directory prefix. A
  helper moved into Bynk-written std source would need first-party exclusion
  verified before it could be trusted not to be attributed — a further reason
  against C's alternative, not a blocker on this one.
- **E. `@bynk/wire-client` is untouched.** It stays a concatenation of `result`,
  `bytes`, `errors`, `boundary` and its own `surface.ts`, bundled from the same
  source modules, and is versioned with the wire format. Emitting the runtime
  as separate files does not change what it concatenates; the one cross-check
  needed is that splitting a module later (F) keeps the four modules' exports
  stable, as `npm run check` already guards.
- **F. Module granularity beyond today's files is a follow-up, measured.** The
  large modules (`http` 962 lines, `boundary` 634, `auth` 389, `agent` 390)
  carry code a program may not use even when the module is reached. Splitting
  them is deferred until A and B land and the playground's 14-line program is
  measured, so the split follows data. The facts to gather: which modules a
  minimal program reaches, and what `Result` plus `errors` alone cost.

**Consequences.**

- The runtime README's "one flat file" contract and "not a tree-shaking bundler"
  are replaced: the unit is the module, selection is by reachability from
  emitted imports, and lockstep is by construction (same binary).
- Follow-up implementation issues (each its own increment): (1) `bundle.mjs`
  emits the per-module table and the drift guard follows; (2) `runtime_import_for`
  and the import-list producers (`RuntimeUse`, `workers.rs`, `workers_entry.rs`,
  `events_fanout.rs`) import per module; (3) `project.rs` emits the reachable
  closure instead of one `runtime.ts`; (4) `coverage.rs` excludes `__bynk_runtime/`, in the same step as (2) and (3);
  (5) the playground linker and `bynk-wasm` file lists, which assume a single
  `runtime.js`; (6) measure, then split large modules (F).
- Every emitted-code golden changes in its import lines in one step, which is a
  reason to land (2) and (3) together.
- A compiled output that is a flat `runtime.js` plus modules cannot exist: the
  change is atomic, and a project mid-upgrade is rebuilt wholesale.
- Not decided here: the interim playground fix for the `kind` field, which does
  not depend on this record.

---
title: "§8 Compilation model"
---
This chapter defines how Bynk sources are organised into a project, how a unit's
name relates to its file, and the pipeline that turns sources into type-correct
TypeScript. Emission itself — what each construct compiles to — is
[§7](/book/spec/emission/).

## §8.1 The manifest

A `bynk.toml` file at a directory's root marks it as a **project** and configures
its layout. Its keys:

| Table | Key | Controls |
|---|---|---|
| `[project]` | `name`, `version` | the project's name and version |
| `[paths]` | `include` | the trees to compile (array; defaults below) |
| | `exclude` | subtrees to skip during discovery (array) |
| | `out` | the default output directory |
| `[fmt]` | `indent`, `indent_width`, `max_line_width`, `trailing_comma` | formatter settings (read by `bynkc fmt` / `bynk fmt` and by format-on-save; CLI flags override them) |
| `[lsp]` | `diagnostics_mode` | language-server settings |
| `[workers]` | `compatibility_flags` | extra Cloudflare compatibility flags for every emitted `wrangler.toml`, after the default `global_fetch_strictly_public` (array of strings) |

## §8.2 Project and legacy modes

Bynk compiles in one of two modes.

**Project mode** — a `bynk.toml` is present (or a `src/` directory exists). The
`.bynk` files under the `[paths] include` trees are compiled together, and
`bynkc test` is available. `include` defaults to the conventional roots that
exist (`src`, and `tests` when present) or the project root itself, so a
conventional `src/`(+`tests/`) project and a flat project both need no `[paths]`
config; `exclude`, and the tool's own `out`/`node_modules`/dot-directories, are
pruned from discovery.

**Legacy mode** — no manifest. A single `.bynk` file compiles as one standalone
unit.

## §8.3 Source layout and test-ness

Test-ness is a property of a **declaration**, not of a file or directory
([§8.3.1](#831-structural-test-ness)). A **source** unit's file path MUST mirror
its qualified name: a `context commerce.orders` MUST live at
`commerce/orders.bynk` under an `include` root. A path that does not match the
declared name is rejected (`bynk.project.inconsistent_commons_name`), as is a name
declared as both a `commons` and a `context` (`bynk.project.kind_conflict`). A unit
split across files carries at most one module doc
([§5.1a](/book/spec/static-semantics/#module-docs),
`bynk.project.duplicate_module_doc`). The source tree therefore mirrors the program's architecture. A **`suite`** carries no
such requirement — it names its target and is legal in any file (see below).

### §8.3.1 Structural test-ness

A `suite` is a test-only declaration (at any tier — `unit` / `integration` /
`system`). A `.bynk` file MAY
hold more than one top-level declaration — an *atomic* file containing a
`commons`/`context` **and** a `suite` together is well-formed. Discovery scans the
whole `include` tree for declarations; a `suite` MAY appear in any file. A
production build (`bynkc compile`) MUST skip every `suite`: it is neither
type-checked for the build nor emitted into the deployable. `bynkc test` compiles
and runs the suites. There is no `.test.bynk` filename marker.

## §8.4 Build pipeline & conformance to TypeScript

A build runs the pipeline **lex → parse → resolve → check → emit**. The first four
stages establish well-formedness ([§3](/book/spec/lexical-grammar/)–[§5](/book/spec/static-semantics/));
only a well-formed program reaches emission ([§7](/book/spec/emission/)).

Emission writes, into the output directory, the per-context and per-test modules,
the composition root, the runtime library ([§7.4](/book/spec/runtime-library/)), and a
generated `tsconfig.json`. Every emitted module imports the runtime as
`./runtime.js` (or `../runtime.js` by directory depth). The `tsconfig.json`
enables `strict` and targets `ES2022` with `NodeNext` module resolution; a
generated `package.json` declaring `"type": "module"` makes that resolve and
build the output as ES modules.

**First-party commons** (v0.20b). Alongside the injected first-party
*adapters* (the `bynk` surface and platform adapters,
[§7.3.6](/book/spec/emission/#736-adapters)), the toolchain ships first-party
*library* units inside the reserved `bynk.*` prefix: `bynk.list` and
`bynk.map`, ordinary commons **written in Bynk** over the collection kernel
([§5.10](/book/spec/static-semantics/#510-collections)). A project that
`uses bynk.list` (or `bynk.map`, which itself `uses bynk.list`) has the unit
injected as a synthetic source file ahead of grouping; it then flows through
the ordinary commons pipeline — tables, `uses` resolution, type-checking,
and emission to `bynk/list.ts` / `bynk/map.ts` beside the other modules.
Unconsumed, nothing is injected and the output is unchanged.

v0.22a adds `bynk.string` on the same path (`uses bynk.string` →
`bynk/string.ts`): derived helpers **written in Bynk over the string
kernel** ([§5.2](/book/spec/static-semantics/#52-well-typedness)) — currently
`join(parts: List[String], sep: String) -> String`. The kernel operations
themselves are compiler built-ins, not commons functions.

A successful Bynk build emits TypeScript that is **type-correct end to end**: it
compiles under `tsc --strict` with no errors. This is the final gate of the
compilation model — a Bynk program's well-formedness is realised, not merely
asserted, in a type-checked TypeScript program. `bynkc test` continues past this
gate, running the compiled, aggregated test runner on Node.

> [!NOTE]
> The detailed validation requirements a conforming build and test run MUST meet
> — that the emitted runtime and modules compile under `tsc --strict`, that
> refinement validation and agent-state lifecycle behave as specified, and that
> deliberate failures are reported — are part of conformance, specified in §10.
> This note is informative.

## §8.5 The platform axis

The deploy **platform** (`--platform`, values `cloudflare` — the default — and
`node`) is a selection axis **distinct from** the emit topology (`--target
{bundle,workers}`, [§7.2](/book/spec/emission/#72-targets)): the target chooses *how the
output is laid out*, the platform chooses *which host the ambient surface binds
to*. The platform selects the first-party `bynk` binding module linked into the
output (`bynk-cloudflare.ts` / `bynk-node.ts`,
[§7.3.6](/book/spec/emission/#736-adapters)). Because the `bynk` contract names canonical
provider symbols, changing platform changes only that one imported module;
porting Bynk to a new host means supplying this one binding.

As of v0.19 the axis also carries the **platform lock**: a deployment unit
whose closure reaches a platform-native capability MUST be built with the
matching `--platform`
([§5.8](/book/spec/static-semantics/#58-boundaries--cross-context)).

## §8.6 Binding modules & npm dependencies

An adapter's `binding "<module>"` path is resolved **relative to the adapter's
source file**; the module is copied verbatim into the output tree beside the
adapter's emitted module, so the composition root's import resolves and the
`tsc --strict` gate of §8.4 checks the binding's `implements` contract. The
union of all adapters' pinned `requires` entries
([§5.8](/book/spec/static-semantics/#58-boundaries--cross-context)) is emitted as the
`dependencies` map of the generated `package.json`, beside its
`"type": "module"`; a project with no `requires` emits the marker alone.

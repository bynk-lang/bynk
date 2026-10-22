# Bynk editor tooling — roadmap (LSP + VS Code)

A forward plan for the editor experience: the `bynk-lsp` language server and the
`vscode-bynk` extension that hosts it. Goal — a **complete, rich** editor experience
that rivals a modern language server, and a discipline that keeps it current as the
language grows. This is a design reference, not a per-increment proposal; concrete
slices become `proposals/` entries when scheduled.

> **Status note (refresh):** the A‑1/A‑2/A‑3 navigation and refactor features
> (references, rename, signatureHelp, codeAction, codeLens, inlayHint, semantic
> tokens, workspace symbols, call hierarchy, document highlights, implementation
> nav, folding/selection) **all shipped** across v0.24–v0.37; §1 below has been
> corrected to match, and §2's A‑lists now read as "done." What remains is the
> **completion debt** and the **B‑1/B‑2 editor polish** — both now **shipped**
> via the LSP track (decisions in ADRs 0093–0095, feature spec in
> [`bynk-lsp-spec.md`](bynk-lsp-spec.md) §3.15). The only remainder is
> editor-agnostic setup docs ([#257](https://github.com/accuser/bynk/issues/257))
> and marketplace publishing ([#258](https://github.com/accuser/bynk/issues/258)).
> This roadmap stays the high-level parent.

---

## 0. Why the LSP feels absent today (fix first)

The extension spawns a separate **`bynkc-lsp`** binary over stdio, discovered on `PATH`
or via `bynk.executablePath` (`vscode-bynk/src/extension.ts`); **the VSIX does not bundle
the server**. With no `bynkc-lsp` on `PATH`, the editor still shows tmLanguage syntax
highlighting (no server needed) but **no hover, diagnostics, or completion** — the server
never starts. Start-up failures surface in the "Bynk LSP" output channel and a toast, but
are easy to miss.

This is the single biggest gap: **a fresh install of the extension does not give a
working LSP.** It is roadmap item **B‑0** below, and it gates every other LSP feature
being *felt*.

---

## 1. LSP — current state

Implemented (`bynk-lsp`, advertised in `lib.rs`'s `server_capabilities()`):

- **Live diagnostics** — a debounced project-wide round (`bynk_ide::diagnose_project_with`,
  open buffers overlaid on disk) on change, single-file `diagnose` for a rootless buffer;
  these are the compiler's *authoritative* diagnostics, a genuine strength.
- **Hover** — signatures.
- **Go‑to‑definition.**
- **References, rename/prepareRename** (v0.25); **code actions** from diagnostics (v0.26);
  **signature help** (v0.32); **code lens** (v0.33); **inlay hints** (v0.27); **semantic
  tokens** full+range (v0.28); **workspace symbols** + **document highlights** (v0.26);
  **call hierarchy** (v0.34); **implementation nav** (v0.35); **folding + selection
  ranges** (v0.37) — i.e. the A‑1/A‑2/A‑3 table-stakes all shipped across v0.24–v0.37.
- **Completion** — *overhauled and shipped*: the `.` trigger char,
  expression-position breadth, free-function/stdlib completion, builtin sum/static
  coverage, and the error-tolerant receiver typing that lifts the clean-file
  ceiling. **The surface contract is ADR 0093 (the ceiling lift, ADR 0094); the
  as-built spec lives at [`bynk-lsp-spec.md`](bynk-lsp-spec.md) §3.15.**
- **Formatting** — document + range.
- **Document symbols.**
- **Document links** (§3.21); **go-to-type-definition** (v0.35).
- **The foundation under the surface** (LSP foundations track, ADRs 0198/0201/0202/0204):
  one project model shared with `bynkc`, a freshness contract, **real multi-root
  workspace folders** (not just the advertised flag — implemented v0.182), startup
  analysis + server-registered file watchers (v0.183), and one diagnostics scheduler
  (v0.184).

---

## 2. LSP — roadmap

### A‑0 — The foundation: a project‑wide semantic index ✅ *(shipped — the binding index, v0.25/ADR 0053)*

Most rich features (references, rename, workspace symbols, call hierarchy, document
highlights) need a **cross‑file symbol + reference graph**, not the current per‑document
recompile. Build (or expose, from `bynkc`'s resolver/project analysis) a persistent
project model the server queries. **This gates all of A‑1's navigation/refactor work** —
do it first.

### A‑1 — Table‑stakes + the cheap Bynk‑specific win ✅ *(shipped, except completion — code actions v0.26, references/rename v0.25, signature help v0.32)*

> Completion is now complete — the overhaul shipped against the ADR 0093 surface
> contract (ceiling lift ADR 0094); as-built in [`bynk-lsp-spec.md`](bynk-lsp-spec.md) §3.15.

- **Code actions from diagnostics** *(highest leverage — do early).* Bynk's diagnostics
  are unusually **prescriptive** — they already say "add `X` to the `given` clause", "add
  a `consumes` for `B`", "construct via `T.of(...)`". Turning those notes into one‑click
  quick fixes is nearly free (the suggestion text exists) and makes Bynk feel *more*
  polished than languages with vaguer diagnostics. **Shipped:** the `given`-clause
  fixes (v0.26), the InRange-swap (v0.40), and — #852 — the **capability-aware** set:
  fill missing record field(s), add a missing `consumes` for an unconsumed cross-context
  call, and auto-`uses`/`consumes` an unresolved name (the Bynk analogue of auto-import).
  Follow-ups: snippet-placeholder field fills, remove/rename fixes for unknown/duplicate
  fields, and remove-unused-`consumes`.
- **Find references** and **rename** (`prepareRename` + `rename`) — the two refactor
  table‑stakes; both ride A‑0.
- **Comprehensive completion** — today `consumes`/`given` only. Extend to: types, fns,
  **methods** (now incl. `List`/`Map`/`String` + generics), capabilities, record fields,
  enum variants, keywords, and snippets. (See §5 — this is partly catch‑up debt.)
- **Signature help** — parameter hints while calling fns/methods/capabilities (and lambda
  arguments to combinators).

### A‑2 — Rich experience ✅ *(shipped — inlay hints v0.27, semantic tokens v0.28, document highlights + workspace symbols v0.26, code lens v0.33)*

- **Inlay hints** — and these matter *more* for Bynk now: v0.20a/b added inferred generic
  type args and lambda param types, and `let`‑binding types are inferred — all otherwise
  **invisible**. Hints for inferred `let` types, lambda params, and generic instantiations
  make that legible.
- **Semantic tokens** — type‑aware highlighting beyond tree‑sitter's syntactic pass:
  distinguish capability vs type vs **refined** vs **opaque** vs generic‑param vs
  **platform‑native** capability.
- **Document highlights** (occurrences of the symbol under cursor); **workspace symbols**
  (project‑wide search); **codeLens** (test‑run lenses, reference counts).

### A‑3 — Advanced *(shipped — call hierarchy v0.34, implementation nav v0.35; go-to-type-definition + document links landed (ADR 0095); type-hierarchy closed won't-do (refinement-families follow-up [#259](https://github.com/accuser/bynk/issues/259)); `completionItem/resolve` shipped)*

- **Call hierarchy**; **type‑definition / implementation** navigation tuned to Bynk —
  `given Cap` → its provider/adapter; a capability → its providers; a consumed context →
  its source.
- **File operations** — renaming a `.bynk` file updates the unit name and consumers,
  given the source‑path‑mirrors‑qualified‑name rule.
- **On‑type formatting**; completion‑resolve (lazy docs).

---

## 3. VS Code extension — current state

`vscode-bynk`: a tmLanguage grammar (syntax highlighting — works with no server), a
`language-configuration.json`, a `LanguageClient` that spawns `bynkc-lsp` over stdio
(PATH or `bynk.executablePath`), a status bar (project name from `bynk.toml` + compiler
version), and an `openProjectConfig` command. Distributed as a VSIX (built at 0.17.0).
**The server is not bundled** — the extension assumes it is already on `PATH`.

---

## 4. VS Code extension — roadmap

### B‑0 — Server provisioning ✅ *(done — download‑on‑activate)*

A fresh install now provisions a working LSP. **Download‑on‑activate** was chosen over
per‑platform VSIX bundling: it ships on the existing release infrastructure (the raw
`bynkc-lsp-<target>` binaries + `SHA256SUMS` the release now publishes) as one small VSIX,
and the `bynk.executablePath` escape hatch covers offline/air‑gapped use. Implemented:

- **Resolution order** (`src/server.ts`): `bynk.executablePath` → `bynkc-lsp` on PATH →
  cached download (global storage) → download the pinned release binary, **verified against
  `SHA256SUMS`**, cached, `chmod 0o755`.
- **Loud, actionable failure** — an error toast with *Download Server / Open Settings / Show
  Output*, and a status‑bar item (`$(error) Bynk LSP: not running`).
- **Commands** — `Bynk: Restart Language Server`, `Bynk: Download Language Server`,
  `Bynk: Show Language Server Output`.
- **Version‑compatibility check** — warns when the running `bynkc-lsp --version` differs from
  the extension's pinned `bynkServerVersion` (package.json).
- **Release side** — `release.yml` publishes raw per‑target `bynkc-lsp` binaries (+ checksums
  + provenance) so there is no in‑extension archive extraction.

*Note:* download needs a published Release at the pinned tag (`v0.23.0`); the infra is ready,
so cutting that release activates the path. Per‑platform VSIX **bundling** stays deferred to
Tier 4 (with marketplace publishing), if air‑gapped installs become a need.

### B‑1 — Surface the LSP's features in the UI

As A‑1/A‑2 land, wire the client so the features are *usable*: code‑action lightbulbs and
rename UI (mostly automatic once the server advertises them), an **inlay‑hint toggle** and
**semantic‑token theme** mappings, and codeLens for tests.

### B‑2 — Extension polish

- **Settings** — format‑on‑save, server trace level, inlay‑hint granularity.
- **Snippets** — `context`, `adapter`, `capability`, `service`, `on call`, `test` scaffolds.
- **Commands / scaffolding** — new project (`bynk.toml` + layout), new context/adapter.
- **Tasks / problem matcher** — run `bynkc` builds with diagnostics in the Problems panel.
- **Getting‑started walkthrough**; **marketplace publishing** (currently a hand‑built VSIX).

---

## 5. Cross‑cutting

- **Keep tooling current with the language — a standing rule.** The LSP is *accruing
  debt*: v0.20a/b and v0.21 added lambdas, generics, `List`/`Map`, JSON, soon `Float`, but
  completion is still `consumes`/`given`‑only and there's no inlay‑hint surface for the new
  inference. Each language increment's **tooling delta must explicitly enumerate LSP**
  (completion, hover, semantic tokens for the new constructs), not just tree‑sitter and
  fmt. Fold this into the proposal template's tooling‑delta line.
- **Editor‑agnostic.** The LSP is a standalone server; "rival modern languages" means a
  documented setup for **Neovim / Helix / Zed** too, plus a generic install path — not
  VS Code only.
- **Distribution / CI.** Build and bundle `bynkc-lsp` per platform alongside the extension;
  publish to the VS Code Marketplace (and Open VSX for the non‑VS‑Code editors).

## 5.1 The `bynk` driver & the project-lifecycle arc

Distinct from the LSP/extension thread above: a **`bynk` driver** — a thin
orchestrator over `bynkc` and the Node toolchain, as `cargo` is to `rustc` (ADR
0083). The compiler stays pure (compile / check / fmt / test); environment
orchestration lives in the driver. The arc is **`doctor` → `new` → `dev`**:

- **`doctor`** *(shipped v0.46)* — an upfront, capability-grouped environment
  check (compile / test / dev-deploy / editor / build-from-source), reporting
  presence + version + provenance, with a bare-informational / `--only` /
  `--strict` exit contract and `--format short|json` (ADRs 0083–0084). Chosen to
  go first because it has no language surface and mutates nothing, so it is the
  safe place to stand the driver up.
- **`new`** *(shipped v0.58)* — scaffold a **complete, runnable** single-context
  HTTP service (`bynk.toml` + `.gitignore` + `src/<name>.bynk`) that `dev` serves
  unmodified, closing the arc; offline, no-toolchain `std::fs` file-writing; an
  embedded compile-tested template (ADR 0086 precedent); the name validated by
  the real lexer with a `--name` fix-it (ADR 0097). Supersedes B‑2's "Commands /
  scaffolding" line for the CLI path. Deferred as named follow-ups: `init`
  (scaffold in place), `--template` (a second project shape), and in-project
  generators (`bynk new context …`).
- **`dev`** *(shipped v0.57; watch v0.163, multi-context v0.166)* — build +
  `wrangler dev` orchestration: locate the project, pre-flight the `deploy`
  capability, compile to a managed `.bynk/dev/`, and serve in local mode with
  `--` passthrough to wrangler. Both v0.57 follow-ups have landed: the **watch /
  incremental recompile** loop (#524), and **multi-worker local dev with live
  cross-context Service Bindings** (ADR 0192, #552) — every context is served,
  one `wrangler dev` each, wired through wrangler's dev registry, with
  `--context` narrowing and per-context port allocation. Still open: the v1
  first-party `workerd` dev-server overlap noted in
  `bynk-status-and-roadmap.md` — now an optimisation (one front door, unified
  logs) rather than the price of cross-context calls, which the registry already
  delivers. Provisioning + remote deploy are `deploy`'s problem — and `deploy` is
  no longer single-context either (slice 2, v0.170, ADR 0193), nor KV-only
  (slice 1, v0.171, ADR 0194: queues and DO migrations).

With `doctor` (v0.46), `new` (v0.58), and `dev` (v0.57) shipped, the on-ramp arc
is complete; `deploy` (provisioning + remote) follows.

## 5.2 The test runner

`bynkc test` (and its `bynk test` pass-through) already owns discovery, rich/JSON
output (ADR 0098), seeded `property` tests, per-case filtering, and an inspector
path (ADR 0104). **Coverage** now sits on top: `bynkc test --coverage` reports
**statement/line** coverage attributed to `.bynk` source — a rich summary table
or a `coverage` block under `--format json` — collected via V8's
`NODE_V8_COVERAGE` and remapped through the emitted source maps ([ADR 0103](decisions/0103-source-map-contract.md)),
scoped to user source (the `tests/` tree and workers scaffold excluded), on the
`tsc → node` path only (#854, closes it). **Follow-ups:** per-**branch** coverage
in `.bynk` terms (a coarser-than-JS notion of a Bynk branch), and widening
`--coverage` beyond `tsc → node` (the `tsx`/`--inspect` paths).

**In the editor** (Test Explorer depth track,
[#2257](https://github.com/bynk-lang/bynk/issues/2257)): the Testing view honours
the selection and shows a remote run's skipped cases (Slice A), and now tags each
case with its tier and offers **Run against…** for the `system` suites (Slice B,
[#2261](https://github.com/bynk-lang/bynk/issues/2261)). The JSON document gains an
optional per-case `tier`, and a failed property reports its `seed` and
`counterexample` as data, so the editor can re-run and pin a seed (Slice C,
[#2266](https://github.com/bynk-lang/bynk/issues/2266)). `bynk test` takes a
repeatable `--case` and a new `--suite`, so one process runs a chosen set, and a
value that selects nothing now fails the run instead of passing vacuously
(Slice G, [#2273](https://github.com/bynk-lang/bynk/issues/2273)). `bynk test --format
ndjson` streams the run as one pinned event per line, and the Testing view fills in
suite by suite (Slice D stage 1, [#2267](https://github.com/bynk-lang/bynk/issues/2267);
per-case events and durations are stage 2).

## 6. Suggested sequencing

1. **B‑0** (server provisioning) — without it nothing else is *felt*. Smallest, highest impact.
2. **A‑1 code actions** (cheap, high polish) + **A‑0 index** in parallel (the index unblocks the rest of A‑1).
3. **A‑1** references / rename / completion / signature help.
4. **A‑2** inlay hints + semantic tokens (close the v0.20/v0.21 visibility debt) → the rest of A‑2.
5. **A‑3** + **B‑1/B‑2** polish; editor‑agnostic docs.

Each becomes a `proposals/` slice when scheduled; the LSP spec (`bynk-lsp-spec.md`) is
updated in place as features land, the way the normative spec is.

---

## 7. Remaining backlog (the concrete queue)

_Subsumes the former `bynk-tooling-proposal-queue.md` (status @ v0.43). The
v0.24–v0.43 line shipped nearly the whole original queue — comprehensive
completion, signature help, call hierarchy, implementation navigation,
folding/selection ranges, the inlay-hint follow-ups, the InRange quick-fix, B-2
extension polish. What follows is the short tail. Each line becomes a
`proposals/vX.Y-*.md` when scheduled; "gated" notes a prerequisite._

**Advertised today:** hover, definition, completion (types, fns, members,
locals, keywords, snippets), formatting (+range), document symbols, references,
rename, code actions, inlay hints (types, parameter names, generic
instantiation), semantic tokens, workspace symbols, document highlights,
signature help, CodeLens (reference counts), call hierarchy, implementation
navigation, folding & selection ranges.

### 7.1 Open tooling work (server, `bynkc` + `bynk-lsp`)

1. **Locals-rename + generic type parameters** — the last unpaid slice of the
   recurring index deferral (v0.25/v0.27/v0.28/v0.31/v0.36). Local bindings
   resolve and colour, but **rename** for them is still deferred (subtler
   scope/shadowing edits); **generic type parameters** are not indexed at all.
   Also out: match-arm / `is`-narrowing pattern bindings, and the
   `parameter`-vs-`variable` token split. *Meaty; sliceable. Highest-value
   remaining item.*
2. **Test-run CodeLens** — the "▶ Run" lens above tests. *Gated:* needs test
   discovery + a run command. *Small once the gate lands.*
3. **`inlayHint/resolve`** — lazy hint tooltips. *Small, ungated.*
3a. **`bynk.toml` as a server document** — #2096 shipped the manifest's JSON
   Schema (generated by `cargo xtask manifest-schema`, served at
   `https://bynk-lang.org/schemas/bynk.toml.json`, associated by the extension's
   `tomlValidation`), so completion, hover and shape validation come from a TOML
   language server. Deferred by its DECISION A: the server opening `bynk.toml`
   itself, for checks that need the project (an `[unstable] allow` entry naming
   no item, positioned on the line) and for editors with no schema-aware TOML
   support. *Medium; do when a schema-only check proves too shallow.*
4. **Refactorings and source actions**
   ([#2100](https://github.com/bynk-lang/bynk/issues/2100)) — shipping one
   slice per PR, in the order organise `uses`/`consumes` → fix-all → fill
   match arms → inline variable → generate provider stub → extract function.
   Each action re-checks its edited text before it is offered.
   - [x] Organise `uses`/`consumes` (`source.organizeImports`).
   - [x] Fix all (`source.fixAll.bynk`).
   - [x] Fill match arms.
   - [x] Inline variable (`refactor.inline`).
   - [x] Generate provider stub.
   - [x] Extract function: stays capability-free (the refusal says why), never lifts a `?`, and re-checks its edit.

(Type-definition navigation — `textDocument/typeDefinition` — **shipped** with
go-to-type-definition + document links, ADR 0095; see §2 A-3. It is no longer
open work.)

### 7.2 Deferred optimisations (do when scale demands)

4. **Semantic-tokens `delta`** — re-encode only changes. *No scale signal yet.*
5. **Incremental recompute** — a salsa-style incremental recompute replacing the
   per-debounce full project analysis. *Deferred since v0.24.*

### 7.3 Distribution

Extension + grammar release automation (Marketplace + Open VSX), per-platform
VSIX bundling, and binary signing/notarisation are **CI Tier 4** and live in
[`bynk-engineering-roadmap.md`](bynk-engineering-roadmap.md) — both gated on
marketplace tokens / signing certificates.

### 7.4 Not tooling, but it gates tooling

- **`given` on free functions** (the v0.23 discovered limitation) — language
  core; until it lands, no capability can be driven from a factored helper,
  which caps what capability-iteration tooling can demonstrate.

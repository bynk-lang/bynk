---
title: "`bynk-lsp`"
---
The Bynk language server. The `bynk-lsp` crate builds the `bynkc-lsp` binary, a
[tower-lsp](https://github.com/ebkalderon/tower-lsp) server that communicates
over **stdio**. Editors talk to it; most users reach it through the
[VS Code extension](/docs/tooling/vscode-bynk/) rather than directly. See the
[Set up editor support](/docs/editor-and-tooling/editor-support/) how-to for
wiring it into an editor.

Text is synchronised in full: on every change the whole document is re-sent
(`TextDocumentSyncKind::FULL`), and the server holds the current buffer in
memory. When a project root with a `bynk.toml` is found, the server enables
cross-file features; otherwise it operates in single-file mode. Project
discovery and the analysis model that underpins these features are described in
[Architecture](#architecture) and [Project discovery and performance](#project-discovery-and-performance).

## Capabilities

Every capability below is advertised in the server's `ServerCapabilities` and
backed by a request handler.

| Capability | What it does |
|---|---|
| Diagnostics | Recovering compilation surfaced as squiggles — live by default (debounced at the configured `diagnostics_debounce_ms`, default 300 ms) and re-run when watched files change. With a project root diagnostics are project-wide: the whole bundle is analysed, open buffers overlaid on disk, so an error in one file shows on the file that owns it. Each diagnostic carries its `bynk.*` code; for the curated, highest-traffic codes the code is rendered as a **clickable link** (`codeDescription`) to its explanation in the Book — the same explanation `bynk explain <code>` prints. Codes without a curated explanation carry no link (graceful fallback, not an error). |
| Hover | Type signatures and doc blocks for the symbol under the cursor, resolved through the binding index so the description matches the actual definition — a name-match fallback answers only where the index does not resolve. Works on a name's *uses*, not just its declaration: inside an agent handler body, a `store`/`key` field reference describes the field, a record-construction label (`Stored { title: … }`) describes the type's field, and a store operation (`items.put(…)`) describes the operation over the field's declared kind. Every kind the index carries is described: an actor (`by u: User`), a method — the one the call *binds* to, so `g.bump()` describes `Gauge.bump` and not a same-named `Counter.bump` — and a capability operation, attributed to the capability that declares it, including where a project declares one whose name a built-in shares. A `[Name]`/`[Owner.member]` intra-doc link inside the rendered doc comment becomes a clickable link when it resolves the same way document links do; unresolved names render as plain text. Hovering a single-handler service also appends its **wire contract** — the request envelope, the cross-context contract form + hash (`on call` only), and reachable HTTP responses — and hovering a handler's own header (`on GET(...)`, `on call`) answers with that same wire contract standalone, a rung no earlier hover behaviour ever occupied. A unit name — the file's own header name, or a `uses`/`consumes` target — shows the unit's kind and its module doc, found in whichever of the unit's files carries it. |
| Go-to-definition | Jumps to the declaration of types, functions, capabilities, services, agents, providers, and actors (a handler's `by u: User` clause) — cross-file, via the project index. Local bindings resolve scope-correctly; `uses`/`consumes` unit segments jump to the unit's source. |
| Go-to-type-definition | From a value to the declaration of its inferred type. Reads the value's type from the round's expression types and lands on the named type's declaration. |
| Find references | Project-wide occurrences from the binding index, including clause lists and test units. Local bindings return their definition plus uses within the file. |
| Rename | Project-wide rename; `prepareRename` validates that the symbol is in scope and refuses out-of-scope kinds. Emits versioned edits, and re-analyses with the edits applied to reject collisions and silent re-bindings before returning. |
| Formatting | Whole-document formatting via the shared [`bynk-fmt`](/docs/tooling/bynk-fmt/); a parse error yields no edits (the diagnostic flow reports it). |
| Range formatting | Partial-document formatting. Per spec it may return edits wider than the requested range. |
| Document symbols | The file outline for the editor's symbol view and quick-open. |
| Completion | Scope- and context-aware: units after `consumes `, capabilities inside `consumes U { … }` and after `given `, in-scope locals at keyword and expression positions, and members after `.` on a typed value receiver. Documentation is resolved lazily on the focused item so the initial list stays cheap. |
| Signature help | The active parameter of the call being typed, triggered on `(` and `,`. Covers named callees and value-receiver methods. |
| Code lens | A reference-count lens above each top-level definition, clickable to peek the references, plus a "Show Sequence" lens above every handler and a "Show State" lens on every agent. |
| Sequence diagram | `bynk/sequenceModel` — a custom (non-standard) request, advertised via `ServerCapabilities.experimental` rather than a dedicated provider field — classifies the handler under the cursor's calls into runtime-participant lifelines (consumed capabilities, consumed contexts, agents) for the VS Code extension's "Bynk: Show Sequence Diagram" webview. In an agent's handler it also reports each write to the agent's own state (a `:=` or a mutating store operation) as a `StateWrite` message from the entry to itself, in source order. Served from the committed round; the client re-issues it on each invocation and, for an open view, after edits (no refresh-push mechanism). |
| Documentation view | `bynk/documentationModel` — a second custom request (same `experimental` advertisement) — aggregates the whole unit's declarations — every file of a multi-file unit, under its one module doc — into a rendered reference page for the VS Code extension's "Bynk: Show Documentation" webview: each declaration's heading, signature, and doc comment rendered as Markdown, in outline order and hierarchy, with undocumented declarations shown as a coverage signal (toggle to hide). Reuses hover's signature/doc assembly so the page agrees with hover. Served from the committed round; re-issued on each invocation and, for an open view, after edits. |
| Architecture map | `bynk/architectureModel` — a third custom request (same `experimental` advertisement), **project-scoped** rather than file-scoped — maps every context/adapter in the active file's project: one node per unit, a directed edge per `consumes` (labelled with its selected capabilities where braced), and each node's locally-declared and consumed capabilities/providers plus its services/agents, for the VS Code extension's "Bynk: Show Architecture Map" webview. The macro counterpart to the sequence diagram above. Served from the committed round; re-issued on each invocation and, for an open view, after edits. |
| Wire contract | `bynk/wireContract` — a fourth custom request (same `experimental` advertisement), file- and position-scoped like the sequence diagram — the wire shape a boundary handler's request/response actually takes: the request envelope (bare value for a single param, an empty body for zero, a keyed object for two or more), the cross-context contract form + hash for an `on call` handler, the reachable HTTP response set (declared, constructed, and boundary-implicit — e.g. the 400 every param re-validation can produce), and every referenced boundary type's shape with its re-validation strategy (owner constructor, inlined predicate, structural-only for an opaque type, or base64 decode for `Bytes`) — for the VS Code extension's "Bynk: Show Wire Contract" webview and a matching hover rung on the handler header. Served from the committed round; re-issued on each invocation and, for an open view, after edits. |
| Agent state | `bynk/agentStateModel` — another custom request (same `experimental` advertisement), agent-scoped: the cursor anywhere in an `agent` selects it — the agent's state machines, derived from its handlers: for each `store` Cell of a user enum or sum type, its states and initial state, every edge a handler can produce (the states it can start in, its guards, the target — a state, a computed write, or unchanged — its outcome and range), the states reachable from the initial one, and any handler too large to analyse; for each edge, the `transition` step invariants it would violate at commit (known, or possible for a computed write), and the field's `transition`s outside the evaluated fragment; plus each handler's name and range, and its effects — one row per path with its guards, the store fields it writes (with ranges), the fields it only reads, the agents it calls and its outcomes. For the VS Code extension's "Bynk: Show Agent State" webview. Served from the committed round; re-issued on each invocation and, for an open view, after edits. |
| Explain a diagnostic | `bynk/explain { code } → { title, markdown, href } \| null` — a fifth custom request (advertised as `experimental.explain`), keyed by the code alone (no document) — the curated explanation `bynk explain <code>` prints, read from the server's own compiled-in table, so it matches the installed compiler and needs no network. `null` for a code with no curated explanation. A client that advertises `experimental.explainDiagnosticCommand` in its own capabilities also gets an **Explain `<code>`** quick fix on each curated diagnostic, placed after every real fix and never preferred; it runs the client's `bynk.explainDiagnostic` command with the code. |
| Call hierarchy | Incoming and outgoing calls over the binding index's call graph. |
| Implementation | From a capability to its providers (the reverse direction, provider to capability, is served by go-to-definition). |
| Document links | `uses`/`consumes` unit names — and a test file's `suite <target>` header — become clickable links to the unit's source file. Inside doc comments, a `[Name]`/`[Owner.member]` intra-doc link also becomes clickable when it resolves against the declaring unit's scope (itself, its `uses`, its `consumes`); an unresolved one renders as plain text. |
| File rename awareness | Renaming or moving a `.bynk` file rewrites its own declaration name and every other file's `uses`/`consumes` reference that pointed at it. Renaming or moving a **folder** does the same for every unit under it, nested folders and multi-file units included, in one edit. A suite, a folder with no `.bynk` files, and a move into a different project are left untouched. |
| Document highlight | The matching binding's occurrences highlighted across the active file. |
| Folding ranges | Structural folds and comment runs, driven by the recovered AST (no analysis round needed). |
| Selection ranges | Expand-selection by syntactic nesting — the enclosing-node chain for each position. |
| Code actions | Quick-fixes built from the structured suggestions carried on diagnostics, served from the cached round so they agree with the squiggles on screen. **Capability-aware fixes** turn boundary/resolution errors into one click: fill a record construction's missing field(s) with a typed default, add a missing `consumes` for an unconsumed cross-context call, and — Bynk's analogue of auto-import — add the `uses <commons>` or `consumes <context> { … }` that brings an unresolved name into scope (one action per resolving candidate; a braced `consumes` is extended in place). **Fill match arms** adds an arm for every variant (or `Bool` value, or the `_` an `Int`/`String` match needs) a non-exhaustive `match` leaves out, with a typed default body (`0`, `""`, `false`, `()`, `None`, `[]`, also when the match is an `Effect` of one of those). It is offered only when every missing arm gets a body, so a match producing a record, a sum or a refined type gets none. Its edit is computed and re-checked when you pick it (`codeAction/resolve`). Plus two refactors over the same selection: extract-variable binds the smallest covering expression node to a fresh `let`; extract-function lifts it — a single expression, or a contiguous run of full statements (optionally including the block's tail) — into a new top-level `fn`, threading its free identifiers as parameters — offered only when the selection uses no capability (extracted functions stay capability-free: a plain `fn` has no `given` clause) and contains no `?` (lifted, it would return from the new `fn`), and, for a statement run, introduces no binding still used afterward and contains no `store`-field write. A selection refused for a capability or a `?` shows as a disabled action with the reason, in a client that displays those. Its edit is computed and re-checked when you pick it. **Inline variable** (`refactor.inline`), with the cursor on a `let`'s name or a use, replaces every use with the initialiser (parenthesised when it isn't atomic) and deletes the `let`. It is offered only for a pure `let =` (never a `let <-`, which performs an effect where it stands), and declines when a name the initialiser reads would resolve differently at a use, or when the `let` carries a comment. Its edit is computed and re-checked when you pick it. **Generate provider stub**, with the cursor on a capability's name (or as a quick fix on an exported capability with no provider), scaffolds `provides Cap = CapImpl { … }` below the declaration, each operation's signature copied and its body a typed default. It is offered only when nothing in the context provides the capability yet and every operation returns a type with a default. And two source actions (see [On save](#on-save)): **organise `uses`/`consumes`** (`source.organizeImports`) sorts the header's `uses` and `consumes` clauses and the names in a braced selection, drops duplicates, and drops what the unit never references; **fix all** (`source.fixAll.bynk`) applies every machine-applicable diagnostic fix in the file as one edit. |
| Inlay hints | Inferred-type hints for the visible range, plus materialisable ghost `given` hints for uncovered capability requirements. |
| Semantic tokens | Resolution-aware highlighting (full document and range), additive over the client's syntactic layer, read from the cached index. |
| Workspace symbols | Symbol search across the index's definitions, filtered by query — aggregated over **every** open project (the one cross-project query in a multi-root window). |
| File watching | Re-checks diagnostics when `.bynk` files change on disk, and reloads a project's `bynk.toml` when the manifest changes. The server **registers the watchers itself** (dynamic `didChangeWatchedFiles`), so any client is notified — not just those that supply watchers client-side. The server doesn't analyse `bynk.toml` as a document: completion, hover and validation there come from the manifest's [JSON Schema](/docs/manifest/#editor-support), through a TOML language server. |
| Startup analysis | On activation the server discovers and analyses every project under the workspace folders, so diagnostics appear **before any file is opened**. Folders added later are warmed the same way. |
| Workspace folders | Real multi-root support: several projects open at once, each analysed independently; a file routes to its nearest `bynk.toml`. Adding or removing a folder adds or prunes its projects. |

## On save

Source actions are computed only when the client asks for them by kind: the
Source Action menu, the editor's organise-imports command, or
`editor.codeActionsOnSave`. Before one is offered the server re-analyses the
project with the edit applied, and offers the edit only if it introduces no new
diagnostic. The check compares per-category counts for each file: the edit is
refused if any file ends up with more diagnostics of some category than it had
before. It is never on by default; to organise a file's header each time it
is saved, and apply its auto-fixable problems, opt in from VS Code's `settings.json`:

```json
"[bynk]": {
  "editor.codeActionsOnSave": {
    "source.organizeImports": "explicit",
    "source.fixAll.bynk": "explicit"
  }
}
```

Organise reads "unused" from the binding index, across every file of the unit
and the suites on it. A reference inside a `uses`/`consumes` clause doesn't
count as a use. A whole-unit clause on a target outside the project (such as
`uses bynk.list`) is never dropped. It is kept because the index can't show
it unused. A clause moves with its own comments, and a clause carrying a
comment is never dropped. A `bynk-fmt`-canonical file stays canonical. If a
drop would break the project, the action falls back to sorting and
deduplicating only.

Fix all takes a fix only from a diagnostic that offers exactly one
machine-applicable fix. A diagnostic offering a choice between candidates stays
an individual quick fix. A fix whose edits would overlap one already taken is
left for the next run.

## Build

From the workspace root:

```sh
cargo build --release -p bynk-lsp
```

The binary is `target/release/bynkc-lsp`. Put it on `PATH`, or point your editor
at it explicitly (in VS Code, the `bynk.executablePath` setting).

`bynkc-lsp --version` prints the version and exits without entering the protocol
loop, so tooling (such as the VS Code status bar) can query it without the
server blocking on stdin.

## Architecture

The `Backend` holds the mutable server state behind a `tokio::sync::RwLock`:

- a **map of projects** keyed by discovered root — one editor window may hold
  several. Each entry carries its own parsed **configuration**, analysis round,
  freshness generation, and published-diagnostics set (so two projects analyse
  and publish independently),
- the **workspace-folder seeds** — where projects are discovered and pruned, not
  how requests route, and
- the **open documents**, keyed by URI, each with its current text and version —
  a client-global set; each doc routes to its project by its nearest `bynk.toml`.

A document change runs `recompile_and_publish`, which routes the URI to its
project. In single-file mode (no manifest) that diagnoses the one buffer
directly; in a project it schedules that project's debounced project-wide round.
Hover and go-to-definition first consult the binding index,
falling back to a re-parse of the AST under the cursor for kinds the index does
not carry; formatting delegates to `bynk-fmt`.

### The analysis round

Each project-wide analysis retains one round's outputs, held together so that
every position converts against the text the analysis actually saw — not the
live buffer, which may already have moved on. A round carries:

- the **binding index** — the call graph and cross-file symbol table that
  references, rename, definition, hover, call hierarchy, implementation, and
  workspace symbols all read;
- per-file analysed **snapshots** — the exact text each span is an offset into;
  every span-to-position conversion uses these;
- the **open-document versions** captured when the overlay was built — the
  freshness contract gates on these (below), rename emits versioned edits
  against them, and published diagnostics carry them so a client can drop a
  range its buffer has moved past;
- the full **diagnostics** per file, including the structured suggestions that
  code actions ride on (clean files retain an empty entry);
- **inferred-type hints** and the **capability-requirement ledger** that drive
  the two kinds of inlay hint;
- **local bindings with scope ranges**, for scope-correct local navigation;
- **expression types**, which back go-to-type-definition; and
- a **unit-name-to-source map** and a **doc-link scope map**, which back
  document links (including intra-doc links inside doc comments) and hover's
  doc-link rendering.

**The freshness contract.** An index-backed request always answers against the
buffer the client currently holds. Each analysed file records the document
version it was analysed at; when a request arrives for a file whose buffer has
moved past the last round, the server refreshes — runs a round over the current
buffers — before answering, so a position is never resolved against text the
user has already edited past. When it cannot reach the requested version (a file
outside the project, or a concurrent edit that raced the refresh) it returns
nothing rather than a stale answer. A round is fast (single-digit milliseconds
for a typical project), and concurrent requests after one edit share a single
refresh.

The requests the editor re-fires on *every* keystroke — semantic tokens, inlay
hints, code lenses, document links, and code actions — take a lighter path.
Because they resolve nothing against your live cursor (each decoration's ranges
are computed against the snapshot the round analysed), they are served from the
last committed round as-is, without forcing a fresh round on every keystroke.
The decorations may lag your typing by at most one debounce cycle; when the
debounced round commits, the server asks the editor to re-pull them, so they
catch up on their own. This keeps typing responsive on large projects, where
forcing a whole-project round per keystroke grew slower with project size.

## Project discovery and performance

The server resolves each file to its project by walking upward from the file to
the nearest `bynk.toml` (the same attribution `bynkc` gives it) — so several
projects across one window's workspace folders each analyse independently, and a
file's workspace folder does not decide its project. Whether a file resolves to
a project decides its feature set:

- **In a project** — cross-file lookups, project-wide diagnostics,
  workspace symbols, rename, and the index-backed navigation features all apply.
  The files analysed are **exactly the files `bynkc` compiles**: every tree in
  the manifest's [`[paths] include`](/docs/manifest/), with `exclude` honoured,
  read through the compiler's own discovery. A conventional project needs no
  `[paths]` at all — `src/` and, when present, `tests/` are picked up; a flat
  project (`.bynk` at the root) works as-is.
- **Single-file mode (no manifest)** — each buffer is analysed on its own and
  the workspace features are unavailable; diagnostics still work per buffer.

Diagnostics are debounced at `diagnostics_debounce_ms` (the
[`[lsp]` key](/docs/manifest/) in `bynk.toml`, default 300 ms) — one
generation-based debounce over both project and single-file mode. Every change
bumps a generation, and a scheduled round runs only if it is still the latest
when the delay elapses, so a burst of keystrokes coalesces into a single
analysis. The analysis itself runs off the async runtime; a round already in
flight when a newer edit arrives runs to completion but has its result discarded
(never published) rather than being cancelled. There is a real, if narrow,
window between analysing a round and publishing it — which is exactly why
positions convert against the analysed snapshots rather than the live buffer.

## Internals

The crate is split into focused modules:

| Module | Role |
|---|---|
| `lib.rs` | The server: `Backend`, the per-project state map, the `LanguageServer` impl, request dispatch, advertised capabilities, and `run()`. |
| `main.rs` | Thin binary entry point — `bynk_lsp::run().await`. |
| `position.rs` | Byte-offset ↔ LSP position conversion. |
| `symbols.rs` | Symbol lookups for hover and go-to-definition. |
| `hover.rs` | Hover's resolution ladder — the order the lookups are tried in, as one pure function the handler and its tests share. |
| `index_queries.rs` | Pure queries over the project binding index: references, rename planning and validation, call hierarchy, semantic tokens, code lenses. |
| `completion.rs` | Context detection and candidate generation for completion. |
| `signature_help.rs` | Call-context detection and signature labels. |
| `inlay_hints.rs` | Inferred-type and ghost `given` hint rendering. |
| `code_actions.rs` | Quick-fixes from diagnostics' structured suggestions. |
| `organise.rs` | Organise `uses`/`consumes`: the sorted, deduplicated, pruned header as one edit, with unused clauses read from the binding index. |
| `fill_arms.rs` | Fill match arms: collects the checker's per-witness arm suggestions on one `match` into one edit, laid out as `bynk-fmt` prints arms, resolved lazily. |
| `inline.rs` | Inline variable: the binding's uses from the locals index, the shadowing check, and the `let` deletion, re-formatted when the file was canonical. |
| `provider_stub.rs` | Generate provider stub: the unprovided capability (from the index's implementation edges), its operations' signatures, and typed default bodies. |
| `fix_all.rs` | Fix all: every lone machine-applicable suggestion in a file, overlapping ones skipped, as one edit. |
| `capability_fixes.rs` | Capability-aware header fixes: add `consumes`, and auto-`uses`/`consumes` for an unresolved name (from the binding index + a fresh reparse). |
| `extract.rs` | Extract-variable: smallest covering expression node, insertion-point tracking through nested blocks, collision-avoiding placeholder naming. Extract-function: same selection, lifted to a new top-level `fn` with free-identifier parameters, capability-free-only, never across a `?`; listed lazily and resolved with a re-check. |
| `locals_nav.rs` | Scope-correct navigation for local bindings. |
| `structure.rs` | Folding and selection ranges from the recovered AST. |
| `document_symbols.rs` | The document-symbol outline. |
| `publish.rs` | The pure publish plan (which files to publish, which to clear). |
| `project.rs` | `bynk.toml` project configuration. |
| `sequence_request.rs` | `bynk/sequenceModel`: the enclosing-handler-at-cursor lookup, the "Show Sequence" CodeLens site list, and the wire-shape conversion from `bynk_ide::sequence::SequenceModel`. |
| `documentation_request.rs` | `bynk/documentationModel`: the whole-file model builder and the wire-shape conversion from `bynk_ide::documentation::DocModel`. |
| `architecture_request.rs` | `bynk/architectureModel`: the project-wide model builder and the wire-shape conversion from `bynk_ide::architecture::ArchModel`, lowering each node/member/edge's span against its own file (not just the request's document). |
| `agent_state_request.rs` | `bynk/agentStateModel`: the enclosing-agent-at-cursor lookup, the per-agent `Show State` lens sites, and the wire shape of `bynk_ide::agent_state::AgentStateModel`. |
| `wire_contract_request.rs` | `bynk/wireContract`: the enclosing-handler-at-cursor lookup and the wire-shape conversion from `bynk_ide::wire_contract::WireContractModel` (and, transitively, `bynk_check::wire::WireModel`), resolving each referenced boundary type's own declaring file through the project index rather than assuming the request's document. |
| `explain_request.rs` | `bynk/explain`: the curated explanation as Markdown, and the **Explain** quick fix for curated diagnostics in range. |
| `transport.rs` | The framing pump between stdin and `tower-lsp`: forwards each client message whose body parses, repairs a lone UTF-16 surrogate escape to U+FFFD, and logs and skips a body that can't be parsed, so one bad message can't end the session. |

## Logging

The server logs to `~/.bynk-lsp.log`; the verbosity is tunable via the
`BYNK_LSP_LOG` environment variable (default `warn`).

## Failures

- **A panic during project analysis** is logged with its message, and the
  editor shows one error saying the project's diagnostics may be out of date.
  It shows once per run of failures, not on every edit, and the next round
  that analyses cleanly replaces the stale diagnostics.
- **A client message that isn't valid JSON** is logged and skipped. One whose
  only fault is a lone UTF-16 surrogate escape (`\udcff`, which
  `JSON.stringify` writes for a lone surrogate) is repaired to U+FFFD and
  delivered.
- **Exit code:** 0 after an orderly `shutdown`, 1 when the session ends any
  other way (the client closes the connection, or sends `exit` without
  `shutdown`), with the reason on stderr.

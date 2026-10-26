# Bynk LSP — Specification

A Language Server Protocol implementation for Bynk. Provides syntax highlighting (via tree-sitter, specified separately in `bynk-tree-sitter-spec.md`), live diagnostics, hover, go-to-definition, formatting, and a status-bar integration. Initial scope is VS Code only.

This is the first tooling increment for Bynk — a pause from language development to make the language usable in practice. The compiler reaches v0.5 (intra-context behavioural layer); the LSP makes that capability accessible through an editor.

---

## 1. Scope

### In scope

- **Project discovery** via `bynk.toml` at the project root.
- **Tree-sitter syntax highlighting** (specified in `bynk-tree-sitter-spec.md`).
- **Live diagnostics** — compile errors and warnings shown as the user types, with debouncing. Configurable to on-save for users on slow machines.
- **Hover** — type information, declarations, and doc blocks shown on cursor hover.
- **Go-to-definition** — F12 / Cmd-click jumps from a name to its declaration, across files in a project.
- **Format-on-save** — canonical formatting applied when files are saved; available as a manual command.
- **`bynkc fmt` CLI command** — format Bynk source files from the command line.
- **Document symbols** — outline view of a file's declarations, shown in VS Code's outline pane.
- **Status-bar integration** — VS Code status bar shows the project name and Bynk compiler version.
- **VS Code extension** packaged for local sideload.
- **References & rename** *(v0.25, §3.8–3.9)* — binding-index-backed, project-wide.
- **Quick-fix code actions** *(v0.26, §3.10)* — from the diagnostics' structured suggestions.
- **Workspace symbol search & document highlights** *(v0.26, §3.11–3.12)* — index queries.
- **Inlay hints** *(v0.27, §3.13)* — inferred-type hints from the analysis round's harvested set.
- **Semantic tokens** *(v0.28, §3.14)* — resolution-aware highlighting from the binding index.

### Out of scope (deferred to later tooling increments)

The initial increments deferred several of the items originally listed here;
they later shipped and moved into the capability set above — general completion
(§3.15), code lenses (§3.17), inlay hints (§3.13), and semantic tokens (§3.14).
What remains genuinely deferred:

- Refactorings beyond rename (extract function, inline); non-quick-fix code-action kinds.
- Editor commands beyond what LSP standard provides (no "Bynk: Build" / "Bynk: Run tests" yet — those come later).
- Marketplace publication.
- Editor support beyond VS Code.
- Configuration formats other than `bynk.toml` (no `.yaml`, no `.json` for now).

---

## 2. Project model

### 2.1 Project discovery

A Bynk project is a directory containing `bynk.toml` at its root. The LSP discovers the project a file belongs to by walking upward from that file until it finds `bynk.toml` (or, for a rootless `src/` tree, the nearest enclosing `src/`). This is the **same** attribution `bynkc` gives the file. If none is found before the filesystem root, the LSP treats the file as a single-file project (no workspace features).

**A request routes to its file's project, not to a workspace folder** (v0.182, §2.4): the server holds a *map* of projects keyed by discovered root, so several projects — a monorepo's packages, two folders in one window — coexist, each analysed, versioned, and published independently.

### 2.2 `bynk.toml` schema

```toml
[project]
name    = "my-bynk-project"     # required
version = "0.1.0"               # optional, free-form

[paths]
include = ["src", "tests"]      # source trees the compiler discovers (default: src/ and tests/ when present, else the project root — ADR 0147)
exclude = []                    # extra trees to skip; out/ and node_modules are always excluded

[fmt]                           # read by the language server *and* the fmt CLI
indent             = "tab"      # "tab" (default) or "spaces"
indent_width       = 2          # spaces per level; only consulted with "spaces" (default: 2)
max_line_width     = 100        # soft target for line wrapping (default: 100)
trailing_comma     = true       # trailing comma in multi-line lists (default: true)

[lsp]
diagnostics_mode   = "live"     # "live" (default) or "on_save"
diagnostics_debounce_ms = 300   # debounce delay for live mode (default: 300)
```

All fields are optional. A minimal `bynk.toml` is:

```toml
[project]
name = "my-bynk-project"
```

The LSP applies defaults for everything else. The file's *presence* is what marks the project root; its contents are advisory.

Future configuration alternatives (`.yaml`, `.json`) are not supported in this increment but the schema is designed to translate cleanly.

### 2.3 Startup analysis and watchers (v0.183)

On `initialized`, the server **discovers every project under the workspace folders** — a bounded walk for `bynk.toml` (skipping `out`, `node_modules`, `target`, `.git`, and dot-directories, and guarding against symlink cycles), plus any manifest at or above a folder — and analyses each, so diagnostics appear at activation **without a file being opened**. A workspace folder added later (`workspace/didChangeWorkspaceFolders`) is warmed the same way, and a `bynk.toml` created later is warmed via its watcher event. Each project's file set is exactly what `bynkc` compiles: the manifest's `[paths] include`, with `exclude` honoured (§2.1, [ADR 0201](../decisions/0201-the-lsp-analyses-the-compilers-project-model.md)).

The server **registers its file watchers itself**. On `initialized`, if the client supports `workspace/didChangeWatchedFiles` dynamic registration, the server registers `**/*.bynk` and `**/bynk.toml` (one global registration, folder-independent — §2.4). A `.bynk` change re-analyses the owning project; a `bynk.toml` change reloads that project's config first. A client that does *not* support dynamic registration supplies the watchers itself (as the VS Code extension did before v0.183, and no longer needs to). Either way the server is notified — it no longer depends on a specific client to provide watchers.

### 2.4 Per-workspace state (v0.182)

The server implements the workspace-folders capability it advertises: one editor window may hold many projects at once. The model (settled as Q4 of the LSP-foundations track):

- **Routing is by discovered project root, not by workspace folder.** A URI routes to its nearest enclosing `bynk.toml` (§2.1) — so nested or overlapping folders raise no ambiguity (each file lands in its nearest project), a folder holding two `bynk.toml` projects yields two projects, and two folders sharing one project share one entry. Workspace folders are **discovery seeds** — they bound where the server looks for and prunes projects — never the routing key.
- **A file under no project stays in single-file mode** — per-buffer diagnostics, no cross-file navigation. Requests that need an index decline for it; they do not error.
- **`workspace/didChangeWorkspaceFolders`** adds and removes seeds. A project no longer reachable from any remaining folder is dropped and its diagnostics cleared — unless it still holds an open buffer, which retains it until the last buffer closes.
- **One global `workspace/didChangeWatchedFiles` registration** (`**/*.bynk`, `**/bynk.toml`), registered dynamically by the server at `initialized` (§2.3), covers every folder; the folder set does not change it, so folder changes never re-register.

Each project carries its own `bynk.toml` config, analysis round (with its own freshness generation, §3.2.1), and published-diagnostics set. `workspace/symbol` (§3.11) is the one cross-project query — it aggregates over every project.

---

## 3. LSP capabilities

### 3.1 Syntax highlighting

Provided via a tree-sitter grammar (see `bynk-tree-sitter-spec.md`). The VS Code extension ships the compiled tree-sitter grammar. VS Code natively supports tree-sitter for syntax highlighting when configured.

The tree-sitter grammar covers all Bynk syntactic forms from v0 through v0.5. Highlighting groups follow standard tree-sitter conventions (`@keyword`, `@type`, `@function`, etc.) mapped to VS Code theme tokens.

### 3.2 Live diagnostics

The LSP server runs the existing Bynk compiler on the project's source corpus and reports errors and warnings as LSP diagnostics back to the editor.

**Behaviour (REVISED v0.24, ADR 0052 — project-wide):**
- With a project root (`bynk.toml`): every change triggers a **debounced
  whole-project analysis** via `bynkc::diagnose_project` — non-bailing
  (every file's diagnostics, not the first failure's), **overlay-aware**
  (open buffers layered over disk, so unsaved edits are diagnosed), and
  **file-attributed** (collection-point tagging; no `Span` change).
  Context files get full resolve/check diagnostics — the pre-v0.24 server
  resolved/checked `Commons` units only.
- Publish: **all-and-clear** — every file with diagnostics is published;
  every file that carried diagnostics last round and is now clean gets an
  **empty publish**. The publish/clear diff is a pure function
  (`bynk-lsp/src/publish.rs`), unit-tested without a transport. **Each publish
  carries the document version the round analysed the file at** (v0.179, the
  freshness contract §3.2.1 — a disk-only file carries none), so a client drops
  a range whose buffer has already moved past it.
- **Positions convert against the analysed snapshot** — `diagnose_project`
  returns the per-file text it analysed; spans never convert against a
  newer buffer (the analyse→publish window is real; debounce narrows but
  does not close it).
- Project-level diagnostics with no single owning file (group/cycle/
  directory validations) surface on `bynk.toml` at position 0:0.
- Single-file mode (no `bynk.toml`): the per-buffer `diagnose` path — also
  debounced (v0.184), so a burst runs one `diagnose`, not one per keystroke.
- Debounce (v0.184, one scheduler over both modes): a single generation-based
  debounce at the configured `[lsp] diagnostics_debounce_ms` (default 300 ms).
  A typing burst produces one analysis. Before v0.184 the project path
  debounced twice — the configured delay *plus* a hardcoded 200 ms — and
  single-file mode had no generation; both are now the one scheduler. A round
  already in flight when a newer edit arrives is not aborted (a whole-project
  analysis runs on a blocking thread and cannot be cancelled) — its result is
  discarded at commit instead, never published. Incremental/salsa-style
  recompute is deferred — full re-analysis is acceptable at current scale.
- Reported back via `textDocument/publishDiagnostics`.

**Severity levels:**
- *Error* — compile errors that prevent valid output (type errors, unresolved references, etc.).
- *Warning* — compile warnings (unused capabilities in `given`, orphan doc blocks, etc.).
- *Hint* — suggestions (not currently used; reserved for future).

**Error recovery:** The LSP shows *all* errors in a file, not just the first one. This requires the parser and checker to recover and continue after errors rather than bail. If the existing compiler doesn't do this fully, extending it is part of this increment.

**Diagnostics include:**
- Primary message (the error description).
- Primary range (the source span).
- Secondary ranges where helpful (e.g., the conflicting declaration, the type mismatch source).
- Error category code (e.g., `bynk.types.if_branch_mismatch`) included in the diagnostic for filterability.
- **Code explanation link (`codeDescription`, #853).** When the compiler curates an explanation for a code, the diagnostic carries `codeDescription.href` — a link to that code's Book concept page — so the editor renders the code as a clickable link in the Problems panel and hover. The link is composed from the compiler-owned `code → { blurb, href }` mapping (`bynk_syntax::diagnostics::EXPLANATIONS`), the same table that backs the `bynk explain <code>` CLI, so the two surfaces never drift. Coverage is incremental: a code with no curated explanation carries no `codeDescription` (the designed graceful-fallback state — no link, no error), never a broken one.

**Configuration:** Users can set `[lsp].diagnostics_mode = "on_save"` to disable live diagnostics. In on-save mode, diagnostics run only when the file is saved.

### 3.2.1 The freshness contract (v0.179, ADR 0202)

An **index-backed request always answers against the buffer the client
currently holds.** Each analysed file records the document version it was
analysed at (`Analysis.versions`, keyed project-relative). A request for a file
whose open buffer has moved past the last round triggers a **refresh** — a round
over the current buffers — before the request is answered, so a position is
never resolved against text the user has already edited past.

- **Uniform for cursor requests.** Every handler that resolves the client's
  cursor against the analysed snapshot routes through one strict gate
  (`Backend::analysis_for`). The pre-v0.179 split — some handlers on the cached
  round, `rename` alone on a fresh one, several reading `state.analysis` raw —
  is gone; freshness is the contract, not a per-handler choice. `completion` and
  `document_symbol` remain outside it (they resolve against **live** buffer text
  directly, so there is no snapshot to be stale). The pull-based *decoration*
  requests take a distinct, non-refreshing gate (next bullet).
- **Stale-while-revalidate for pull-based decorations (#733).** The requests the
  editor auto-fires on every `didChange` — `semanticTokens`, `inlayHint`,
  `codeLens`, `documentLink`, `codeAction` — go through `committed_analysis`,
  which serves the **last committed round as-is**, without forcing a refresh.
  This is safe where the strict gate is required elsewhere because these handlers
  resolve nothing against the client's live cursor: every range and span they
  emit converts against the round's own snapshot (or, for `documentLink`, against
  live text plus the project-level `unit_sources` map). A committed round lagging
  the buffer by at most one debounce cycle is therefore internally consistent —
  and forcing a whole-project round on each keystroke was defeating the debounce,
  the latency growing with project size. The revalidation is the debounce round
  already scheduled by the edit; when it commits, the server nudges the client to
  re-pull via `workspace/{semanticTokens,inlayHint,codeLens}/refresh` (each sent
  only if the client advertised `refresh_support`). Cold start (no round yet)
  answers empty, and the client re-pulls on the first refresh nudge.
- **A multi-file edit needs more than the per-file gate.** `rename` emits
  versioned edits across every file referencing the symbol, so it refreshes
  **every open buffer** (`analysis_covering_open_buffers`), not just the
  cursor's — a buffer edited but not under the cursor would otherwise be stamped
  with a stale version and the client would reject the whole rename. The per-URI
  gate is right for a request that reads one file, not one that writes many.
- **Decline, not stale.** The gate returns nothing — the request answers empty —
  only when it cannot reach the client's version: single-file mode (no project),
  a file outside every `include` root (never a snapshot key), or a concurrent
  edit that raced the refresh (rare; the next request is current). It never
  returns a snapshot older than the client's buffer.
- **Affordable, and coalesced.** A round is whole-project (no incremental layer)
  but fast — single-digit milliseconds for a typical project, under ~100 ms at
  200 files, measured before this was settled (Q3). A request-driven refresh
  supersedes the pending debounce round rather than racing it, and concurrent
  requests after one edit share a single refresh (a serialising lock). The
  scaling cliff — a many-hundred-file project would make a per-request full
  round noticeable — is the one thing that would revisit this; not present at
  Bynk's scale.

This **revises** the pre-v0.179 rule that requests convert against "the analysed
snapshot" of "the cached round": for an index-backed request the snapshot is now
*made* current before it is read. The rule still holds for **diagnostics** — a
published range converts against the round's snapshot (the analyse→publish window
is real) and now carries that round's version so the client can reject it.

### 3.3 Hover

On cursor hover over a name, the LSP returns information via `textDocument/hover`. Specifically:

**On a type name:**
- The type's declaration (the source form, formatted).
- Any attached doc block.
- For exported context types: the visibility (`opaque` or `transparent`).
- For opaque types: a note that the representation is hidden.

```
type Money = { minorUnits: Int where NonNegative, currency: CurrencyCode }

---
The Money type represents an amount in a specific currency...
---
```

**On a function or method name:**
- The full signature.
- The doc block.
- For methods: whether instance or static.
- For service handlers: the `given` clause.

**On a variable / `let` binding:**
- The inferred or declared type.

**On a field access:**
- The field's declared type, plus any inline refinement.

**On a capability name:**
- The capability's declaration with all operations.
- The doc block.
- The available providers in the current context.

**On a capability operation** (`Logger.info`, v0.166):
- The operation's signature, attributed to the capability that declares it.
- The doc block.

**On an actor name** (v0.166) — the declaration, as written:
- For the normal form: the `auth` scheme with its config, and the `identity` type.
- For the refinement form (ADR 0091): the base actor and the `where` claim predicate.
- The doc block.

```
actor User { auth = Bearer(secret = "AUTH_JWT_SECRET"), identity = UserId }
```

**On a keyword:** nothing (hovers only fire on identifiers).

**References, not just declarations (v0.165, ADR 0190).** Hover resolves a name's *use*, not only the line that declares it — the three gaps #611 found were all inside an `agent` handler body:

- **An agent-state reference** — a `store`/`key` field used in a handler body or an invariant/transition predicate (a bare read `lastSeq + 1`, a `:=` write target, a store op's receiver) — renders exactly what its declaration renders (ADR 0161's field signature plus the contextual-keyword doc). State fields are not index symbols and not `let`/param locals, so this resolves **by name**, scoped to the positions where a bare name actually binds to state: the declaration region is excluded, so `@indexed(by: id)` — a field of the *stored value* — never masquerades as a same-named `key id`. A local of the field's name shadows it, matching the checker's by-provenance dispatch.
- **A record-construction field label** (`Stored { seq: …, title: … }`) renders the `Stored` field. The checker already records labels as `Field` refs keyed `"Type.field"` (ADR 0069), so the index resolves the offset; the renderer now has an arm for the compound key. Before, it fell through to the locals path and a `title:` label rendered the same-named handler param — a *wrong* hover, not a missing one.

  **Every kind the index carries now has an arm (v0.166, ADR 0191).** ADR 0190 closed `Field` and left `Method` (`"Counter.bump"`), `CapabilityOp` (`"Cap.op"`) and `Actor` resolved-but-unrendered, on a measurement that they fell through to *nothing*. Re-measured at reference offsets, two of the three fell through to a **wrong** answer instead — the fall-through passes rung 4, the lexical name match over the live buffer, which answers before `qualified_callee_at` is ever reached. `Method` matched a method on its *bare* name, so in a file declaring `fn Counter.bump` and `fn Gauge.bump` every `bump` rendered `Counter.bump`; and `resolve_label` is not project-scoped, so a context's own `capability Logger { fn info(message: String) }` rendered the embedded `platform.log.Logger.info(msg: String)`. Both are gap B again: a correct structural resolution discarded for a guess. See §3.3's kind list above for what each now renders.

  **The property is pinned, not just asserted (ADR 0191 D4).** A missing arm has now shipped silently twice, with no test failing either time, so a coverage guard sweeps every key the real index produces for a fixture declaring all ten `SymbolKind`s through the real renderer and fails on any that answers `None`; an exhaustive `match` over `SymbolKind` sits beside it, so a new kind stops the crate compiling until someone declares one for the sweep to see. The pin is in the tests, not the ladder — rung 1 still guards on the renderer returning `Some`, so an unrendered key would still fall through at runtime; it simply cannot reach `main` unnoticed.
- **A `store` field's operation** (`items.put(…)`) renders the operation's signature over the field's declared kind. The signatures come from the enumerable `bynk_check::store_ops` registry — the storage analogue of `kernel_methods` (§3.3/ADR 0063), pinned to the checker's `check_store_*_op` dispatch by a drift test, and generic in the kind's `K`/`V`/`T` (the field's declared kind grounds them, so it is rendered alongside). Entry operations only: a `Log`'s time-window roots are covered, the lazy-`Query` vocabulary they feed into is not.

Hover over a **value-receiver** method (`xs.fold`) on an ordinary value remains unresolved — that needs signature help's receiver-typing path, not a structural lookup.

Hover content is rendered as Markdown by the editor. The LSP returns `MarkupContent` with `MarkupKind.Markdown`.

**Markdown layout.** Hover content follows a consistent structure:

1. A fenced code block (```` ```bynk ```` ) containing the declaration's source form (formatted via the canonical formatter so it matches the project's style).
2. A blank line.
3. The attached doc-block content (if any), as plain Markdown — with any `[Name]`/`[Owner.member]` intra-doc link inside it that resolves against the declaration's own unit (§3.21) rewritten to a Markdown link; an unresolved one renders exactly as written. This rewrite applies only to rung 1 (the binding-index path, above) — the lexical rungs keep plain doc rendering.
4. For types with additional metadata (exported visibility, opaque representation note, etc.), a short list below the doc content.

Example for a `Money` record type:

````markdown
```bynk
type Money = {
  minorUnits: Int where NonNegative,
  currency:   CurrencyCode,
}
```

The Money type represents an amount in a specific currency. The minorUnits
field is the smallest indivisible unit of the currency.

— exported transparently from `commerce.payment`
````

Hover content stays compact — typically under twenty lines. For declarations that are long (e.g., a capability with many operations), the full declaration is rendered; the editor's hover popup handles overflow with scrolling.

**Wire contract (#855).** Hovering a resolved `Service` symbol (rung 1, above) that declares exactly one handler gets a **wire-contract appendix** appended after everything the declaration's own rendering already produced — a `---` rule, a **Wire contract** heading, then the request envelope (all three cases, §3.26), the cross-context contract form + hash (`on call` only), and (HTTP handlers only) the reachable response status set. This changes only the *tail* of what rung 1 already returned `Some` for; a service with more than one handler gets no appendix (there is no way to tell, from a bare name hover, which handler's contract the author means), which is the pre-#855 behaviour, unchanged — never a wrong answer, only sometimes an absent one.

A handler's own **header** — the `on` keyword, the HTTP method/handler-kind token, and any route string literal — resolves through no rung before #855 (`GET` lexes as a bare `Ident` with no index symbol, and no top-level declaration is named after it). #855 adds a new rung, tried after the locals rung and before the lexical fallbacks (the full ladder is enumerated in `bynk-lsp/src/hover.rs`'s module doc), guarded to exactly the handler's header byte range: from the handler's span start up to (but excluding) its first param's span start, or its body's span start with no params. That range holds only tokens no other rung can ever resolve, so the new rung is provably unable to shadow rung 1 — a param's *type* name (`client: ClientId`) always sits past the guarded range and keeps resolving to its declaration exactly as before. Hovering the header renders the same wire-contract body as the appendix above, standalone (headed by the service/handler name rather than appended after a declaration render) — see §3.26.

### 3.4 Go-to-definition

`textDocument/definition` returns the location of a name's declaration.

**Resolution:**
- Type names → the `type` declaration.
- Function names → the `fn` declaration.
- Method names (`value.method()`) → the method's `fn TypeName.method` declaration.
- Field names (`record.field`) → the field's declaration in the record type.
- Variant names (`Pending`, `Ok`, etc.) → the variant's declaration in the sum type.
- Capability names → the `capability` declaration.
- Service operation names → the service's `on call` handler (the `on` keyword's location).
- Agent names → the `agent` declaration.
- Actor names (a handler's `by u: User` clause) → the `actor` declaration (v0.168, #619).

**Cross-file (required).** Definitions in other files within the same project must be resolved. The returned location points to the correct file and source range. This is a hard requirement — the language explicitly supports multi-file commons (v0.3) and context consumes graphs (v0.4); navigation that doesn't cross file boundaries is unusable for any non-trivial project. The LSP's project module (which loads all `.bynk` files at startup) already has the symbol tables needed; the definition lookup walks those tables, not just the open file's local tables.

**Binding-correct via the index (v0.25, ADR 0053).** Definition (and hover) resolve through the project **binding index** first — the use→def edges recorded at the compiler's own resolution sites — so duplicate names in different units navigate to the *bound* declaration, not the first name match. The legacy name-matching walk remains only as a fallback for the few kinds the index still defers (match-arm / `is`-narrowing bindings).

**Local bindings (v0.31, ADR 0064).** A `let`/`let <-` binding or a fn/handler/lambda parameter resolves to its declaration via the per-file **locals** (recorded with scope ranges), tried after the index and **before** the name-matching fallback — so a local navigates to its scope-correct binding, not the first textual match of the name. Match-arm and `is`-narrowing bindings are deferred.

**Imported names:** When a context uses a commons, names from the commons resolve back to the commons declaration (not to the context's rebranded copy — the original source location is more useful).

### 3.5 Formatter

The Bynk formatter applies canonical style to source files.

**Style rules (the defaults; all configurable via `bynk.toml`):**

- **Indentation:** tabs by default. One tab per nesting level. (This is for accessibility — users set their preferred tab width in VS Code; tab-based indentation respects that, space-based indentation does not.)
- **Brace style:** K&R / same-line. `if cond {` on one line, not `if cond` then `{` on the next.
- **Trailing commas:** present in multi-line records, sums, parameter lists, argument lists.
- **Blank lines:** one blank line between top-level declarations (types, functions, services, agents, capabilities, providers). No blank lines between fields within a record or arms within a match.
- **Doc blocks:** immediately above the declaration they document, no blank line between. Doc content has the common indent stripped (per v0.3's clarification).
- **Spacing:** one space around binary operators (`a + b`, not `a+b`); one space after commas; no space inside parens.
- **Line width:** soft target of 100 columns; the formatter wraps long lines where natural (after commas in long parameter lists, at `&&`/`||` boundaries in long expressions, before the calls of a long `.`-chain). For lists with delimiters (parameter lists, argument lists, record fields/constructions, list literals, exports clauses), the formatter emits single-line form when it fits within the line width and multi-line form otherwise. A construct is measured against the *whole* line: the column it starts at, plus the text that will follow it there (a `-> Ret {` signature tail, a closing paren, a match arm's comma). Multi-line form uses trailing commas, except in parameter and argument lists, where the grammar rejects one; single-line form never does. The target is soft: a line whose overflow sits inside an unbreakable token (a long string literal, a `Matches("…")` regex) is left long rather than mangled.

**Idempotency:** Running the formatter twice produces the same result. This is a requirement.

**Preservation:**
- Comments preserve their position relative to surrounding code.
- Doc blocks preserve their content verbatim (only the indentation is normalised).
- The semantic meaning of the code is preserved (same AST after parse → format → re-parse).

**Comment-preservation implementation requirement.** The formatter must not drop line comments (`-- ...`). This is a hard requirement — dropping user comments is data loss, which destroys trust in format-on-save and ultimately in the canonical-style discipline. Implementing this requires the lexer to emit comments as trivia tokens (or similar) so that the parser/formatter can track their positions and emit them in the formatted output. A side-pass scan of original source is acceptable as a fallback but trivia-tracking is the principled approach.

Specifically:
- Comments before a top-level declaration go above the declaration in the formatted output (with the doc block, if any, between the comment and the declaration).
- At the top level (between unit-level declarations, `uses`/`consumes`/`exports` clauses included, and between the unit header and the first of them), a blank line after a comment on its own line is kept, collapsed to one: a comment set apart from the declaration below stays apart, and does not read as describing it (#1884). A comment directly above a declaration stays attached. The same holds inside a service or agent body, between the sections and handlers that are set apart by a blank line. Where items have no blank lines between them (statements, record fields, capability ops, policy fields, `store` fields), a comment always attaches to what follows.
- Comments at the end of a line stay on that line (`expr  -- note`).
- Comments on their own line within a block preserve their position relative to surrounding statements.
- Multi-line groups of comments stay together.

Doc blocks (`---`) are separate from line comments and are already preserved via the AST.

**Integration:**

- **Format-on-save:** the LSP responds to `textDocument/formatting` requests. VS Code with `editor.formatOnSave: true` calls this on every save. The LSP returns the formatted document as a single text edit.
- **Range formatting:** `textDocument/rangeFormatting` formats a selected range. Useful for "format this function." Implemented best-effort — the formatter operates on whole declarations, so the returned range may be slightly wider than requested.
- **CLI:** `bynkc fmt [file...]` (and `bynk fmt`, the same body through the driver) formats files in place. `bynkc fmt -` reads from stdin, writes to stdout. Options resolve in three layers, each overriding the one before: the style rules above, then the project's `bynk.toml` `[fmt]` section, then the flags this run passes (`--indent tab|spaces`, `--indent-width`, `--max-line-width`, `--trailing-comma`/`--no-trailing-comma`). `--check` judges files against those resolved options, so a project on a non-default style has a working CI gate; `--no-config` skips the manifest layer for a run that wants the canonical style whatever project it is pointed at. The manifest is resolved **per input file** — the nearest `bynk.toml` at or above it — so formatting a path outside the current project obeys that project's style, and the CLI and the language server read the section through one shared reader.

### 3.6 Status-bar integration

The VS Code extension shows two status-bar items when a Bynk file is open:

- **Project name** — from `bynk.toml`'s `[project].name`. Clicking opens `bynk.toml`.
- **Compiler version** — the version of the bundled `bynkc` binary. Clicking does nothing (informational).

If `bynk.toml` is missing, the project-name slot shows "no project" (clicking suggests creating one).

The status bar items only appear when the active editor has a `.bynk` file open.

### 3.7 Document symbols

The LSP responds to `textDocument/documentSymbol` requests with a hierarchical outline of the file's declarations. This populates VS Code's "Outline" pane (in the explorer sidebar) and powers the "Go to Symbol in File" command (Cmd-Shift-O).

**Symbols and their kinds:**

The LSP maps Bynk declarations to LSP `SymbolKind` values:

- `commons` declaration → `Module` (top-level container).
- `context` declaration → `Module`.
- `type T = ...` → `Struct` (for records), `Enum` (for sums), `Class` (for opaque types), `TypeParameter` (for refined values).
- `fn name(...) -> T` (free function) → `Function`.
- `fn TypeName.method(...)` → `Method` (nested under the type).
- `capability X { ... }` → `Interface`, with operations as `Method` children.
- `provides X = Y { ... }` → `Object`, with operations as `Method` children.
- `service X { ... }` → `Class`, with handlers as `Method` children.
- `agent X { ... }` → `Class`, with the state block as `Property` children and handlers as `Method` children.
- Record fields → `Field` children of their type.
- Sum variants → `EnumMember` children of their type.

**Hierarchy:**

The top-level container (commons or context) is the root. All other declarations are children. Methods nest under their type; record fields nest under the record; variants nest under the sum.

For multi-file commons or contexts, each file has its own document symbol tree — the LSP returns symbols for the current file only. The outline view shows the contents of the current file.

**Ranges:**

Each symbol carries two ranges:
- `range` — the full extent of the declaration (from the `type`/`fn`/etc. keyword to the closing brace).
- `selectionRange` — the identifier itself (the name being declared).

Clicking a symbol in the outline jumps to the `selectionRange`. The `range` is used for highlighting and breadcrumb display.

**Documentation:**

If a declaration has an attached doc block, its content (truncated to one line if multi-line) appears as the symbol's detail. VS Code shows this alongside the symbol name in the outline.

### 3.8 References (v0.25)

`textDocument/references` returns every reference to the symbol under the cursor, project-wide, from the **binding index** (ADR 0053): the index is assembled from use→def edges recorded at the compiler's resolution sites during the project analysis pass — binding-correct by construction, never name-matched. Two same-named symbols in different units never conflate.

**Coverage.** Top-level named declarations: types, free `fn`s, capabilities, services, agents, providers. **Members (v0.36, ADR 0069)** — instance methods, record fields, and capability ops — are also indexed, each keyed by a compound name (`"Type.method"`, `"Type.field"`, `"Cap.op"`) and recorded already-spelled from the parent type/capability the checker resolved at the use site, then qualified through the same `uses`/`consumes` walk as a cross-file type reference (so a same-named member on two parents stays distinct: `Counter.bump` ≠ `Gauge.bump`). **Agent handlers (#304)** follow the same convention — `"Agent.handler"`, recorded at the dispatch call site (`agentInstance.handler(...)`); service handlers have no per-handler name and stay uncovered. A method **call** (`x.m()`), a **field** in any form (read `r.field`, construction label `T { field: … }`, spread override), a capability-**op** call (local or cross-context), and an agent-**handler** dispatch call are all reference sites — so rename and references are complete. Reference sites include every way such a symbol is named — annotation and static-receiver type positions (`T.of`, `T.Variant`, `T { … }`, pattern qualifiers, `Mock[T]`), fn calls and first-class fn values, `given` clauses (bare, dotted `B.Cap`, flattened), capability op-call receivers, cross-context service calls, the clause lists (`exports opaque/transparent { T }`, `exports capability { Cap }`, `consumes U { Cap }` selections), and references from **test and integration units** (including a test body's `svc.call(…)`). Spans cover the **name segment only** — in `shop.billing.Pay` the reference is `Pay`; in `Counter.bump` the method site is `bump`.

**Local bindings (v0.31, ADR 0064).** `let`/`let <-` bindings and fn/handler/lambda params are **not** in the cross-file index (they are file-local); references for them come from the per-file **locals** instead — the definition plus every use that resolves to it within the binding's scope, recovered by a pure lexer scan over the snapshot (shadowing-safe). References/definition/highlight fall back to this when the index has no symbol at the cursor.

**Deferred kinds** (no entries yet): match-arm / `is`-narrowing bindings, generic type parameters (methods, record fields, and capability ops are now indexed — see above). First-party `bynk.*` units are excluded — they are not user-editable.

Positions convert against the **analysed snapshot** (§3.2's rule); `includeDeclaration` is honoured (the definition site is first). Requests outside a project (no `bynk.toml`) return no results.

### 3.9 Rename (v0.25)

`textDocument/rename` renames a symbol project-wide; `textDocument/prepareRename` (declared via `prepareProvider: true`) validates the position first and **refuses** (returns null) on anything the index does not cover — locals and unit/context names (renaming a unit implies a file move; §3.22 covers that path). Member rename (v0.36): renaming a method, field, or op edits the member segment of the compound key (`"Type.method"`, `"Type.field"`, `"Cap.op"`), never the parent prefix.

**Plan.** The edit set is exactly the index's sites for the symbol — definition plus every reference, name segments only — built against a **fresh analysis** of the current buffers. The new name must lex as a single identifier (keywords refuse).

**Validation — two checks, both correct-by-construction (ADR 0053):**

1. **Collisions by re-analysis.** The candidate edits are applied to an in-memory overlay and the project is re-analysed; any **new** diagnostic (per file + category) refuses the rename. This catches every collision class — same-unit name clash, `uses` import conflicts, flattened-capability clashes — without enumerating them.
2. **Capture/escape by index equality.** Re-analysis alone misses *silent re-binding* (a rename can make an existing name resolve somewhere new with no diagnostic — declared fns shadow fn-typed locals in call position). The re-built index must equal the pre-rename index **modulo the rename**: the renamed symbol's sites are exactly the edited ones, every other symbol's reference set is unchanged (after remapping the edit deltas). Any difference refuses.

A refused rename surfaces as an LSP request error with the reason — never a partial edit, and never a `bynk.*` diagnostic.

**Versioned edits.** The `WorkspaceEdit` uses `documentChanges` with `TextDocumentEdit`s carrying the document **versions captured when the analysed snapshots were taken** (disk-only files carry none). A buffer that drifted past its analysed version makes the client reject the rename rather than mis-apply it.

### 3.10 Code actions (v0.26)

`textDocument/codeAction` offers **quick-fixes** (`CodeActionKind.QuickFix`, the only kind advertised) computed from the **structured suggestions** riding on the diagnostics (ADR 0054): `bynkc` attaches machine-applicable `Suggestion`s — message, span→replacement edits, an `Applicability` — at the diagnosis site, the only place the exact spans and replacement are known. The LSP never re-derives such a fix from a diagnostic's category or message. The one scoped exception is the **capability-aware** family (#852, below): its fix location is a *unit-header* edit that only exists once the buffer is reparsed, and (for auto-import) its resolution is a *whole-project* query over the binding index — neither available at the per-unit diagnosis site — so these are computed in the LSP, keyed on category, but stay honest by reading the flagged name from the source at the **diagnostic's own span** rather than from the message text.

**Keying.** A diagnostic's suggestions are offered when the requested range intersects the **diagnostic's span** — never the edits' spans, which can land far from the squiggle (both `given` fixes do: the diagnostic sits on the usage site or the return type; the edit lands in the clause).

**Serving.** Actions are computed from the **last committed round**, served through the non-refreshing decoration gate (§3.2.1's stale-while-revalidate rule, #733) — `codeAction` fires on selection changes, so it does not force a whole-project round. A request for a file **outside the project**, or one with no committed round yet, returns the empty list. The round retains the per-file diagnostics the suggestions ride on; the request range and those diagnostics both convert against the round's snapshot, and each action's `WorkspaceEdit` is a **versioned** `TextDocumentEdit` against the round's document version, so a drifted buffer rejects the edit rather than mis-applying it. The request range is the client's *live*-buffer selection: converting it against a round up to one debounce cycle behind can momentarily map it to a slightly wrong location (a stray or empty action) until the refresh nudge re-pulls — an **accepted** trade-off, bounded by the versioned edit (never a mis-application), the same as `semanticTokens/range` (§3.14).

**Applicability.** Only `MachineApplicable` suggestions surface as quick-fixes; `HasPlaceholders` exists for a future CLI `--fix` and is never offered as a one-click edit.

**Available fixes.** The seed quick-fixes (ADR 0054) are the `given`-clause ones — add an undeclared capability, remove an unused one. v0.40 (ADR 0073) adds the **`InRange`-swap**: an inverted `InRange(hi, lo)` refinement bound (`bynk.types.inverted_range`) offers a two-edit fix that swaps the bounds in place (ints and floats; float lexemes preserved), backed by per-bound source spans recorded in the AST.

**The seed catalogue (v0.26):** remove an unused capability from the `given` clause (`bynk.given.unused_capability`) and add an undeclared one (`bynk.given.undeclared_capability`, bare and cross-context `B.Cap` — the cross-context entry inserts the canonical context path). Both edits are **list-aware**, authored in the checker: removal takes one adjacent comma and surrounding space with it, removing the *only* capability deletes the `given` keyword too, and adding inserts `, Cap` after the last entry or synthesises ` given Cap` after the handler's return type when the clause is absent. The result never double-commas, leading-commas, or leaves a dangling `given`.

**Extract variable (v0.213, ADR 0239).** A selection offers `CodeActionKind::RefactorExtract` when it sits inside an `fn`/handler/provider-op/test-case body: the smallest AST expression node whose span fully contains the selection is bound to a fresh `let` inserted immediately above its enclosing statement (or block tail), and the selection's span is replaced with the new name. Descending through `if`/`match`/block-expression boundaries resets the insertion point to that nested block's own statement/tail — extracting from inside an `if` branch inserts the `let` there, not above the whole `if`. Computed **live** — it reparses the current buffer text rather than reading a cached AST (`Analysis` retains none; same posture as `structure.rs`'s folding/selection ranges) — but served through the same `committed_analysis` gate as the rest of §3.10 (extraction never crosses files, so it needs no whole-project refresh). The placeholder name (`extracted`, `extracted2`, …) is chosen by a whole-file word-boundary scan, not scope-aware binding analysis — a client's rename-on-extract is the expected next step for a better name. **Tooling delta (ADR 0156):** hover, completion, semantic tokens, and signature help are unchanged.

**Extract function (track #800, settled capability-free-only).** A selection offers a second `CodeActionKind::RefactorExtract` action — "Extract function `extractedFn`" — reusing extract-variable's exact selection algorithm, but only within a `Commons`/`Context` file's top-level `fn`/`Provider`/`Service`/`Agent` item (a new top-level `fn` needs somewhere top-level to live, so an `Adapter` or a `Suite` test case offers nothing). The selected expression's free identifiers — its `Ident` references whose nearest enclosing local binding sits outside the selection, via the same `locals` scope data hover/completion already use — become the new `fn`'s parameters; the selection itself becomes the fn's body (its tail expression), and the call site replaces the selection. `fn` has no `given` clause to propagate a capability-using body into, unlike `Handler`/`Provider` — a language change deliberately not taken on here — so the action is **capability-free-only**: it declines whenever the selection's site carries any recorded capability requirement (the same `requirements` ledger §3.9's ghost `given` inlay hint reads), covered or not. It also declines whenever a parameter or return type isn't available (both `expr_types` and `locals`' rendered types are Ok-path captures, ADR 0063's clean-file ceiling) or whenever two distinct outer-scope bindings share a free variable's name (a rare nested-shadow collision safer to decline than guess).

**Extract function, multi-statement (#813).** The selection may also resolve to a contiguous run of one or more full statements from a single block, optionally including the block's tail, when the selection's span aligns exactly with statement boundaries (whitespace-trimmed at either end, so a "select these lines" editor gesture matches) — a selection that doesn't align falls back to the single-expression algorithm unchanged. Free-variable synthesis is the same walk, seeded from every selected statement's values instead of one expression. When the tail is included the new `fn`'s return type is the tail's type, as before; when the run stops short of the tail, the new `fn` returns `()` — or `Effect[()]` when the run itself contains a `~>`/`do`/`<-` statement (recursively, through nested `if`/`match`/block bodies), so those forms stay legal in the lifted body — with no explicit tail, the same implicit-unit-tail shape the parser already synthesises for any statements-only block (v0.146, ADR 0170). Because Bynk has no expression-statement form, a tail-excluded run's call site can't stand alone as a bare `extractedFn(args)`: it becomes `let _ = extractedFn(args)` (pure) or `do extractedFn(args)` (effectful) instead. The action declines whenever a `:=` (`Cell` store write) statement falls inside the run — a lifted top-level `fn` has no store fields, so this would always fail to typecheck — or whenever a binding the run introduces (a `let`/`<-` name) is still referenced later in the same block: lifting it away would strand that reference.

**Capability-aware quick-fixes (#852).** Four fixes that repair a boundary/resolution diagnostic, split by where the information they need lives.

- **Missing record field(s)** — checker-authored, like the seed catalogue. `check_record_field_set` attaches to each `bynk.resolve.missing_field` an "add field `x`" fix and, when several fields are missing, an "add all missing fields" convenience **[DECISION B]**. Both insert a *valid default value* per field type (`Int`→`0`, `Float`→`0.0`, `String`→`""`, `Bool`→`false`, `Option`→`None`, `List`→`[]`), so the applied buffer re-checks clean. A field with an inline refinement or a user-named type (possibly refined/sum/opaque) has no synthesised default and is not offered; "add all missing" is withheld unless the whole missing set is defaultable. The insertion anchor is a fmt-stable zero-width span after the last provided field, or just inside the closing brace when the literal is empty.
- **Add missing `consumes`** — LSP-computed. On `bynk.resolve.unconsumed_context`, insert `consumes <chain>` for the cross-context chain the diagnostic flagged.
- **Auto-`uses`/`consumes`** (Bynk's analogue of auto-import) — LSP-computed. On `bynk.resolve.unknown_name` / `unknown_type`, query the binding index for every unit that declares the name and offer **one action per candidate [DECISION A]**, never a guess: a commons declaration → `uses <commons>`; a capability exported by a context/adapter → `consumes <context> { <name> }`. The env-free `bynk` surface capabilities (`Clock`, `Random`, `Logger`, `Fetch`, `Secrets`, `Locale`) are first-party synthetic symbols excluded from the index, so they are offered from an explicit list kept in sync with the adapter source. Commons-vs-context is inferred from a candidate unit's own index symbols (a context/adapter additionally declares a service/agent/actor/capability/provider); an unclassifiable unit is conservatively not offered for `uses`. A capability auto-`consumes` resolves the name but leaves the ordinary `given` requirement, whose existing add-`given` fix then reaches clean — the two compose as the intended repair sequence.
- **Clause placement [DECISION C].** A new clause inserts fmt-stably — after the last existing clause of its kind, else on a fresh line under the unit name (before any `uses`, the consumes-first order) — so the edit round-trips through `fmt` unchanged; a braced `consumes <unit> { … }` for the same target is extended in place rather than duplicated.

All four emit **versioned** `WorkspaceEdit`s and are offered only when sound and non-redundant (a clause already present, or one the current unit cannot host — a commons cannot `consumes` — is dropped). The LSP fixes reparse the committed round's buffer for the header, and read the index from the same committed round, like the extract refactors. **Tooling delta (ADR 0156):** hover, completion, semantic tokens, and signature help are unchanged.

### 3.11 Workspace symbols (v0.26 rider)

`workspace/symbol` enumerates the binding index's **definitions** (ADR 0055) — the same coverage as §3.8: types, free fns, capabilities, services, agents, providers. The query is a case-insensitive substring match on the symbol name (an empty query lists all), results ordered by (name, unit) with the owning unit as the container name. Positions convert against the analysed snapshot. **Workspace-wide across projects (v0.182, §2.4):** the query aggregates over *every* open project — the one cross-project handler — so a symbol in any package of a multi-root window is reachable.

### 3.12 Document highlights (v0.26 rider)

`textDocument/documentHighlight` returns the symbol-at-cursor's occurrences **within the active file** — the §3.8 references query, file-scoped, definition included (ADR 0055). The index does not distinguish read from write references, so highlight `kind` is omitted. **Local bindings** (v0.31) highlight via the same per-file locals resolver as references. Requests on still-uncovered kinds (methods, fields, op names) return no highlights.

### 3.13 Inlay hints (v0.27; v0.39, ADR 0072)

`textDocument/inlayHint` returns hints for the request's visible range, of two anchor flavours driven by a `HintKind` discriminator on the harvested set:

**Inferred-type hints** (v0.27, `HintKind::Type`) — `let` and `let <-` bindings and lambda parameters whose annotation is **absent** (an explicit annotation needs no hint; `_` binds nothing and gets none). Each anchors at the **end of the binding name**, label `: T` with `T` in Bynk surface syntax via the checker's display rendering (`List[Int]`, `Option[String]`, `Int -> Int`); a `let <-` hint shows the **peeled `Effect[T]` payload**. `kind` is `Type`, no padding — the separator is part of the label, so it reads as source syntax (`x: Int`). *(The proposal sketched padding-left; implementation drops it.)*

**Parameter-name hints** (v0.39, `HintKind::Parameter`) — at a call argument, the callee's parameter name before the argument: anchored at the argument span's **start**, label `name:`, `kind` `Parameter`, `padding_right` (renders `count: 5`). Recorded by the checker at the free-fn, generic, method, and cross-context op/service argument loops. **Suppressed** when the hint would be noise — the `_`/`self` placeholders, or an argument that **is the identically-named identifier** (`f(count)` for parameter `count`); literals and complex expressions always get it. Local capability-op dispatch stores parameters type-only (no names) and gets none.

**Generic-instantiation hints** (v0.39, `HintKind::Type`) — at a generic call the user wrote *without* type arguments, the **inferred** ones after the function name: `identity`⟨`[Int]`⟩`(5)`. Recorded at the end of `check_generic_call` from the ground substitution, in type-parameter declaration order, anchored at the function-name span (a `Type`-kind hint, label `[A, B]`). Shown only when the call omitted the arguments (an explicit `identity[Int](5)` gets none) and every type variable resolved.

**Harvesting (ADR 0056).** Hints are a curated per-file set recorded by the **checker** at each binding site as it computes the binding's final type — never a tool-side re-inference, and not the raw typed model (which cannot position a hint). The sink is a `&mut` parameter (the `RefSink` shape), so recorded hints **survive a transient type error** at every site the checker still reaches; a fn-body error short-circuits that file's v0.5 declaration pass, so its **handler-body** hints are suppressed until the error clears. Synthetic and test/integration files record nothing.

**Serving.** Hints are served from the **last committed round** through the non-refreshing decoration gate (§3.2.1's stale-while-revalidate rule, #733), like code actions (§3.10) — the editor re-fires `inlayHint` on every edit, so it does not force a round; the `workspace/inlayHint/refresh` nudge on round-commit re-pulls the fresh hints. A request for a file outside the project, or one with no committed round yet, returns the empty list — as does a file whose group's composition failed (no analysed model). The visible range and the hint positions convert against the analysed snapshot (§3.2's rule). The server always produces hints; visibility is the client's (the editor's built-in inlay toggle; a `bynk.inlayHints.enable` extension setting is a B-1 surface item). `inlayHint/resolve` is not declared — labels are computed eagerly.

### 3.14 Semantic tokens (v0.28)

`textDocument/semanticTokens/full` (and `…/range`) returns resolution-aware tokens for the **index kinds** — the classification tree-sitter / the extension's TextMate grammar *can't* make (is this `Foo` a type, a capability, or a value?). Tokens are **additive over the client's syntactic layer**: keywords, literals, comments, and the uncovered identifiers (locals, params, generic type parameters — not in the index) keep their syntactic colour.

**The legend (frozen — ADR 0057).** The legend's array order **is the wire encoding**; entries are append-only and never reordered (a stability test pins both arrays):

| | |
|---|---|
| **Token types** | `type`, `function`, `capability`, `service`, `agent`, `provider`, `variable` |
| **Token modifiers** | `declaration`, `refined`, `opaque`, `platformNative` |

`type`/`function`/`variable` are the standard LSP types; the other four are **custom** (theme defaults ship with the extension — a B-1 item; unthemed clients fall back to the syntactic colour). `declaration` marks a `def` site (references carry none); `refined` requires a refinement **present** (`type X = Int` is a plain alias and carries neither `refined` nor `opaque`); `opaque` is orthogonal, so `opaque B where …` carries both; `platformNative` marks symbols declared by a platform unit (e.g. `bynk.cloudflare`'s `Kv`).

**Local bindings (v0.31.1, ADR 0064).** `let`/`let <-` bindings and fn/handler/lambda params (and their uses) carry the standard **`variable`** token — appended to the frozen legend at index 6, so existing indices are unchanged and VS Code themes it without an extension declaration (the legend-drift test adds `variable` to its standard-types allowlist). The producer merges local occurrences (the def carries `declaration`) into the same sorted stream as the index symbols — disjoint, since locals are never top-level. Occurrences come from the same per-file scope-resolved lexer scan as references (§3.8). A `parameter`-vs-`variable` split and match-arm/`is` bindings are later refinements.

**Sources (ADR 0057).** A pure `index_queries` producer reads **two** sources from the cached round: `ProjectIndex.symbols` (user-defined defs + refs) and **`ProjectIndex.foreign_refs`** — references to first-party (`bynk.*`) symbols, which `symbols` deliberately drops (synthetic defs point at files not on disk; definition/rename/workspace-symbol must never surface them). The side table is tokens-only; the v0.25 navigation invariants on `symbols` are untouched. `test`/`integration` files' references are in the index, so semantic tokens light up test files too.

**Serving & encoding.** Tokens are served from the **last committed round** through the non-refreshing decoration gate (§3.2.1's stale-while-revalidate rule, #733) — the editor re-fires `semanticTokens` on every edit, so it does not force a round; the `workspace/semanticTokens/refresh` nudge on round-commit re-pulls the fresh tokens (non-project file, or no committed round yet → empty). Positions and the `range` request convert against that round's snapshot — the `range` variant's window is the client's live-buffer range, so up to one debounce cycle behind it can momentarily cover slightly-off tokens until the nudge re-pulls (the accepted trade-off noted for code actions, §3.10). The token array is delta-encoded per the protocol — relative line/char, position-sorted (name segments never overlap), lengths in UTF-16 code units. The **`delta`** request variant is not declared (a later optimisation).

**Client theming (v0.29, ADR 0058).** The custom token types (`capability`/`service`/`agent`/`provider`) and modifiers (`refined`/`opaque`/`platformNative`) render with **no colour** under default themes unless the *client* declares them. The VS Code extension therefore declares them in `contributes.semanticTokenTypes` / `semanticTokenModifiers` (each custom type with a standard `superType` — `interface`/`type`/`class`/`function` — so semantic-highlighting themes colour it) and maps fallback TextMate scopes in `contributes.semanticTokenScopes` for theme without semantic rules. The declared **names are a cross-component contract** with the server's frozen legend — they must match exactly, or those tokens silently go unthemed — enforced by a `bynk-lsp` test that parses the extension's `package.json` against `semantic_tokens_legend()` (the single source of truth). Token *visibility* is the client's: the built-in `editor.semanticHighlighting.enabled`, with no Bynk-specific toggle.

### 3.15 Completion (v0.17; positional in v0.30)

`textDocument/completion` returns context-keyed candidates. **Context detection is lexical** — it keys off the line up to the cursor, not the parse tree, because completion fires mid-edit when the buffer rarely parses; **candidates are semantic** (drawn from parsing the *other* project files with recovery, plus the static `bynkc` registries). The `completion::complete(line_prefix, doc_text, src_root)` producer is a pure function, fully unit-tested; the handler builds `line_prefix` and maps `CompletionKind` → `CompletionItemKind`.

**Lazy docs (`completionItem/resolve`, slice 5).** The list carries a one-line `detail` (the signature) eagerly; the richer **`documentation`** (markdown — doc comment + full rendering) is filled in only when an item is *focused*, via `completionItem/resolve`, so the initial list stays cheap. Each item stashes its document URI in `data` (a resolve request carries only the item, not a position); `completion_resolve` looks the symbol up by `label` through the **same hover renderer** (`symbols::describe_symbol`, local file → project-wide §3.4 → the embedded first-party sources). It is a no-op for an item that names no declared symbol (a keyword, kernel method, or local), whose `detail` already suffices. **First-party docs (slice 9):** hover and this resolver share a `describe_firstparty_symbol` fallback that scans the embedded first-party sources — the single `firstparty::FIRSTPARTY_SOURCES` list (`bynk`/`bynk.cloudflare`/`bynk.list`/`bynk.map`/`bynk.string`/`bynk.locale`/`bynk.locale.types`) — so stdlib combinators, the `bynk` surface, and the locale API (which `walk_bynk_files` never sees) surface their signature, and any `---` doc block once the sources carry one. Auto-import via resolve is deferred. The detail strings themselves are **typed signatures** (params + return, via `type_ref_str`) for capability operations as well as free functions, matching hover and signature help.

The recognised contexts and their candidate sources:

| Context (lexical trigger) | Candidates | Item kind |
|---|---|---|
| `consumes <prefix>` (v0.17) | consumable units (contexts/adapters + `bynk`) | `MODULE` |
| `consumes U { … ` (v0.17) | the capabilities `U` exports | `INTERFACE` |
| `given … ` (v0.17) | in-scope capabilities (local, flattened, `U.Cap`) | `INTERFACE` |
| **type position** (`: T`, `-> T`, `[ … ]` type args) (v0.30) | built-in types + `bynk`-surface transparent types + project `type` decls | `STRUCT` |
| **keyword position** (a bare word at a decl/statement start) (v0.30) | reserved keywords (with registry docs) + declaration snippets | `KEYWORD` / `SNIPPET` |
| **name-receiver member** (`UpperIdent.`) (v0.30.1; built-in sums + full statics in slice 1) | sum-type variants (project + built-in `HttpResult`/`QueueResult`); refined/opaque `of`/`unsafe`; capability ops; built-in type statics (`Int.parse`/`Float.parse`, `Json.encode`/`decode`, `List.empty`/`Map.empty`, `Effect.pure`) | `ENUM_MEMBER` / `METHOD` |
| **value-receiver member** (`lowercase.`) (v0.30.2) | the receiver type's kernel methods (`xs.fold`/`s.split`/`o.map`) + record fields | `METHOD` / `FIELD` |
| **expression position** (after `=`/`(`/`,`/`=>`/an operator) (slice 2; free fns in slice 3) | the value constructors (`Ok`/`Err`/`Some`/`None`/`true`/`false`) + in-scope type names (static receiver / record construction) + in-scope free functions (the current unit's own `fn`s + `uses`-imported stdlib/project combinators) | `CONSTRUCTOR` / `STRUCT` / `FUNCTION` |
| **locals** (keyword / expression position) (v0.31.2) | in-scope `let`/param bindings (with inferred type) | `VARIABLE` |

**Surface contract (ADR 0093).** The table above is the *as-built* state; the
**canonical** surface — the full *cursor context × candidate-kind* matrix every
slice implements against — is fixed by ADR 0093. Three properties are normative:
**(a) completeness** — every populated cell offers *everything* its source
registry holds (`bynkc::keywords` + `bynk_check::{builtin_names, kernel_methods, firstparty}` +
the AST sum-variant tables), enforced by a registry-driven coverage test so a new
base type / keyword / kernel method / static / stdlib function must surface in
completion or CI fails; **(b) the ceiling boundary** — only the value-receiver
cell (and a local's inferred-type *detail*) may depend on the analysis overlay;
every other cell is registry/project-parse and offers candidates even in a broken
buffer. (Since slice 4 even that cell is best-effort, not all-or-nothing — ADR
0094.) **(c) `.` is a trigger character** so the member cells auto-fire. **Slice
1 closed the registry-sourced gaps** (per the ADR 0093 surface contract) — the `.`
trigger (G1), the statics table's missing `List.empty`/`Map.empty`/`Effect.pure`
(G2), and the built-in `HttpResult`/`QueueResult` variants (G3) — and added the
coverage test (the table above reflects them). **Slice 2 closed G4** — expression
position now offers the value constructors and in-scope type names (the
`complete()` arm), with locals/params still appended handler-side. **Slice 3
closed G5** — expression position also offers in-scope free functions: the current
unit's own `fn`s and the combinators of every `uses`-imported module (project +
the embedded first-party commons — `bynk.list`/`bynk.map`/`bynk.string` and the
locale API `bynk.locale`, all from `firstparty::FIRSTPARTY_SOURCES`), gated on the
`uses` set. (Those sources joined `for_each_unit`, so signature help resolves their
labels too.) **Slice 4 closed G6** — the value-receiver clean-file ceiling: Analyse
mode records best-effort partial `expr_types` (ADR 0094), so value-member
completion and signature help survive an unrelated type error elsewhere in the
file (the receiver types whenever it itself checks). The completion arc is
complete; only the upstream resolve gate (an unresolved name elsewhere) remains a
known limitation.

**Built-ins/surface come from static registries, not the index (ADR 0061).** Because first-party symbols aren't indexed (§3.14's finding), the built-in types (`Int`/`Bool`/`Float`/`String`/`Option`/`Result`/`Effect`/`List`/`Map`), keyword docs, the `bynk`-surface transparent types, and the built-in type statics (`Int.parse`/`Float.parse`/`Json.encode`/`decode`/`List.empty`/`Map.empty`/`Effect.pure`) are sourced from `bynkc::keywords` + `bynk_check::{builtin_names, firstparty}` (and a small static statics table; built-in sum variants come from the `bynkc::ast` `HTTP_VARIANTS`/`QUEUE_VARIANTS` registries) — the index (here, the project parse) supplies only *project* symbols. Keyword candidates are the lowercase-initial reserved words (declaration/statement keywords); uppercase type/value names belong to type/expression position. Snippets carry LSP `${n:…}` tab stops (`InsertTextFormat::SNIPPET`).

**Name-receiver members (v0.30.1, ADR 0062).** The `.`-member context is split by *what sits before the dot*. A **name** receiver — a single uppercase-initial identifier (`Color.`, `Email.`, `Clock.`) — is resolved from the project/surface parse to a sum/refined/opaque type or a capability, and its members are enumerated from the AST (no typed model, no scope query — the same mid-edit-safe recovery parse). A *plain* alias `type Id = Int` **does** carry `of`/`unsafe` (the emitter brands every `Refined` body), so they are offered; a **record** type has no name-receiver members (its fields are value-receiver).

**Value-receiver members (v0.30.2, ADR 0063).** A **value** receiver — a lowercase `x.` — needs the receiver's *type*, which the checker discards on the analyse path and which a bare mid-edit `x.` doesn't even parse. So the LSP **rewrites** the buffer to drop the trailing `.partial` (then `x` parses), **re-analyses** it, and types the receiver via the retained `expr_types` (`type_at_offset`). The type maps to its **kernel methods** — from the enumerable `bynk_check::kernel_methods` registry (`List`→`fold`/`get`/…, `String`→`split`/…, `Option`/`Result`→`map`/`andThen`/…, `Int`/`Float`→`abs`/`round`/…), drift-pinned against the checker's dispatch — plus **record fields** from the AST. (`bynk.list`/`bynk.map` combinators like `map`/`filter` are *free functions* `map(xs, f)`, not members.) **Error-tolerant since slice 4 (ADR 0094):** the checker already types every well-typed sub-expression, then discarded the file's whole `expr_types` map on a final `errors.is_empty()` gate. Analyse mode now records that **best-effort partial map** at every per-file check exit (`check_record` *and* the context/declaration checks, where handler bodies are typed), so the receiver types whenever it *itself* type-checks — completion and signature help work mid-edit despite an unrelated *type* error elsewhere. Build stays Ok-only (codegen untouched). One ceiling remains: an unresolved name elsewhere bails before the checker runs (the resolve gate), so it still blanks the receiver.

**Store-map query vocabulary (#596).** `kernel_methods` gained a `Query[T]` entry — the ADR 0115/0116/0120 lazy-query vocabulary (builders `map`/`filter`/`flatMap`/`sortBy`/`take`/`skip`/`distinct`/`distinctBy`/`joinOn`/`leftJoin`/`join`/`groupBy`, terminals `collect`/`first`/`firstOrElse`/`count`/`fold`/`any`/`all`/`sum`/`min`/`max`/`average`/`forEach`/`parTraverse`/`traverseAll`/`parTraverseAll`/`traverseTry`/`parTraverseTry`) — so completion on any `Query`-typed receiver (a `store Map`'s `.entries`/`.keys`/`.values`, a `store Log`'s time-window roots, or a bare `store Map` used as a value) now offers it; previously the type-based path alone answered `[]` for every `Query` receiver. A **second, provenance-based path** closes the rest of the gap: a bare `store` field receiver (`items.`) types, via the checker's ADR 0120 "whole map as a value" reading, to plain `Ty::Query` — indistinguishable from an ordinary `Query`-typed local — so the entry ops (`put`/`get`/`update`/`upsert`/`remove`/`contains`/`size`, from the existing `bynk_check::store_ops` registry hover already used) and, for a `Map`, the `.entries`/`.keys`/`.values` accessors (a new `MAP_QUERY_ACCESSORS` table, refused on a held `Map[K, Connection]`) are offered by reading the receiver's *syntactic provenance* instead: is it a bare, non-dot-qualified identifier, not shadowed by a local, naming a `store` field of the enclosing agent — the same by-provenance check hover's `describe_store_op_at` makes, generalised to start from the receiver's own span (`symbols::store_field_kind_at`) rather than walking back from an already-typed operation name. Both paths merge into one completion list.

**Conservative detection.** Type-position triggers exclude a list-literal `[` (its bracket isn't preceded by a type constructor); the one accepted false positive is a record *construction* value (`Order { id: ` — lexically identical to a record field-type declaration), where offering type names is mild noise. Name-receiver detection requires a *single* uppercase-initial segment, excluding the decimal `1.` and the `.`-qualified `a.B.`. Out-of-context prefixes (e.g. `let x = `) yield `[]`.

**Locals (v0.31.2, ADR 0064).** In-scope local bindings are offered at **keyword position** (appended to the keywords + snippets) and at **expression position** — after `=`/`(`/`,`, a `=>` lambda arrow, or a binary operator (the type arrow `->` excluded) — as `variable` items with their inferred type. They come from the **cached** analysis's `FileLocals` (the last good round's bindings around the cursor), so they survive the mid-edit buffer; positions convert against the cached snapshot. Locals are appended to a specific context's results only at keyword position — never to type/member completion.

**Pattern-position variants (v0.124/v0.128; built-ins + nested in v0.145).** At an `<expr> is <cursor>` position and at a `match <scrutinee> { <arm-start>`, the editor offers the scrutinee's **variants**, typed through `type_receiver` (the value-member `expr_types` path, so the same clean-file ceiling — silent, never wrong, on a broken buffer). The scrutinee/arm-position detectors are lexical (`is_scrutinee_offset`, `match_scrutinee_offset`). v0.145 (ADR 0169) widened this: the candidate resolver `variants_for_ty` now offers the **built-in** `Result` (`Ok`/`Err`) and `Option` (`Some`/`None`) variants — which are not declared types, so the source-walked `sum_type_variants` cannot see them — closing a pre-existing gap where pattern completion fired only for a user-declared sum. Built on that, **nested-variant completion**: at `OuterVariant(<cursor>` inside a match arm (the position `match_scrutinee_offset` bails on, detected by `nested_pattern_offset`), the payload field type is resolved — `Result`/`Option`/`HttpResult` generic args off the `Ty`, or a user-sum decl's field walked from source (the `payload_field_ty` shape) — and **that** type's variants are offered (`Ok`/`Err` inside `Some(‸)` on an `Option[Result[…]]`). Nested is `match`-only (guards/nesting are `match`-only; `is` stays flat, ADR 0169).

**Later work.** The completion overhaul (the LSP tooling track's completion arc, against the ADR 0093 contract) is **complete**: the registry-sourced quick wins (G1–G3), the expression-position surface (G4), free-function/stdlib completion (G5), and the error-tolerant receiver typing that lifts the clean-file ceiling (G6, ADR 0094). Beyond the arc: the upstream resolve-gate ceiling (an unresolved name elsewhere still blanks the receiver), match-arm/`is`-narrowing local bindings, a `parameter`-vs-`variable` token split, `completionItem/resolve` (lazy docs / auto-import), and postfix expansion.

### 3.16 Signature help (v0.32)

`textDocument/signatureHelp` shows the callee's signature with the **active parameter** highlighted while the cursor is inside a call's argument list. **Context detection is lexical** (it must work mid-edit): scan back to the **innermost unclosed `(`** before the cursor (bracket-balanced — a depth-0 `[` or `{` means the cursor is in type args / a list / a block, not a call), take the **callee** identifier (`name` or `Recv.member`) immediately before it, and the **active parameter** from a top-level, bracket-aware comma count (so `f(g(x|))` targets `g`, and commas inside nested `()`/`[]` don't count). **Signatures are semantic**, resolved the same name-vs-value way as completion (§3.15):

| Callee | Source | Slice |
|---|---|---|
| free fn `bar(` | the `FnDecl` from the recovery parse | v0.32 |
| capability op `Clock.now(` | the `CapabilityOp` from the parse | v0.32 |
| refined/opaque `Email.of(` / `.unsafe(` | synthesised from the type's base | v0.32 |
| built-in static `Int.parse(`/`Json.decode(` | the `BUILTIN_STATICS` registry string | v0.32 |
| `Ok`/`Err`/`Some` constructor | built-in | v0.32 |
| value-receiver method `xs.fold(` | the receiver typed (§3.15's machinery) → the kernel-method registry signature | v0.32.1 (clean-file ceiling) |

Signatures render through the **same `type_ref_str` renderer as hover** (§3.3) — one format, never divergent; the kernel/static **registry signature strings are reused verbatim**. The response is a single `SignatureInformation` (Bynk has no overloads); `ParameterInformation` offsets parse the parenthesised parameter list (top-level-comma-aware). Trigger characters: `(` and `,`. The value-receiver path (v0.32.1) types the receiver by re-analysing the buffer rewritten so it parses (the `.method(args` dropped) — the shared `type_receiver` helper, with value-member completion's clean-file ceiling. Generic type-argument display in a signature waits on the checker recording instantiations queryably.

### 3.17 CodeLens (v0.33)

`textDocument/codeLens` returns one **reference-count lens** above each top-level **index definition** in the file — types, free fns, capabilities, services, agents, providers (the v0.25 index set; locals/methods/fields aren't indexed and get none). Served from the **last committed round** through the non-refreshing decoration gate (§3.2.1's stale-while-revalidate rule, #733), positions against that round's snapshot; the `workspace/codeLens/refresh` nudge on round-commit re-pulls the fresh counts. The count is `refs.len()` from the binding index (a pure `code_lenses(index, path)` query returning `(def site, reference sites)`, sorted by definition position). The lens title is `"{n} reference(s)"` with the standard **`editor.action.showReferences`** command (args: the def URI, the def position, the reference `Location`s) — clicking peeks the references, no extension support required; non-VS-Code clients still render the title. **`"0 references"` is shown** (a dead-code signal). Computed eagerly (`resolve_provider: false`) — the count is `O(1)` off the index. The **test-run lens** ("▶ Run") needs test discovery + a run command and is deferred.

### 3.18 Call hierarchy (v0.34)

Call hierarchy is a three-call protocol served from the binding index's **call graph**. `textDocument/prepareCallHierarchy` resolves the symbol under the cursor to a `CallHierarchyItem` (the goto-def resolution, anchored on the definition); `callHierarchy/incomingCalls` lists its callers, `callHierarchy/outgoingCalls` what it calls — each with the call-site ranges.

The graph is a `CallEdge { caller, callee, site }` side table on `ProjectIndex`, built by **preserving `RefEdge.owner`** — the enclosing top-level declaration already recorded around each fn/service/agent/provider/capability body (`index.rs:73`) and, until now, used only for file re-attribution and then dropped. At assembly the owner resolves to the caller's `SymbolKey` through an `owner_keys` map populated in `add_def` alongside the existing `owner_files`, the same `(namespace, owner)` lookup the re-attribution uses. **Callees**: `Fn`, `Method` (v0.36, ADR 0069), and — closing the under-reporting gap in #304 — `CapabilityOp` and agent `Handler` (`"Agent.handler"`, recorded at the `agentInstance.handler(...)` dispatch call). **Any indexed owner may be a caller** (so a service handler calling a free fn shows the service as a caller); caller granularity is per top-level declaration, not per-handler — a call inside one agent handler's body attributes to the agent, not that specific handler. **Service cross-context dispatch callees remain uncovered** — a service exposes one dispatchable `on call` with no per-handler name to index, unlike an agent's named handlers. The call site (the callee-name span) is the `fromRanges` for both directions.

Pure `index_queries::{prepare_call_hierarchy, incoming_calls, outgoing_calls}` over `ProjectIndex.calls`, served from the round the freshness contract (§3.2.1) keeps current for the requested file. The resolved `SymbolKey` round-trips through `CallHierarchyItem.data` (a `SerKey`, since the index kind isn't `Serialize`) so the follow-ups resolve straight off it; a missing/garbled payload returns no calls. **Implementation navigation** (`given Cap` → provider) is §3.19.

### 3.19 Implementation navigation (v0.35)

`textDocument/implementation` on a **capability** symbol — its `capability Cap` declaration, a `given Cap` use, or a `provides Cap` use — returns the definition site(s) of every **provider** that implements it (the Bynk analogue of "go to implementations" on an interface). Served from the binding index's **implementation graph**: an `ImplEdge { capability, provider, site }` side table on `ProjectIndex`, built like the v0.34 call graph by resolving a reference's `owner` through `owner_keys`.

The edge comes from the `provides Cap = Provider` clause, which records a `Capability` reference whose owner is the provider. A provider may *also* declare `given Cap2` (its own deps) — also a capability ref owned by the same provider — so the owner alone can't tell "implements" from "depends on". The `provides` clause therefore records its ref with a **`provides` flag**; only flagged edges whose owner resolves to a `Provider` become `ImplEdge`s (the ref still counts as an ordinary capability reference). Cross-context `provides` links the capability's key in its defining unit to the provider's key in the providing unit, by construction.

`implementation` resolves the symbol at the cursor, requires it be a `Capability`, and returns the providers' def sites (a provider is an index symbol — the edge only names it), sorted by position; a non-capability symbol or a capability with no providers returns `None`. The **reverse** (provider → its capability) is already **goto-definition** on the `provides Cap` name and isn't re-plumbed. **External/adapter providers** are included — navigation lands on the Bynk `provides Cap = Name { external }` declaration, never the off-tree `.binding.ts`. Pure `index_queries::implementations(index, key)` over `ProjectIndex.impls`, served from the round the freshness contract (§3.2.1) keeps current for the requested file.

**Go-to-type-definition (`textDocument/typeDefinition`, slice 6).** From a **value** at the cursor to the declaration of its (user-declared) type. The round's `expr_types` is now retained in the LSP `Analysis` (alongside the index/snapshots/locals); the handler reads the value's `Ty` at the offset (`type_at_offset`), unwraps a single-parameter container (`Option`/`Effect`/`List`/`HttpResult`) to its `Named` element, and returns that `Type` symbol's definition site(s). The lookup is a **bare-name match** (`Ty::Named.name` and the index both use bare names), so a type name shared across units yields several locations — the LSP-conventional resolution, the client lets the user choose. Built-in, function, actor, and two-parameter (`Result`/`Map`) types have no single type-declaration target and return `None`, as does a cursor not on a typed expression in a clean round. The **consumed-context** half (`uses B` / `B.Cap` → the context's source file) stays deferred — context units aren't index symbols, so it needs the unit→file map (also the basis for document links).

### 3.20 Folding & selection ranges (v0.37)

`textDocument/foldingRange` and `textDocument/selectionRange` are **structural** — served from the per-file **recovered AST** (the document-symbols parse path), not the binding index or the analysis round, so they answer even when the project doesn't check. One span visitor (`structure::collect`) walks the AST and yields every node's `(span, foldable)` pair; both providers consume it. AST-driven, consistent with the other structural providers — `bynk-lsp` carries no tree-sitter dependency.

**Folding** keeps the `foldable` (multi-line block-like) nodes: the `commons`/`context`/`adapter`/`test` container, type record/sum bodies, service/agent handler lists and their block bodies, provider/op and fn block bodies, `match` (and its arms), `if`, block expressions, and record/spread/list literals. A range is emitted only when `endLine > startLine` (LSP folds ≥2 lines); a decl and its body sharing both lines collapse to one. Structural ranges carry no `kind`. **Multi-line comment runs** fold as `FoldingRangeKind::Comment`, from a scan of the lexer's `Comment` tokens (the trivia table keeps only bodies, so spans come from the tokens) grouping consecutive comments on adjacent lines.

**Selection** filters the same node list to spans **containing** the cursor, de-duplicates, sorts by size, and links them into the `SelectionRange { range, parent }` chain — innermost first, widening to the whole file (a well-nested AST guarantees each parent contains its child). Falls back to an empty range at the cursor when nothing contains it or the file doesn't parse. Clause-list (`given`/`exports`/`consumes`) folding and per-statement folding within blocks are deferred.

### 3.21 Document links (slice 6b, ADR 0095)

`textDocument/documentLink` underlines each `uses`/`consumes` **unit name** — and the `suite <target>` header's target (#609) — and makes it clickable to that unit's source. It joins two pieces: the clickable **range** comes from parsing the *live buffer* (`symbols::unit_reference_spans` walks the recovered AST's `uses`/`consumes` declarations, plus a suite's `target` and its own `uses`, and returns each target's name span — so links track the document even mid-edit); the link **target** comes from the round's **unit→source map** (`unit_sources`), keyed by qualified unit name, resolving to the unit's first source file (a unit may span files). The map is **new analysis surface** (ADR 0095) — context/unit names aren't index symbols, the gap ADR 0068 flagged — built in one pass over the non-synthetic parsed units, available whenever the project structurally analyses (type errors included; empty only on a parse bail), and threaded `ProjectAnalysis` → `ProjectDiagnostics` → the LSP `Analysis`. A **first-party `uses`** (`bynk.list`, embedded via `include_str!`, no on-disk file) and an unresolved unit yield no link, by design — their *symbols* still surface through hover/completion (slice 9). **Go-to-definition on the same `uses`/`consumes` names** rides the same map: when the cursor sits on a unit-reference span and the index/locals paths don't resolve it, `goto_definition` returns the unit's first source file (at the top — units have no finer def site than their header). This closes the consumed-context half of §3.19's deferral for unit *declarations*; a unit-qualified capability reference (`B.Cap`) inside an expression is not yet a navigation source.

**Intra-doc links (#848).** Inside a `--- … ---` doc comment, a bare `[Name]` or dotted `[Owner.member]` — plus a `` [`Name`] `` code-span-wrapped shortcut and a `[text][Name]` full reference — becomes a clickable link when it resolves, exactly as go-to-definition would resolve the same name at that point in the source: the *declaring unit's* doc-link scope order (itself, then its `uses` targets, then its `consumes` targets — `Analysis::doc_scope`, a sibling map built alongside `unit_sources` at the same assembly point, from the same `uses`/`consumes` phase outputs), first scope-order unit with any match wins, more than one candidate there is unresolved rather than guessed (no `kind@` disambiguation prefix). `[text](url)`, an author-defined `[name]: url` reference, and fenced code-block content keep their ordinary Markdown meaning and are never scanned as candidates. An unresolved name — including any first-party/synthetic-unit name, which has no on-disk location by construction — renders as plain text; there is no diagnostic for a broken intra-doc link in this increment. The clickable **range** comes from scanning the live buffer's `DocBlock` tokens directly (`symbols::doc_link_spans`), the same live-buffer-tracks-mid-edit approach `unit_reference_spans` uses above; the **target** is the resolved symbol's definition site (`index_queries::resolve_doc_link`), built the same way `goto_definition` builds one. Resolves against the full `analysis.index` under the same `committed_analysis` (stale-while-revalidate, §3.2.1) gate as the `uses`/`consumes` links above — consistent with `code_lens`, which already resolves full-index cross-references under that gate.

### 3.22 File-rename awareness (#302)

`workspace/willRenameFiles` keeps `uses`/`consumes` references in sync when a `.bynk` file is renamed or moved in the editor — the file-move half of §3.9's deferral for unit names. The capability is registered with two filters: `**/*.bynk` files (`FileOperationPatternKind::File`) and, since #2105, every folder (`**`, `FileOperationPatternKind::Folder`; a folder's name can't say whether it holds `.bynk` files).

For each `FileRename`, the handler parses the *old* file's own snapshot to get its declared qualified name and the span of that declaration (`symbols::own_declaration_name` — `None` for a `suite`, whose `SourceUnit::name()` is its *target*'s name, not one of its own, so nothing addresses it by name and a suite rename produces no edits). It then derives the name the *new* path implies, preserving whichever single-file/multi-file arrangement the old path satisfied against the compiler's own path↔name consistency rule (`bynk_ide::renamed_unit_name`, delegating to `bynk-emit`'s `check_path_name_alignment` logic rather than re-deriving it in the LSP). Renaming one member file within a multi-file unit's directory is a no-op by construction — the qualified name is the directory, not the filename.

When the name does change, the edit set is: the moved file's own declaration header (targeting its **old** URI — the client applies the returned edit against pre-move locations, then performs the actual rename, so the file lands at its new path already correct), plus every other project file's matching `uses`/`consumes` reference spans (`symbols::unit_reference_spans`, the same span-finder `documentLink` uses). Edits are versioned `TextDocumentEdit`s, gated by `analysis_covering_open_buffers` (§3.2.1) — the same whole-project freshness `rename` needs, since this handler emits the same kind of multi-file versioned edit and a stale open buffer would otherwise carry a version the client rejects. The handler never refuses: a filesystem rename isn't something this edit-only hook can block, so anything it can't confidently resolve is skipped rather than erroring the batch.

**Folder renames (#2105).** Unit names derive from paths, so renaming or moving a folder is the same operation as one file rename per `.bynk` file under it. A `FileRename` whose old path has no snapshot in the round is treated as a folder and expanded server-side: every snapshot under it (a component-wise prefix match, so `src/bill` never matches `src/billing/…`) is mapped to the same relative position under the new folder, and each goes through the single-file computation above. A folder holding no project file expands to nothing, so it produces no edit. Across one request, a file is handled once (a request naming a file and its enclosing folder is deduplicated by resolved path) and each renamed unit's references are rewritten once (every file of a multi-file unit maps to the same new name, and rewriting per file would emit overlapping edits). All of it merges into one `WorkspaceEdit`. A move whose destination resolves to a different project root (its canonicalised parent walked up to the nearest `bynk.toml`, a nested project included) is skipped with a log line: moving units into another project changes their identity beyond a rename (#2105 DECISION B).

### 3.23 Sequence diagram (#846)

`bynk/sequenceModel` — a **custom (non-standard) request**, this server's first: everything in §3.1–3.22 is a standard `LanguageServer` trait method, registered automatically; this one has no trait slot and is wired via `tower-lsp`'s `LspService::build(...).custom_method("bynk/sequenceModel", Backend::sequence_model)`. Advertised through `ServerCapabilities.experimental` (`{"sequenceModel": true}`) — the only feature-detection surface a client has for a custom method. Params are the usual two-field cursor shape (`textDocument`, `position`); the response is a `SequenceModel | null` (`null` when no handler encloses the cursor).

Backs the VS Code extension's **"Bynk: Show Sequence Diagram"** command and a per-handler **"Show Sequence"** CodeLens (sourced from a direct AST walk over `Service.handlers`/`AgentDecl.handlers`, not `index_queries::code_lenses` — that indexes only agent handlers, since `SymbolKind::Handler` excludes service handlers, which have no per-handler name). Served from the **committed round** through the non-refreshing decoration gate (§3.2.1, #733), same as CodeLens/inlay hints. **No refresh-nudge exists for it** — Tier 1 (see the design ADR, `design/pending/sequence-diagram-846.md` pre-stamp / its post-stamp ADR home) is on-demand: the client re-issues the request each time the command/lens fires, since there is no generic "refresh a custom method" in the LSP spec or in `tower_lsp::Client`.

The query itself, `bynk_ide::sequence::sequence_model`, classifies each call in the handler body as a **lifeline** — a consumed capability, a call into a consumed context, or an agent (same-context included) — or as local computation that folds into the entry activation. It needs the handler's owning unit's cross-context/agent tables, which the per-file checking pass builds transiently and never retains; #846 adds `ProjectAnalysis.sequence_info` (mirrors the `unit_sources`, ADR 0095, precedent: a project-wide analysis table, not a per-file `bynk-check` captured one, since cross-context classification is inherently project-wide), threaded `ProjectAnalysis` → `ProjectDiagnostics` → the LSP `Analysis`, alongside the others. Cross-context/agent calls are **boundary-stop**: one Call+Return message, the callee's own body never walked. `if`/`match` render as `alt`/`opt` up to a depth budget (~2 levels); deeper nesting collapses to a single marker, still click-to-code-able via its own span. Each branch also carries its **outcome** (`Branch.reply`) — the value the handler yields on that path (`Ok(view)`) — so a return-gating `if`/`match` whose branches call no lifeline (the rate-limiter's `if view.allowed { Ok(view) } else { TooManyRequests(...) }`) has real content rather than an empty `alt` that Mermaid draws as a mangled zero-width box. An `if` branch is labelled by its condition (`view.allowed`), and an else-less `if` renders as a single-branch `opt` rather than an `alt` with an empty second branch. A handler's **principal** — its `by <Actor>` clause (or the service-level `default_by` it inherits) — becomes a leftmost `Actor` participant that originates the request (an inbound message carrying the route, so the entry box drops to the bare owner name `api`); every return-position outcome then renders as a `Return` message *to* that actor rather than a note. Handlers with no principal (agents, services without `by`) keep the combined entry label and render outcomes as entry-anchored notes.

The Mermaid diagram text itself is generated **client-side** (`vscode-bynk/src/webview/mermaid-gen.ts`) from the structured `SequenceModel` JSON — `bynk-check`/`bynk-ide`/`bynk-lsp` stay Mermaid-agnostic; the extension's first webview owns rendering, CSP (vendored Mermaid, no CDN, per the documentation track's convention), and click-to-code (a DOM-order zip against the rendered SVG, not Mermaid's inconsistent `click`-directive support for sequence diagrams).

### 3.24 Documentation view (#847)

`bynk/documentationModel` — the server's **second custom request**, wired the same way as §3.23 (`LspService::build(...).custom_method("bynk/documentationModel", Backend::documentation_model)`, advertised through `ServerCapabilities.experimental` as `{"documentationModel": true}`). Unlike `bynk/sequenceModel` it carries **no cursor position**: a documentation page is the *whole file's* declarations (Tier 1 is file-scoped — see the design ADR), so the params are a bare `textDocument`; the response is a `DocModel | null` (`null` for a non-project file, a `suite` unit, or a file with no committed round).

Backs the VS Code extension's **"Bynk: Show Documentation"** command. Served from the **committed round** through the non-refreshing decoration gate (§3.2.1, #733), on-demand with **no refresh nudge** (same reasoning as §3.23 — a custom method has no generic refresh in the spec; the client re-issues the request each time the command fires).

The query, `bynk_ide::documentation::documentation_model`, aggregates every declaration in the file into an ordered, hierarchical page — for each: a heading name, a short kind label, its nesting `depth` (a top-level declaration is `0`; a capability's ops and a service/agent's handlers are `1`), its rendered Markdown, a `documented` flag, and the name span each heading links back to. It reuses two things rather than re-deriving them: the traversal shape is `document_symbols`' exhaustive `CommonsItem` walk (a new declaration kind is a compile error, not a silently-missed row — §3.7), and each entry's Markdown (a fenced `bynk` signature followed by its doc-comment prose) is produced by **hover's own `describe_*` assembly** (§3.3), so the page cannot drift from hover. An **undocumented** declaration still renders its signature, flagged for the "no documentation" coverage placeholder the webview shows (with a toggle to hide it for a clean reading page).

Markdown is rendered **client-side** (`vscode-bynk/src/webview/doc-render.ts`, vendored `markdown-it`) with **HTML disabled** — a doc comment's raw `<script>`/`<img>` never reaches the DOM as markup — under the same `default-src 'none'` webview CSP #846 established (now factored into a shared `webviewHost.ts` substrate this view and the sequence view both consume). A link written `[text](url)` (or a `<url>` autolink) renders as an anchor, but a click is gated through an **http(s) allow-list** before the host opens it externally — the security posture holds whatever markdown-it turns into a link; a *bare* URL is not auto-linkified. Click-to-code posts a `{uri, range}` reveal, hydrated host-side into a real editor navigation (the DOM is built element-by-element here, so each heading holds its own span directly — no SVG-order zip).

### 3.25 Architecture map (#851)

`bynk/architectureModel` — the server's **third custom request**, wired the same way as §3.23/§3.24 (`LspService::build(...).custom_method("bynk/architectureModel", Backend::architecture_model)`, advertised through `ServerCapabilities.experimental` as `{"architectureModel": true}`). Unlike both siblings it is **project-scoped**, not file-scoped: the params are a bare `textDocument`, used only to resolve which project's committed round to read (§3.2.1's `committed_analysis` gate, keyed off the active file), never to restrict the result to that one file — the response covers every context/adapter in the resolved project. The response is an `ArchModel | null` (`null` for a non-project file or no committed round).

Backs the VS Code extension's **"Bynk: Show Architecture Map"** command — the *macro* counterpart to §3.23's per-handler sequence diagram (a whole-project map, not one handler's interaction), sharing the same webview substrate. Served from the **committed round**, on-demand with **no refresh nudge** (same reasoning as §3.23/§3.24).

The query, `bynk_ide::architecture::architecture_model`, is a citation correction from the original increment proposal, recorded here and in `design/pending/architecture-map-851.md`: the issue described it as "a query over the binding index and call graph" (`bynk-check/src/index.rs`, ADR 0053/0067). That index has no `Context`/`Adapter` symbol kind, and its call graph does not carry `consumes` edges (the index's own comment: cross-context dispatch is "the one uncovered relation"). The actual sources are `ContextSequenceInfo::cross_context` (§3.23's own table — resolved consumed-context names, aliasing applied) for edges, plus a re-parse of each unit's own committed snapshot text for local declarations and raw `consumes` clauses — there is no retained AST after a round, the same constraint §3.23/§3.24's own request modules re-parse against. Because the model is project-scoped, every node/member/edge carries its own project-relative **file**, not just a span against one document — the wire layer lowers each to its own `{uri, range}` pair (`WireLoc`), so click-to-code opens the right file even when two nodes live in files the request's own document never touched.

One node per context/adapter unit (excluding the toolchain's synthetic `bynk` capability surface, which has no project file and so is never itself a node), each carrying its locally-declared capabilities/providers and its "services in, agents below" members (`design/bynk-design-notes.md`'s concentric-region model). A directed edge per `consumes` clause, labelled with its selected capabilities where braced (`consumes U { Cap, … }`) — a whole-unit consumes (no braces) draws an unlabelled edge. A braced selection also flattens onto the *consuming* node as a bound capability (origin `Consumed { from }`), which is what makes the built-in `consumes bynk { Clock }` binding visible on a single-context project even though the synthetic `bynk` unit never becomes a node (an edge to it would dangle, so §3.25's query drops the edge but keeps the binding).

**DECISION A (context-level nodes, expand-on-demand)**: every node renders compact by default; the wire model carries its declaration-level members inline, and "expand" is a client-side re-render (`vscode-bynk/src/webview/architecture-gen.ts` re-invoked with the toggled node added to a local `expanded` set) — no second request. **DECISION B (per-project, nearest `bynk.toml`)**: the map covers the active file's project, mirroring the extension's own `findBynkToml` upward walk; a non-project file yields no map. **DECISION C (show binding, defer residency)**: the map shows which context consumes/provides each capability and the consume edges themselves; residency/boundary-tier annotation is deferred to a later refinement.

The diagram is generated **client-side** as a Mermaid `flowchart` (`vscode-bynk/src/webview/architecture-gen.ts`), reusing the vendored Mermaid asset §3.23 introduced — a second diagram type from the same dependency, no new one added. Click-to-code is wired by **Mermaid node id**, not a DOM-order zip: unlike a `sequenceDiagram` render (§3.23's own click-wiring note), a `flowchart`'s rendered SVG preserves the node id this module assigns (`id="flowchart-<mermaidId>-<n>"`), so each click target is looked up directly by id — robust against Mermaid's dagre layout reordering the DOM, which a non-linear flowchart is free to do (unlike the sequence view's strictly left-to-right participants).

### 3.26 Wire contract (#855)

`bynk/wireContract` — the server's **fourth custom request**, wired the same way as §3.23–§3.25 (`LspService::build(...).custom_method("bynk/wireContract", Backend::wire_contract)`, advertised through `ServerCapabilities.experimental` as `{"wireContract": true}`). File-scoped **and** position-scoped, like §3.23's `bynk/sequenceModel` rather than §3.25's project-scoped `bynk/architectureModel` — the panel is per-handler. Params are the usual two-field cursor shape (`textDocument`, `position`); the response is a `WcModel | null` (`null` when no handler encloses the cursor, the file is not part of a project, or the pipeline bailed before the checker for that unit).

Backs the VS Code extension's **"Bynk: Show Wire Contract"** command and the header hover rung (§3.3). Served from the **committed round** through the non-refreshing decoration gate (§3.2.1, #733), same as every custom request in this server — on-demand, with **no refresh nudge**: there is no generic "refresh a custom method" in the LSP spec or in `tower_lsp::Client`, so the client simply re-issues the request each time the command or hover fires.

**What the model carries.** The query, `bynk_ide::wire_contract::wire_contract_at`, re-parses the committed snapshot (`sequence_model_at`'s convention, §3.23) to find the `Handler` enclosing the cursor, then builds:

- the **request envelope** — see below;
- (`on call` only) the **cross-context contract form + hash** — the exact `CrossContextService` projection and canonicalisation `bynk-emit`'s `own_contract_hashes` uses, over the same combined type table (`bynk-emit`'s retained `ContextBoundaryInfo`, threaded through `ProjectAnalysis` → `ProjectDiagnostics` → the LSP `Analysis`), so the hash the peek shows is provably the hash the emitted `X-Bynk-Contract` constant stamps;
- the **boundary type shapes** the envelope and responses reference — `bynk_check::wire::WireModel`, the same IR `bynk-emit`'s codec generation renders (DECISION C, `design/pending/wire-contract-855.md` pre-stamp / its post-stamp ADR home), narrowed to this one handler's params/return via a synthetic single-handler service so `wire::collect_boundary_types` does not pull in the rest of the service's handlers;
- (HTTP handlers only) the **reachable response set**, tagged by origin.

**The three-case envelope (not two).** `workers_entry.rs`'s `params.len() == 1` branch is the *only* bare-value case; everything else, including **zero** params, takes the object branch. The wire shape mirrors this exactly: `Empty` (no params — the request body is not read at all, not an empty keyed object), `Bare { param, shape }` (exactly one param — the request body **is** the value of `param`, unwrapped), `Keyed { params }` (two or more, an object keyed by parameter name in declaration order). Rendering only two cases would misstate a zero-arg service as accepting an empty object body it never inspects.

**Boundary-implicit responses.** An HTTP handler's reachable status set has three origins, tagged `DeclaredSuccess` / `Constructed { range }` / `BoundaryImplicit { why }` so the panel can render a boundary-injected outcome honestly rather than implying the author wrote it: the declared success case (`Effect[HttpResult[T]]` stripped to `Ok`/200), every `HttpResult` variant literally constructed in the handler body, and the responses the body never names — a 400 whenever the handler has any param (every param is structurally re-validated on the way in) and a 404 on an `Option?` short-circuit (ADR 0177). A 401 on caller-binding is deferred (needs actor/`by` resolution this phase does not do); shipping only the two above is a deliberate, recorded scope cut (`design/pending/wire-contract-855.md` pre-stamp / its post-stamp ADR home), not an oversight. The `Ok`-overload disambiguation (a bare `Ok(_)` vs. a same-named ordinary value) uses the file's recorded `expr_types`, empty for a file with errors (ADR 0063's clean-file ceiling) — the peek then falls back to the declared-return-type heuristic, the same fallback `bynk-emit`'s own lowering uses for the identical ambiguity.

**"No cross-context contract here."** `NoCrossContextReason::NotACallHandler` (an HTTP/cron/message/websocket/event handler — never reached by another context's call) or `SingleContext` (the project has only one real context/adapter, so the question does not arise) — rendered on the **panel only**; hover's rung-1 appendix stays quiet about an absence rather than adding a line most hovers would never want.

**Rendering: plain DOM, not Mermaid or markdown.** A JSON-shape tree is a nested list, not a graph and not prose — `vscode-bynk/src/webview/wire-render.ts` builds it element-by-element (`bodyCss`-styled, `var(--vscode-*)` tokens, under the same `default-src 'none'` + nonce CSP §3.24 established), so every node owns its own click-to-code location directly rather than needing a DOM-order zip against an SVG. Each boundary type's block carries a re-validation badge derived from its `Revalidation` (`ViaConstructor`/`Inline` → "re-validated `<predicate>` at the boundary"; `StructuralOnly` → the honest counter-statement, "**not** re-validated here — opaque to this context; skew is caught by the contract hash instead"; `Base64Decode` → "wire `string` (base64), decoded not cast") — DECISION B, `design/pending/wire-contract-855.md` pre-stamp / its post-stamp ADR home.

---

### 3.27 Explain a diagnostic (#2098)

`bynk/explain` — the server's **fifth custom request**, wired the same way as §3.23–§3.26 (`LspService::build(...).custom_method("bynk/explain", Backend::explain)`, advertised through `ServerCapabilities.experimental` as `{"explain": true}`). Unlike every sibling it is **not document-scoped** and reads no analysis round: the params are `{ code }` alone, and the response is `{ title, markdown, href } | null`, built from `bynk_syntax::diagnostics::explain(code)` — the compiled-in table `bynk explain <code>` and the diagnostics' `codeDescription` links (#853) also read, so the three surfaces cannot drift and the text always matches the installed compiler. `title` is the code; `markdown` is the blurb followed by the example as a fenced `bynk` block (no link, no raw HTML); `href` is the hosted-Book URL. `null` covers a known-but-uncurated code and an unknown string alike — the client words the fallback.

**The Explain quick fix.** `textDocument/codeAction` appends, after every real fix and the refactors, one `quickfix` titled ``Explain `<code>` `` per distinct curated code among the diagnostics intersecting the requested range (the §3.10 keying rule). It carries no edit, only a `Command` naming the **client-side** `bynk.explainDiagnostic` with the code as its single argument, and `isPreferred: false`, so it never preempts a real fix. Because the client executes it, the action is offered only to a client that advertises `experimental.explainDiagnosticCommand: true` in its `ClientCapabilities` at `initialize` — the VS Code extension does, via a static client feature; any other client sees no extra lightbulb entry.

The VS Code extension renders the result in a reusable **"Bynk: Explanation"** webview through the documentation view's markdown-it renderer and CSP (§3.24), and its **"Bynk: Explain Diagnostic Code…"** command looks up a code picked from the Problems panel or typed in.

### 3.28 Refined-type explorer (#2470)

`bynk/refinedTypeModel` — the sixth custom request, wired like §3.23–§3.27 (`custom_method("bynk/refinedTypeModel", Backend::refined_type_model)`, advertised as `experimental.refinedTypeModel: true`). Params are a text-document position (`{ textDocument, position }`); the response is a `RefinedModel` or `null` when the position names no refined (or opaque-refined) `Int`, `Float` or `String` type that carries a predicate. The name resolves through the binding index (any file of the project), else lexically over the live buffer and the project's files, as hover's declaration rungs do.

The model carries `name`, `base`, `opaque`, `declaration`, `clauses`, `examples`, `examplesNote` and `seed`. Each clause has the predicate as written (`source`), in words (`description`), and the emitter's own lowering of it: `check`, the JavaScript condition over `value` that `.of` evaluates (`bynk_emit::emitter::pred_condition_and_message`, printed by `bynk_ts`), and `message`, the failure text `.of` reports. Clauses are in runtime check order (a `Matches` last, #1651). The server evaluates nothing: the client runs the `check` strings, so what it reports is what the compiled `.of` would decide. No project code is involved — the text is generated from the closed set of predicate forms, and a `Matches` pattern is only ever a string literal in it.

`examples` are candidate input texts drawn deterministically from the refinement's domain — the boundaries first, then draws from the test runtime's `__bynkRng` under the fixed seed `seed` — so the list is identical on every open. A `Matches` type has no generator (`bynk.val.needs_pin`): drawn strings are kept only if they match, and `examplesNote` says so when none does.

**Hover.** A hover on a refined type (its declaration or any reference) gains a section that names the base and lists each clause in words. For a client that advertises `experimental.refinedTypeExplorerCommand: true` at `initialize` it also gains a `[Try values…](command:bynk.exploreRefinedType?…)` link carrying `[uri, position]`; any other client sees no link.

The VS Code extension runs the command, opens a **"Bynk: Refined Type"** webview, and checks each typed candidate in the extension host in a fresh `node:vm` context (string `eval` off, a 250 ms limit per clause), naming every failing clause. Its `provideHover` middleware marks only that one command as trusted in the hover.

## 4. Implementation architecture

### 4.1 Component layout

The tooling project consists of four components:

```
bynk-tooling/
├── tree-sitter-bynk/        -- Tree-sitter grammar (separate sub-project)
│   ├── grammar.js
│   └── ...
├── bynk-lsp/                -- LSP server (Rust, in the compiler workspace)
│   ├── src/lib.rs           -- the server: Backend, the LanguageServer impl, run()
│   ├── src/main.rs          -- thin binary entry point (bynk_lsp::run())
│   ├── src/hover.rs, completion.rs, index_queries.rs, …  -- flat handler modules
│   └── ...
├── bynk-fmt/                -- Formatter (Rust, in the compiler workspace; used by both LSP and CLI)
│   └── src/lib.rs
└── vscode-bynk/             -- VS Code extension (TypeScript)
    ├── package.json
    ├── src/extension.ts
    └── ...
```

The tree-sitter grammar lives in its own repo / subdirectory because tree-sitter has its own build tooling (`tree-sitter generate`, `tree-sitter test`).

The LSP server and formatter live in the existing compiler workspace as new Rust crates. They depend on the compiler's existing modules (parser, resolver, checker).

The VS Code extension is a minimal TypeScript project that activates the LSP and ships the tree-sitter grammar.

### 4.2 Dependencies

**For `bynk-lsp`:**
- `tower-lsp` — LSP server framework for Rust. Handles protocol plumbing.
- `tokio` — async runtime (tower-lsp uses it).
- The existing compiler modules (in-tree dependency).

**For `bynk-fmt`:**
- The existing compiler's AST and parser (in-tree dependency).
- `std::fmt` for output rendering.

**For the tree-sitter grammar:**
- `tree-sitter-cli` (npm package; used for development and code generation).
- No runtime dependencies beyond tree-sitter itself.

**For the VS Code extension:**
- `vscode` (`@types/vscode`) — VS Code API.
- `vscode-languageclient` — LSP client for VS Code.
- `tree-sitter-bynk` (the compiled grammar).

### 4.3 LSP protocol surface

The LSP server declares support for these capabilities:

```
textDocument.synchronization: Full
textDocument.publishDiagnostics
textDocument.hover
textDocument.definition
textDocument.typeDefinition        (v0.35, §3.19)
textDocument.implementation        (v0.35, §3.19)
textDocument.formatting
textDocument.rangeFormatting
textDocument.documentSymbol
textDocument.completion            (v0.17 consumes/given; positional v0.30, §3.15; resolveProvider: true)
textDocument.signatureHelp         (v0.32, §3.16; triggers "(" ",")
textDocument.references            (v0.25, §3.8)
textDocument.rename                (v0.25, §3.9; prepareProvider: true)
textDocument.codeAction            (v0.26, §3.10; kinds: [quickfix])
textDocument.codeLens              (v0.33, §3.17)
textDocument.documentHighlight     (v0.26, §3.12)
textDocument.documentLink          (§3.21)
textDocument.inlayHint             (v0.27, §3.13)
textDocument.semanticTokens        (v0.28, §3.14; full + range)
textDocument.callHierarchy         (v0.34, §3.18)
textDocument.foldingRange          (v0.37, §3.20)
textDocument.selectionRange        (v0.37, §3.20)
workspace.symbol                   (v0.26, §3.11; aggregated across projects, §2.4)
workspace.workspaceFolders         (real multi-root, v0.182, §2.4)
workspace.didChangeWatchedFiles    (registered dynamically by the server, v0.183, §2.3)
workspace.fileOperations.willRename (#302, §3.22; `**/*.bynk` files, and every folder since #2105)
```

Custom (non-standard) requests — no `LanguageServer` trait slot, registered via `tower-lsp`'s `custom_method` builder, advertised through `experimental` (the only feature-detection surface a client has for one):

```
bynk/sequenceModel               (#846, §3.23; served from the committed round, no refresh nudge)
bynk/documentationModel          (#847, §3.24; whole-file, served from the committed round, no refresh nudge)
bynk/architectureModel           (#851, §3.25; project-scoped, served from the committed round, no refresh nudge)
bynk/wireContract                (#855, §3.26; file+position-scoped, served from the committed round, no refresh nudge)
bynk/explain                     (#2098, §3.27; keyed by the code alone, no document or round)
bynk/refinedTypeModel            (#2470, §3.28; file+position-scoped, served from the committed round, no refresh nudge)
```

Not declared (genuinely out of scope so far):
- inlayHint/resolve, semanticTokens/delta (throughput, not correctness)
- pull diagnostics (`textDocument/diagnostic`) — the server pushes

### 4.4 Error recovery for diagnostics

The LSP needs the parser and checker to recover and continue after errors so multiple errors can be reported. The existing compiler may currently bail at first error in each phase; extending it to recover is part of this increment.

**Parser recovery:** Skip to the next synchronisation point (closing brace, semicolon, top-level declaration keyword) and continue. Each recovered region produces a separate parse error but doesn't prevent parsing the rest.

**Resolver recovery:** Continue resolving even when individual names fail (mark them as unresolved in the AST). The checker then sees an annotated AST with both resolved and unresolved nodes.

**Checker recovery:** Continue type-checking even when individual expressions fail. Unresolved types propagate but don't cascade — once an unresolved type is encountered, further errors on the same expression are suppressed to reduce noise.

The result: a parse-resolve-check pipeline that produces a complete list of errors per file, not just the first.

### 4.5 Performance targets

- **Project load:** under 2 seconds for projects with ~100 source files. Larger projects are out of scope for performance optimisation in this increment.
- **Live diagnostics:** under 100ms for a single-file change (post-debounce). The 300ms debounce plus 100ms diagnostics = response within ~400ms after the user pauses typing.
- **Hover:** under 50ms for any hover query.
- **Go-to-definition:** under 100ms.
- **Format:** under 200ms for a 1000-line file.

These are targets, not requirements. If real-world performance falls short, the on-save fallback for diagnostics is the immediate mitigation; deeper optimisations (incremental compilation, persistent caches) come later.

---

## 5. The VS Code extension

### 5.1 Activation

The extension activates when:
- A workspace folder contains a `bynk.toml` file, OR
- A `.bynk` file is opened.

On activation:
1. Locate the `bynkc-lsp` binary (bundled with the extension or installed separately — for first cut, bundled).
2. Start the LSP server as a child process.
3. Connect via stdio.
4. Register file watchers on `**/*.bynk` and `bynk.toml`.
5. Register the tree-sitter grammar for `.bynk` files.
6. Show status-bar items.

### 5.2 Configuration

The extension reads VS Code settings under the `bynk.*` namespace:

```json
{
  "bynk.executablePath": "bynkc-lsp",    // path to the LSP server binary
  "bynk.trace.server": "off"             // "off" | "messages" | "verbose"
}
```

VS Code's built-in `editor.formatOnSave` is honoured for format-on-save behaviour.

### 5.3 Logging

LSP server logs to `~/.bynk-lsp.log` at warning level by default. The `bynk.trace.server` setting can crank this to verbose for debugging.

The extension exposes a "Bynk LSP" output channel in VS Code for protocol-level traces.

---

## 6. Testing strategy

### 6.1 Tree-sitter grammar tests

The tree-sitter project includes its own test infrastructure (`tree-sitter test`). Tests are organised by syntactic form:

- `corpus/v0.txt` — refined types, pure functions (v0 syntax).
- `corpus/v0.1.txt` — let, if/else, Result, etc.
- `corpus/v0.2.txt` — records, sums, methods, match, is, Option.
- `corpus/v0.3.txt` — opaque, multi-file, doc blocks, uses.
- `corpus/v0.4.txt` — contexts, exports, consumes.
- `corpus/v0.5.txt` — Effect, capabilities, providers, services, agents, commit.

Each test is an input source plus the expected parse tree. `tree-sitter test` verifies.

### 6.2 LSP tests

The LSP server has unit tests for individual handlers (hover, definition, formatting) using synthetic in-memory documents. End-to-end tests use the existing fixture corpus from the compiler — verifying that fixtures produce the expected diagnostics, hover content, and so on.

The v0.25 references/rename surface is tested as **pure functions** (`index_queries`): the `bynkc` fixture matrix proves the index captures every reference kind with name-segment spans and no same-name conflation, and the rename pipeline (plan → apply → re-analyse → validate) is exercised over real multi-file temp projects, including a collision refusal and a genuine capture refusal. The handlers are thin position/packaging shims over that core; the JSON-RPC round-trip harness remains deferred to the first feature that needs it.

The v0.26 code-action surface follows the same pattern: fix **correctness** is pinned in `bynkc` (each seed diagnostic carries its expected suggestion; the list-aware `given` fixtures assert exact text for first/middle/last/only positions, and every applied fix re-diagnoses clean), and the pure quick-fix computation (`code_actions`) is exercised end-to-end over a real temp project — diagnostic-span keying, the versioned `WorkspaceEdit`, and the applied-edit round-trip. The capability advertisement is a unit check over the extracted `server_capabilities()`; no transport round-trip is claimed. The riders are `index_queries` unit tests.

### 6.3 Formatter tests

The formatter is tested via snapshot tests using `insta` (same pattern as the existing compiler). For each input file, the expected formatted output is checked in. The formatter is run on the input; the result is diffed against the snapshot.

Idempotency tests: for each input, run the formatter twice and verify the output is identical between runs.

### 6.4 VS Code extension tests

Minimal — VS Code's testing infrastructure is verbose. For first cut, manual smoke tests:
- Open a fixture in VS Code with the extension installed.
- Verify highlighting appears.
- Verify diagnostics show.
- Verify hover works on a few identifiers.
- Verify go-to-definition works.
- Verify format-on-save works.

A more thorough test suite can come later if the extension grows complex.

---

## 7. Build and distribution

### 7.1 Building

The Rust components build via standard `cargo build --release` from the compiler workspace. Outputs:
- `target/release/bynkc` — the compiler CLI (already exists).
- `target/release/bynkc-lsp` — the LSP server.
- The formatter lives as a library used by both `bynkc` (for `bynkc fmt`) and `bynkc-lsp` (for the formatting capability).

The tree-sitter grammar builds via `tree-sitter generate` in the grammar directory. Output: `tree-sitter-bynk.so` (or `.dylib`/`.dll` per platform).

The VS Code extension builds via `npm run package` in its directory. Output: `bynk-vscode-<version>.vsix`.

### 7.2 Installation

For local sideload:

1. Build all three components.
2. Place `bynkc-lsp` in a location on `PATH` (or configure `bynk.executablePath` in VS Code).
3. Install the VS Code extension: `code --install-extension bynk-vscode-<version>.vsix`.

A bundled installer (single script or installer package) is not in scope for this increment; manual install is acceptable for first cut.

### 7.3 Updates

Updates are manual: rebuild, reinstall. Auto-update via a marketplace is deferred.

---

## 8. What's deferred (future tooling increments)

The editor surface is, by this repo's standard, complete: every capability §4.3
declares is backed by a handler, and the foundation under it — one project model
shared with `bynkc`, a freshness contract, real multi-root workspaces, startup
analysis, dynamic watchers, and one diagnostics scheduler — landed across the
**LSP foundations** track (ADRs 0198, 0201, 0202, 0204). What remains deferred:

**Capability depth** (genuinely unbuilt, not throughput):
- Rename for local bindings; match-arm and `is`-narrowing navigation; service cross-context dispatch callee edges in call hierarchy (§3.18 — `Fn`/`Method`/`CapabilityOp`/agent-`Handler` edges shipped, #304); the consumed-context (`uses B`/`B.Cap`) navigation half; auto-import via `completionItem/resolve`; a test-run ("▶ Run") codelens; `extract`/`inline` refactorings.

**Throughput** (correctness is unaffected):
- Semantic-token delta, `inlayHint/resolve`, pull diagnostics, and an incremental (salsa-style) analysis layer — full re-analysis is acceptable at current scale (§3.2, the named scaling cliff at many-hundred-file projects).

**Packaging / reach:**
- Marketplace publication and auto-update; editor support beyond VS Code (the tree-sitter grammar and the stdio LSP already work in any LSP-capable editor — the open question is which extensions to package and maintain).

These come as practice surfaces the need.

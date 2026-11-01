# The Bynk compiler, as it could have been

**A greenfield reference for the Foundations layer**

**Status:** reference. Not a proposal, not a plan, not scheduled. This document describes the
compiler Bynk could have had if its architecture had been chosen with what is known at v0.245.0,
after the 27 July 2026 pipeline review and the pre-1.0 retrospective. It exists to be *consulted* —
when a decision comes up in the shipping compiler, look up the counterfactual here and find both the
answer and what moving toward it would cost.

**Date:** 30 July 2026 · **Companion to:** `reviews/2026-07-27-compiler-pipeline-review.md`

---

## Part 0 — How to read this

### 0.1 What this is

A specification of a compiler for the **Foundations layer** of Bynk — per `bynk-design-notes.md` §2,
"bounded contexts as namespaces; the service/agent split; actor declarations for authentication;
value types with opaque-versus-transparent visibility; basic handlers returning `Result[T, E]`;
atomic-handler semantics; `Cell` and `Map` storage with the `:=` and `.update(fn)` write forms;
cross-agent calls via `Ref[A]`" — specified to the depth at which someone could build it, with
**named extension points** through which the coordination and advanced layers attach.

§18 of the design notes supplies the membership test, adopted here as the scope boundary:

> The foundations layer uses **no capabilities at all**; the coordination layer uses platform
> capabilities (`Clock`, `Http`, `Events`); the advanced layer uses first-party and domain-specific
> capabilities (`Sagas`, others).

So **Foundations is the subset expressible with an empty `given` clause**. Part 12 specifies the
seam through which everything above it enters; Part 13 walks four real post-Foundations features
through that seam and states what each costs, because an extension point nobody has pushed on is a
claim rather than a mechanism.

The language surface is taken as given. Nothing here proposes a change to `.bynk` syntax or
semantics; where a conclusion would require one, it is boxed as a *Language-surface note* and flagged
as needing its own ADR.

### 0.2 What this is not, and what it deliberately omits

Deployment orchestration (`bynk deploy`), `bynk dev`, the language server's feature set, the
formatter's layout algorithm, the test runner's process model, wasm packaging.

Two omissions are large enough that silence would mislead. **The test sublanguage** — `suite`,
`case`, `stub`, `property`, and the `Expect`/`Val`/`Observation`/`Trace` expression forms — is
represented in the IR (Part 6) and not in emission (Part 8); its runner is out of scope. **The
coordination and advanced layers** are specified only as far as Part 13's worked cases take them.

### 0.3 Fidelity

Sections are marked where it matters:

- **[source]** — specified from the shipped compiler's source, and the rule states either what it
  does or what it should have done instead, with the divergence recorded in Appendix D.
- **[design]** — specified from the design corpus (`bynk-design-notes.md`, `bynk-type-system.md`,
  the decision records) without source verification.

Parts 4, 5, 6 and 8 are predominantly [source]. Parts 12 and 13 are mixed.

### 0.4 Conventions

Normative rules are numbered **R‹part›.‹n›** and stated as single sentences in the imperative, each
with a one-line rationale. **Appendix B** maps every rule to the review finding or ADR hedge it
exists to make impossible; a rule with no Appendix B entry is doing no work and should be deleted.
**Appendix D** maps every rule to where the current tree diverges, so the document is usable without
a rewrite ever happening.

Rust-shaped type sketches illustrate *shape*, not final naming. Where a shape is load-bearing the
rule says so. TypeScript excerpts are from real emitted output unless marked otherwise.

### 0.5 The one meta-rule

**R0.1 — A decision about the compiler's representation earns an ADR before code, on the same terms
as a decision about the language.**

*Rationale:* the corpus contains 304 decision records and none of them argues for or against the
emitter's substrate. ADR 0059 §4 — "Refactors are not language-defining, so they do not each earn a
decision record" — is correct for a file split and wrong for a representation. The costliest
implementation decision in the project was never written down, so it was never argued, so it was
never re-examined when its cost curve crossed. The trigger for an ADR is *irreversibility*, not
*topic*.

### 0.6 Revisions

- **r1–r4, 30 July 2026 —** the document as an argument: principles, representation, phases, the IR's
  expression half, the TS tree, diagnostics, crates, testing, extension points, capability tiers, the
  refusal register.
- **r6, 30 July 2026 —** Appendix D re-measured against the working tree rather than inferred from
  the 27 July review. Nine of the fourteen items it listed as open had already landed in the eight
  versions since; fourteen rows are now ✅. One row moved the wrong way: `bynk-emit`'s diagnostic-code
  count grew. The appendix now states that it **must** be generated, and names the track slice that
  generates it.
- **r5, 30 July 2026 —** reframed from argument to **reference**. Under r1's framing ("specified far
  enough to be argued with rather than admired") the settled parts were left thin; a reference cannot
  do that. Added Part 4 (types), Part 5 (patterns and matching), Part 8 (emission targets), Part 13
  (worked extension cases) and Appendix D (the migration index); expanded Part 6 with the callee
  taxonomy, pattern IR and declaration IR. Parts renumbered: the r1–r4 Parts 4–11 are now 6–15.

---

## Part 1 — Governing principles

Five principles. Every rule in this document is a consequence of one of them; a rule that cannot be
traced to one is preference wearing a specification's costume.

**P1 — Every phase is a total function from a value to a value.**
No ambient state, no filesystem, no mutation of the caller's world. The July review's single most
useful measurement is that in-file test density is bimodal and tracks exactly this property:
`bynk-check` 5.8%, `bynk-emit` 6.5%, `bynk-syntax` 8.6% against `bynk-ide` 36.9%, `bynk-render`
38.6%, `bynk` 30.6%, `xtask` 32.0% — same author, same standards, same fortnight. Testability is
determined by API shape, not by discipline. P1 is therefore the testing strategy, stated once, at the
top.

**P2 — Every fact that two places need travels in a type that only one place can construct.**
A grep for the phrases the shipped codebase uses to pin invariants by convention — "in sync",
"mirrors", "parity", "must match" — returns 233 hits, and four of those duplications have already
drifted. A sentence in a doc comment is a bug report against the type system, filed and ignored.

**P3 — The compiler never touches the filesystem.**
One `Sources` value, constructed at the CLI edge, is the only view of the world. This makes the
editor, the CLI, the wasm build and the test harness the same program rather than four programs that
agree by inspection.

**P4 — If a second copy can be derived, it is derived; if it cannot, the enum is exhaustive.**
Wildcard arms over the compiler's own enums are banned at the lint level. A new `ExprKind` variant
must be a compile error at every site that cares, not a silent `_ => false`.

**P5 — When the right shape lands, the wrong one is deleted in the same change.**
The recurring failure in the shipped compiler is not a wrong abstraction; it is a correct abstraction
introduced as *available* while the old path stayed reachable. `UnitInfo` landed as an addition, not
a replacement. `CompileOptions` collapsed at the public boundary while the twelve-positional-argument
`run_checks` beneath it did not. The overlay was plumbed and two reads still hit disk.
`expr_children` exists with twelve adopters and three hand-rolled holdouts. In a compiler where one
fact is needed at six sites, *available* means *adopted at four*, and the stragglers are where the
bugs are.

---

## Part 2 — Representation

### 2.1 Identity and location

```rust
struct FileId(u32);

struct FileMeta {
    abs:      PathBuf,   // absolute location on disk (source-map entry)
    tree:     PathBuf,   // relative to its own include root (validates unit name; names the artefact)
    identity: PathBuf,   // relative to the project root (keys every analysis map)
}

struct Sources { files: IndexVec<FileId, FileMeta>, text: IndexVec<FileId, Arc<str>> }

struct Span { file: FileId, lo: u32, hi: u32 }
```

**R2.1 — A file has exactly three path forms and they are never collapsed.**
*Rationale:* ADR 0198's finding, in its own words: "a third path form joins `abs_path` … Three is the
honest count … collapsing any two of them is what produced this defect." A flat `include` list made
file names non-unique and "nothing noticed for sixty increments."

**R2.2 — `Span` carries a `FileId`. There is no file-less span anywhere in the compiler.**
*Rationale:* without it a renderer can only check that a label is in-bounds of whichever file it
happens to be rendering, so cross-file `declared here` labels underline unrelated text (#46).

**R2.3 — `Sources` is constructed once, at the process edge, and is the compiler's only view of file
contents. No crate below the driver may name `std::fs`, enforced by a lint and a CI grep.**
*Rationale:* P3. In the shipped compiler `compile_in_memory` drives the whole pipeline from a `&str`,
but hard-codes one file, `CompileOptions` has no sources field, and two reads bypass the overlay
anyway (#57, #13). The seam was 90% built and 0% mandatory.

### 2.2 Node identity

```rust
struct ExprId(u32);  struct StmtId(u32);  struct PatId(u32);
struct ItemId(u32);  struct DefId(u32);   struct TyId(u32);

struct Ast {
    exprs: IndexVec<ExprId, Expr>,   // Expr = { kind: ExprKind, span: Span }
    stmts: IndexVec<StmtId, Stmt>,
    pats:  IndexVec<PatId,  Pat>,
    items: IndexVec<ItemId, Item>,
}
```

**R2.4 — Node identity is an arena index allocated at parse time. Position is never identity.**
*Rationale:* the shipped checker→emitter channel is `expr_types: HashMap<Span, Ty>` (`checker.rs:242`)
over a shared keyspace, and the failure mode is a *collision*, not a miss: bug #844 (a synthetic
single-expression block whose span equalled its tail's — the shipped guard is a literal
`block.span != block.tail.span` test at the single write site), the else-less `if` where three nodes
share one key, and `is_binding_cache: HashMap<Span, Vec<(String, Ty)>>` serving stale narrowings on
collision. The shipped code knows: a `#[cfg(debug_assertions)]` walk at `checker.rs:314-393` recurses
with `ast::expr_children` and asserts no two typed nodes share a span, with the message *"two typed
AST nodes share span {:?} in `expr_types` — one node's recorded type silently clobbered another's"*.
An arena index makes the assertion unnecessary.

**R2.5 — Every side table keyed by a node is an `IndexVec`, i.e. total. A phase that needs a fact
about every node must be unable to express "I have no fact about this node".**
*Rationale:* see R4.9 for what the shipped `Option`-based recovery costs downstream.

**R2.6 — AST nodes carry no *trivia*. They do carry *documentation*.**
A `--` comment is formatting debris: it belongs to the token vector (R2.8) and no phase after the
formatter sees it. A `--- … ---` doc block is a declaration's content: a field on the node, resolved
and indexed like any other.
*Rationale:* the shipped AST already draws this line correctly — every declaration carries
`documentation: Doc` *beside* `trivia: Trivia` — and it must, because hover, the documentation view
(ADRs 0265–0269), inline doc rendering (0263) and intra-doc links (0257) all read it, and ADRs
0188/0219 give doc blocks their own lexical rules. The failure being prevented on the trivia side:
`Trivia` is attached to declarations and statements but not expressions, which forces
`Block::tail_leading_comments` and unit-level `trailing_comments` as compensating fields, and
expression-interior comments are simply lost.

**R2.7 — `size_of::<Expr>()` is pinned by `const _: () = assert!(…)`. The bound is part of the spec,
not a test.**
*Rationale:* the one size discipline the shipped compiler already gets right, and it works —
`ExprKind::Observation` and `Is.pattern` are boxed precisely because the assertion fired. Measured:
boxing takes `Expr` from 176 to 120 bytes and `MatchArm` from 488 to 376.

### 2.3 Lexical layer and trivia

```rust
struct Tokens { kinds: Vec<TokenKind>, spans: Vec<Span>, trivia: Vec<TriviaPiece> }
struct NodeTokens { ranges: IndexVec<ItemId, Range<u32>>, /* … per node kind */ }

fn lex(src: &str, file: FileId) -> (Tokens, Vec<Diagnostic>);
fn parse(tokens: &Tokens) -> (Ast, NodeTokens, Vec<Diagnostic>);
```

**R2.8 — The token vector, including all trivia, is returned by the parse entry point and outlives
parsing. It is never drained into AST nodes.**
*Rationale:* `split_trivia` currently builds a complete, lossless comment index and then hands
*copies* out through `mem::take` into 34 separate `Trivia` fields; the table is private and dropped
unchecked when parsing ends. Comments are lost in three brace forms and in every expression interior
(#26, #66). Returning the table makes loss structurally impossible and gives the formatter one source
of truth instead of 34.

**R2.9 — The compiler has no concrete syntax tree. A CST exists, for editor distribution, and is
never on a compilation path.**
*Rationale:* the July review's rejection stands and is narrow: "a rowan CST — the assertion plus
returning the trivia table gets the formatter what it needs at a small fraction of the cost, and
nothing else in the language obviously wants lossless trees." R2.8 is the cheap 90% *for the
compiler*. See R2.13 for the other tree.

**R2.10 — Byte-stable re-emission facts live on the node that owns them, and only where a canonical
printed form does not exist.**
*Rationale:* already correct in the shipped AST — `IntBound` deliberately stores no lexeme because
"ints have one canonical printed form, so the formatter stays idempotent", while `FloatLit` keeps its
lexeme so `1e10` does not normalise. Preserve the asymmetry and its reasoning.

### 2.4 Walkers, and the grammar

**R2.11 — There is exactly one child enumeration per node kind, it is exhaustive with no wildcard
arm, and it is the only way any phase reaches a child.**
*Rationale:* `ast::expr_children` already exists, is exhaustive by design with a doc comment naming
the bug it fixes, and has twelve adopters — and hand-rolled walks still shadow it. The rule is not
"write a walker"; it is "make the alternative unwritable". Note the one legitimate exception, which
the shipped `block_writes_state` documents: a walker that must observe *statement* structure cannot
use `expr_children`, "because an `expr_children` descent flattens a block straight to its statements'
*values*, losing exactly the `Statement::Assign` tag". Such a walker states that in its own doc and
enumerates `Block`/`If`/`Match` by hand — and R2.12 still applies to it.

**R2.12 — `#![deny(clippy::wildcard_enum_match_arm)]` is set in `[workspace.lints]` for every crate
that matches on a compiler-owned enum.**
*Rationale:* P4, mechanised. There is no `[workspace.lints]` table in the shipped workspace. The
drift class — `type_refs_match` covering 9 of 18 `TypeRef` variants with `_ => false`, with a
consumer where a false negative is silently wrong emitted TypeScript (#8) — is exactly what this lint
refuses to compile. The shipped `Ty` operations get this right already: `compatible`, `unify`,
`substitute` and the structural-compat walk all enumerate with no `_` arm, "a deliberate device so
adding a variant is a compile error".

**R2.13 — The project publishes a declarative grammar. The compiler's parser is authoritative; the
grammar reflects what parses.**
The grammar is not a second parser and is never used to compile. It exists to (a) render `.bynk` as
code in editors and hosts that do not run the language server, and (b) state the language's syntax
declaratively, which a recursive-descent parser structurally cannot do — it resolves ambiguity by
execution order rather than by declaration.
*Rationale:* the direction rule is ADR 0189 D2's, verbatim — "the **grammar moving to the compiler**,
not the reverse: the compiler already parses a general expression and restricts it … exactly as
function contracts do (parse broad, check narrow). The grammar should reflect what *parses*." The
grammar's demonstrated value is as a **forcing function**: ADR 0253 D4's entire analysis — that
`refinement`'s `&&`-joined predicate list collides with the surrounding expression grammar wherever a
pattern is expression-continuable — exists because writing the tree-sitter rule forced the question
"where is `refined_pattern` reachable from?", and the answer exposed a real defect in the
hand-written parser. Conformance is R11.7.

---

## Part 3 — Phase contracts

The spine. Every arrow is a total function; every box is a value.

| # | Phase | Input | Output | Crate |
|---|---|---|---|---|
| 0 | Load | `&Path`, overlay | `Sources`, `Manifest`, `SchemaRegistry` | driver only |
| 1 | Lex | `Sources` | `Tokens` per file | `bynk-lex` |
| 2 | Parse | `Tokens` | `Ast`, `NodeTokens` | `bynk-parse` |
| 3 | Project | `Manifest`, `Ast`s | `ProjectGraph` | `bynk-project` |
| 4 | Resolve | `ProjectGraph`, `Ast`s | `Resolved` (`DefId` bindings) | `bynk-resolve` |
| 5 | Check | `Resolved` | `TypedProgram` | `bynk-check` |
| — | **Certify** | `TypedProgram` | `CheckedProgram` **or** diagnostics | `bynk-check` |
| 6 | Lower | `CheckedProgram` | `Ir` | `bynk-lower` |
| 7 | Emit | `Ir` | `TsProgram` | `bynk-emit` |
| 8 | Print | `TsProgram` | `Artefacts` | `bynk-ts` |
| 9 | Strip | `Artefacts` | JS `Artefacts` | `bynk-strip` |

**R3.1 — Phases 1–5 are `fn(Input) -> (Output, Vec<Diagnostic>)`. Their outputs are *total*: always
constructible, carrying explicit error nodes where a fact could not be established. The only other
parameter is an options value that is `Clone` and `Debug`.**
*Rationale:* P1, and the editor. `run_checks` currently takes twelve positional arguments beneath a
public entry point that takes one; `CompileOptions` is not `Clone`, so `bynkc test` rebuilds it from
scratch and loses `contracts: true` and `import_ext: Ts` (#19).

**R3.2 — Diagnostics accumulate. No phase bails. Bailing is a property of the *driver*.**
*Rationale:* `bynk check` runs the bailing, emitting path and reports fewer errors than the editor
does over the same project (#64). One pipeline, one answer.

**R3.3 — A phase's output is serialisable and structurally comparable on its own terms, not only as a
substring of the pipeline's final emitted text.**
*Rationale:* this is the architectural property Appendix B and D cite under this number — a refactor
whose only net is a whole-file golden one crate up (#58; ADR 0059's gate), and phase outputs that are
not snapshot-testable in isolation. It is a distinct claim from the acceptance-*policy* ADR 0309 states
(which tier of change needs which gate): R3.3 is the property that makes a phase-local snapshot fixture
possible to write at all; ADR 0309 is the policy that then requires one. Section 3.4's demand-driven
precondition table depends on the same property for a different reason — a query cannot be memoised
against an output that cannot be compared for equality.

**R3.10 — The gate between analysis and emission is a type, not a control-flow decision.**
`fn certify(p: TypedProgram, d: &[Diagnostic]) -> Result<CheckedProgram, Vec<Diagnostic>>` is the
single place the question "may we emit?" is asked. `CheckedProgram` is constructible only by
`certify`, so no error node can reach phase 6 and R6.1's "every `IrExpr` carries its type" stays true
by construction.
*Rationale:* a `Result` at every phase is a batch-compiler contract. In an editor the input is
mid-keystroke and a `TypedProgram` frequently cannot be completed — yet completion, hover and
signature help must still answer, which is why the shipped compiler has `parse_unit_with_recovery`,
ADR 0094's error-tolerant receiver typing, and a `RecordCheck { result, partial_expr_types }` return
that carries a best-effort map on the error path. Under R3.1/R3.10 the editor consumes the total
output of phases 1–5 and never calls `certify`; the CLI calls it and stops.

**R3.11 — Cross-build state is an explicit input and an explicit output. The compiler is a pure
function of (sources, manifest, prior state, options).**
*Rationale:* `compile(sources, manifest, opts) -> Artefacts` is false today. The event schema registry
is a persisted lockfile: per `bynk.bynk`'s `EventEnvelope` documentation, `bynk.schema.lock` holds a
version where "unchanged shape keeps its stored version, a purely additive shape change (only new
defaulted fields) auto-bumps it, and a non-additive change **fails the build**". A diagnostic depends
on the previous build, and P3 hid an ambient dependency rather than removing one.

### 3.1 Where semantics live

**R3.5 — All semantic checking — declaration-level and context-level alike — happens in phase 5.
`bynk-emit` performs no checks and emits no diagnostics.**

*Rationale:* the single largest structural correction. Today `checker::check_record` matches exactly
two item kinds, `Type` and `Fn`; everything that makes Bynk *Bynk* — capability and provider
signatures, handler shape, actor `by` contracts, agent `store` fields, `@indexed` hygiene, CORS and
rate-limit policy — is checked in `bynk-emit/src/project/validate.rs`. The count: 207 distinct
`bynk.*` codes originate in `bynk-check`, 190 in `bynk-emit`, 110 of them in `validate.rs` alone.
ADR 0099's hedge — "read the name as 'build orchestration + TS emission' — orchestration drives
emission" — is the most consequential sentence in the corpus.

The causal chain is what R3.5 prevents rather than repairs: context-level semantics were written in
`project.rs` because that is where the project model was, and the project model was in the emitter
because emission needed it first. Ordering produced the shape. Extracting `bynk-project` as its own
phase *below* both check and emit is what makes phase 5 the only place context checking can live.

The measured cost of the current arrangement: `validate.rs` reaches back across the crate boundary to
drive the checker, and to do so hand-rolls `ResolvedCommons` **six times** against one legitimate
construction in the resolver, producing three mutually incompatible readings of one documented field
and turning **three checker gates off inside agent handler bodies** — `.raw` on an opaque type,
`T.unsafe(…)`, and owner-only event emission, the last of which its own comment calls "the primary
boundary guarantee the threat model names".

**R3.6 — Types resolved by the checker are compared as resolved types. There is no syntactic type
comparison anywhere in the compiler.**
*Rationale:* `type_refs_match` (#8). "Delete `type_refs_match` and do that. If a purely syntactic
comparison is genuinely wanted somewhere, make it exhaustive so the next variant is a compile error."

### 3.2 The project model

```rust
struct ProjectGraph {
    units:    IndexVec<UnitId, Unit>,          // name, kind, contributing FileIds
    files:    IndexVec<FileId, UnitId>,
    edges:    Vec<(UnitId, UnitId, EdgeKind)>, // uses / consumes / provides
    contract: IndexVec<UnitId, ContractHash>,
}

enum UnitKind { Commons, Context, Adapter, Suite }
```

**R3.7 — A unit's identity is its declared qualified name, resolved from `FileMeta.tree`; a unit may
be contributed to by many files; a file contributes to exactly one unit.**

**R3.8 — The manifest is parsed into a typed `Manifest` value with a `Result` return. There is no
total, silently-defaulting manifest reader.**
*Rationale:* `read_project_paths` is "total by construction and returns `ProjectPaths`, not a
`Result`", so a malformed `bynk.toml` is silently ignored — "the one input the user hand-edits that
the compiler reads without checking" (#13). Separately, the LSP and CLI each read `[paths]` and
disagreed until ADR 0201 deleted one of the readers.

**R3.9 — Test-ness is structural, never directory-derived. `Roots` models exactly what the manifest
says and nothing that a previous manifest schema said.**
*Rationale:* ADR 0147 replaced role-named `src`/`tests` keys with a flat `include` list; `Roots` still
models the removed role split, drops `include[2..]`, and mis-reports suite locations (#12).

### 3.3 The editor's queries are requirements on phase outputs

**R3.12 — Each phase output must answer the editor queries assigned to it below. A query that needs a
fact the phase does not publish is a defect in the phase, not a request.**

| Query | Answered from | Needs published |
|---|---|---|
| syntax highlighting, folding, selection ranges | `Tokens`, `Ast` | token kinds, node→token ranges (R2.8) |
| document symbols, breadcrumbs | `Ast` | declaration spans + doc blocks (R2.6) |
| hover, signature help, inlay hints | `TypedProgram` | per-`ExprId` types, resolved `DefId`, doc blocks |
| completion (member, value-receiver, positional) | `TypedProgram` | receiver type at cursor under recovery; the kernel registry (R11.6) |
| go-to-definition, references, call hierarchy | `Resolved` | `DefId` → def span, and the inverse index |
| implementation navigation, capability op sites | `Resolved` + `ProjectGraph` | provider↔capability edges |
| project diagnostics, cross-file attribution | phases 1–5 diagnostics | `Span.file` (R2.2), identity paths (R2.1) |
| rename, extract function/variable, quickfixes | `Ast` + `Resolved` + `Tokens` | binding scopes; token ranges for text edits |
| unit→source map, architecture map, sequence diagrams | `ProjectGraph` | unit↔file mapping, edges, handler call structure |

*Rationale:* the one structural correction to the document's original shape. r1 defined every phase
output by what the *next phase* needed and treated the editor as a downstream consumer of whatever
fell out — exactly the mistake ADR 0095 records: the unit→source map "already exists at analysis
time … **It is simply not exposed.**" It took a decision record to publish data the compiler already
had. Without the table that pattern repeats once per feature, and ADR 0201's own note shows it does
not converge — its proposal "said 'three callers' and was wrong by an order of magnitude" at roughly
fifty.

### 3.4 Query granularity

Incrementality is specified here, not in Part 15's refusal register, because it is a property of the
phase contracts rather than a feature bolted beside them.

**The greenfield arithmetic is not the retrofit arithmetic.** Inverting a shipped 250k-line pipeline
into a demand-driven one is an enormous fixed cost — the July review is right to refuse it on those
terms. But this specification has already imposed nearly the whole of that cost for unrelated
reasons, and has so far collected none of the benefit:

| Precondition for demand-driven recomputation | Already a rule here |
|---|---|
| Queries are pure functions of values | P1 |
| No ambient filesystem or global state | P3, R2.3 |
| Stable interned identity usable as a key | R2.4 |
| Total side tables, no fallible lookups | R2.5 |
| Cross-build state is explicit | R3.11 |
| Outputs are serialisable and comparable | R3.3 |

A straight batch pipeline needs none of these. Having paid for all six, declining to memoise is
installing the plumbing and not connecting the taps.

**R3.13 — Every phase output is decomposed to the granularity at which it is *invalidated*, not the
granularity at which it is *computed*.**

| Level | Query | Invalidated by |
|---|---|---|
| File | `Tokens(FileId)`, `Ast(FileId)` | an edit to that file |
| Unit | `UnitSignature(UnitId)` — declaration signatures only, **no bodies** | a signature edit in that unit |
| Definition | `Body(DefId)`, `TypeOf(DefId)` | an edit to that definition |
| Project | `ProjectGraph` | a manifest change, or a file appearing or disappearing |

The phase table above is the **batch composition** of these queries, not an alternative to them:
`check` is the fold over per-definition queries, not a monolith that happens to run once.

*Rationale:* the measurable failure today is granularity, not scheduling. "Every IDE query re-lexes
and re-parses from `&str`; the analysis re-parses the first-party surface each round" (#65), and
"`for_each_unit` never filters the cursor's own path out of `files`, so the current file is parsed
twice per keystroke, once live and once stale" (#62).

**R3.14 — `UnitSignature(UnitId)` is stable under an edit to any `Body(DefId)` within that unit. A
signature that changes when a body changes is a defect in the signature.**

*Rationale:* this is the firewall, and it is the difference between incremental and
incremental-in-principle. **Bynk is unusually well shaped for it, and the corpus already says why.**
The annotation policy in design notes §15 — required at "function and handler declarations
(parameters and returns); agent storage declarations; cross-context type references; capability sets
via `given`", inferred for "local `let` bindings", under "visible boundaries, invisible internals" —
*is* a firewall specification restated in ergonomic language. In a language with whole-program
inference the boundary has to be invented and defended; here it was mandated for readability and the
incremental property falls out.

Second, the query already exists in substance. ADR 0200's cross-context contract hash is a canonical
content hash over a unit's public surface — "one canonical normal form, owned by `bynk-check`, shared
by the matcher and the hash" — built to make cross-context calls fail closed, and guarded over the
whole fixture corpus by `bynkc/tests/contract_hash.rs`. That is `UnitSignature`'s identity function,
pointed at a different problem.

**R3.15 — The granularity is committed; the scheduler is deferred.** Whether memoisation is provided
by salsa, by a hand-rolled memo table, or by nothing at all for the first year is a separable decision
with its own trigger (Part 15).
*Rationale:* the hard part is R3.13/R3.14. Once queries have the right keys and the firewall holds, a
scheduler is a few hundred lines. Committing to a framework instead buys the easy half and takes on a
dependency with an MSRV policy and a `deny.toml` to satisfy.

**The honest costs.** The batch path pays for the incremental path — a cold `bynkc compile` does
strictly more work under memoisation, and CLI runs are the deploy path. Debuggability degrades: you
can no longer read the phase table and know what ran. And recursive queries need a stated cycle
policy rather than a stack overflow — the context graph is a checked DAG, but
`generic_record_is_recursive` exists.
---

## Part 4 — Types

**[source]** — specified against `bynk-check/src/checker.rs`, `resolver.rs`, `checker/refinements.rs`.

### 4.1 The representation

```rust
struct TyId(u32);

struct Types {
    tys:   IndexVec<TyId, Ty>,
    intern: HashMap<Ty, TyId>,
}

enum Ty {
    Error,                                              // R4.3
    Base(BaseType),                                     // Int String Bool Float Duration Instant Bytes
    Named  { def: DefId, kind: NamedKind, args: Vec<TyId> },
    Var    { name: Symbol, rigidity: Rigidity },        // R4.7
    Result(TyId, TyId),  Option(TyId),  Effect(TyId),
    HttpResult(TyId),    QueueResult,
    List(TyId),          Map(TyId, TyId),
    Query(TyId),         Stream(TyId),   Connection(TyId),
    Actor(TyId),         ActorSum(Vec<(DefId, TyId)>),
    ValidationError,     JsonError,      Unit,
    Fn { params: Vec<TyId>, ret: TyId },
}

enum NamedKind { Record, Sum, Refined(BaseType, RefinementId), Opaque(BaseType, RefinementId) }
enum Rigidity  { Rigid, Flexible }
```

**R4.1 — Types are interned. `TyId` is the only currency above the intern table; `Ty` values are
constructed only by the interner.**
*Rationale:* the shipped `Ty` derives `Debug, Clone, PartialEq, Eq` — no `Hash`, no `Copy` — and
nothing is interned or `Rc`'d. Recursion is `Box<Ty>`/`Vec<Ty>`, so every `Ty` is a fully owned deep
tree that is deep-cloned on every `.clone()`, and `Ty` can never be a map key. The review's finding
on declaration cloning is the same cost one level up: `check_unit_files` rebuilds four whole-project
maps per unit, and "each of those clones is a deep copy of every declaration body in the program,
performed once per unit" (#51). Interning also gives R3.13's memoisation a cheap key and makes
structural identity an integer comparison. Note the shipped code already reaches for the right shape
one layer out — declarations are `Arc<TypeDecl>`/`Arc<FnDecl>` explicitly "so cloning a
`ResolvedCommons` is a pointer bump".

**R4.2 — `Ty` and `TyId` are `Copy`-cheap, `Hash` and `Ord`. A type is usable as a map key without
ceremony.**

**R4.3 — `Ty::Error` is a real variant. Resolution failure produces it; it never produces "no type".**
`Ty::Error` is assignable to and from everything, suppresses all downstream diagnostics that mention
it, and is rejected by `certify` (R3.10).
*Rationale:* the shipped compiler has no `Ty::Error`, `Unknown`, `Never` or `Any`; recovery rides
entirely on `Option<Ty>`, where `None` means "an error was already pushed; do not pile on". Three
consequences follow, all of them costs. **First**, `None` is not recorded into `expr_types` — the
write site guards on `Some` — so downstream emitter lookups miss and silently take a fallback branch;
there is no marker distinguishing "unchecked" from "checked and broken". **Second**, a single
unresolvable type suppresses every downstream diagnostic in that subtree, which is why the resolver
needs a *separate* `check_type_ref_resolves_in` pass to report the name error that
`resolve_type_ref` swallowed. **Third**, it is the mechanism by which an editor gets nothing from a
broken file (R3.10). A poisoned type is one variant and removes all three.

### 4.2 Refinements

**R4.4 — A refined type carries its predicates *in the type*, in canonical form. There is no side
table keyed by type name.**
`RefinementId` indexes an interned, canonicalised predicate set (Part 12, E3). Two refined types are
identical iff their base and their `RefinementId` are identical.
*Rationale:* today the predicates are not on `Ty` at all. `NamedKind::Refined(BaseType)` records the
base and *the fact of* refinement; the predicate list lives on the AST declaration, reached by name
through `ResolvedCommons::types: HashMap<String, Arc<TypeDecl>>`. So every predicate question is a
name lookup into a table that may or may not be the right one — which is exactly the
`local_type_names` failure at a different level. Canonicalisation is not new work: `refinements_match`
already routes both sides through `contract::canon_refinement` and compares the results, its doc
comment specifying set-not-list semantics, de-duplication, and the deliberate `Some`/`None`
asymmetry ("sending side more restrictive is fine"). Making that the *representation* rather than a
comparison performed at one call site is the whole of R4.4.

**R4.5 — Assignability is one named relation, `assignable(a, b)`, distinct from type identity, and it
is total over the variant set with no wildcard arm.**
*Rationale:* correct in the shipped compiler and worth pinning. `compatible(t, u)` is asymmetric
("`t` usable where `u` is expected"), widens refined→base, is covariant in `List`/`Option`/`Effect`/
`Stream`/`Query`/`Connection`/`Result`/named-generic args, **contravariant in `Fn` params** and
invariant in `Map` keys — while derived `PartialEq` is used separately for exact identity. Two
relations, two names, no `_` arm.

**R4.6 — Opacity is a property of the type; the authority to pierce it is a property of the checking
context, carried as a value.**
`Ty::Named { kind: Opaque(base, _) }` hides `base` from `assignable`; `.raw` and `T.unsafe(v)` require
a `DefiningContext(UnitId)` capability value that only the owning unit's checking environment holds.
*Rationale:* today both gates call `ResolvedCommons::is_local_type(name)` — a `HashSet<String>`
lookup whose backing field is deliberately private, with a hard-won comment explaining it must be the
*pre-merge* table because "reusing the merged table silently widens all three [`.raw`/`.unsafe()`/
owner-only event emission] to any consumed/used type or event". Six hand-rolled constructions of
`ResolvedCommons` in `bynk-emit` then produced three incompatible readings of that field and turned
all three gates off inside agent handler bodies (#16). A capability value cannot be hand-rolled into
existence by a caller that does not have one.

### 4.3 Resolution

```rust
fn resolve_ty(r: &TypeRef, env: &TyEnv) -> TyId;   // total; yields Ty::Error on failure
struct TyEnv<'a> { types: &'a UnitTypes, vars: &'a TyVarScope }
```

**R4.7 — A type variable's rigidity is in the type, not in a convention about which phase is running.**
*Rationale:* the shipped `Ty::Var(String)` has two lives — *rigid* while checking a generic
function's own body (name-equality in `compatible`), and *flexible* during call-site instantiation,
where its own doc says it is "fully eliminated by `substitute` before any `compatible` runs… Vars
never escape call checking into the caller's expression types." That invariant is real, load-bearing,
and pinned by nothing. `Rigidity` in the variant makes an escaped flexible var a type error rather
than a silent mis-comparison.

**R4.8 — Unification returns a verdict, and a structural mismatch is a mismatch.**
*Rationale:* the shipped `unify(pattern, actual, subst)` is argument-directed and one-sided, which is
right — but its ground-vs-ground catch-all is `_ => true`, pinned by a test literally named
`unify_surprise_concrete_mismatch_returns_true`. The design intent is sound (`compatible` after
substitution owns all mismatch diagnostics, so `unify` need only bind variables), but a function
called `unify` returning `true` for `Int` against `String` is a fact enforced by a test name. Return
`Bindings` and let the absence of a binding be the absence of a binding.

### 4.4 The checker→emitter channel

**R4.9 — `TypedProgram` carries `expr_ty: IndexVec<ExprId, TyId>`. It is total. There is no lookup
that can miss.**
*Rationale:* this is R2.5 applied at the seam that actually broke, and the shipped consequences are
worth enumerating because they are all silent. The channel is `HashMap<Span, Ty>`; every consumer
uses `.get(&span)` inside an `if let`, and a miss is a *fallback*, never an error:

| Site | On miss |
|---|---|
| brand cast for a refined value | no cast emitted |
| `HttpResult` vs `Result` at a handler tail | falls to `Result.Ok(...)` |
| the `?` operand | `debug_assert!` in debug; in release, takes the `Result` branch — "which on an untyped `Option` would leak `None` → `undefined`" |
| `receiver_namespace` for a UFCS method call | emits the literal text `/* unknown */` |
| codec root collection | the type is silently omitted from the codec set |

The `fold` kernel is the one place that gives up honestly: it reads the accumulator's TS type back out
of `expr_types` and `panic!`s if absent. A total `IndexVec` removes the branch rather than improving
it.

**R4.10 — The context brand is a property carried in the IR from the checker, not a decoration
invented at emission.**
*Rationale:* `__ctxBrand` exists only in the emitter today — `emit_context_rebrands` writes
`export type {name} = __Commons{name} & { readonly __ctxBrand?: "{owning}" }` so two contexts that
both `uses` the same commons type see nominally distinct TS types. The brand is optional (#1704), so
an unbranded commons value fits either context's type while a value branded by one context does not
fit the other's. The checker mirrors the predicate
separately, through `ResolvedCommons::is_uses_commons_type`, whose doc says it mirrors
`emit_context_rebrands`'s predicate *exactly* — a P2 violation with the mirror stated in prose. The
cost when they disagree is a `tsc` failure in generated code the author never wrote, which is exactly
what ADR 0226 records (#655: "a single named binder took the entire test run down, pointing at
generated code the author never wrote"). One `Ty` carrying its brand, one emitter reading it.

### 4.5 Phase-boundary values

**R4.11 — A phase-boundary value has private fields and a total constructor that derives its
invariants. It is never a bag of `pub` fields.**
*Rationale:* `ResolvedCommons` has eleven fields, ten of them public, whose invariants the resolver
establishes — and six hand-rolled constructions in another crate each independently decide what they
mean (#16). `TypedCommons` then *drops* `cross_context`, `agents`, `is_context` and all three privacy
sets, so a consumer needing any of them must go back to the resolver's output. The review's own
prescription: "give `ResolvedCommons` a constructor that takes the local table and the merged table
as separate arguments and computes `local_type_names` itself, with private fields, so the class of
bug above becomes unrepresentable" — and it calls that "the cheap 90% of the whole finding".

> **Language-surface note.** `MapEntry[K, V]` is a compiler-known nominal record with no
> `TypeDecl`, existing because "bynk has no tuple/pair type (ADR 0120)" — while the type-system spec
> §2.7.6 lists tuples as a built-in and design notes §11 types `join`/`groupBy` as returning
> `Query[(T, U)]`. That contradiction is in the corpus, not in this document, and it should be
> resolved deliberately. This reference assumes ADR 0120: no tuples, `MapEntry` as a real nominal
> type in the prelude rather than a compiler-synthesised one.

---

## Part 5 — Patterns and matching

**[source]** — specified against `bynk-emit/src/emitter/lower.rs` and `bynk-check/src/checker/expressions.rs`.

### 5.1 Pattern IR

```rust
enum IrPat {
    Wild,
    Bind    { local: LocalId },
    Const   { value: ConstVal },                            // Int | Str | Bool
    Variant { def: DefId, tag: VariantId, fields: Vec<(FieldId, PatId)> },
    Refined { inner: PatId, refinement: RefinementId },
    Or      { alts: Vec<PatId> },
}

struct IrArm { pat: PatId, guard: Option<IrExpr>, body: IrExpr, binds: Vec<LocalId> }

enum Exhaustive { Total, Partial { witnesses: Vec<PatternWitness> } }
```

**R5.1 — Patterns are IR nodes. The emitter receives a pattern, never a pre-rendered test string.**
*Rationale:* today `pattern_match_tests` builds a `Vec<String>` of JavaScript fragments during
lowering and the caller `&&`-joins them, which means the pattern's structure is destroyed at exactly
the point the printer would want it (R7.3) and the source map has nothing to attach to.

**R5.2 — The lowering form is computed once, from the arm set, and recorded in the IR.**

The shipped decision procedure is two orthogonal axes, and it is correct — keep it, but decide it in
phase 6 rather than re-deriving it at three emission sites:

*Axis A, position.* Tail position emits statements in place, each arm body ending in `return`. Any
other position wraps in an arrow. *Axis B, shape.* Both positions ask one predicate:

```rust
fn match_needs_if_chain(arms: &[MatchArm]) -> bool {
    arms.iter().any(|a| {
        a.guard.is_some()
            || pattern_has_nested_test(&a.pattern)
            || matches!(a.pattern, Pattern::Refined { .. })
    })
}
```

with `pattern_has_nested_test` true for a `Variant` any of whose bindings is refutable-or-nested, a
`Refined` whose inner is, and an `Or` any of whose alternatives binds a name or is itself nested.
`Pattern::is_irrefutable` is `Wildcard | Binding`, and `Or` is irrefutable iff *any* alternative is.

| Position | Shape | Form |
|---|---|---|
| tail | flat | `switch`, then the trailing throw |
| tail | needs chain | a flat sequence of independent `if` blocks |
| value | flat | arrow-wrapped `switch` |
| value | needs chain | arrow-wrapped `if`-chain |

The scrutinee is the value itself when `is_literal_match(ty)` — `Base(Int|String|Bool)` or a
`Refined` over one — and `.tag` otherwise.

**R5.3 — Arms are independent, not chained.** A failing guard must fall through to the next arm's
test, which is why the shipped if-chain emits separate `if` blocks rather than `else if` and relies on
each body's tail `return` to short-circuit. Keep this; record it in the IR so the printer does not
have to re-derive it.

**R5.4 — A guard is never folded into the pattern test.** The order within an arm is: structural
tests, then the refinement predicate, then bindings, then the guard. The guard must be able to read
the bindings; a refinement predicate must not, because it is a *test*, not a guard.
*Rationale:* both facts are load-bearing in the shipped emitter and neither is stated anywhere a
future author would look. Note the consequence the shipped code gets right: a guard's own lowering may
hoist statements, and they land *inside* the pattern-test block, not before it.

**R5.5 — Bindings are IR locals with a declared binding mode. The or-pattern mode is part of the IR,
not an emission-time discovery.**
*Rationale:* an or-pattern cannot use `const`, "because different alternatives can bind the same name
at different structural paths". The shipped emitter discovers this and switches to `let a, b;` plus an
`if/else if/else` dispatch over the alternatives. That is the right lowering; it should be a recorded
property of the pattern, because the same fact is needed by the checker (which enforces that every
alternative binds the same names at the same types *including refinement*) and by the LSP.

### 5.2 Exhaustiveness

**R5.6 — Exhaustiveness is a checker verdict carried in the IR. The emitter never decides it, and
never emits a runtime guard the IR did not ask for.**
*Rationale:* today the trailing `throw new Error("non-exhaustive match")` is emitted at three sites
with two different policies. On the switch path it is **unconditional** — emitted even for a
provably exhaustive match, because the switch path does no catch-all analysis at all. On the if-chain
path it is **conditional**, gated on `!has_catchall` where a catch-all is an unguarded irrefutable
arm. So the same source program gets a reachable-dead throw or no throw depending on whether some
*other* arm happened to carry a guard. With `Exhaustive` in the IR there is one policy: emit the
throw iff `Partial`.

The checker's side is sound and should be kept as-is: guarded arms are excluded from coverage,
`saw_wildcard` is set only by an unguarded irrefutable arm, and `missing_patterns` is deliberately
bounded structural coverage — or-patterns flattened one level, a single-field payload recursed into,
a multi-field refutable payload conservatively reported as `Name(…)`. Three diagnostics share the code
`bynk.types.non_exhaustive_match`, and the literal-Int/String case is unconditional because "a literal
match over an unbounded type can never be complete".

**R5.7 — A refined arm never satisfies exhaustiveness.** `Refined` is refutable by construction, the
same as a guard.

### 5.3 `is` and narrowing

**R5.8 — `is` and a match arm are one mechanism at two checkpoints.** Both produce a pattern test; the
difference is only that `is` yields a boolean in expression position and binds at depth 1, while an
arm opens a scope and binds at full depth.
*Rationale:* already nearly true in the shipped compiler — `refined_check_as_bool` serves
`is RefinedType`, `_ where P` arms and `T.of`'s runtime checks, and ADR 0253's consequences record
that the refined-pattern increment reused `check_refinement` and `refined_check_as_bool` *verbatim*.
Part 12's E3 pins the predicate side; R5.8 pins the pattern side.

**R5.9 — Narrowing is a scope operation over `(LocalId, TyId)` pairs computed from the condition, and
the mapping is recorded in the IR rather than recomputed by the emitter.**
*Rationale:* the shipped checker computes `Vec<(String, Ty)>` via `collect_is_bindings`, memoised in
`is_binding_cache: HashMap<Span, Vec<(String, Ty)>>` because "without it a left-nested `&&` chain is
O(N²)" — a memo table keyed by span, which is R2.4's collision surface serving *stale narrowings*.
It applies at exactly two sites, `&&`/`implies` right operands and an `if` then-block, both *after*
the condition is checked because "collecting earlier would cache empties". The emitter then
independently rebuilds the same bindings through `gather_is_bindings_for_emit`. Two computations of
one fact, one of them cached on a colliding key.

**R5.10 — The refined-`is` receiver is a bound temporary, always.**
*Rationale:* the shipped `is_receiver_ref_forced` lifts even a bare identifier to a temp, because the
narrowing binding `const n = <temp> as Quantity` would otherwise hit a temporal dead zone on its own
name. This is a real constraint of the emission strategy and belongs in the IR as an explicit
`Let`, not as a special-case in a receiver helper — which is also the review's Class A finding
(`simple_expr` classifies `ExprKind::Is` as hoisting nothing while `is_receiver_ref_forced`
unconditionally pushes a statement).

> **Language-surface note.** Narrowing flows into the **then**-branch only; there is no else-branch
> complementary narrowing, and the type-system spec records that as explicitly out of v1. Under
> R4.4's canonical refinements and E3's entailment relation, negative narrowing becomes cheap for the
> interval and length domains (the complement of an interval is at most two intervals). Recorded
> because the prerequisite is here, not because this document proposes the feature.

### 5.4 Built-in sums

**R5.11 — `Result`, `Option`, `HttpResult` and `QueueResult` are ordinary sums with declared field
names. No emission site hard-codes a payload field name.**
*Rationale:* the checker already gets this right — `variants_of` synthesises `VariantInfo`s so
`Result` matches as `Ok`/`Err` with payload field `value`/`error` and exhaustiveness works like any
user sum. The emitter does not: `positional_field_name` hard-codes the runtime layout *before*
consulting any declaration —

```rust
match (variant, idx) {
    ("Ok", 0) | ("Some", 0) => return "value".to_string(),
    ("Err", 0)              => return "error".to_string(),
    _ => {}
}
```

— followed by `Ty::ActorSum` → `"identity"`, then the user-sum lookup, then a single-field fallback
to `"value"`. That fallback is what makes a nested `is` test land on an untyped `{ tag, value }`
scrutinee. One declaration, read by both sides, removes the table and the fallback together.
---

## Part 6 — The intermediate representation

This part is the reason the document exists.

### 6.1 What the IR is for

Not optimisation. Bynk emits TypeScript and V8 does the optimising; a CFG or SSA form would be fixed
cost against no benefit. The IR exists for five properties:

1. **All names resolved.** The emitter never looks up a string.
2. **All types attached by construction.** `IrExpr` carries a `TyId`; there is no lookup and
   therefore no miss.
3. **All sugar desugared exactly once**, in one place, with one table.
4. **The statement/expression distinction is explicit**, so hoisting is not an operation that happens
   during lowering — it is a shape the IR already has.
5. **Every dispatch decision is a resolved enum**, so the emitter matches rather than re-deriving.

Property 4 is load-bearing and deserves stating plainly. The entire hoisting defect class in the
shipped compiler exists *only* because lowering targets TypeScript expressions directly, and JS
grammar forbids statements in expression position. The current signature is
`lower_expr(e, stmts: &mut Vec<String>, cx) -> String` — twenty-nine signatures in `lower.rs` (32
workspace-wide) carry that sink — which means the IR is really the pair `(String, &mut Vec<String>)`
while the crate documentation states there is no IR at all.

### 6.2 The core node set

```rust
struct IrExpr { kind: IrExprKind, ty: TyId, span: Span }

enum IrExprKind {
    Const(ConstVal),                                   // Int Float Str Bool Unit Bytes
    Local(LocalId),
    Global(DefId),

    Record   { def: DefId, fields: Vec<(FieldId, IrExpr)> },   // always complete, always ordered
    Variant  { def: DefId, tag: VariantId, payload: Vec<IrExpr> },
    Field    { base: Box<IrExpr>, field: FieldId },
    List     { elems: Vec<IrExpr> },

    Block    { stmts: Vec<IrStmt>, tail: Box<IrExpr> },
    If       { cond: Box<IrExpr>, then_: Box<IrExpr>, else_: Box<IrExpr> },
    Match    { scrutinee: Box<IrExpr>, arms: Vec<IrArm>, exhaustive: Exhaustive, form: MatchForm },
    And      { lhs: Box<IrExpr>, rhs: Box<IrExpr> },
    Or       { lhs: Box<IrExpr>, rhs: Box<IrExpr> },
    Not      { operand: Box<IrExpr> },
    Return   { value: Box<IrExpr> },

    Call     { callee: Callee, targs: Vec<TyId>, args: Vec<IrExpr> },
    Lambda   { params: Vec<LocalId>, body: Box<IrExpr>, captures: Vec<LocalId> },

    Await    { effect: Box<IrExpr> },                  // `<-`
    Send     { effect: Box<IrExpr> },                  // `~>`, fire-and-forget, type is Unit
    Pure     { value: Box<IrExpr> },                   // Effect introduction
}

enum IrStmt {
    Let    { local: LocalId, value: IrExpr },
    Expr   { value: IrExpr },                          // value type must be Unit
}
```

**R6.1 — Every `IrExpr` carries its type. The constructor requires it; there is no side table and no
fallible lookup from the emitter to the checker.**
*Rationale:* R4.9, applied at the phase boundary that actually broke.

**R6.2 — A value-position construct that requires statements is represented as
`Block { stmts, tail }`. There is no mechanism by which a lowering function can hand statements to
its caller.**
*Rationale:* the whole of the review's Class A. Two ternary sites build a throwaway vector,
`debug_assert!` it is empty and drop it, guarded by a `simple_expr` classifier that misclassifies
`ExprKind::Is` — "in a debug build the compiler panics on valid user code; in release it emits a
reference to a variable that was never declared." `lower_and_with_is` splices statements into a
string. And `lower_match_as_iife` wrapped hoisted discriminant statements in a fresh synchronous
arrow, so `let x = match risky()? { … }` early-returned into the wrapper: "a miscompile of ordinary
code with no diagnostic and no assertion."

**R6.3 — Short-circuit is structural: in `And`/`Or`, the right operand's statements are inside the
right operand. No phase may lift them.**
*Rationale:* `lower_bin_op`'s general path lowers both operands into the same caller vector, so a
hoist from the right operand is emitted unconditionally before the operator (#3). The type-system
spec is explicit that this is not an optimisation: "This is both a performance property and a safety
property. An invariant like `state is Held(_) implies <expensive query>` only runs the query when
state is in the `Held` variant … **Developers can rely on this**."

**R6.4 — Effectfulness is a property of the IR node, not of a scan over emitted text.**
`Await`, `Send`, and calls to effectful callees are the only sources; a printer question like "is this
arrow async?" is answered by walking the IR.
*Rationale:* `maybe_async_iife` decides async-ness with `if !iife.contains("await ")`, over-matches on
five iterator terminals, and is not called at all by the `if` IIFE path — "the same bug class was
found and patched for `match` and left unpatched for `if`, which is the giveaway that the fix was a
local text patch rather than a property of the lowering" (#2). Note the shipped compiler *also*
carries the right rule elsewhere: `is_effectful_return(r) = matches!(r, TypeRef::Effect(_, _))` is
one predicate applied identically at free functions, provider ops, service handlers, `__makeSurface`
methods, agent handlers and WebSocket DO methods. Two rules for one question, one of them a substring
search. And the project has already made and won this argument once, in the same file:
`runtime_use.rs` exists because deciding things by scanning generated text "over-matches … worse, it
under-matches", and `RuntimeUse` makes import decisions travel in a type.

**R6.5 — The IR contains no names. Every reference is a `DefId`, `LocalId`, `FieldId`, `VariantId` or
`UnitId`.**

*Rationale:* the shipped compiler resolves by string at emission time in several places, and each is a
distinct defect. The clearest is the agent write-detection that decides whether a handler commits at
all. `block_writes_state`'s `mutating_op` fires only on a bare `ExprKind::Ident` receiver whose *name*
is in a store-kind set:

```rust
if let ExprKind::MethodCall { receiver, method, .. } = &e.kind
    && let ExprKind::Ident(id) = &receiver.kind
{
    if (maps.contains(&id.name) || caches.contains(&id.name))
        && matches!(method.name.as_str(), "put" | "remove" | "update" | "upsert")
    { return true; }
    …
}
```

It never resolves the identifier. A local shadowing a store field's name is a false positive (safe,
over-approximating). A mutation reached through any non-`Ident` receiver is **missed** — and if it is
missed, `writes_state` is false, `await this.commitState(__state)` is never emitted, and a durable
write is silently lost. That is exactly the review's #54: `items.forEach((x) => orders.put(x.id, x))`
returned false because the walker had no `Lambda` arm. The shipped fix replaced the hand-rolled arm
list with `expr_children`, which closes the class — but leaves the name-matching intact.

Under R6.5 the defect is unrepresentable: a store write is
`IrStmt::Expr(Await(Call(Callee::Store { field, op: StoreOp::Put }, …)))`, and detection is a walk for
`Callee::Store` with a mutating op. There is no name to match, no receiver shape to enumerate, and no
lambda to forget. **This is the strongest single argument in the document for the rule**, because the
same rule that makes the emitter simpler also makes a data-loss bug impossible.

**R6.6 — JavaScript's grammar restriction is accommodated in phase 7 by rewriting the *enclosing
statement*, which the IR tree makes reachable. A function wrapper may be introduced only for a subtree
for which `contains_control_transfer(expr)` is false, and that predicate is computed over the IR.**
*Rationale:* the rule the shipped compiler lacks, and its absence is the miscompile. The grammar
problem is real, but the accommodation is a tree-to-tree rewrite with a return value, not a side
channel, and the safety condition is a property of the IR rather than a substring search.

### 6.3 Worked example: the miscompile this shape forbids

```
-- Bynk (Foundations)
let view = match lookup(id)? {
  Some(it) => render(it)
  None     => empty()
}
```

**Today.** `lower_expr` lowers the discriminant `lookup(id)?` and pushes its `Err`-guard statements
into the caller's `stmts` vector. `lower_match_as_iife` then wraps the arms in a fresh synchronous
arrow and the `?`'s early return exits the wrapper rather than the handler. The match expression
silently evaluates to the `Err` object. No diagnostic, no assertion.

**Under this reference.** Phase 6 desugars `Question` (§6.4) into a `Match` whose `Err` arm is a
`Return`:

```
Block {
  stmts: [ Let(view, Match {
             scrutinee: Match {                     // from `?`
               scrutinee: Call(Fn(lookup), [id]),
               arms: [ Ok(v)  => Local(v),
                       Err(e) => Return(Variant(Err, [convert(e)])) ],
               exhaustive: Total },
             arms: [ Some(it) => Call(Fn(render), [it]),
                     None     => Call(Fn(empty), []) ],
             exhaustive: Total }) ],
  tail: …
}
```

Phase 7 sees a `Match` in the initialiser position of a `Let`. `contains_control_transfer` is **true**
— there is a `Return` in the subtree — so R6.6 forbids the arrow wrapper, and the only available
rewrite is on the enclosing statement: a `let view;` declaration followed by a `switch` that assigns.
The `return` targets the handler, correctly.

The property worth noticing is not that the right code is produced. It is that **the wrong code is
not expressible**: there is no vector for the discriminant's statements to escape into, and the
wrapper is gated on a predicate over the tree rather than on `iife.contains("await ")`.

### 6.4 The desugaring table

**R6.7 — Desugaring happens exactly once, in phase 6, driven by an exhaustive match over `ExprKind`.
The table below is normative and total.**

| AST form | IR form |
|---|---|
| `DurationLit { millis }` | `Const(Int(millis))` at type `Duration` |
| `InterpStr [Chunk\|Hole]` | nested `Call(Kernel(StrConcat), …)` |
| `Paren(e)` | *elided* — parenthesisation is a token-level fact, recovered by the printer from precedence |
| `Ok(e)` / `Err(e)` / `Some(e)` / `None` | `Variant { def: Result\|Option, tag, payload }` |
| `Question(e)` | `Match { scrutinee: e, arms: [Ok(v) => v, Err(e) => Return(Err(convert(e)))] }` — the `embeds` conversion is applied here and nowhere else |
| `Is { value, pattern }` | `Match { scrutinee: value, arms: [pat => true, _ => false] }` at `Bool` |
| `Implies(a, b)` | `Or { lhs: Not(a), rhs: b }` — preserving R6.3 |
| `RecordSpread { base, overrides }` | `Block { stmts: [Let(tmp, base)], tail: Record { … } }` — complete field list, resolved |
| `Expect(e)` | `If { cond: Not(e), then_: Call(TestFail, …), else_: Unit }` |
| `Val { ty, args }` | `Call(Generator(ty), args)` |
| `Observation { … }` | `Call(ObservationPredicate, …)` over the recorded call log |
| `Trace { cap, op }` | `Call(TraceRead { cap, op }, [])` |
| `Wire(e)` | `Call(WireMarker, [e])` — system tier only |
| `EffectPure(e)` | `Pure(e)` |
| `Statement::Do(e)` | `IrStmt::Expr(Await(e))`, with a checker precondition that `e : Effect[()]` |
| `Statement::Send(e)` | `IrStmt::Expr(Send(e))` |
| `Statement::Assign` on a `Cell` | `IrStmt::Expr(Await(Call(Store { field, op: CellWrite }, [v])))` |
| `Cell` in value position | `Await(Call(Store { field, op: CellRead }, []))` |
| `Block` with `implicit_tail` | `Block { stmts, tail: Const(Unit) }` — the flag does not survive into the IR |

**R6.8 — Nothing in the table above may be desugared in an earlier phase.**
*Rationale:* the checker must see the user's construct to report on it, and the formatter must see it
to print it. Desugaring in the parser is how a language loses its own error messages.

**R6.9 — Sugar that carries a checker precondition records that precondition as a *checked* fact, not
as a comment on the desugaring.**
*Rationale:* `DoStmt.value` "MUST be `Effect[()]`" and `SendStmt` is "legal only when the reply is
`Effect[()]`" are doc-comment invariants today.

### 6.5 The callee taxonomy

```rust
enum Callee {
    Fn        (DefId),                                  // free function
    Value     (LocalId),                                // a function-typed local or parameter
    Ctor      { sum: DefId, tag: VariantId },           // sum variant construction
    Refine    (DefId),                                  // T.of  → Result[T, ValidationError]
    Unsafe    (DefId),                                  // T.unsafe — opaque, owning unit only (R4.6)
    Static    (DefId),                                  // user-declared static method
    Method    (DefId),                                  // user-declared instance method (UFCS)
    Kernel    { recv: KernelRecv, op: KernelOp },       // built-in method on a value
    Query     { op: QueryOp, role: QueryRole },         // builder | terminal
    Store     { field: FieldId, op: StoreOp },
    Capability{ cap: DefId, op: OpId },
    Agent     { agent: DefId, handler: DefId },
    Cross     { unit: UnitId, service: DefId },
    CrossCap  { unit: UnitId, cap: DefId, op: OpId },
    Intrinsic (Intrinsic),                              // StrConcat, TestFail, generators, …
}
```

**R6.10 — Call dispatch is a closed enum resolved in phase 5. The emitter matches on `Callee`; it
never re-derives the classification.**

*Rationale:* there is no `CallKind`, `Callee` or `CallTarget` enum anywhere in the shipped compiler.
Call position is classified by three AST nodes — `Call`, `ConstructorCall`, `MethodCall` — fed into
**ordered `if let … && …` ladders, twice**: once in `bynk-check`, once in `bynk-emit`, kept in sync by
hand. The emitter says so in its own doc comment:

> Lower an `ExprKind::MethodCall`. This is a dispatcher: a sequence of independent
> guard-and-`return` branches, tried in order (**the order is load-bearing** — earlier guards take
> precedence), falling through to the UFCS instance-call tail.

An ordering that is load-bearing, duplicated across a crate boundary, is P2's exact shape. Three
concrete consequences of the current arrangement, all of which R6.10 removes:

**The parse does not match the name.** `T.of(v)` and `T.unsafe(v)` written in source do *not* produce
`ExprKind::ConstructorCall`; they produce `MethodCall` with an `Ident` receiver, and the source
carries a comment saying so because someone got it wrong. Two AST nodes funnel into one checker
function on a rule no name expresses.
*(2026-10-09: `ExprKind::ConstructorCall` has since been deleted. The parser had not built it since
grammar v0.2, so a qualified call now has exactly one AST shape, `MethodCall`.)*

**Precedence is expressed as guard clauses that both crates must repeat.** The refined-inherits-kernel
rule (ADR 0168) is a *fallback after* the type's declared methods in the checker; the emitter mirrors
that precedence in a `match` guard:

```rust
Ty::Named { name, kind: NamedKind::Refined(base), .. }
    if !cx.commons().methods.get(name).is_some_and(|t| t.instance.contains_key(&method.name)) =>
```

That guard *is* the checker's precedence rule, restated in a pattern guard in another crate.

**Built-in variant resolution reads the surrounding type.** `HttpResult`'s 31 variants are matched by
bare name, so `NotFound` could be `HttpResult.NotFound` or a user sum's variant. The disambiguation
consults the expected type *and* the enclosing return type *and* whether the name is owned elsewhere —
a three-term condition duplicated wherever it is needed. In the IR it is a `Ctor { sum, tag }` and the
question does not arise.

**R6.11 — The kernel surface is one machine-readable table. Dispatch, the `method_not_found` message
and the LSP's member list are generated from it.**
*Rationale:* the kernel vocabulary exists in **six** hand-synced copies today, and
`kernel_methods.rs`'s own doc comment is candid that it is the non-authoritative one — the
authoritative typing lives "in `match` arms, authoritative for *typing* but not enumerable", with the
table as "the enumerable view the LSP reads". The drift test drives every listed method through the
real checker, so the table cannot list a phantom — but nothing catches a *missing* entry, and four
methods the checker accepts (`join`, `joinOn`, `leftJoin`, `groupBy`) are absent from `LIST_METHODS`,
so `.`-completion silently omits them. R11.6 makes the test bidirectional; R6.11 removes the second
copy.

**R6.12 — The builder/terminal split is a field on the callee, not a name list.**
*Rationale:* `is_query_op` is a *third* classifier, a bare name list used to decide whether a `store`
map receiver means "entry op" or "lift to `Query`". It works today only because of a naming
coincidence the source documents: "`count` is the query terminal; the map's entry-count op is `size`"
— one name cannot be both, so the list is unambiguous. That is a real constraint on future vocabulary
maintained by nothing.

### 6.6 Declarations

```rust
enum IrItem {
    Type     { def: DefId, shape: TypeShape },
    Fn       { def: DefId, params: Vec<LocalId>, ret: TyId, body: IrExpr, effectful: bool },
    Agent    { def: DefId, key: (LocalId, TyId), state: Vec<StoreFieldIr>,
               handlers: Vec<IrHandler>, invariants: Vec<IrPredicate>,
               transitions: Vec<IrPredicate> },
    Service  { def: DefId, protocol: ProtocolIr, handlers: Vec<IrHandler>, policy: PolicyIr },
    Actor    { def: DefId, scheme: AuthScheme, identity: Option<TyId>, claims: Option<IrExpr> },
    Capability { def: DefId, ops: Vec<OpSig> },
    Provider { def: DefId, cap: DefId, body: ProviderBody },   // Bynk ops | External(module)
}

enum TypeShape {
    Record { fields: Vec<(FieldId, TyId)> },
    Sum    { variants: Vec<(VariantId, Vec<(FieldId, TyId)>)>, embeds: Vec<EmbedIr> },
    Refined{ base: BaseType, refinement: RefinementId, opaque: bool },
}

struct StoreFieldIr { field: FieldId, kind: StoreKindIr, init: Option<IrExpr>, indexed: Vec<IndexIr> }
enum StoreKindIr { Cell(TyId), Map(TyId, TyId), Set(TyId), Cache(TyId, TyId, Duration), Log(TyId, Option<Duration>) }
```

**R6.13 — Declarations are IR nodes. Phase 7 consumes `IrItem`, never an AST declaration.**
*Rationale:* the shipped emitter reads `AgentDecl`, `ServiceDecl`, `ActorDecl` and `TypeDecl` directly
from the AST, which is why `EmitProjectCtx` has 28 fields (five with no readers anywhere), why the
per-file emit prologue rebuilds five unit-invariant tables per emitted file (#53), and why
`validate.rs` needs to reach back into the checker at all (R3.5). It is also the reason the review
could observe that "`emitter.rs` and `lower.rs` have almost no pure helpers — their units are
`fn(&mut String, &Ast, &mut LowerCtx)`", which is the same measurement as P1's test-density finding.

**R6.14 — The store-field state shape is derived in phase 6 and recorded in the IR, including index
side-tables.**
*Rationale:* the derivation is real and non-obvious — a `Cell[T]` becomes a plain field, a `Map[K,V]`
becomes `Record<string, V>`, a `Set[T]` becomes `Record<string, boolean>`, a `Cache` becomes
`Record<string, { v: V; exp: number }>`, a `Log` becomes `Array<{ t: number; v: T }>`, and an
`@indexed(by: g)` map adds a sibling `m__idx_g: Record<string, string[]>`. The shipped emitter also
has to sort index entries deterministically "because `HashMap` iteration would otherwise drift the
emitted bytes" — R9.4's ordering rule, rediscovered locally. All of it belongs upstream of the
printer.

### 6.7 Handlers and the atomic commit

```rust
struct IrHandler {
    kind:   HandlerKind,          // Call | Http{method,path} | Cron | Queue | Message | Open | Close | Event
    params: Vec<(LocalId, TyId)>,
    given:  Vec<DefId>,           // capabilities, resolved
    binder: Option<ActorBinder>,  // `by u: User`
    body:   IrExpr,
    commit: CommitShape,
    effectful: bool,
}

enum CommitShape {
    ReadOnly,
    FlushEvents,
    Transactional { invariants: Vec<IrPredicate>, transitions: Vec<IrPredicate> },
}
```

**R6.15 — `CommitShape` is computed in phase 6 from resolved store writes and recorded in the IR. The
emitter discharges it; it never decides it.**

*Rationale:* the Foundations layer's central runtime obligation is that "each handler is a
transaction: storage writes commit at the end, outbound messages release at the end, both or neither."
The shipped emitter discharges it in three shapes — a writing handler snapshots
(`const __state = { ...(await this.loadState()) }`), runs the body in an IIFE, then
`await this.commitState(__state)`; an emitting read-only handler loads without copying and flushes
events; a plain read-only handler loads and splices the body flat with **no commit at all**. Those are
the right three shapes. The problem is that the choice between them is made by
`block_writes_state`, a syntactic walk over the AST at emission time — see R6.5 for what that costs.
With `CommitShape` in the IR the choice is made once, from resolved data, by the phase that has the
type information.

**R6.16 — Handler invocation is origin-independent in the IR. No IR node may branch on caller kind.**
*Rationale:* the design notes state this as a hard structural rule — "An agent's handler is invoked
identically whether the caller is another agent, a service …, the runtime delivering a platform
event …, or a unit test harness. The agent never branches on origin." An IR that cannot express the
branch cannot violate it.
---

## Part 7 — The TypeScript tree and printer

### 7.1 Shape

```rust
enum TsStmt { Const, Let, Assign, Return, If, Switch, Throw, ForOf, ExprStmt, Block }
enum TsExpr { Ident, Member, Index, Call, New, Arrow { async: bool }, Object, Array,
              Binary, Unary, Cond, Await, TemplateLit, Lit, As, Spread }
enum TsType { Named, Union, Intersection, Object, Array, Fn, Literal, TypeParam, Readonly }
enum TsDecl { Import, Export, Function { async: bool }, Class, TypeAlias, Interface, ConstDecl }

struct TsNode { /* … */ span: Option<Span> }
```

**R7.1 — The tree contains no `enum`, no `namespace`, no decorator, no constructor parameter
property, and no `TsType::Any`.**

*Rationale:* the cleanest single win available anywhere in the design. ADR 0136 makes strip-only
erasability "a standing invariant, not a per-target branch" and forbids exactly those constructs —
and it is tested against six hand-written snippets while the ADR describes it as a whole-compiler
property. Crucially, "`tsc --strict --noEmit` passing does *not* imply erasability: `enum`,
`namespace` and constructor parameter properties all type-check cleanly and are exactly what
`strip_types` cannot erase." Omitting the nodes converts a standing invariant into a thing that
cannot be typed.

The `Any` omission does the parallel job for the `tsc --strict` backstop, which the current emitter
disarms in the hottest places: emitted test modules destructure every target name through `as any`,
over the output of the largest emitter file (#18), and `http_value_serialiser`'s `any` "swallows the
mismatch at `tsc --strict`, so the crate's stated backstop cannot catch it either" (#15).

**R7.2 — Emission is `Ir -> TsProgram`. It performs no string formatting, owns no buffer, and has no
notion of indentation.**

**R7.3 — Printing is `TsProgram -> Artefacts`. The printer owns the buffer, the indentation and the
offset arithmetic. It is the only code in the compiler that writes a character.**
*Rationale:* the CodeWriter proposal was shelved on a correct measurement — only 4 `writeln!` sites
used a computed indent while 48 hardcoded leading spaces as *content* — and the measurement then
rotted: 128 literal-indent sites against 4 in `emit.rs` by July, and inverted in `lower.rs`, a file
that did not exist when the note was written. A printer written before the first emitted line costs
nothing to adopt; adopted at 506 write sites it is a large diff nobody schedules.

**R7.4 — The source map is produced by the printer from `TsNode.span`. No phase before the printer
records an offset.**
*Rationale:* `record_span(out.len(), …)` has no idea which buffer `out` is, so IIFE-local offsets
corrupt the map (#4). ADR 0103 hedged for *missing* spans; what materialised was *wrong* spans, a
direct consequence of the one-way string pipe.

**R7.5 — Readable output is a printer policy with a name and a test, not a property of how carefully
strings were typed.**
*Rationale:* the README's readable-output claim is "a property, not a byte-for-byte promise" per the
1.0 definition, and the emitted TypeScript is explicitly not part of the frozen contract.

**R7.6 — Downstream consumers couple to nodes, never to emitted text.**
*Rationale:* `bynk-strip` rewrites the wrangler `main` key by literal substring match against a string
`wrangler.rs` happens to emit — "reformat that line's spacing and `--emit js` silently produces a
`wrangler.toml` pointing at a file that no longer exists."

**R7.7 — The hand-written runtime is TypeScript source in the repository, type-checked by the
project's own CI, and included by `include_str!`. No runtime code is a Rust string literal.**
*Rationale:* ~300 lines of harness TypeScript currently live as Rust string literals *beside* an
`include_str!` of `runtime.ts` (#17). One of these two is right.

### 7.2 Artefacts

```rust
struct Artefacts { docs: BTreeMap<PathBuf, Document> }

enum Document {
    Ts(TsProgram), Js(String, SourceMap), SourceMap(SourceMap),
    DebugSidecar(DebugMap),      // `.bynkdbg.json`, ADRs 0104/0105
    Toml(TomlDocument),          // wrangler.toml
    Json(JsonDocument),          // tsconfig.json
}
```

**R7.8 — `Artefacts` is a keyed set of *typed documents*. A compilation emits more than TypeScript,
and no emitted document is a `String` at the point it is constructed.**
*Rationale:* a real `out/` directory contains `.ts`, `.ts.map`, `.ts.bynkdbg.json`, `tsconfig.json`,
`wrangler.toml`, plus generated `index.ts` and `compose.ts`. A `TsProgram` cannot represent TOML, so
under a `(String, SourceMap)` return R7.6 was **unsatisfiable for `wrangler.toml`** — precisely the
defect R7.6 exists to prevent.

---

## Part 8 — Emission targets

**[source]** — specified against `bynk-emit/src/emitter/{emit,workers,workers_entry,wrangler,serialisation}.rs`
and the emitted output of `examples/todo`.

**R8.1 — Emission is a total function over `IrItem` with one arm per variant and no wildcard. Every
mapping below is that function's body.**

| `IrItem` | TypeScript |
|---|---|
| `Type{Refined}` | branded type alias + `const` namespace with `of` (+ `unsafe` iff opaque) |
| `Type{Record}` | `export interface` of `readonly` fields + `const` namespace for attached methods |
| `Type{Sum}` | tagged-union alias + `const` namespace of variant constructors |
| `Fn` | `export function` (`async` iff effectful) |
| `Agent` | state interface + registry + zero + rehydrator + DO class + factory |
| `Service` | `export const <Svc> = { … }`, one method per handler |
| `Actor` | no declaration; drives the boundary wrapper in `compose.ts` |
| `Capability` | `export interface` |
| `Provider{Bynk}` | `export class` with a `deps` constructor |
| `Provider{External}` | nothing — the binding module supplies it |

### 8.1 Types

```ts
export type Title = string & { readonly __brand: "todos.Title" };
export const Title = {
  of(value: string): Result<Title, ValidationError> {
    if (!(value.length > 0))    return Err({ field: "Title", message: "must be non-empty", value });
    if (!(value.length <= 200)) return Err({ field: "Title", message: "length must be at most 200", value });
    return Ok(value as Title);
  },
};
```

**R8.2 — The brand string is the type's recorded brand from R4.10, prefixed by its owning context.
The emitter reads it; it does not compute it.**
*Rationale:* today `brand_prefix` is computed at emission from `ctx.owning_context` "so two contexts'
locally-declared `Order` types have distinct brands at the TS level", while the checker mirrors the
same predicate through `ResolvedCommons::is_uses_commons_type` with a doc comment saying it mirrors
`emit_context_rebrands` *exactly*.

**R8.3 — `unsafe` is emitted iff the type is opaque; it is a field of `TypeShape`, not an emission
decision.**
*Rationale:* ADR 0182 — a refined alias has no bypass, an opaque type does. Today the two share one
code path (`RefinedShape { base, refinement, is_opaque }`) and the distinction is a boolean read at
the emission site, which is nearly right; R8.3 moves the boolean upstream so the checker's gate
(R4.6) and the emitter's output cannot disagree.

**R8.4 — Numeric refinements emit their base-type guard before their predicates.**
`Int` gets `Number.isInteger`, `Float` gets `Number.isFinite`.
*Rationale:* ADR 0040 — "validated `Float` values are finite — `.of` and the boundary codec agree".
This is a property of the base, so it belongs in the generated check, not in a predicate.

**R8.5 — A context re-branding a `uses`-imported commons type emits a type-level intersection and,
for refined or opaque bases, value-side forwarders.**

```ts
export type Cents = __CommonsCents & { readonly __ctxBrand?: "billing" };
export const Cents = {
  of(value: number): Result<Cents, ValidationError> { return __CommonsCents.of(value); },
};
```

*Rationale:* three rules the shipped emitter learned the hard way and that a reference must state.
**The brand is optional** (#1704): every unbranded commons value (a record literal, a nested field
typed by the commons, a commons function's result, a commons codec's result) then fits the
context's type with no cast, while a value carrying another context's brand still does not. A
required brand made each of those a `tsc` failure that `bynkc check` accepted, patched one call
site at a time.
**Only types are rebranded** — "a `uses`-imported function is a value and imports plainly". And the
generic parameters must be threaded (`Paginated<T> = __CommonsPaginated<T> & …`), because otherwise
every `Paginated<User>` reference errors "type is not generic" (#592). Without the value-side
forwarders a consumer's `Cents.fromInt(n)` passes `bynkc check` and fails `tsc` (#481) — a
checker/emitter disagreement of exactly the kind R3.5 and R4.10 exist to remove.

### 8.2 Agents

```ts
export interface TodosState { readonly lastSeq: number; readonly items: Record<string, Stored>; }
const __TodosRegistry = new StateRegistry();
function __zeroOfTodosState(): TodosState { return { lastSeq: 0, items: {} }; }
function __rehydrateTodosState(s: TodosState): void { /* per-field codec checks */ }

export class Todos {
  constructor(state: DurableObjectState) { this.state = state; }
  private async loadState(): Promise<TodosState> {
    const stored = await this.state.storage.get<TodosState>("state");
    if (stored === undefined) return __zeroOfTodosState();
    const __merged = { ...__zeroOfTodosState(), ...stored };
    __rehydrateTodosState(__merged);
    return __merged;
  }
  private async commitState(s: TodosState): Promise<void> { /* invariants, then put */ }
  …
}
export function __makeTodos(key: UserId, env?: { TODOS?: DurableObjectNamespace }): Todos {
  return makeAgent(__TodosRegistry, env?.TODOS, key, (state) => new Todos(state));
}
```

**R8.6 — The three commit shapes are discharged from `CommitShape` (R6.15), not decided at emission.**

| `CommitShape` | Emitted |
|---|---|
| `Transactional` | `const __state = { ...(await this.loadState()) };` → body in an IIFE → `await this.commitState(__state); return __result;` |
| `FlushEvents` | `const __state = await this.loadState();` → body in an IIFE → event flush, no commit |
| `ReadOnly` | `const __state = await this.loadState();` → body spliced flat, no commit |

**R8.7 — `loadState` merges the zero value under the stored value, so a field added in a later deploy
takes its default; rehydration validates the merged value and a failure is an internal fault, never a
caller-facing 400.**
*Rationale:* ADR 0124 D4. The shipped shape is `{ ...zero(), ...stored }` followed by the rehydrator,
which `throw`s `rehydrationViolation(agent, err)`. Worth pinning because it is the one place the
compiler decides what happens to *data that already exists* when the program changes, which is 1.0
gate 3.

**R8.8 — Invariants are evaluated in `commitState` before the write; transitions are evaluated against
the prior stored value and skipped on the genesis commit.**
*Rationale:* the design notes' contract — "a fault during execution aborts the entire transaction" —
plus the shipped detail that a transition needs `__prior !== undefined`. Ordering is diagnostics-only
(the type-system spec: "semantically all invariants are conjoined"), so the IR carries them as a set
and the emitter picks a stable order.

**R8.9 — A single factory helper keeps the call site target-agnostic.**
`makeAgent(registry, binding, key, ctor)` routes to the Workers DO stub when a binding is present and
to the in-process registry otherwise.
*Rationale:* correct in the shipped compiler and worth keeping — it is the one place the two targets
converge cleanly, and `__resetAgents()` over the registries is what gives tests isolation for free.

### 8.3 Services

**R8.10 — Handler key mangling is a pure, total function of `(HandlerKind, method, path)`, defined
once, with a stated inverse.**

```rust
"http_" + METHOD + ("_" + segment | "_Param_" + name)*     // non-alphanumeric → "_"
"cron_{service}_{i}"   "queue_{service}_{i}"   "call"   "message"   "open"   "close"   "event"
```

*Rationale:* `POST /todos/:id/complete` → `http_POST_todos_Param_id_complete`. The cron and queue
indices are the handler's position among that service's handlers *of that kind*, in declaration order
— and the shipped emitter recomputes that index identically at three sites (the handlers object, the
compose wrapper and the dispatcher). Three recomputations of one number is P2 at its smallest scale.
The inverse matters because the route table, the debug sidecar and the test harness all need to get
back from a key to a handler.

**R8.11 — The `deps` object type is derived once, from the handler's resolved `given` set, its actor
binder, and the target.**
Contents: one field per capability; then `surface` (bundle, if cross-context is used) or `env`
(Workers, if cross-context or agents are used); then conditionally `identity` / `who`, `__exec` (if
the handler uses `~>`), `__eventsDispatch` (if it emits).
*Rationale:* the shipped builder appends each conditional field by trimming the closing brace off a
string and concatenating — six times. It is also where the target divergence lives, and having it in
one function is why that divergence is auditable.

**R8.12 — `on call` is the internal door and is the only handler whose wrapper carries real parameter
types.**
*Rationale:* the shipped scoping is deliberate and the comment says why: "The other wrappers keep
`: any` because their parameters are not all codec-produced: an HTTP or WebSocket wrapper mixes a
deserialised `body` with route/query params the entry lifts from the URL as raw strings." That is a
sound reason, and under R7.1's ban on `TsType::Any` it becomes a *requirement to type the lifted
params*, which is the better answer.

### 8.4 Actors

**R8.13 — Verification is emitted at the boundary, never in a handler prologue. A handler body reads
`deps.identity` or `deps.who` and cannot observe how they were obtained.**

*Rationale:* this is R6.16 discharged. The shipped wrappers are chosen by a three-way exclusive rule
(sum → OIDC → bearer) and each verifies, mints, and forwards. Four properties are worth pinning
because each is a security-relevant decision that a reader would otherwise have to reverse-engineer:

- **A verified-but-unauthorised request is 403, not 401** — "scheme verified (401 above), so a failed
  claim predicate is 403".
- **An actor sum owns the whole boundary**: it reads the body once, tries each member's scheme in
  declared order, first-wins into a tagged `who`, and fails closed with 401 if none verifies.
- **A WebSocket upgrade is authenticated in the Worker before any request reaches the Durable
  Object**, with the token taken from the first `Sec-WebSocket-Protocol` element because "a browser
  cannot set `Authorization`", and the verified identity carried inward on a trusted internal header.
- **OIDC sources no secret** — issuer, audience and JWKS URL are public literals from the declaration.

### 8.5 Codecs

**R8.14 — The codec set is computed from the IR by one collector, parameterised by target.**

Two seeds, closed transitively through record fields and sum payloads, with refined and opaque types
as leaves:

- **boundary** — every service handler's parameter and return types, **on the Workers target only**,
  plus every agent store field's element types, **on every target**;
- **json** — the `T` of every `Json.decode[T]` and the argument type of every `Json.encode`.

*Rationale:* the target asymmetry is exactly right and non-obvious, and the shipped comment states it:
"Service handler types cross the *cross-Worker call* boundary, which only exists on the `workers`
target; on `bundle` calls are in-process… The agent **rehydration** boundary (ADR 0124), in contrast,
exists on both targets, so agent store-field types are always collected." A reference that omitted
that would produce a compiler that either over-emits on bundle or under-emits on Workers.

**R8.15 — There is one codec dispatch. A second, parallel serialiser is a defect by definition.**
*Rationale:* the shipped `workers_entry.rs` carries a comment recording that this bug class was found
and fixed **twice** — "Both of these used to be a *parallel* dispatch that shadowed
`serialisation.rs`'s — and drifted from it" — and a third survivor, `http_value_serialiser`, is still
there, where root `Float` loses its non-finite guard and root `Bytes` ships raw (#15).

### 8.6 The Workers target

**R8.16 — The compose root is generated from the `ProjectGraph`'s edges, and the per-consumer surface
is built wherever a callee binds a `Caller`.**
*Rationale:* ADR 0226. A consumer edge builds `B.__makeSurface(BDeps, "<consumer>")` so the callee reads
the consuming context's qualified name, matching Workers' `X-Bynk-Caller`; the shared instance is kept
only where no consumer needs a distinct caller. One shared predicate decides which providers take the
extra argument "so the two seams cannot disagree".

**R8.17 — The HTTP route table is sorted by (parameter count, method, path), and the ordering is part
of the specification.**

```rust
param_count(a).cmp(param_count(b)).then(a.method.cmp(b.method)).then(a.path.cmp(b.path))
```

*Rationale:* "a path with fewer parameter segments (more literals) wins". This is user-observable
routing behaviour derived from an internal sort; it belongs in a document, not only in a comparator.

**R8.18 — The internal door validates the contract hash before reading the body, and the caller
identity after it.**
A mismatch is **409**, not 400 — "the payload is not malformed, and the caller cannot fix it by
sending different bytes; the two deployments conflict". An absent header fails closed. A missing
caller on a `Caller`-binding handler is 401.
*Rationale:* the ordering is the interesting part and the shipped comment states the reason: "if the
contracts disagree, nothing about the request is trustworthy, including the identity."

**R8.19 — Every user-supplied string reaching a generated config file goes through that format's
escaper.**
*Rationale:* a queue name decoding to `"` plus a newline would otherwise inject TOML config keys.
`escape_toml_basic_string` exists for this; the reference states it as a rule because the next
generated format will need its own.

**R8.20 — Deploy-time placeholders are typed, not textual.**
The KV namespace id is emitted as a placeholder the driver substitutes immediately before a remote
Wrangler command.
*Rationale:* under R7.8 a `TomlDocument` can carry a `Placeholder` value; today it is the literal
string `"<KV_NAMESPACE_ID_PLACEHOLDER>"` matched by the driver, which is R7.6's failure mode waiting
for a reformat.

**R8.21 — `async` follows the effect type, everywhere, by one predicate.**
`is_effectful(ret)` decides the keyword at free functions, provider ops, service handlers, surface
methods, agent handlers and DO methods alike. Boundary shims — compose wrappers, the DO `fetch`, the
entry's `fetch`/`scheduled`/`queue`, `loadState`, `commitState` — are unconditionally `async` because
they are not lowered bodies.
*Rationale:* correct in the shipped compiler, and the counter-example is R6.4's substring scan. Two
rules for one question; keep the good one.

**R8.22 — A generated JSON response never stringifies `undefined`.**
*Rationale:* a void agent method resolves to `undefined`, and `JSON.stringify(undefined)` is the
*string* `undefined` — not JSON. The shipped fix is `result ?? null`. Recorded because it is the kind
of defect that reaches production and is invisible in a golden test whose fixture has a return value.
---

## Part 9 — Diagnostics

```rust
struct Diagnostic {
    code:        Code,               // registry key; severity derives from it
    primary:     Span,
    message:     String,             // rendered from the registry template
    labels:      Vec<Label>,
    notes:       Vec<String>,
    suggestions: Vec<Suggestion>,    // structured, per ADR 0054
}
```

**R9.1 — Severity is derived from the code registry. There is no severity computed at a use site.**
*Rationale:* `Severity::for_error` classifies six categories as warnings while `report_with_config`
hardcodes `ReportKind::Error` — the only `ReportKind` in the workspace — so `bynk check` prints
`Error: …` and exits 0 (#44). Separately, severity "is a hardcoded six-arm match with no link to the
registry" and the public reference has no severity column (#50).

**R9.2 — Every renderer is a total function over `Diagnostic`. A renderer may not silently drop a
channel.**
*Rationale:* "Every renderer except ariadne and the LSP discards `notes` and `labels` — 339 notes, 87
labels invisible" (#47); ADR 0054's structured suggestions never reach the CLI and the fix text is
hand-duplicated as a note (#49); ADR 0100's separation left "four independent copies with four
different fallbacks" (#48).

**R9.3 — Message text is generated from the registry. A code has one template.**
*Rationale:* one code currently carries 42 hand-written templates that the registry documents as one
sentence (#45).

**R9.4 — Diagnostic ordering is deterministic. Any map on a user-visible output path is ordered.**
*Rationale:* fourteen loops iterate `HashMap` keyed on unit name and push straight into the sink;
`into_all` concatenates without sorting (#11, #52). Note the correct fix and the explicitly wrong one:
use `BTreeMap` for the producing maps; do **not** sort in `into_all`, because the within-unit sequence
is load-bearing.

**R9.5 — There is no error enum.**
*Rationale:* explicitly rejected in verification — 434 variants in one enum, and `miette`'s renderer
would displace an ariadne setup already tuned for byte indexing. The registry-as-data instinct was
right; it needed severity and templates moved *into* it, not replacing.

**R9.6 — A warning carries a location.**
*Rationale:* project warnings currently carry no line or column "because `ProjectOutput` keeps no
snapshots" (#44).

---

## Part 10 — Crate graph

```
bynk-span      FileId · Span · Sources · Diagnostic · code registry          [leaf]
bynk-lex       tokens + trivia
bynk-parse     AST arena · parser · recovery
bynk-project   manifest · discovery · unit graph · contract hashes · schema registry
bynk-resolve   names → DefId
bynk-check     ALL semantics, decl-level and context-level  →  TypedProgram · certify
bynk-ir        the checker's output; the only thing lower consumes
bynk-lower     CheckedProgram → Ir
bynk-ts        TS tree · printer · source map
bynk-emit      Ir → TsProgram                       (depends on bynk-ts, NOT on bynk-check)
bynk-strip     TS → JS (oxc)
bynk-render    Diagnostic → ariadne / short / json   (depends on bynk-span ONLY)
bynk-fmt       Tokens + Ast → text
bynk-ide       queries over parse/resolve/check      (may NOT depend on bynk-emit)
bynk-driver    command bodies
bynkc / bynk / bynk-lsp / bynk-wasm                  thin binaries

bynk-grammar   the published declarative grammar (R2.13)   — no compiler dependency
xtask          repo automation; owns the CI gates named in Part 11
```

**R10.1 — A crate is named for what it produces, and if its input type and output type cannot be
stated in one line, it is not a crate yet.**
*Rationale:* "build orchestration + TS emission" fails this test, and failing it was worth knowing in
June rather than in July. The July review's second theme: "the crate names do not describe the
layering … a contributor reading the crate docs will be misled about where to make a change."

**R10.2 — `bynk-ide` may not depend on `bynk-emit`, enforced by the manifest and by a CI check on the
dependency graph.**
*Rationale:* `bynk-ide/src/lib.rs` states that `bynk-lsp` "deliberately does not depend on
`bynk-emit`" — "**both are true only at the manifest level**", and the LSP links ~24,850 lines of pure
emission code, about 63% of the crate, that it never executes. Every edit to `lower.rs` rebuilds and
relinks six crates (#39).

**R10.3 — A crate is carved when a dependency arrives that only one consumer needs — prospectively,
at the moment the dependency appears.**
*Rationale:* `bynk-strip` is the control case for this entire document. It is the one crate carved
prospectively, to keep oxc out of `bynk-emit` and therefore out of the LSP — and it is the one crate
whose name does not lie.

**R10.4 — No crate exports a facade over another crate's internals. The published surface of each
crate is enumerated and reviewed.**
*Rationale:* #41 and #42 — "of thirty-eight world-reachable items in `bynk_emit::emitter`, exactly
five have an external user; the rest are `pub` only because `pub mod` promotes sibling-visibility to
world-visibility."

**R10.5 — Command bodies live in `bynk-driver`. A binary is argument parsing and dispatch.**
*Rationale:* the entire `bynkc test` runner lives in `bynkc/src/main.rs`, unreachable from any
library. The cost is a *detection gap*: `tool_exists` is PATH-only while `bynk doctor` prefers
`<root>/node_modules/.bin`, so for a project with `typescript` as a devDependency and no global
`tsc`, doctor reports green and the runner type-checks with a different `tsc` than the project pins
(#20).

**R10.6 — The CLI argument set has one definition, shared by every caller including any subprocess
path.**
*Rationale:* spelled four times today, "one copy across a process and version boundary" (#24, #72).

**R10.7 — Diagnostic rendering lives in one crate whose only dependency is `bynk-span`.**
*Rationale:* a `Diagnostic` type with no renderer home is how you get four flattening copies (#48).

---

## Part 11 — Testing architecture

**R11.1 — P1 is the testing strategy. Any phase that cannot be tested with a literal input value and
an asserted output value is mis-designed, and the fix is the signature, not the test.**

**R11.2 — Fixtures support five assertion granularities from the first commit.**

| Granularity | Form | Catches |
|---|---|---|
| Byte-identical | `expected.ts` | emission regressions |
| Contains / absent | `expected_contains.txt`, `expected_absent.txt` | targeted properties without whole-file churn |
| Diagnostics with location | `code<TAB>path:line:col` | attribution |
| IR snapshot | `expected.ir` | phase-6 refactors without a whole-file diff |
| Type snapshot | `expected.tys` | phase-5 refactors, and R4.9's totality |

*Rationale:* the shipped format offers exactly two — whole-file byte identity and category strings —
"and nothing between". Two escaped defects trace directly to the diagnostic side, and both repairs
were bespoke Rust tests with hand-rolled harnesses rather than a widening of the format (#58). ADR
0198's own words on the cost: "**No fixture asserts an attributed path.** … the identity could be
wrong for every split project and every test would still pass. It did, and they did." And: "'the gate
is green' is the weakest possible evidence here."

**R11.3 — Erasability is verified by running the stripper over every fixture's emitted output.**
*Rationale:* #60, plus R7.1's caveat — `tsc --strict` cannot see erasability violations.

**R11.4 — Fuzz targets are owned, listed in the CI inventory, and reach the back end.**
*Rationale:* three source comments cite fuzz-found bugs as standing guards, "the word 'fuzz' appears
in no design document, and the roadmap's otherwise exhaustive CI inventory does not list them"; the
existing target stops before the back end (#59).

**R11.5 — Rust-side coverage is instrumented and reported.**
*Rationale:* "the blind spot a byte-identical golden gate cannot see" (#61).

**R11.6 — A registry has a bidirectional conformance test.**
*Rationale:* the kernel vocabulary exists in six copies, `LIST_METHODS` is missing four methods the
checker accepts, and "the drift test is documented as one-directional" (#34, #68). R6.11 removes the
second copy; R11.6 is the belt for whatever registries remain.

**R11.7 — Grammar conformance is two totality assertions over corpora that already exist.**

1. **Grammar not narrower than the compiler.** Every file the compiler's parser accepts — all of
   `examples/`, the vendored first-party `.bynk` sources, every positive fixture — parses under the
   published grammar with **zero ERROR nodes**.
2. **Grammar not wider than the compiler.** Every negative fixture that is a *parse* error is an ERROR
   node under the published grammar too.

*Rationale:* the shipped project has a conformance gate (ADR 0213) and two documented divergences, and
**the gate caught neither**. ADR 0189's — `actor Admin = User where hasClaim("admin")` producing an
**ERROR node in an editor** on valid, compiling code — was found by a language design review. ADR
0253 D4's — the `where` check leaking into every recursive call so `Ok(_ where P)` "silently parsed
and compiled" — was "caught in review". Neither assertion above needs a tree comparison; assertion 1
alone would have failed on ADR 0189's bug the day it landed, on files already in the repository.

**R11.8 — Every unbounded recursion, chain and user-supplied pattern carries a stated bound with a
fixture.**
*Rationale:* the shipped compiler has closed this four times — ADRs 0215, 0217, 0223, 0229. It is not
hypothetical: the compiler ships in a browser, the playground accepts arbitrary source, a panic there
is a user-visible failure, and a `Matches` predicate becomes a runtime regex in *emitted* code. Note
that R12.3's observation about regex containment being PSPACE-complete concerns the *compile-time*
comparison; R11.8 is about the *runtime* one.

---

## Part 12 — Extension points

Each entry names the type that grows, and what must not change when it does.

**E1 — The capability seam.** `consumes` declares, `given` requires, `provides` supplies (ADR 0154).
Everything above the Foundations layer enters here. The design notes are explicit that no new lowering
is needed: "**No special lowering** for compensation or idempotency — those are ordinary capability
calls", and "there is no special compiler support and no syntactic distinction between using a
first-party capability and using one written by an application team or a third party."
*Grows:* the capability registry and the resolver's provision table. *Must not change:* `IrExprKind`,
the printer, the phase table.
*Constraint carried forward:* capability requirement propagation is transitive and **not inferred** —
"if `fn foo() given Clock { … }` and `fn bar() { foo() }`, then `bar` either declares `given Clock` of
its own or fails to compile."
*Refined by Part 14:* E1 as stated is true of the capability *mechanism* and false of three shipped
capabilities. Part 14 gives the test that separates them, and adds E7.

**E2 — Service protocols.** A closed nominal set (ADR 0079). *Grows:* one `ProtocolIr` variant per
**real trigger**, and one arm in R8.1's emission function. *Must not change:* the rule that every
variant names something that actually invokes the program. The course-feasibility work established
the precedent — a proposed `from playground` variant was rejected because it "names a person clicking
a button, i.e. a dev affordance wearing architecture's costume".

**E3 — Refinement predicates: normalised domains with per-domain entailment.**

A refinement predicate is a decision procedure `P : τ → Bool`. Every surface form in the language is
that one function, applied to a *value*, at a different *checkpoint*, licensing a different amount:

| Form | Checkpoint | Licenses |
|---|---|---|
| `type T = τ where P` | `T.of`, boundary deserialisation, storage rehydration, refined-field write | a standing fact: ∀ x:T. P(x) |
| field `f: τ where P` | construction, boundary, rehydration | the same, field-scoped |
| `v is T` | here, now | narrowing — the branch sees `v : T` |
| `match v { _ where P }` | here, now | nothing (ADR 0253 D2) |

The shipped compiler already agrees these are one thing, more firmly than the design corpus does. ADR
0253 D1: `_ where P` "reuses the exact `Refinement`/`RefinementPred`/`PredKind` grammar and AST a
`type X = Base where P` declaration already uses", and its consequences reuse `check_refinement` and
reuse `refined_check_as_bool` — "previously `lower_is`'s only caller" — **verbatim**. ADR 0007 reuses
`.of`'s predicate logic as a boolean expression. Three surface forms, one lowering.

The residual difference between rows 3 and 4 is **nominal versus anonymous**, not two concepts:
`v is T` can narrow because `T` has a name for the branch to hold; `_ where P` cannot because `P` has
none.

**R12.1 — There is one predicate evaluator. `T.of`, `v is T` and `match v { _ where P }` are three
checkpoints of one function, never three implementations.**
*Rationale:* already nearly true, and worth pinning because the same fact is written twice today — the
runtime check exists inline in `emit.rs` and again in `serialisation.rs`, and the two already differ
in substance, one anchoring its `Matches` regex with `^(?:…)$` and the other not, while both emit the
identical message (#70).

**R12.2 — Predicates are stored in canonical normal form and grouped into domains. A new predicate is
either sugar for an element of an existing domain, or it declares a new domain.**

| Domain | Members | Canonical form |
|---|---|---|
| Interval | `InRange`, `InRangeF`, `NonNegative` ≡ `InRange(0, ∞)`, `Positive` ≡ `InRange(1, ∞)` | `[lo, hi]` over the base's ordering |
| Length | `MinLength`, `MaxLength`, `Length`, `NonEmpty` ≡ `MinLength(1)` | a length interval `[lo, hi]` |
| Language | `Matches(regex)` | the canonicalised pattern; **opaque** |

*Rationale:* three of the eight shipped predicates are already sugar for two others — the schema
mapping says so out loud (`NonEmpty` → `minLength: 1` for strings, `minItems: 1` for arrays).
Normalising shrinks the vocabulary, collapses the drift surface, and turns R12.3 from a
theorem-proving problem into interval arithmetic. This is also what R4.4 stores.

**R12.3 — Each domain supplies an entailment relation `P ⊨ Q`. A domain that cannot supply a decidable
one declares itself opaque, where `P ⊨ Q` holds iff `P` and `Q` are canonically identical.**
*Rationale:* interval and length entailment are containment — total and cheap. Regex containment is
decidable but PSPACE-complete, so the Language domain is opaque and fail-closed. Uninhabitance falls
out free: `InRange(5, 3)` and `MinLength(3) && MaxLength(2)` are both ⊥, which ADR 0158 deferred
separately as "refinement inhabitance".

**Why `⊨` is worth its cost: four items deferred independently are the same missing judgement.**

| Deferred item | Where | Is really |
|---|---|---|
| Refinement propagation, "the largest open question" | type-system spec §2.5.4 | `⊨` restricted to operations — the spec's own sketch is "a table mapping (predicate, operation) → preservation rule" |
| Statically-provable invariants (`bynk.invariants.statically_provable`) | type-system spec §2.10.2 | `field_refinement ⊨ invariant_predicate`; "the detection algorithm is named as remaining work" |
| Refined-arm subsumption, "a static-analysis rabbit hole" | ADR 0253 S2 | `P ⊨ Q` between two arms |
| Subsumption across refined types | nominality today | `P ⊨ Q` at an argument position |

**R12.4 — A predicate vocabulary is closed exactly when the compiler must *reason about* predicates
rather than merely *evaluate* them.**
*Rationale:* the criterion behind ADR 0189's three tiers, stated as a rule rather than a list. Tier 3
(`requires`/`ensures`/`invariant`/`transition`/`expect`) is open because a contract is only ever
evaluated — nothing compares two invariants. Tiers 1 (type refinement) and 2 (the actor claim
catalogue) are closed because those predicates must additionally be decidable at a boundary,
serialisable to JSON Schema, comparable across a context seam, and — once any row of the table above
is wanted — orderable by `⊨`. Note ADR 0189's own correction: ADR 0144's "one predicate surface" names
tier 3 only, and "read as covering all three, it oversells".

**R12.5 — Admission has four clauses, not three.** Compile-time semantics; a runtime check *generated
from the table row*; schema serialisation; **and the predicate's domain together with its entailments
in that domain.**
*Rationale:* the type-system spec's three-part rule omits the only clause that scales badly.
Entailment against a flat vocabulary is quadratic; against R12.2's domains it is one row.

**The seam already exists.** ADR 0200 DECISION A puts "one canonical normal form, owned by
`bynk-check`, shared by the matcher and the hash", motivated by exactly R12.2: refinement predicates
"were compared — and would be hashed — in **source order**", so `String where NonEmpty &&
MaxLength(10)` and the same predicates reordered "would have produced different hashes";
order-insensitivity "is not adjacent to the hash; it is a **precondition** for it." Normalising
`NonEmpty` into `MinLength(1)` inside that existing canonical form also makes the contract hash *more*
correct — two contracts differing only by `Positive` versus `InRange(1, ∞)` hash differently today and
would 409.

> **Language-surface note.** ADR 0253 D2 declines narrowing on refined patterns because "static
> narrowing waits on §2.5.4 (refinement propagation)". Under the nominal-versus-anonymous reading
> above, the missing feature is not narrowing-in-patterns but an **anonymous refined type** `{τ | P}`:
> given one, `_ where P` narrows to it mechanically, and it does not need propagation settled first,
> because the nominal case already ships the necessary fallback — "unprovable → the result is
> unrefined". `x : {Int | Positive}` with `x + 1 : Int` is a serviceable v1. Recorded because R12.2
> and R12.3 are its prerequisite and would make it cheap.

**E4 — Storage kinds.** `Cell`, `Map`, `Set`, `Log`, `Queue`, `Cache`. *Grows:* one `StoreKindIr`
variant, its op set, its state-shape derivation (R6.14) and its zero value. *Must not change:* the
atomic-commit contract (R6.15), and the idempotency split the design notes make explicit — `:=` is
idempotent on final state, `.update(fn)` is not, and "if the right-hand side of `:=` references the
left-hand side … it errors with a suggested rewrite to `.update`".
*Design note:* model store operations as one enum with a payload, not as N parallel maps. The shipped
checker has five parallel `store_*` maps, a five-clause `else if` dispatch and five near-duplicate op
checkers, accreted one per store kind (#36) — but the asymmetry is sometimes real: `check_store_map_op`
carries a held-resource rejection the cache version correctly lacks, so a naive merge would be wrong.
An exhaustive match keeps the asymmetry visible.

**E5 — Authentication schemes.** Closed by design — "the conservative starting position is a closed set
that can be opened later". *Grows:* one `AuthScheme` variant and its boundary wrapper (R8.13). *Must
not change:* the guarantee that by handler-body entry verification has run and the identity is a typed
value; and that multi-actor routes dispatch structurally, never by runtime credential-presence
branching.

**E6 — Targets.** `bundle` and `workers`. *Grows:* one `Target`, one codec parameterisation (R8.14),
one compose-root shape. *Must not change:* `Ir`. A target reads the IR and the `CommitShape`; it does
not get its own lowering.
*Carried forward as a known divergence:* ADR 0226 owns a deliberate cross-target difference in caller
identity at the unattributed top-level path. A new target inherits the obligation to state its
divergences, not the right to have none.
---

## Part 13 — Worked extension cases

An extension point nobody has pushed on is a claim, not a mechanism. This part walks four real
post-Foundations features through Parts 6, 8 and 12 and states what each costs. Each case ends with a
verdict; one of them is a partial failure, which is the point of doing the exercise.

The unit of cost is *new variants and new emission arms*, because those are what R8.1's totality rule
makes visible: adding a variant is a compile error at every arm that must handle it.

### 13.1 Events (ADRs 0284–0300)

**What it needs.** An `event` declaration kind; `from Events(E { field: value, .. })` as a service
protocol with pattern dispatch; `on event` handlers; `via schema(N)` version dispatch; an owner-only
emission gate; a runtime envelope; a fan-out substrate; and a cross-build schema registry that
auto-bumps on additive change and fails the build otherwise.

**How it attaches.**

| Piece | Mechanism | Cost |
|---|---|---|
| `event E = { … }` | `TypeShape::Record { fields, schema: Option<SchemaVersion> }` | 1 field |
| `from Events(E)` | E2 — `ProtocolIr::Events { event, pattern, schema_dispatch }` | 1 variant |
| `on event` | `HandlerKind::Event` | 1 variant |
| pattern dispatch | **desugaring**, not a new pattern kind — `EventPattern` becomes an `And` chain of `Field == Const` in the R6.7 table | 1 table row |
| owner-only `emit[E]` | **already carried** — R4.6's `DefiningContext(UnitId)` capability | 0 |
| release at commit | **already carried** — `CommitShape::FlushEvents` (R6.15) | 0 |
| schema registry | **already carried** — R3.11's `registry_in → registry_out` | 0 |
| emission | one arm in R8.1 | 1 arm |

**Cost: 3 variants, 1 table row, 1 emission arm, 0 new IR expression nodes.**

Two of the "already carried" rows are worth dwelling on, because they were introduced for unrelated
reasons and paid for themselves here. R4.6 replaced the `is_local_type(name)` string-set gate with a
capability value, to fix opacity — and the shipped compiler's own evidence is that the *same* gate
backs `.raw`, `T.unsafe(…)` and owner-only event emission, and that hand-rolling `ResolvedCommons`
turned off all three together. One capability value, three gates, no name lookup. Likewise R3.11 was
introduced because the schema lockfile made `compile` impure; Events is the feature that needs it.

The pattern-dispatch row also confirms a shipped decision rather than overturning it: `EventPattern`
is deliberately *not* a `Pattern`, because "events are records, no tag to test". Under Part 5 that is
exactly right — `IrPat::Variant` requires a discriminant — and the desugaring to a conjunction of
field equalities is the honest lowering.

**Verdict: fits.** Events is the feature that most looks like it needs new language machinery, and
three of its eight pieces are free.

### 13.2 Messages and locale (ADRs 0256, 0272–0280)

**What it needs.** A `messages` declaration; ICU MessageFormat template parsing; placeholder agreement
across every locale in a bundle; a locale-tag refinement; locale negotiation reading the request; and
a provider whose constructor arguments depend on whether the emitting context has a unique bundle.

**How it attaches — and where it does not.** This is the case where Part 14's Tier 3 rule bites.
`Locale.current()` is a capability; `messages` is not, because it carries a compile-time-checked
payload (R14.4). Splitting them is the whole design:

| Piece | Mechanism | Cost |
|---|---|---|
| `Locale.current()` | E1, Tier 1 (needs the request → env-taking metadata) | 0 new |
| `messages <tag> { … }` | `IrItem::Messages { tag, entries: Vec<(Code, IcuTemplate)> }` | 1 variant |
| ICU parse + placeholder agreement | a phase-5 analysis in `bynk-check` | ~1,000 lines, irreducible |
| locale tag refinement | E3 — a Language-domain predicate | 1 table row |
| provider constructor threading | Tier-1 metadata (R14.2), not an emitter probe | 0 new |
| emission | one arm in R8.1 | 1 arm |

**Cost: 1 variant, 1 table row, 1 emission arm, and an ICU parser.**

The parser is irreducible — it is a real sub-language with real semantics — but *where it lives* is the
architectural question, and the shipped answer is the wrong one. `emitter/icu.rs` is a 1,012-line
dependency-free ICU parser **in `bynk-emit`**, whose own doc comment names its consumer as "the
checker (`bynk-emit/src/project/validate.rs`)". The review's dry observation: "**the author already
thinks of `validate.rs` as checker-side code.**" And because `validate.rs` also uses
`emitter::placeholder_names`, `template_format_kinds` and `icu_dispatch_placeholders`, that reverse
edge is what blocks the obvious relocation — moving the context checks down "drags `icu.rs` (1,012
lines) and the WebSocket shape analysis with it".

Under R3.5 the question does not arise: a compile-time-checked payload is checked in phase 5, so the
ICU parser is a `bynk-check` module from the first line, and `bynk-emit` never sees a template it has
to understand.

The provider-constructor row is the second correction. Today `request`, `declaredLocales` and
`referenceLocale` are threaded into `LocaleProvider` by the emitter "only when this Worker's context
has a uniquely-detected message bundle", via a `detect_context_message_bundle` probe. Under R14.2 that
is a declared Tier-1 fact about the capability, not a probe the emitter runs.

**Verdict: fits, once `messages` stops being counted as part of a capability.** The naming is the
architecture here, not a label on it.

### 13.3 WebSockets (ADRs 0131–0135, 0187)

**What it needs.** `from websocket(In, Out)`; `on open` (mandatory, with a mandatory `by`), `on
message`, `on close` (optional); a `Connection[F]` held-resource type with a linearity discipline;
hibernation survival; broadcast over a stored connection map; and authentication at the edge before
the Durable Object.

**How it attaches.**

| Piece | Mechanism | Cost |
|---|---|---|
| `from websocket` | E2 — `ProtocolIr::WebSocket { in_ty, out_ty }` | 1 variant |
| `on open`/`message`/`close` | `HandlerKind` variants | 3 variants |
| `Connection[F]` | **already carried** — `Ty::Connection(TyId)` in Part 4 | 0 |
| linearity | a `LinearityMode { Consuming, Borrowing, NonConsuming }` field on kernel ops, plus a phase-5 walk over `Callee::Kernel` | 1 field, 1 analysis |
| `Map[K, Connection]` state shape | R6.14's derivation table — K→connId strings | 1 row |
| edge authentication | **already carried** — R8.13 | 0 |
| emission | one arm in R8.1 | 1 arm |

**Cost: 4 variants, 1 field, 1 derivation row, 1 emission arm, 1 analysis.**

The linearity analysis is the substantive addition, and R6.5 is what makes it tractable: an operation
on a held resource is `Callee::Kernel { recv: Connection, op }`, so the walk classifies by a resolved
enum rather than by matching identifiers against a set of store-field names. Compare the shipped
`block_writes_state`, which does the latter and misses any receiver that is not a bare `Ident`.

R8.13 pays for itself here in the same way R4.6 did in §13.1. The WebSocket case is the one where
edge authentication is not merely tidy but *required* — a browser cannot set `Authorization`, so the
token rides the first `Sec-WebSocket-Protocol` element, and the DO must never see an unverified
upgrade. A rule written for HTTP turns out to be the rule that makes WebSockets safe.

**Verdict: fits.** Note what did *not* appear in the table: no new IR expression node, no printer
change, no new phase.

### 13.4 Idempotency (ADRs 0282–0283) — the partial failure

**What it needs.** A capability with generic operations; a per-handler key scope so two handlers
cannot collide on the same literal key; and — when the provider becomes durable — participation in the
enclosing handler's atomic commit.

**How it attaches.**

| Piece | Mechanism | Cost |
|---|---|---|
| generic capability ops | E1 + explicit type arguments (ADR 0281) | 0 new here |
| per-handler key scope | a field on `Callee::Capability { cap, op, scope: Option<HandlerScope> }`, computed in phase 5 | 1 field |
| durable commit participation | **E7 (Part 14)** — the two-function runtime ABI | the whole of E7 |

**Cost: 1 field — and one extension point that does not yet exist.**

The key-scope row is the case where this architecture removes something genuinely unpleasant. ADR
0283 records that the shipped emitter rewrites the *first argument* of `dedup`/`remember` in place,
threaded via `LowerCtx::handler_scope`, with a **compiler panic** if the scope is unset — and,
decisively, that the rewrite must be keyed on the capability's *identity* rather than its name,
because three shipped fixtures declare their own `capability Idempotency` that must not be scoped. The
implementation is a literal `"bynk"` string comparison plus an `in_bynk_unit` flag.

Under R6.5 and R6.10 that entire decision dissolves. `Callee::Capability { cap: DefId, … }` already
*is* the identity; a third-party `Idempotency` has a different `DefId`, and no comparison is needed
because no name was ever consulted. The scope becomes a value the checker computes and records, so the
emitter has nothing to decide and nothing to panic about.

**But the third row is a real failure of E1 as written.** A durable `Idempotency` provider needs its
dedup record to commit or abort in lockstep with an agent it does not own. The idempotency track names
this precisely, and names it as unbuilt: every existing capability "is an independent side effect with
no participation in the calling agent's storage transaction", and a durable provider "is asking for a
third shape: capability-provider state that must commit-or-abort in lockstep with an *agent it doesn't
own*", needing "**a new narrow transactional-participation contract capability providers can opt
into**".

E1 cannot carry that, and no amount of metadata makes it. This is why Part 14 adds E7 and why
`CommitShape` (R6.15) is in the IR rather than being emitter control flow: the participation points
have to be enumerable by something other than the emitter before a provider can opt into them.

**Verdict: fits at Tier 2, and only because Part 14 adds an extension point for it.** Recorded as a
partial failure because the honest reading is that the exercise found a hole rather than confirming
its absence.

### 13.5 What the four cases establish

Across four features: **11 new variants, 3 new fields, 4 new emission arms, 2 new analyses, 1 ICU
parser, and 1 new extension point.** No new IR expression node, no printer change, no phase change,
and no change to the phase table.

Three rules introduced for unrelated reasons paid for themselves in a case that came later — R4.6
(opacity) carried owner-only event emission, R3.11 (purity) carried the schema registry, and R8.13
(HTTP boundary auth) carried the WebSocket upgrade. That is the signal the exercise was looking for:
extension points that only ever cost, and never pay, are usually the wrong seams.

---

## Part 14 — Capabilities: the tier taxonomy

E1 claims everything above Foundations enters through the capability seam and needs no new lowering.
The design notes make the same claim twice. Both are true of the *mechanism* and false of three of the
eight shipped capabilities. Part 14 states the test that separates them.

### 14.1 The evidence

`bynk-check/src/firstparty/bynk.bynk` is already a plain `adapter bynk { … }` exporting
`{ Clock, Random, Logger, Fetch, Secrets, Locale, Idempotency, Events }` with bodiless providers and a
TS binding per platform. ADR 0086 makes it real vendored `.bynk` source, `include_str!`'d, run through
the compiler's own pipeline and `bynk-fmt`-clean. The userland position is not speculative; it is 80%
shipped. The residue is three capabilities, each failing differently:

- **`Events` — syntax.** Four AST variants plus an owner-only checker gate. The tell is in the adapter
  itself: `bynk.bynk` says `provides Events = EventsProvider` and `bindings/bynk-cloudflare.ts`
  exports no `EventsProvider`. The declaration is telling a story the binding does not back.
- **`Idempotency` — a compiler-synthesised argument.** §13.4.
- **`Locale` — a compile-time-checked payload.** §13.2.

### 14.2 The test

A capability may be a pure adapter iff **(i)** it needs no syntax, **(ii)** it has no
compile-time-checked payload, **(iii)** it does not join the handler transaction, and **(iv)** it needs
no compiler-synthesised argument. An encoding derived from a `Codable` bound declared in the
operation's own signature, using the boundary codec, is not a compiler-synthesised argument
(amended by constrained-generics slice 3, #2715).

| Tier | Definition | Shipped members |
|---|---|---|
| **0 — substrate-free** | provider needs nothing from the deployment environment beyond a declared JS runtime baseline | `Clock`, `Random`, `Logger`, `Fetch` |
| **1 — adapter + declared metadata** | needs a fact the checker consumes that the adapter can state: platform nativity, env-taking, determinism | `Secrets` (env-taking), and all of `bynk.cloudflare` including `Kv` |
| **2 — adapter + runtime ABI** | participates in the handler transaction | `Idempotency`, once durable |
| **3 — not a capability** | has syntax or a compile-time-checked payload | `Events`' subscription half, `Locale`'s `messages` half |

The tier is not about effects. `Fetch` performs arbitrary network I/O and is Tier 0; `Secrets`
performs a dictionary lookup and is Tier 1. The axis is *substrate*, not *purity*.

**Ambient ≠ substrate-free.** ADR 0012 reserves the `bynk` namespace for "ambient primitives only:
`Clock`, `Random`, `Logger`, `Fetch`, `Secrets`" and states "no infrastructure capability ever joins
it" — which is why `Kv` correctly lives in `bynk.cloudflare`. But `Secrets` is ambient *and*
env-taking (`env[name]` on Workers, `globalThis.process.env[name]` on Node, with ADR 0025's optional
`env?` compose parameter). The reserved-namespace line and the tier line are two different lines.

### 14.3 Rules

**R14.1 — Tier 0 membership is verified, not declared.** Three facts the compiler already holds: the
provider's constructor takes no `env?: unknown`; its adapter's `binding` declares no `requires`
package dependencies; its unit is not registered platform-native by ADR 0024's metadata.
*Rationale:* every other tier depends on an annotation an author could omit or lie about. Tier 0 is
the only tier safe to leave open to third parties without a trust story, and it is safe precisely
because nothing about it is asserted.

**R14.2 — Tier 1 and Tier 2 facts are declared, and every default is fail-closed.** An unannotated
capability is assumed non-deterministic, non-native and non-transactional.
*Rationale:* the checker must know which ops are non-deterministic, because invariant bodies may not
call them — `Clock.now()` and `Random.next()` are unavailable there since they "would make invariants
non-deterministic, which contradicts their role". Under a userland vocabulary that fact comes from a
third-party annotation the compiler cannot verify. Fail-closed converts a forgetful author's silent
hole into a diagnostic.

**R14.3 — Closure under `given`: a provider may depend only on capabilities at its own tier or below.**
*Rationale:* generalises a rule ADR 0012 already wrote for one case — "a `bynk` capability may not
depend on a platform-native one". `provides Idempotency = IdempotencyProvider given Clock` is legal
because `Clock` is Tier 0; the converse would not be. Without closure the taxonomy is a labelling
exercise rather than a property.

**R14.4 — A capability declaration may not carry a compile-time obligation the compiler cannot
discharge from the declaration itself. If it needs a checker pass over its payload, it is not a
capability.**
*Rationale:* this is what puts `Events`' subscription half and `Locale`'s `messages` half in Tier 3.
The remedy is nominal, not engineering: `Events.emit` is a capability op and `from Events(E)` is
language; `Locale.current()` is a capability op and `messages` is a declaration kind. One name
spanning a userland surface and a compiler feature is what makes the boundary hard to see — §13.2 is
that split done concretely.

**R14.5 — Tier 0 declares a runtime baseline, verified in CI, and its capabilities' guarantees hold
identically on every runtime in that baseline or are weakened until they do.**
*Rationale:* two failures, both live. `Random.uuid()` presumably lowers to `crypto.randomUUID()`,
which is Web Crypto — not "the JS runtime" unqualified, and secure-context-only in browsers, which
matters given the browser target. And `Clock` fails the project's own platform-provider eligibility
rule (a platform provider is warranted when the implementation changes the observable *guarantee*, not
the *mechanism*): Cloudflare's documentation states that in deployed Workers "APIs that return timers,
including `performance.now()` and `Date.now()`, only advance or increment after I/O occurs", as a
Spectre mitigation — while under Wrangler locally "timers will increment regardless of whether I/O
happens or not". So `Clock`'s guarantee differs between production and `bynk dev`, for the capability
that is otherwise the cleanest Tier 0 example. The remedy is one sentence on the capability's doc
comment — *monotonic non-decreasing; may not advance within a handler between I/O operations* — and it
is also the sentence that stops someone writing a window that never expires.

### 14.4 E7 — the runtime ABI

**Grows:** a capability declares that it participates in the handler transaction. **Must not change:**
`Ir`, the printer, the phase table.

**R14.6 — Transaction participation goes through a named, versioned runtime API with exactly two entry
points — register-a-commit-action and register-an-abort-action — and `CommitShape` (R6.15) enumerates
the participation points. No adapter binding may reach an emitted symbol outside the published ABI.**
*Rationale:* §13.4. Two functions is the narrowest such contract, and narrowness is the point, because
this is the part that gets frozen.

**The 1.0 interlock, stated because it is a decision nobody has made.** ADR 0086 already calls the
bindings and runtime "part of the compiler's **emit ABI**", coupled to `Result`/`Option` tag layout,
`JsonError`, `Uuid.of`, `FetchError` — and defers publishing them "gated on runtime-ABI stability
(≈1.0)". Meanwhile `bynk-1.0-definition.md` states that stability does **not** freeze "the emitted
TypeScript. The compile target is an implementation detail, not part of the frozen contract; the
codegen may improve within a 1.x release as long as documented behaviour holds."

**An earlier revision of this section read those two as irreconcilable.** They are not, and the
settling review of the compiler-architecture track established why: they describe surfaces of
different size. ADR 0086's enumeration is *four shapes*; the codegen is the entire back end. A
binding is hand-written TypeScript that constructs `Ok(…)`, reads `.tag === "Err"` and calls
`Uuid.of` — every one of those is on the list. So the four can be published under their own semver
and held stable, while the codegen underneath stays as free as the 1.0 definition promises.

What survives the correction is the *shape* of the risk. The compiler never type-checks a
third-party binding, so a shape that drifts off the enumerated list without anyone noticing breaks
capabilities silently at runtime. **"All capabilities are adapters" is, in substance, a request that
the enumeration be maintained** — made at the release where the project issues its first
compatibility promise to code it did not write. That is a defensible thing to want, it is cheap while
the list is four items long, and it is not a thing to arrive at by refactor. The decision is recorded
as ADR 0310; the enforcement is a build-time guard that the vendored
first-party bindings reference only the enumerated surface.

> **Language-surface note.** R14.2's metadata (`@nondeterministic`, platform nativity,
> `@participates(commit)`) and R14.4's renaming are `.bynk` surface, which §0.1 excludes. What is in
> scope is the compiler-side shape: verified Tier 0 membership (R14.1), closure under `given`
> (R14.3), and the two-function commit ABI (R14.6). ADR 0024's own limit is why the metadata cannot be
> assumed — its platform registry is a hardcoded first-party table keyed on unit name, sound only
> because `bynk` is reserved, and it says so: "**Marker syntax is premature while no user-authored
> platform adapters exist** — it can be added additively when they become a goal." The userland
> position requires exactly the thing ADR 0024 deferred.
---

## Part 15 — The refusal register, and the honest cost

### 15.1 The register

**R15.1 — A refusal is recorded with four fields: the claim, the cost that justifies it, the trigger
that reverses it, and the evidence that checks the trigger. A refusal with no trigger is not a
decision; it is a preference that will be re-litigated on vibes.**

*Rationale:* the corpus records at least one refusal as an explicit *conditional* — the July review
refused a query engine because "inverting the pipeline **before the phase sequence has settled** would
be paying a large fixed cost for a problem that does not yet bite". 1.0 is the phase sequence
settling; that is what the stability commitment means. So the condition was expiring and nothing named
the trigger. That is R0.1's failure mode in a different costume.

---

**A lossless CST (rowan).**
*Cost avoided:* a rewrite of the formatter, the IDE layer and the checker's AST walks.
*Trigger:* green-node reuse becomes wanted — reparsing a whole file on each edit is measured as
costly, or the formatter needs to preserve regions it cannot parse.
*Evidence:* per-file reparse timings on the largest real `.bynk` file. The invalidation unit is the
file (R3.13), so this is currently far from the line.
*Note:* refusing rowan is not refusing a CST — the project publishes one (R2.9, R2.13).
*Tracked:* [#1524](https://github.com/accuser/bynk/issues/1524) — not left to memory.

**A demand-driven query framework (salsa or equivalent).**
*Status:* the **architecture is adopted** (§3.4) at the file and unit levels — the shared
`Tokens`/`Ast` cache and `UnitSignature` with its R3.14 stability proof; the definition and project
levels (`Body(DefId)`/`TypeOf(DefId)`, `ProjectGraph`) were built by phase 8 and **deleted** on
2 September 2026 ([#1537](https://github.com/accuser/bynk/issues/1537)) after the 30 August review
found nothing called them and no scheduler existed to — the same P5 posture the IR cutover took for
phase 6's expression IR. They are a few hundred lines to rebuild against the checker's own
per-definition entry points when the trigger below fires; R3.13's table stays the specification. The
**framework is deferred**.
*Cost avoided:* a dependency with a live API surface against an MSRV policy and a `deny.toml`; plus
the batch path paying for the incremental path on the deploy-critical route.
*Trigger:* a hand-rolled memo table over R3.13's four query levels is measurably the bottleneck.
*Evidence:* keystroke-to-diagnostic latency on a multi-context project, attributed by level.

**A typed expression IR between the checker and the string-emitting lowerer.**
*Status:* the **declaration-level IR is adopted** (`bynk-ir`'s shapes, read through `bynk-lower`'s
helpers — R6.13 at the level phase 6 retired it); the **expression-level IR is refused**.
*Claim:* `bynk-emit/src/emitter/lower.rs` reads `bynk_syntax::ast` directly for expression,
statement and body dispatch, and stays the single lowering path.
*Cost avoided:* a second expression code generator brought to feature parity with the first — measured
at ~4,400 lines and seven follow-on issues for three of seven entry points, with the remaining four
flagged as higher risk — plus the gate that keeps both reachable meanwhile, plus three `bynk-ir`
representation widenings that would exist only to let the new path reproduce the old one's bytes.
*Trigger:* an emit-side change that needs expression-level facts the checker already resolved and
`emitter/lower.rs` cannot get from `TypedCommons` without re-deriving them — **and** which the
tree-native route (lowering to `bynk_ts` nodes, retiring `ts_writes`) would not serve better. A second
miscompile of the kind T2.1 closed (hoisting, short-circuit) originating in AST re-classification would
fire this; a desire for R6.13 purity alone does not.
*Evidence:* the gated `unconsumed_ir_items` probe (`cargo xtask greenfield-status`), reading 0 and
ratcheted so it can only fall; `emit_diagnostics`/`ts_writes` unchanged by the deletion.
*Note:* refusing the expression IR is not refusing the IR — 19 `bynk-ir` items and 17 `bynk-lower`
entry points stay, each with a reader outside both crates. It is not a refusal of tree-native emission either, which is
the other way to reach the same end state and was never the cutover's scope.
*Tracked:* [#1542](https://github.com/accuser/bynk/issues/1542) (the IR cutover track, retired) —
the measurements, the priced alternative and the deletion inventory are in its closing summary in
`design/archive/retired-tracks.md`.

**An optimising IR, CFG or SSA form.**
*Cost avoided:* the whole apparatus, for a target that has a JIT.
*Trigger:* a supported target without a JIT appears.
*Evidence:* a target decision, not a measurement.
*Note:* incrementality argues *for* the typed IR — memoisation needs a stable identity for what it
caches, and `Ir` keyed on `CheckedProgram` supplies one.

**A second backend.**
*Cost avoided:* an IR that is mediocre at the one target you have.
*Trigger:* ADR 0016's no-portable-infrastructure position is reversed.
*Evidence:* a positioning decision. It does not belong to this document.

**Effect inference.**
*Cost avoided:* r1 recorded this as resolving a contradiction — design notes §15 says "**No effect
inference** — capabilities are declared, not inferred", type-system spec §2.8.4 says the system infers
effectfulness from the body. **A second, independent reason strengthens it:** inferred effects mean a
body edit can change a signature, which punches straight through R3.14's firewall — every dependent
re-checks on every keystroke inside any function. Declared capabilities are a precondition for cheap
incrementality.
*Trigger:* none identified; both reasons would have to fail.
*Evidence:* n/a — but the §15 versus §2.8.4 contradiction should still be resolved deliberately.
*Tracked:* [#1529](https://github.com/accuser/bynk/issues/1529) — not left to memory.

**A diagnostic error enum.**
*Cost avoided:* 434 variants in one enum, and `miette`'s renderer displacing an ariadne setup already
tuned for byte indexing.
*Trigger:* the registry-as-data approach (R9.1–R9.3) fails to carry severity, templates or `explain`
bodies without hand-duplication.
*Evidence:* the count of message templates not generated from the registry. It was 42 for one code
as of the July review (#45).
*Tracked:* [#1531](https://github.com/accuser/bynk/issues/1531) — not left to memory.

**Tuples.**
*Cost avoided:* a second product type beside records, and a second codec story.
*Trigger:* the query algebra's `join`/`groupBy` return types cannot be expressed with `MapEntry`-style
nominal records without combinatorial growth.
*Evidence:* the corpus already contradicts itself here — ADR 0120 says no tuples, type-system spec
§2.7.6 lists them as built-in, and design notes §11 types `join` as returning `Query[(T, U)]`. Resolve
before it becomes evidence.
*Tracked:* [#1530](https://github.com/accuser/bynk/issues/1530) — not left to memory.

### 15.2 What this design costs

**Arena ASTs are less pleasant than boxed trees.** You lose `match e.kind { Binary(l, op, r) => … }`
and gain explicit `ast[id]` indexing at every site. A daily ergonomic tax for the life of the project,
paid by every contributor, in exchange for a property that matters at exactly one seam.

**A TS tree plus printer is 2,000–3,000 lines written before the first emitted character.** At the
moment you write that first line, `writeln!` is genuinely faster and the tree looks like ceremony.
This is why nobody builds the printer first, and the reason is not carelessness — the cost is certain
and immediate while the benefit is diffuse and eighteen months away.

**An IR is a five-place edit per language change.** Part 13 measured it: four post-Foundations features
cost 11 variants, 3 fields and 4 emission arms. That is cheap *given the features are known*. It is
not cheap while the language is being discovered.

**Interning and totality cost memory.** `IndexVec<ExprId, TyId>` over every node is larger than a
sparse map for programs where most expressions are trivially typed.

### 15.3 The objection, taken seriously

The shipped compiler has the shape it has because **the language and the compiler were discovered
together**. An arena AST, a typed IR and a TS printer are a heavy fixed cost to pay when you do not
yet know whether `agent` will have a `store` block, whether `where` will have three predicate tiers, or
whether `Held[T]` will gain instances beyond `Connection[F]`. There is a serious argument that the
current architecture is precisely what allowed the language to cover 304 decision records' worth of
design ground in roughly a year, and that a from-scratch build against this reference would have
produced a smaller, more rigid language and a better compiler for it — which is the worse trade.

That argument is right, and it changes what the document is for. The conclusion is not "build the IR
first". It is:

**R15.2 — Build the throwaway substrate deliberately, record that it is throwaway, and write down the
trigger for replacing it.**

ADR 0060 already sets a line-count trigger for splitting a file — "~2,000 lines is 'eye it'; ~5,000 is
'split it'". There has never been an equivalent trigger for "the emitter's substrate is now costing
more than it saves", so the crossing point was passed in silence. Candidate triggers, stated so a
future reader can check them against evidence rather than feel:

- a defect class recurs in the lowering after being patched once at a different site (this fired, at
  `maybe_async_iife`, and was not noticed as a trigger);
- a documented language-level semantic property — short-circuit, evaluation order, atomicity — is found
  to be violated by the emitter rather than by the checker;
- the emitter's test density falls below a stated floor for two consecutive releases;
- a second consumer of the emitted artefact appears (a second target, a debugger, a course).

The missing artefact in the post-mortem was never an IR. It was the ADR that says: *we are lowering to
text on purpose, here is what it costs, and here is the signal that means stop.*

---

## Appendix A — Phase signature reference

```rust
// Phase 0 — driver only; the single point of filesystem contact.
fn load(root: &Path, overlay: &Overlay)
    -> Result<(Sources, Manifest, SchemaRegistry), Vec<Diagnostic>>;

// Phases 1–5 — pure, TOTAL outputs: always constructible, Ty::Error where a fact is unknown.
fn lex(src: &str, file: FileId)               -> (Tokens, Vec<Diagnostic>);
fn parse(tokens: &Tokens)                     -> (Ast, NodeTokens, Vec<Diagnostic>);
fn project(m: &Manifest, asts: &AstSet,
           reg: &SchemaRegistry)              -> (ProjectGraph, SchemaRegistry, Vec<Diagnostic>);
fn resolve(g: &ProjectGraph, asts: &AstSet)   -> (Resolved,     Vec<Diagnostic>);
fn check(r: &Resolved)                        -> (TypedProgram, Vec<Diagnostic>);

// The gate. The only place "may we emit?" is asked. CheckedProgram has no other constructor.
fn certify(p: TypedProgram, d: &[Diagnostic]) -> Result<CheckedProgram, Vec<Diagnostic>>;

// Phases 6–9 — pure, no diagnostics: a CheckedProgram is by construction emittable.
fn lower(p: &CheckedProgram, opts: &LowerOptions) -> Ir;
fn emit(ir: &Ir, target: Target)                  -> TsProgram;
fn print(ts: &TsProgram, opts: &PrintOptions)     -> Artefacts;
fn strip(a: &Artefacts)                           -> Result<Artefacts, Vec<Diagnostic>>;

// The two entry points. They share phases 1–5 exactly; only the editor skips `certify`.
fn compile(sources: Sources, manifest: Manifest, registry: SchemaRegistry, opts: Options)
    -> (Result<Artefacts, Vec<Diagnostic>>, SchemaRegistry);

fn analyse(sources: Sources, manifest: Manifest, opts: Options)
    -> (TypedProgram, Vec<Diagnostic>);        // never fails; what bynk-ide consumes
```

Phases 6–9 return no diagnostics. Every diagnostic the current emitter produces is either a checker
diagnostic in the wrong crate (R3.5) or an invariant that should have been a type (P2). If a
diagnostic is genuinely needed after `certify`, that is evidence the type is wrong — not a reason to
widen the signature.

**These signatures are the batch composition, not the unit of computation.** Under R3.13 each is a
fold over finer queries — `check` over `TypeOf(DefId)` and `Body(DefId)`, `parse` over `Ast(FileId)` —
with the firewall (R3.14) at `UnitSignature(UnitId)` between them. The batch form is what the CLI
calls; the query form is what the editor calls.

---

## Appendix B — Rule → prevented failure

| Rule | Prevents | Evidence |
|---|---|---|
| R0.1 | the substrate decision never being argued | 304 ADRs, none on the emitter's representation; ADR 0059 §4 |
| R2.1 | file-identity collision | ADR 0198; `count=2 unique=1`; undetected for 60 increments |
| R2.2 | cross-file labels underlining unrelated text | #46 |
| R2.3 | overlay-blind disk reads; untestable back end | #57, #13 |
| R2.4 | span-keyed collisions | #28; bug #844; `is_binding_cache`; the debug uniqueness walk |
| R2.5 | fallible lookups where a fact must exist | R4.9's consumer table |
| R2.6/R2.8 | lost comments; 34-field trivia drain; doc blocks deleted with the trivia | #26, #66; ADRs 0257, 0263, 0265–0269 |
| R2.7 | node-size regressions | #31 (`Expr` 176→120, `MatchArm` 488→376, measured) |
| R2.9 | claiming the project has no CST when `tree-sitter-bynk` is one | the document's own earlier overstatement |
| R2.10 | a non-idempotent formatter | shipped `IntBound`/`FloatLit` asymmetry, kept |
| R2.11/R2.12 | hand-rolled partial walks; `_ => false` over open enums | #54, #67, #8 |
| R2.13 | ambiguity found by the grammar becoming a language change | ADR 0189 D2; ADR 0253 D4 as forcing function |
| R3.1 | twelve-positional-argument sub-entry; lost options | #19, #57 |
| R3.2 | CLI and editor reporting different error sets | #64 |
| R3.3 | a refactor whose only net is a whole-file golden one crate up | #58; ADR 0059's gate |
| R10.4 | published facade re-attaching the monolith | #41, #23; ADR 0099's own invariant |
| R3.5 | semantics living in the codegen crate | #9 (110 codes in `validate.rs`), #16, #32, #14 |
| R3.6 | partial syntactic type comparison | #8 |
| R3.7 | two project models, one path-based | #62, #30 |
| R3.8 | silently-ignored malformed manifest | #13; ADR 0201 |
| R3.9 | a model of a removed manifest schema | #12 |
| R3.10 | an editor that gets nothing from a file that does not compile | #64; ADR 0094; `RecordCheck::partial_expr_types` |
| R3.11 | output depending on state absent from the signature | `bynk.schema.lock` auto-bump / build failure |
| R3.12 | "it already exists at analysis time. It is simply not exposed." | ADR 0095; ADR 0201 |
| R3.13 | re-lexing and re-parsing the world per keystroke | #65; #62 |
| R3.14 | a memo table that invalidates everything on every body edit | design notes §15; ADR 0200's contract hash |
| R3.15 | buying the easy half of incrementality and a dependency with it | the July review's conditional refusal |
| R4.1/R4.2 | deep-cloning every type on every comparison; `Ty` unusable as a key | no `Hash`, no interning, `Box<Ty>` trees; #51's quadratic clone |
| R4.3 | "unchecked" indistinguishable from "checked and broken" | `Option<Ty>` recovery; the separate `check_type_ref_resolves_in` pass |
| R4.4 | predicates reached by name through a table that may be the wrong one | `NamedKind::Refined(BaseType)` + `ResolvedCommons::types` |
| R4.5 | identity and assignability conflated | `compatible` vs derived `PartialEq`, kept |
| R4.6 | three security gates turned off by one hand-rolled struct | #16 (`.raw`, `unsafe`, owner-only emission) |
| R4.7 | a flexible var escaping into a caller's types | `Ty::Var`'s doc-comment invariant, pinned by nothing |
| R4.8 | `unify` returning true for `Int` vs `String` | the test named `unify_surprise_concrete_mismatch_returns_true` |
| R4.9 | five silent emitter fallbacks on a missing type | the consumer table in §4.4 |
| R4.10 | checker and emitter disagreeing about brands | `is_uses_commons_type` "mirrors `emit_context_rebrands` exactly"; #655 |
| R4.11 | six hand-rolled constructions of one phase value | #16 |
| R5.1 | pattern structure destroyed before the printer sees it | `pattern_match_tests` returning `Vec<String>` |
| R5.2 | the lowering form re-derived at three emission sites | `match_needs_if_chain` called from two entry points |
| R5.3 | a failing guard not falling through to the next arm | independent `if` blocks, kept |
| R5.4 | a guard unable to read its arm's bindings | the shipped nesting order, unstated |
| R5.5 | or-pattern bindings emitted as `const` | the `let a, b;` dispatch, discovered at emission |
| R5.6 | a reachable-dead `throw` on one path and none on the other | unconditional on switch, `!has_catchall` on if-chain |
| R5.7 | a refined arm counted toward exhaustiveness | `is_irrefutable` false for `Refined`, kept |
| R5.8 | `is` and match arms drifting apart | ADR 0253's "reuses … verbatim" |
| R5.9 | narrowing computed twice, once cached on a colliding key | `is_binding_cache` + `gather_is_bindings_for_emit` |
| R5.10 | a TDZ on the narrowed binding's own name | `is_receiver_ref_forced`; and #1's `simple_expr` misclassification |
| R5.11 | payload field names hard-coded before any declaration is consulted | `positional_field_name`'s `("Ok", 0) => "value"` table |
| R6.1 | fallible checker→emitter channel | #28 |
| R6.2 | dropped, spliced and IIFE-trapped statements | #1 (incl. the `match … ?` miscompile) |
| R6.3 | defeated short-circuit evaluation | #3; type-system spec §2.10.2 |
| R6.4 | async-ness decided by text scan | #2; `runtime_use.rs`'s own argument; `is_effectful_return` as the counter-example |
| R6.5 | a silently lost durable write | #54; `mutating_op`'s bare-`Ident` receiver match |
| R6.6 | IIFE-trapped early return; wrapper chosen by text scan | #1, #2 |
| R6.7 | sugar lowered inconsistently at N sites | #69 |
| R6.8 | losing the user's construct before the checker sees it | ADR 0161; `bynk-fmt` reprints the surface form |
| R6.9 | preconditions carried as doc comments | `DoStmt`/`SendStmt` "MUST be `Effect[()]`" |
| R6.10 | a load-bearing guard order duplicated across a crate boundary | `lower_method_call`'s own doc comment; ADR 0168's precedence in a `match` guard |
| R6.11 | six copies of the kernel vocabulary; four methods missing from completion | #34, #68 |
| R6.12 | a name list that works by the `count`/`size` coincidence | `is_query_op` |
| R6.13 | the emitter reading AST declarations | #53, #56; "almost no pure helpers" |
| R6.14 | store state shapes and index tables derived at emission | `HashMap` iteration drifting emitted bytes |
| R6.15 | atomicity as emitter control flow | design notes §13; ADR 0109; #54 |
| R6.16 | handlers branching on caller origin | design notes §9 |
| R7.1 | erasability violations; disarmed `tsc --strict` | #60, #18, #15; ADR 0136 |
| R7.2/R7.3 | 128 literal-indent sites; 48-field mutable emitter context | CodeWriter note re-measured July; #5, #56 |
| R7.4 | corrupted source-map offsets | #4; ADR 0103's hedge |
| R7.5 | readability as 506 independent judgements | the write-site census |
| R7.6 | `wrangler.toml` broken by a reformat | #40 |
| R7.7 | runtime TypeScript as Rust string literals | #17 |
| R7.8 | five emitted document kinds typed as one string | a real `out/` directory |
| R8.1 | a new declaration kind silently unemitted | R8.1's totality |
| R8.2/R8.5 | brand computed twice, mirror stated in prose | #481, #592, #655 |
| R8.3 | a refined alias gaining a bypass constructor | ADR 0182 |
| R8.4 | a non-finite `Float` passing `.of` | ADR 0040 |
| R8.6/R8.7/R8.8 | commit shape decided syntactically at emission | #54; ADR 0124 D4; ADR 0109 |
| R8.9 | two agent construction paths that can diverge by target | `makeAgent`'s target-agnostic call site, kept |
| R8.10 | one handler index recomputed at three sites | the cron/queue index |
| R8.11 | a deps type assembled by trimming braces off a string, six times | the shipped builder |
| R8.12 | `any` at a boundary the type checker should have covered | the `on call`-only typing carve-out |
| R8.13 | verification in a handler prologue; an unverified DO upgrade | ADRs 0085, 0175, 0132; the 403-vs-401 rule |
| R8.14/R8.15 | a second, drifting serialiser | #15 ("found and fixed twice") |
| R8.16 | two seams disagreeing about which providers take a caller | ADR 0226; `any_service_binds_caller` as the shared predicate |
| R8.17 | user-observable routing derived from an unstated comparator | the param-count sort |
| R8.18 | trusting an identity across a contract mismatch | the shipped ordering comment |
| R8.19 | config injection through a queue name | `escape_toml_basic_string` |
| R8.20 | a deploy placeholder broken by a reformat | the KV id literal |
| R8.21 | two rules for one async question | `is_effectful_return` vs `contains("await ")` |
| R8.22 | `JSON.stringify(undefined)` reaching the wire | the `result ?? null` fix |
| R9.1 | warnings printed as `Error:` with exit 0 | #44, #50 |
| R9.2 | 339 notes and 87 labels invisible | #47, #48, #49 |
| R9.3 | 42 templates for one code | #45 |
| R9.4 | nondeterministic diagnostic order | #11, #52 |
| R9.5 | a 434-variant enum displacing the tuned renderer | the rejection register |
| R9.6 | a warning with no line or column | #44 |
| R10.1 | crate names that mislead contributors | review theme 2 |
| R10.2 | LSP linking 63% of the emitter | #39 |
| R10.3 | retrospective carves that name the wrong seam | `bynk-strip` as control case |
| R10.5 | doctor green while the runner uses a different `tsc` | #20 |
| R10.6 | CLI contract spelled four times | #24, #72 |
| R10.7 | four flattening copies with four fallbacks | #48; ADR 0100 |
| R11.1 | coverage determined by API shape, then blamed on discipline | the bimodal ratio |
| R11.2 | attribution wrong for every split project, all tests green | ADR 0198; #58 |
| R11.3 | erasability tested on six snippets | #60 |
| R11.4/R11.5 | unowned fuzz targets; no Rust coverage | #59, #61 |
| R11.6 | one-directional registry drift test | #34, #68 |
| R11.7 | a conformance gate that has never caught a divergence | ADR 0189, ADR 0253 D4 |
| R11.8 | a panic or a ReDoS in a browser tab | ADRs 0215, 0217, 0223, 0229 |
| R12.1 | predicate runtime check written twice, already divergent | #70; ADR 0253's "reuses … verbatim" |
| R12.2 | eight predicates where five would do; order-sensitive comparison | ADR 0200 |
| R12.3 | entailment rediscovered as four separate rabbit holes | §2.5.4, §2.10.2, ADR 0253 S2, ADR 0158 |
| R12.4 | "one predicate surface" read as covering all three tiers | ADR 0189 |
| R12.5 | a vocabulary that grows quadratically in comparisons | the three-clause rule's omission |
| R14.1 | a tier whose membership depends on an author's honesty | ADR 0024's hardcoded registry |
| R14.2 | invariant determinism resting on a third-party annotation | type-system spec §2.10.2 |
| R14.3 | a taxonomy that is labelling, not a property | ADR 0012 |
| R14.4 | `Events`/`Locale` counted as capabilities they are not | ADR 0283 E, ADR 0282, the missing `EventsProvider` |
| R14.5 | "relies on the JS runtime" as an unstated assumption | Workers' frozen timers vs Wrangler's |
| R14.6 | the emit ABI frozen by accident at 1.0 | ADR 0086 vs the 1.0 definition |
| R15.1 | a conditional refusal remembered as a conclusion | the query-engine refusal's own wording |
| R15.2 | passing the crossing point in silence | the absence itself |

---

## Appendix C — What the shipped compiler already does right, and this keeps

1. **The decision-record discipline**, including recorded rejections — "Declined explicitly so the next
   reader knows it was weighed, not missed." The only reason a reference this specific can be written.
2. **Killing proposals on measurement rather than argument.** The CodeWriter shelving note inspected
   the emitter before writing code and contradicted its own premise. That the measurement later rotted
   is an argument for re-measuring, not against the method.
3. **Retiring risk by spike.** ADR 0103 settled source-map granularity by running V8 against two
   lowered functions and counting stops.
4. **Deletion as the repair.** ADR 0201: "Deleting the reduction is what makes the two tools
   structurally incapable of disagreeing." R15.1 and P5 generalise an instinct that was already there.
5. **Actively escaping golden-only verification.** Emission was changed specifically so `tsc --strict`
   becomes the guard instead of the goldens. R7.1 extends that posture.
6. **`const _: () = assert!(size_of::<Expr>() <= 128)`.** A structural fact pinned by the compiler
   rather than by a comment — one of the few places P2 is honoured, and it demonstrably worked.
7. **The exhaustive `expr_children` walker with no wildcard arm**, and its doc comment naming the bug
   it fixes. R2.11 is that pattern made mandatory rather than available. Note also that the shipped
   `Ty` operations — `compatible`, `unify`, `substitute` — already enumerate without a `_` arm "so
   adding a variant is a compile error".
8. **`bynk-strip`'s prospective carve.** The control case for R10.3.
9. **`is_effectful_return` as one predicate applied at six emission sites.** The right shape, sitting
   beside R6.4's substring scan for the same question.
10. **`makeAgent` as one target-agnostic factory**, with `__resetAgents()` giving tests isolation for
    free.
11. **Contract-check-before-body at the internal door**, with the ordering reasoned about and written
    down.
12. **Refusing rowan and salsa-the-framework.** Both calls remain correct; §3.4 changes only which
    half of the second one is refused.

---

## Appendix D — Migration index

Where the current tree diverges from each rule, so the reference is usable without a rewrite. **Rows
exist only for rules this measurement assessed and found open or recently closed — not all 130.** A
missing row is not a claim of conformance; it is either a rule this sweep did not walk (most of
phases 6–8, which have no live probe yet) or a rule with nothing yet worth recording. Appendix C
records what the shipped compiler already gets right independent of this table.

**Measured 30 July 2026 against v0.245.0**, not inferred from the 27 July review. That distinction
turned out to matter: the review is v0.237.1, and a probe sweep found **nine of the fourteen items
this table originally listed as open had already landed** in the eight versions since — including the
`ResolvedCommons` constructor, which re-enabled three security gates. Rows marked ✅ are recorded as
closed rather than deleted, because "this was a finding and it is no longer" is the useful state.

**This table must be generated**, keyed on rule id, by `cargo xtask greenfield-status` (track slice
T0.0). Hand maintenance is the failure mode Appendix B's own discipline warns about, and this
revision is the proof.

**Two rows are hand exceptions, and they say so.** R3.11 was corrected 6 August 2026 by
`design/tracks/project-model.md` §3.5, ahead of the generator this section asks for — a genuine
instance of the failure mode named above, accepted because the fix was one self-contained line. R3.5
was corrected 8 August 2026 by `design/tracks/semantics-in-the-checker.md` §1/§3.2, for the same
reason: two moves that predate that track's own settling review — P4.1 (#1115) and an untracked
"Wave 5" pass (#956) — had already relocated most of what this row's July 30 reading measured. The
table otherwise reflects the 30 July measurement date; those two rows do not.

| Rule | Measured state | Cost to close |
|---|---|---|
| R2.2 | `Span { start: usize, end: usize }` — no `FileId` | large — touches every span construction |
| R2.3 | `CompileOptions.sources` ✅ and `testkit.rs` ✅ **landed**; content-ownership track (#1086, retired 6 Aug 2026) closed `bynk-ide` (0) and `bynk-fmt` (0). `bynk-emit`: 3, all named exceptions — the enumeration walk (`discover_bynk_files`), the adapter-binding carve-out (`read_adapter_binding`), and the manifest-read carve-out (`try_read_project_paths`), each a documented, permanent floor rather than residual migration debt (`design/greenfield-status.md`'s `fs_below_driver`, precision landed per #1104) | none — the residual `bynk-emit` count is a named floor, not open work; closing any of the three would reopen a decision `design/tracks/content-ownership.md` §3.2 (retired) settled as permanent |
| R2.4 | `HashMap<Span` = **27** | large; the review kills the full `NodeId` retrofit — use parallel-data migration |
| R2.6 | correct already (`documentation` beside `trivia`) | none |
| R2.8 | `is_fully_drained` present (5 sites) ✅ **landed**; the 34-field drain itself remains | medium |
| R2.11 | `expr_children` at 34 uses across 8 files (31 July: new consumers in `bynk-check`, `bynk-lsp`); `type_refs_match` deleted ✅ (3 comment mentions only) | small residue |
| R2.12 | `[workspace.lints.clippy]` table ✅ **landed** (T0.3) at `warn`, but **unenforced** — no crate sets `[lints] workspace = true`, so nothing opts in. All 5 statement-aware walkers are hand-converted ahead of it — `bynk-emit`'s `block_writes_state` (#1022) and `block_uses_send` (#1025), and `bynk-lsp/src/extract.rs`'s `find_stmt_run_in_expr`, `expr_matches`, `locate` (#1025). The 5 remaining `_ => expr_children` tails in `bynk-check/src/checker.rs` are plain `Expr` searches that never key on a `Statement` tag, so the wildcard routes a new variant correctly there — they wait for the per-crate `deny`, not a hand fix. **Sweep with a multiline grep**: `locate`'s tail was `_ => {` with `expr_children` on the next line and a single-line `_ => .*expr_children` missed it | small per crate — one manifest edit, then fix what fails |
| R2.13 | `tree-sitter-bynk/tests/conformance.rs` exists, both-parsers-agree, both directions ✅ | none — the mechanism is built |
| R3.1 | `CompileOptions: Clone` ✅ **landed**; `run_checks`'s positional args remain | small residue |
| R3.2 | `bynk check` still runs the bailing path | medium |
| R3.3 | no probe yet — today's pipeline has no discrete phase-output types to check for `PartialEq`/serialisation against; T0.0 or a later phase must define what "measured" means here | not costed |
| R3.5 | `icu.rs`/`websocket::analyse_open_shape` in `bynk-check` ✅; `Ctx` `pub(crate)` ✅ (both #956); registered `bynk.*` codes in `bynk-emit` = **4/6** (true/naive) — true=4 is a named floor, four `#[cfg(test)]` assertion strings referencing `bynk-check`-owned codes as expected output, not live construction — `emit_diagnostics` probe ✅ **landed** (phase 5, retired 9 August 2026, closing summary in `archive/retired-tracks.md`) | none |
| R3.6 | `type_refs_match` = 0 ✅ **landed** | none |
| R3.7 | `bynk-project` crate exists ✅ **landed** (P4.0, #1113) — discovery, the unit graph, path resolution, consistency checks, the schema-registry document's read/write halves, and `AttributedError` moved out of `bynk-emit`; public surface enumerated (R10.4-style), no blanket `pub use` | none |
| R3.8 | `read_project_paths` deleted ✅ **landed** (P4.0, #1113) — all 18 callers moved onto `try_read_project_paths`, surfacing `ProjectPathsError` instead of silently falling back to the conventional layout | none |
| R3.9 | `Roots::trees` walks every `[paths] include` entry ✅ **landed** (P4.0, #1113) — no longer a hardcoded primary/secondary pair; a 3+-entry `include` list is discovered in full instead of silently dropping `include[2..]` | none |
| R3.10 | `certify` = 0; `RecordCheck::partial_expr_types` exists as the seam | small — the shape is there, it needs to be the only path |
| R3.11 | `CompileOptions::schema_registry` threaded end-to-end (`bynkc/src/main.rs`'s `schema_lock::read`/`write` round trip); every non-`Build` `run_checks` caller passes an explicit `SchemaLock::Off` (#1078), not an ambient read ✅ **landed** — closed by prior paydown, not this row's original 30 July 2026 measurement; corrected 6 August 2026 per `design/tracks/project-model.md` §3.5 | none |
| R3.12 | no editor-query table | small to write, valuable immediately |
| R3.13/R3.14 | no query decomposition; no `UnitSignature` | phase 8 |
| R4.1/R4.2 | `Ty` is `Box`-recursive, not interned, no `Hash` | medium |
| R4.3 | no `Ty::Error`; recovery is `Option<Ty>` | medium — unblocks R3.10 |
| R4.4 | `contract.rs` canonicalises ✅; whether it *normalises* `NonEmpty`→`MinLength(1)` unverified | small |
| R4.5 | `compatible` vs `PartialEq` split correct already | none |
| R4.6 | `ResolvedCommons` constructor + private field ✅ **landed**; zero hand-rolls in `bynk-emit` | **none — the three gates are back on** |
| R4.7 | `Ty::Var(String)` with rigidity by convention | medium |
| R4.8 | `unify`'s `_ => true` ground catch-all | small |
| R4.9 | `expr_types: HashMap<Span, Ty>` with five silent fallbacks | large — gated on R2.4 |
| R4.10 | brand computed at emission, mirrored in prose | medium |
| R4.11 | constructor exists ✅ **landed** | none |
| R5.6 | trailing `throw` unconditional on the switch path | small |
| R5.9 | narrowing computed in checker and emitter separately | medium |
| R5.11 | `positional_field_name` hard-codes `Ok`/`Err`/`Some` | small |
| R6.2 | `stmts: &mut Vec<String>` = **0** ✅ **landed** (T2.1, #1017) | — |
| R6.3 | `lower_and_with_is`'s splice and `lower_bin_op`'s shared-vector defect ✅ **landed** (#955, pre-dating this table); the residual gap the fix left — a `?`'s propagating return inside a short-circuited rhs escaping only as far as the operator's own arrow wrapper — ✅ **landed** (T2.3, #1019), via `hoist_if_as_statement` (T2.1) on a `LowerCtx::emitted_early_return`-flagged rhs | — |
| R6.4 | `contains("await` = **1** in `lower.rs` | small — a flag on the IIFE |
| R6.5 | `block_writes_state` now descends via `expr_children` ✅; the name-matched receiver remains | small as a patch; free under R6.2 |
| R6.10 | two hand-synced dispatch ladders, no enum | large |
| R6.11 | `joinOn`/`groupBy` present in `kernel_methods.rs` ✅ **landed**; the one-directional test and the second copy remain | small |
| R6.13 | `bynk_syntax::ast` imported in **13 files** of `bynk-emit` | phase 6 |
| R6.15 | commit shape decided by an AST walk at emission | small once R6.13 exists |
| R7.1 | `strip_project_to_js` in 7 test sites ✅ **landed**; `TsType::Any` equivalents remain at two sites | small residue |
| R7.3 | 506 write sites, 128 literal indents | large; re-scope to `lower.rs` where the measurement inverted |
| R7.8 | `Artefacts` is effectively strings | small |
| R8.15 | `http_value_serialiser` = 0, `serialise_ref_via` = 7 ✅ **landed** | none |
| R8.22 | `result ?? null` ✅ **landed** | none |
| R9.1 | `ReportKind::Warning` present ✅ **landed**, with a comment naming the old defect | none |
| R9.4 | `groups`/`test_groups`/`kinds`/`integration_groups` are `BTreeMap` ✅ **landed**; `unit_info` ✅ **landed** (#952) | none |
| R10.2 | `bynk-ide` → `bynk-emit` edge ✅ **closed** (P4.2, #1122): all nine `bynk_emit::project`/`bynk_emit::emitter` reach points repointed onto `bynk_check::analysis`/`bynk-project` directly; `bynk-ide/Cargo.toml`'s `bynk-emit` dependency line deleted; `ide_emit_edge` (Appendix C) now reads `absent` | none |
| R10.4 | `bynk-ide` demoted to a dev-dependency ✅; the 14 `bynk-check`/`bynk-emit` whole-module re-exports deleted ✅; the remaining `bynk-syntax` (7 modules), `bynk-driver` (2), and `bynk_fmt as fmt` re-exports deleted ✅ (#1048) | none |
| R10.5 | `run_test` in `bynkc/src/main.rs` | medium; the subprocess delegation is a documented trade-off |
| R11.2 | 3 `expected_contains`, 2 `expected_absent`, 4 `expected_diagnostics` against **419** `expected_error` | **the format exists ✅; adoption is ~1%** |
| R11.7 | conformance test is case-scoped to the type surface | **small — widen the corpus to totality** |
| R11.8 | four bounds exist; no rule, no inventory | small |
| R12.2 | `canon_refinement` compares as a set ✅; `NonEmpty` → `MinLength(1)` normalisation ✅ (#1021); `Positive`/`NonNegative` → `InRange` normalisation declined (#1049) — neither base has a writable bound standing for `∞` to fold onto, so there is no second spelling to collapse onto and nothing consumes the normal form until R12.3 exists | none — deferred to R12.3 |
| R14.1 | no tier concept | small — three checks the compiler already has the data for |

**How to read this now.** Fourteen rows are ✅. The remaining short-and-high-value set is: the lint
table (R2.12), conformance totality (R11.7), fixture adoption (R11.2), the typed hoist (R6.2), the
async flag (R6.4), and `ReportKind`'s residue. Everything else is phase 3 or later.

**One row went the wrong way, then reversed once a track existed to absorb it.** R3.5's distance grew
between v0.237.1 and v0.245.0 — 190 counted codes originating in `bynk-emit` then, 200 registered codes
at this table's own 30 July reading. It has since dropped to 49/53 (corrected above, 8 August 2026) —
not because phase 5 opened yet, but because phase 4's own P4.1 slice, scoped for something else
entirely, relocated most of the shared checking pipeline as a side effect of closing R10.2. That is
still the argument this document opened with, sharpened rather than undercut: *small* defects are
fixed by ordinary churn either way; a *structural* one only moved because a track (phase 4) existed
for a slice to attach the fix to, and even then it moved as an accidental side effect rather than by
design — the version of "ordinary work" that still needs a track's own review to notice, name, and
finish (`design/tracks/semantics-in-the-checker.md` §1 is exactly that noticing, after the fact).

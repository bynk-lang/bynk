//! Abstract syntax tree types for Bynk v0 (spec §9.2).

use crate::span::Span;

/// An identifier with its source span.
#[derive(Debug, Clone)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}

/// Comment trivia attached to a declaration or statement (v1.1 LSP spec
/// §3.5). The parser collects line comments from the token stream and
/// attaches them to nearby AST nodes so the formatter can re-emit them.
///
/// - `leading` holds comments that appear immediately above the node,
///   ordered top-to-bottom: each a `--` line, or an orphaned `---` doc block
///   (see [`Comment`]). Above a unit-level declaration it also records where a
///   blank line separated them ([`Comment::Blank`]).
/// - `trailing` holds a single comment that appears on the same source
///   line as the node's final token (e.g. `expr  -- note`).
#[derive(Debug, Clone, Default)]
pub struct Trivia {
    pub leading: Vec<Comment>,
    pub trailing: Option<String>,
}

/// One entry of comment trivia.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Comment {
    /// A `--` line comment: the text after the marker, with its original
    /// inline whitespace preserved.
    Line(String),
    /// #1756: a doc (a `---` block, or since #1888 a run of `--|` lines) that
    /// attaches to no declaration (a blank line separates it from the next
    /// one, or nothing follows it). The parser warns
    /// `bynk.parse.orphan_doc_block` and keeps the block here, so the formatter
    /// can print it where it was. Its content is normalised as an attached
    /// doc's is.
    OrphanDoc(Doc),
    /// #1884: a blank line after a `--` comment, before the next comment or
    /// the declaration. Recorded only where canonical style sets declarations
    /// apart with blank lines: in the leading and closing comments of a
    /// unit-level declaration (an item, a `uses`/`consumes`/`exports` clause,
    /// the unit header), and of a service's sections and handlers or an agent's
    /// invariants, transitions and handlers. There it keeps a comment that is
    /// separated from the declaration below from reading as describing it.
    /// Elsewhere (statements, record fields, capability ops, policy fields,
    /// store fields) style has no blank lines, so the parser drops it. Several
    /// blank lines are one entry.
    Blank,
}

/// #1888: a documentation comment as written: its normalised text and the
/// form it was written in. Every `documentation` field holds one. It derefs to
/// its text, so a reader that only wants the Markdown treats it as a `str`;
/// only the formatter reads [`Doc::form`], to print each doc in the form its
/// author chose (DECISION F: it never converts between them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Doc {
    /// The Markdown text: a `---` block's content with its common indent
    /// stripped, or the `--|` lines' text joined with newlines.
    pub text: String,
    pub form: DocForm,
}

/// #1888: which of the two doc forms a [`Doc`] was written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum DocForm {
    /// A `---` fenced block (ADR 0188; matched-length fences since #1885).
    #[default]
    Block,
    /// One or more consecutive `--|` lines.
    Lines,
}

impl Doc {
    pub fn new(text: impl Into<String>, form: DocForm) -> Self {
        Self {
            text: text.into(),
            form,
        }
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }
}

impl std::ops::Deref for Doc {
    type Target = str;
    fn deref(&self) -> &str {
        &self.text
    }
}

/// A doc built from bare text is a `---` block, the form every doc had before
/// #1888.
impl From<String> for Doc {
    fn from(text: String) -> Self {
        Self::new(text, DocForm::Block)
    }
}

impl From<&str> for Doc {
    fn from(text: &str) -> Self {
        Self::new(text, DocForm::Block)
    }
}

impl std::fmt::Display for Doc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

impl Trivia {
    pub fn is_empty(&self) -> bool {
        self.leading.is_empty() && self.trailing.is_none()
    }
}

/// A whole parsed commons source file.
///
/// In v0.3 a commons may be split across multiple files in a directory; the
/// resolver merges them into one logical commons. Each parsed AST instance
/// represents the contribution from a single source file.
#[derive(Debug, Clone)]
pub struct Commons {
    pub name: QualifiedName,
    pub items: Vec<CommonsItem>,
    /// `uses` clauses declared in this file.
    pub uses: Vec<UsesDecl>,
    /// Optional documentation block attached to the commons declaration.
    pub documentation: Option<Doc>,
    /// Surface form of the file: brace-delimited body or headerless fragment.
    pub form: CommonsForm,
    pub span: Span,
    /// Trivia attached to the commons declaration itself — leading comments
    /// before the `commons` keyword and a trailing comment after the header
    /// or closing brace.
    pub trivia: Trivia,
    /// Comments appearing after the last item but before the file ends
    /// (or the closing brace, for brace form). One entry per `--` line.
    pub trailing_comments: Vec<Comment>,
}

/// The two surface forms in which a commons body may be parsed (v0.3 §3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommonsForm {
    /// `commons name { ... }`
    Brace,
    /// `commons name` followed by top-level declarations to EOF.
    Fragment,
}

/// A `uses other.commons` declaration (v0.3 §3.3).
#[derive(Debug, Clone)]
pub struct UsesDecl {
    pub target: QualifiedName,
    pub span: Span,
    pub trivia: Trivia,
}

/// A whole parsed context source file (v0.4 §3.1).
///
/// Contexts are the architectural-layer declaration kind. Like commons, a
/// context may be split across multiple files in a directory.
#[derive(Debug, Clone)]
pub struct Context {
    pub name: QualifiedName,
    pub items: Vec<CommonsItem>,
    /// `uses` clauses declared in this file.
    pub uses: Vec<UsesDecl>,
    /// `consumes` clauses declared in this file.
    pub consumes: Vec<ConsumesDecl>,
    /// `exports` clauses declared in this file.
    pub exports: Vec<ExportsDecl>,
    /// Optional documentation block attached to the context declaration.
    pub documentation: Option<Doc>,
    /// Surface form of the file: brace-delimited body or headerless fragment.
    pub form: CommonsForm,
    pub span: Span,
    /// Trivia attached to the context declaration itself — leading comments
    /// before the `context` keyword.
    pub trivia: Trivia,
    /// Comments appearing after the last item but before the file ends
    /// (or the closing brace, for brace form). One entry per `--` line.
    pub trailing_comments: Vec<Comment>,
    /// State-migrations S3 (#2248): `removed agent Old` declarations. Kept
    /// beside `items` rather than as a `CommonsItem`: they declare a class's
    /// *absence* (its stored data is deleted), so nothing that walks the
    /// unit's items has anything to do with them.
    pub removed_agents: Vec<RemovedAgentDecl>,
}

/// `removed agent Old` (state-migrations S3, #2248): the agent class `Old`
/// is gone, and its stored data is deleted through an `exports` tombstone.
#[derive(Debug, Clone)]
pub struct RemovedAgentDecl {
    /// The contextual `removed` keyword.
    pub keyword: Span,
    pub name: Ident,
    pub documentation: Option<Doc>,
    pub span: Span,
    pub trivia: Trivia,
}

/// A `consumes other.context` declaration (v0.4 §3.2). May optionally carry
/// an alias introduced by `consumes other.context as Alias` (v0.6 §3.1).
#[derive(Debug, Clone)]
pub struct ConsumesDecl {
    pub target: QualifiedName,
    pub alias: Option<Ident>,
    /// v0.17: `consumes U { Cap, … }` — selected capabilities flattened into
    /// the consumer's local capability namespace under their bare names (§3.3).
    /// `None` for the whole-unit forms; `Some` (possibly empty) for the braced
    /// form. Mutually exclusive with `alias`.
    pub selected: Option<Vec<Ident>>,
    pub span: Span,
    pub trivia: Trivia,
}

/// An `exports visibility { names }` clause (v0.4 §3.3) or, v0.15, an
/// `exports capability { names }` clause.
#[derive(Debug, Clone)]
pub struct ExportsDecl {
    pub kind: ExportKind,
    pub names: Vec<ExportName>,
    pub span: Span,
    pub trivia: Trivia,
    /// #1797: comments before the closing `}`.
    pub trailing_comments: Vec<Comment>,
}

/// One name in an `exports` list, with its comments (#1797).
#[derive(Debug, Clone)]
pub struct ExportName {
    pub name: Ident,
    /// #1797: comments above the name and at the end of its line. A comment
    /// on the list's `{` line leads the first name.
    pub trivia: Trivia,
}

/// What an `exports` clause exposes: types (with a visibility) or, v0.15,
/// capabilities offered for cross-context consumption.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportKind {
    /// `exports opaque { ... }` / `exports transparent { ... }` — type exports.
    Type(Visibility),
    /// `exports capability { ... }` — capabilities offered to consumers (v0.15).
    Capability,
}

/// Visibility level for an exports clause (v0.4 §3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    /// Token-only outside the context: hold, pass, compare; no inspect, no construct.
    Opaque,
    /// Readable shape outside the context: inspect fields, match variants; no construct.
    Transparent,
}

/// An `adapter qualified.name { … }` declaration (v0.17 §3.1). An adapter
/// co-locates a capability contract with a non-Bynk binding: it may declare
/// capabilities, the boundary types they reference, inline pure helper
/// `type`/`fn` (and `uses`), external (bodiless) providers, `exports
/// capability`, and exactly one `binding` clause. It may *not* declare
/// services, agents, or bodied providers. Like commons/contexts it may be
/// split across files in a directory.
#[derive(Debug, Clone)]
pub struct AdapterDecl {
    pub name: QualifiedName,
    pub items: Vec<CommonsItem>,
    /// `uses` clauses declared in this file (pure-vocabulary mixin; allowed
    /// because helpers cannot pierce containment — spec [DECISION B]).
    pub uses: Vec<UsesDecl>,
    /// `exports capability { … }` clauses (adapters export capabilities and
    /// boundary types, never services).
    pub exports: Vec<ExportsDecl>,
    /// v0.18: `consumes U { Cap, … }` clauses — adapter-to-adapter capability
    /// dependencies (spec §4.5, \[N\]). Braced form only; adapter targets only
    /// (both enforced semantically, not in the parser).
    pub consumes: Vec<ConsumesDecl>,
    /// The `binding "<module>" requires { … }` clause, if present. Required
    /// when the adapter declares any external provider (`bynk.adapter.no_binding`).
    pub binding: Option<BindingDecl>,
    pub documentation: Option<Doc>,
    pub form: CommonsForm,
    pub span: Span,
    pub trivia: Trivia,
    pub trailing_comments: Vec<Comment>,
}

/// A `binding "<module>" [includes [ … ]] requires { "pkg": "range", … }` clause inside an
/// adapter (v0.17 §3.5). `module` is the TypeScript module supplying the
/// adapter's external provider symbols, resolved relative to the adapter's
/// source file. `requires` declares npm dependencies folded into the
/// generated `package.json`.
#[derive(Debug, Clone)]
pub struct BindingDecl {
    /// The module path as written (the string-literal contents, no quotes).
    pub module: String,
    pub module_span: Span,
    /// #2321: the `includes ["./x.wasm", …]` files shipped beside the binding,
    /// as written (string-literal contents, no quotes) with their spans.
    pub includes: Vec<IncludeEntry>,
    pub requires: Vec<RequiresDep>,
    pub span: Span,
    pub trivia: Trivia,
}

/// One path in a binding's `includes [ … ]` list (#2321).
#[derive(Debug, Clone)]
pub struct IncludeEntry {
    pub path: String,
    pub span: Span,
}

/// One `"pkg": "range"` entry in a binding's `requires { … }` map.
#[derive(Debug, Clone)]
pub struct RequiresDep {
    pub package: String,
    pub range: String,
    pub span: Span,
}

/// Either a commons or a context — the two declaration kinds at the file
/// level (v0.4 §3.1). v0.7 adds the test declaration kind; v0.17 the adapter.
#[derive(Debug, Clone)]
pub enum SourceUnit {
    Commons(Commons),
    Context(Context),
    Suite(SuiteDecl),
    /// v0.17: an `adapter` unit — the host boundary (capability contract +
    /// external binding).
    Adapter(AdapterDecl),
}

impl SourceUnit {
    pub fn name(&self) -> &QualifiedName {
        match self {
            SourceUnit::Commons(c) => &c.name,
            SourceUnit::Context(c) => &c.name,
            SourceUnit::Suite(t) => &t.target,
            SourceUnit::Adapter(a) => &a.name,
        }
    }

    pub fn span(&self) -> Span {
        match self {
            SourceUnit::Commons(c) => c.span,
            SourceUnit::Context(c) => c.span,
            SourceUnit::Suite(t) => t.span,
            SourceUnit::Adapter(a) => a.span,
        }
    }

    /// The doc-block above this file's unit header, if it carries one. For a
    /// `commons`/`context`/`adapter` this is the unit's *module doc* (#1885);
    /// for a `suite` it is the suite's own doc.
    pub fn documentation(&self) -> Option<&str> {
        match self {
            SourceUnit::Commons(c) => c.documentation.as_deref(),
            SourceUnit::Context(c) => c.documentation.as_deref(),
            SourceUnit::Suite(t) => t.documentation.as_deref(),
            SourceUnit::Adapter(a) => a.documentation.as_deref(),
        }
    }

    /// The doc-block above this file's unit header when it carries prose:
    /// [`SourceUnit::documentation`], with an empty or all-whitespace block
    /// treated as absent. This is what "a file carries a module doc" means
    /// for the one-module-doc-per-unit rule and for the tooling that shows the
    /// doc (#1885): an empty `---`/`---` pair documents nothing, so it neither
    /// counts against the rule nor shadows a sibling file's real doc.
    pub fn module_doc(&self) -> Option<&str> {
        self.documentation().filter(|d| !d.trim().is_empty())
    }

    pub fn kind_name(&self) -> &'static str {
        match self {
            SourceUnit::Commons(_) => "commons",
            SourceUnit::Context(_) => "context",
            SourceUnit::Suite(_) => "suite",
            SourceUnit::Adapter(_) => "adapter",
        }
    }
}

/// A `test <qualified-name> { ... }` declaration (v0.7 §3.1).
///
/// A test targets a commons or context by qualified name and bundles a set of
/// test cases plus optional mock declarations. As with commons and contexts, a
/// test may be split across multiple files (fragment form).
#[derive(Debug, Clone)]
pub struct SuiteDecl {
    /// The targeted commons or context.
    pub target: QualifiedName,
    /// `uses` clauses brought in by this test fragment.
    pub uses: Vec<UsesDecl>,
    /// v0.118: suite-scoped `stub` clauses — per-seam provider overrides
    /// applied to every case (a case-scoped `stub` takes precedence). Formerly
    /// the punned `provides` stub; renamed to `stub` in the keyword-hygiene
    /// batch (#548).
    pub stubs: Vec<StubClause>,
    /// #2134: suite-scoped `env { … }` clauses — the variables and secrets
    /// every participant sees, for every case (a case's own `env` layers over
    /// them name by name).
    pub envs: Vec<EnvClause>,
    /// The individual test cases.
    pub cases: Vec<Case>,
    /// v0.114: generative `property` blocks (testing track slice 2).
    pub properties: Vec<PropertyDecl>,
    /// v0.118: the suite-level tier default (`suite … as integration`). `None`
    /// means the `unit` default; a `case`'s own tier overrides it. A `property`
    /// ignores a suite tier (tiers are a `case`-only affordance).
    pub tier: Option<TestTier>,
    /// Surface form: brace-delimited body or headerless fragment.
    pub form: CommonsForm,
    /// Optional documentation block attached to the test declaration.
    pub documentation: Option<Doc>,
    pub span: Span,
    pub trivia: Trivia,
    pub trailing_comments: Vec<Comment>,
}

/// v0.118: the tier a `case` runs at (testing track slice 6, ADR 0153). One
/// body promoted across the testing pyramid; `unit` is the default and elided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestTier {
    /// Collaborators stubbed (the default).
    Unit,
    /// Real collaborators within one context, no serialisation wire.
    Integration,
    /// Contexts wired across the real serialise → JSON → deserialise boundary.
    System,
}

impl TestTier {
    pub fn as_str(self) -> &'static str {
        match self {
            TestTier::Unit => "unit",
            TestTier::Integration => "integration",
            TestTier::System => "system",
        }
    }
}

/// v0.118: a per-seam provider override `stub Cap.method(<args>) returns <v>
/// | fails` (testing track slice 6, ADR 0154; keyword `stub` since #548).
/// Substitutes one capability method's provision under test; the right-hand
/// side is a value or a fault, never a computed body.
#[derive(Debug, Clone)]
pub struct StubClause {
    /// The capability being overridden (a consumed seam of the unit).
    pub capability: Ident,
    /// The overridden method.
    pub method: Ident,
    /// One argument pattern per parameter (`_` or a value the arg must equal).
    pub args: Vec<ArgPattern>,
    /// The provision: a value, a fault, or a per-call sequence.
    pub rhs: StubRhs,
    pub documentation: Option<Doc>,
    pub span: Span,
    pub trivia: Trivia,
}

/// #2134: a suite- or case-scoped `env { NAME = "value" … }` clause — the
/// variables and secrets a suite's participants see (ADR for #2134). `env` is
/// contextual: a keyword only at the head of a suite or case body member.
#[derive(Debug, Clone)]
pub struct EnvClause {
    /// The entries, in source order.
    pub entries: Vec<EnvEntry>,
    pub documentation: Option<Doc>,
    pub span: Span,
    pub trivia: Trivia,
}

/// #2134: one `NAME = <value>` entry of an [`EnvClause`]. The value is parsed
/// as an expression; the checker requires a `String` literal.
#[derive(Debug, Clone)]
pub struct EnvEntry {
    pub name: Ident,
    pub value: Expr,
    pub span: Span,
}

/// v0.118: one argument pattern in a `stub` call pattern. Patterns for the
/// same method are tried top-to-bottom, first match wins.
#[derive(Debug, Clone)]
pub enum ArgPattern {
    /// `_` — matches any argument.
    Any(Span),
    /// A value the recorded argument must equal (a literal or pure value expr).
    Value(Expr),
}

/// v0.118: the right-hand side of a `stub` clause.
#[derive(Debug, Clone)]
pub enum StubRhs {
    /// `returns <value>` — a single success value, repeated for every call.
    Returns(Expr),
    /// `fails` — inject a capability fault (Principle 3).
    Fails(Span),
    /// `returns each [<outcome>, …]` — one outcome per call, in order; the last
    /// outcome repeats once the sequence is exhausted (DECISION V).
    ReturnsEach(Vec<SeqOutcome>, Span),
}

impl StubRhs {
    pub fn span(&self) -> Span {
        match self {
            StubRhs::Returns(e) => e.span,
            StubRhs::Fails(s) => *s,
            StubRhs::ReturnsEach(_, s) => *s,
        }
    }
}

/// v0.118: one outcome in a sequenced (`returns each`) `stub`.
#[derive(Debug, Clone)]
pub enum SeqOutcome {
    /// A success value.
    Value(Expr),
    /// A fault.
    Fails(Span),
}

/// A `case "name" [as <tier>] { [stub …] body }` block inside a suite
/// (v0.7 §3.3; v0.118 adds the tier clause and case-scoped stubs).
#[derive(Debug, Clone)]
pub struct Case {
    /// The test name, taken from the string literal.
    pub name: String,
    /// The span of the string literal — used for diagnostics and runtime
    /// failure reports.
    pub name_span: Span,
    /// v0.118: the case's own tier, if written (`as integration` / `as system`).
    /// `None` means inherit the suite default (itself `unit` when unset).
    pub tier: Option<TestTier>,
    /// v0.118: case-scoped `stub` clauses (override the suite's, and the
    /// tier default).
    pub stubs: Vec<StubClause>,
    /// #2134: case-scoped `env { … }` clauses, layered over the suite's.
    pub envs: Vec<EnvClause>,
    pub body: Block,
    pub documentation: Option<Doc>,
    pub span: Span,
    pub trivia: Trivia,
}

/// A `property "name" { for all <bindings> [where <pred>] { body } }` block
/// inside a suite (v0.114, testing track slice 2, ADR 0149). The generative
/// sibling of [`Case`]: the runner draws inhabitants of each binding's type from
/// its refinement domain and evaluates the body's `expect`s over them.
#[derive(Debug, Clone)]
pub struct PropertyDecl {
    /// The property name, taken from the string literal.
    pub name: String,
    /// The span of the string literal — used for diagnostics and reports.
    pub name_span: Span,
    /// The `for all` binder: the generated bindings, an optional `where` filter,
    /// and the predicate body.
    pub forall: ForAll,
    pub documentation: Option<Doc>,
    pub span: Span,
    pub trivia: Trivia,
}

/// The `for all x: T, … [where <pred>] { … }` binder inside a [`PropertyDecl`].
#[derive(Debug, Clone)]
pub struct ForAll {
    /// The generated bindings, `x: T` (one or more).
    pub bindings: Vec<ForAllBinding>,
    /// An optional `where <pred>` filter (a pure `Bool`) applied to generated
    /// tuples before the body runs.
    pub where_pred: Option<Expr>,
    /// The body — one or more statements, typically `expect`s.
    pub body: Block,
    pub span: Span,
}

/// One `for all` binding: `name: T`, where the runner generates inhabitants of
/// `T` from its refinements.
#[derive(Debug, Clone)]
pub struct ForAllBinding {
    pub name: Ident,
    pub type_ref: TypeRef,
}

/// A capability reference in a `given` clause (v0.15 §3.2). A bare name is a
/// local capability (`given Cap`); a dotted name refers to a capability a
/// consumed context provides (`given B.Cap` / `given Alias.Cap`).
#[derive(Debug, Clone)]
pub struct CapRef {
    /// `None` for a local capability; `Some(prefix)` for a cross-context
    /// reference where `prefix` is a consumed-context qualified name or alias.
    pub context: Option<QualifiedName>,
    /// The capability's simple name (also the local deps key).
    pub name: Ident,
    pub span: Span,
}

impl CapRef {
    /// The local deps key / capability simple name (e.g. `Clock`).
    pub fn key(&self) -> &str {
        &self.name.name
    }

    /// True when this references a capability provided by a consumed context.
    pub fn is_cross_context(&self) -> bool {
        self.context.is_some()
    }

    /// The cross-context prefix (consumed-context qualified name or alias) as
    /// a dotted string, if any.
    pub fn prefix(&self) -> Option<String> {
        self.context.as_ref().map(|q| q.joined())
    }
}

/// A dotted name like `fitness.units`.
#[derive(Debug, Clone)]
pub struct QualifiedName {
    pub parts: Vec<Ident>,
    pub span: Span,
}

impl QualifiedName {
    pub fn joined(&self) -> String {
        self.parts
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>()
            .join(".")
    }
}

// Finding #31 shrank `Expr`/`ExprKind` enough that clippy's variance check
// between this enum's smallest and largest variants (`Service`/`Actor` vs.
// `Type`/`Fn`) now crosses its threshold — a pre-existing size profile made
// newly visible, not something #31 itself is scoped to fix. Boxing
// `ServiceDecl`/`ActorDecl` here is a separate, unscoped refactor (its own
// blast radius across every `CommonsItem::Service`/`Actor` construction and
// match site) left for a future finding.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum CommonsItem {
    Type(TypeDecl),
    Fn(FnDecl),
    /// `capability Name { fn op(...) -> T ... }` (v0.5; contexts only).
    Capability(CapabilityDecl),
    /// `provides Cap = ProviderName { fn op(...) -> T { ... } ... }` (v0.5).
    Provider(ProviderDecl),
    /// `service Name { on call(...) -> T { ... } ... }` (v0.5).
    Service(ServiceDecl),
    /// `agent Name { key id: T; state { ... }; on call ... }` (v0.5).
    Agent(AgentDecl),
    /// `actor Name { auth = Scheme, identity = T }` (v0.45). A nominal boundary
    /// contract consumed by a handler's `by` clause; not a runnable entity.
    Actor(ActorDecl),
    /// `messages <tag> @reference { "code" => "template" ... }` — a message
    /// bundle for one locale. Commons-only (checker-enforced, not grammar);
    /// legal syntactically wherever any `CommonsItem` is, per the existing
    /// `Service`/`Agent`-in-`adapter` precedent.
    Messages(MessagesDecl),
    /// `event Name = { fields }` (Events track, slice 0, spine #936).
    /// Context-only (checker-enforced, not grammar) — the mirror image of
    /// `Messages`' commons-only restriction, same mechanism.
    Event(EventDecl),
    /// `asset name = "path"` (#1883, slice 1a). A static file the platform
    /// serves beside the Worker. Context or commons ([DECISION G]); an
    /// adapter is rejected by the checker, not the grammar, like `Event`.
    Asset(AssetDecl),
    /// `embed name = "path"` (#2387, slice 1). A project file's text as a
    /// `String` constant. Context or commons; an adapter is rejected by the
    /// checker, not the grammar, like `Asset`.
    Embed(EmbedDecl),
}

impl CommonsItem {
    /// The declaring identifier, when the item is named by one. `Messages` is
    /// the sole `None`: its locale tag is a `LocaleTag` string literal
    /// (`"pt-BR"`), not an identifier, and synthesising an `Ident` from it
    /// would be a lie any identifier-shaped consumer (rename, go-to-def) would
    /// eventually surface.
    pub fn name(&self) -> Option<&Ident> {
        match self {
            CommonsItem::Type(t) => Some(&t.name),
            CommonsItem::Fn(f) => Some(f.name.ident()),
            CommonsItem::Capability(c) => Some(&c.name),
            CommonsItem::Provider(p) => Some(&p.provider_name),
            CommonsItem::Service(s) => Some(&s.name),
            CommonsItem::Agent(a) => Some(&a.name),
            CommonsItem::Actor(a) => Some(&a.name),
            CommonsItem::Messages(_) => None,
            CommonsItem::Event(e) => Some(&e.name),
            CommonsItem::Asset(a) => Some(&a.name),
            CommonsItem::Embed(e) => Some(&e.name),
        }
    }

    /// The whole declaration's span, from its first token to its last.
    pub fn span(&self) -> Span {
        match self {
            CommonsItem::Type(t) => t.span,
            CommonsItem::Fn(f) => f.span,
            CommonsItem::Capability(c) => c.span,
            CommonsItem::Provider(p) => p.span,
            CommonsItem::Service(s) => s.span,
            CommonsItem::Agent(a) => a.span,
            CommonsItem::Actor(a) => a.span,
            CommonsItem::Messages(m) => m.span,
            CommonsItem::Event(e) => e.span,
            CommonsItem::Asset(a) => a.span,
            CommonsItem::Embed(e) => e.span,
        }
    }
}

/// `asset name = "path"` (#1883, slice 1a): one static file, served by the
/// platform at a content-hashed URL and never reaching the Worker, so public
/// by construction. `name` is a value of the built-in type `Asset`
/// ([DECISION H]), whose `.url` is that URL.
///
/// `path` is the literal's value with escapes applied, relative to the
/// project root. The checker rejects a path that is absolute or contains a
/// `..` segment (`bynk.asset.unsafe_path`) or names a file outside the
/// project's `assets/` folder (`bynk.asset.outside_assets_dir`), and one
/// whose file does not exist there (`bynk.asset.missing_file`).
#[derive(Debug, Clone)]
pub struct AssetDecl {
    pub name: Ident,
    pub path: String,
    pub path_span: Span,
    /// `at "/favicon.ico"` (#1883, slice 2, [DECISION C]): a fixed URL path
    /// to serve the file at, for clients that fetch a file by name without
    /// reading a page (`favicon.ico`, `robots.txt`). `None` serves it at a
    /// content-hashed path under `/_assets/`. With its span, for diagnostics.
    pub at: Option<(String, Span)>,
    pub documentation: Option<Doc>,
    pub span: Span,
    pub trivia: Trivia,
}

/// `embed name = "path"` (#2387, slice 1): one project file whose text is the
/// value of `name`, a `String` constant compiled into the code. Unlike an
/// [`AssetDecl`] it is never served, so it is private and comes from the
/// project's `embeds/` folder rather than `assets/`.
///
/// `path` is the literal's value with escapes applied, relative to the
/// project root. The checker rejects a path that is absolute or contains a
/// `..` segment (`bynk.embed.unsafe_path`) or names a file outside the
/// project's `embeds/` folder (`bynk.embed.outside_embeds_dir`).
#[derive(Debug, Clone)]
pub struct EmbedDecl {
    pub name: Ident,
    pub path: String,
    pub path_span: Span,
    pub documentation: Option<Doc>,
    pub span: Span,
    pub trivia: Trivia,
}

/// One locale's message bundle (v0.222+): `messages "<tag>" @reference { ... }`.
/// `tag` is a `LocaleTag` string literal (like an entry's `code`/`template`);
/// its refinement (`bynk.locale.types`) is checked by `check_messages_bundles`,
/// which reports `bynk.messages.invalid_locale_tag` for a tag the pattern
/// rejects.
#[derive(Debug, Clone)]
pub struct MessagesDecl {
    pub tag: String,
    pub tag_span: Span,
    /// Every `@`-annotation attached to this block. The parser stays
    /// permissive (zero or more, same as `store` field annotations); cardinality
    /// (exactly one `@reference` per bundle, counted across every `Messages`
    /// item in the commons) is a checker concern, not a parse error.
    pub annotations: Vec<Annotation>,
    pub entries: Vec<MessageEntry>,
    pub documentation: Option<Doc>,
    pub span: Span,
    pub trivia: Trivia,
}

/// One `"code" => "template"` entry inside a `messages` block. Both sides are
/// plain string literals — a template's `{name}` placeholders are resolved by
/// a compile-time string scan during lowering, not parsed as expressions.
#[derive(Debug, Clone)]
pub struct MessageEntry {
    pub code: String,
    pub code_span: Span,
    pub template: String,
    pub template_span: Span,
    pub span: Span,
    /// #1888: the doc above the entry, in either form.
    pub documentation: Option<Doc>,
}

/// A capability declaration (v0.5 §3.3). Capabilities are interface-like
/// contracts for external dependencies, used inside contexts. They may only
/// appear inside a `context` declaration.
#[derive(Debug, Clone)]
pub struct CapabilityDecl {
    pub name: Ident,
    /// Empty for a derived capability ([`CapabilityDecl::derived`]), whose
    /// operations are its base's; the checker resolves them.
    pub ops: Vec<CapabilityOp>,
    /// Standard-library S4a (#1966): `capability Sessions = Kv @option(…)`, a
    /// distinct instance of a platform capability the unit consumes. `None` for
    /// the braced `capability X { fn … }` form.
    pub derived: Option<DerivedCapability>,
    pub documentation: Option<Doc>,
    pub span: Span,
    /// #1756: comments before the closing `}`, an orphaned doc block among
    /// them, so the formatter keeps them.
    pub trailing_comments: Vec<Comment>,
    pub trivia: Trivia,
}

/// The `= Base @option(…)*` of a derived capability (S4a, #1966): a new
/// capability name whose operations are those of `base`, a platform capability
/// the unit selects with `consumes U { Base }`, provided by the platform
/// adapter's own provider bound to this instance.
#[derive(Debug, Clone)]
pub struct DerivedCapability {
    /// The base capability, by its flattened (bare) name.
    pub base: Ident,
    /// Creation options for the instance, checked against the base kind's
    /// closed set.
    pub annotations: Vec<Annotation>,
}

/// One operation in a capability (signature only; no body).
#[derive(Debug, Clone)]
pub struct CapabilityOp {
    pub name: Ident,
    /// #926: `[T, …]` type parameters on the op itself; empty for a
    /// non-generic op. Resolved only from an explicit type argument at the
    /// call site (`Cap.op[Some](…)`) — never inferred.
    pub type_params: Vec<TypeParam>,
    pub params: Vec<Param>,
    pub return_type: TypeRef,
    pub documentation: Option<Doc>,
    pub span: Span,
    pub trivia: Trivia,
}

/// A provider declaration (v0.5 §3.4). Supplies an implementation for a
/// capability.
#[derive(Debug, Clone)]
pub struct ProviderDecl {
    /// The capability being implemented.
    pub capability: Ident,
    /// The provider's identifier (used in tests/config to select impls).
    pub provider_name: Ident,
    /// v0.12: capabilities this provider depends on (`provides X = Impl given
    /// Y, Z { … }`). The provider's operation bodies may use these. v0.15:
    /// a dependency may be a cross-context capability (`given B.Cap`).
    pub given: Vec<CapRef>,
    pub ops: Vec<ProviderOp>,
    /// v0.17: an *external* provider — `provides Cap = Name` with **no** brace
    /// block — inside an adapter, supplied by the adapter's binding rather than
    /// a Bynk body. When `true`, `ops` is empty and the emitter produces no
    /// class. The absence of the brace block (not an empty one) is the signal.
    pub external: bool,
    pub documentation: Option<Doc>,
    pub span: Span,
    pub trivia: Trivia,
}

/// One operation in a provider (signature plus body).
#[derive(Debug, Clone)]
pub struct ProviderOp {
    pub name: Ident,
    pub params: Vec<Param>,
    pub return_type: TypeRef,
    pub body: Block,
    pub span: Span,
    pub trivia: Trivia,
}

/// A service declaration (v0.5 §3.5). Services are the boundary interface
/// of a context.
#[derive(Debug, Clone)]
pub struct ServiceDecl {
    pub name: Ident,
    /// The protocol the service conforms to, from the `from <protocol>` header
    /// clause (v0.44). `Call` when there is no clause.
    pub protocol: ServiceProtocol,
    /// The optional service-level `by` default (v0.155) — a `by <Actor>` clause on
    /// the service header, `service Api from http by v: Visitor { … }`. Every
    /// handler that omits its own `by` inherits this one (injected by the
    /// normalization pass). `None` when absent — handlers then fall back to the
    /// per-protocol default actor (HTTP/WebSocket have none, so `by` stays
    /// mandatory there). The "public / bearer-authed" fact is usually a service
    /// fact, so this removes the per-handler repetition.
    pub default_by: Option<ByClause>,
    /// The optional service-level `given` default (v0.155) — a `given C1, C2`
    /// clause on the service header, following the `by` default. Every handler
    /// that declares no `given` of its own inherits this list. Empty when absent.
    pub default_given: Vec<CapRef>,
    /// The optional cross-origin (CORS) policy (v0.131, ADR 0159) — a `cors { }`
    /// section in the service body, only meaningful on a `from http` service.
    /// `None` when absent (same-origin default, byte-for-byte unchanged output).
    pub cors: Option<CorsPolicy>,
    /// The optional security-headers policy (v0.141, ADR 0164) — a `security { }`
    /// section in the service body, only meaningful on a `from http` service.
    /// `None` when absent, but unlike `cors` the *absence* still stamps the safe
    /// defaults (`nosniff` on) — the emitter synthesises a default policy for every
    /// `from http` service, so `None` here means "defaults", not "no headers".
    pub security: Option<SecurityPolicy>,
    /// The optional request-body-size policy (v0.142, ADR 0165) — a `limits { }`
    /// section in the service body, only meaningful on a `from http` service. It
    /// declares a per-service `maxBody` ceiling (in bytes) for the service's
    /// body-taking routes; a route may override it with `@limit(maxBody: …)`.
    /// `None` when absent (no cap — byte-for-byte unchanged output, the opt-in
    /// CORS posture, not the `security` default-on posture).
    pub limits: Option<LimitsPolicy>,
    pub handlers: Vec<Handler>,
    pub documentation: Option<Doc>,
    pub span: Span,
    /// #1756: comments before the closing `}`, an orphaned doc block among
    /// them, so the formatter keeps them.
    pub trailing_comments: Vec<Comment>,
    pub trivia: Trivia,
}

/// A cross-origin resource-sharing policy on a `from http` service (v0.131,
/// ADR 0159): the `cors { }` section in the service body. Parsed leniently as a
/// list of `name: value` fields (the grammar accepts any field name — an unknown
/// one is a checker diagnostic, per the `@`-annotation precedent, ADR 0111), and
/// interpreted through the typed accessors below.
///
/// `Access-Control-Allow-Methods` is deliberately **not** a field — it is derived
/// from the service's routes at emit time (the routes already enumerate the
/// methods; a restated list would drift). Likewise `Allow-Headers` defaults to
/// `content-type` (+ `Authorization` when a Bearer route exists) and is only
/// stored here when the author overrides it.
#[derive(Debug, Clone)]
pub struct CorsPolicy {
    /// The `cors { }` fields as written, in source order. Field names are
    /// validated against the closed set (`origins`/`headers`/`credentials`/
    /// `maxAge`) by the checker, not the parser.
    pub fields: Vec<CorsField>,
    pub span: Span,
    /// #1786: comments before the closing `}`, an orphaned doc block among
    /// them.
    pub trailing_comments: Vec<Comment>,
    pub trivia: Trivia,
}

/// One `name: value` field inside a `cors { }` policy (v0.131).
#[derive(Debug, Clone)]
pub struct CorsField {
    pub name: Ident,
    pub value: Expr,
    pub span: Span,
    /// #1786: the field's own comments, above it and at the end of its line.
    pub trivia: Trivia,
}

impl CorsPolicy {
    /// The raw value expression for a field, by name (the last one wins if a
    /// field is repeated — the checker flags the duplicate separately).
    pub fn field(&self, name: &str) -> Option<&Expr> {
        self.fields
            .iter()
            .rev()
            .find(|f| f.name.name == name)
            .map(|f| &f.value)
    }

    /// The allowed origins — the string literals of the `origins:` list. An
    /// absent or malformed field yields an empty list (the checker has already
    /// reported the shape error; the emitter fails closed on an empty list).
    pub fn origins(&self) -> Vec<String> {
        Self::str_list(self.field("origins")).unwrap_or_default()
    }

    /// `true` iff `origins` is exactly the wildcard `["*"]`.
    pub fn is_wildcard(&self) -> bool {
        let os = self.origins();
        os.len() == 1 && os[0] == "*"
    }

    /// Whether credentialed requests are allowed (`credentials: true`); defaults
    /// to `false` when the field is absent.
    pub fn credentials(&self) -> bool {
        matches!(
            self.field("credentials").map(|e| &e.kind),
            Some(ExprKind::BoolLit(true))
        )
    }

    /// The explicit `Access-Control-Allow-Headers` override, if the author gave
    /// a `headers:` list; `None` leaves the emitter to apply its smart default.
    pub fn allow_headers(&self) -> Option<Vec<String>> {
        self.field("headers").and_then(Self::str_list_of)
    }

    /// The `Access-Control-Max-Age` in whole seconds, if a `maxAge:` duration was
    /// given; `None` leaves the header off (the browser default).
    pub fn max_age_secs(&self) -> Option<i64> {
        match self.field("maxAge").map(|e| &e.kind) {
            Some(ExprKind::DurationLit { millis, .. }) => Some(millis / 1_000),
            _ => None,
        }
    }

    /// Interpret an expression as a list of string literals, if it is one.
    fn str_list(expr: Option<&Expr>) -> Option<Vec<String>> {
        expr.and_then(Self::str_list_of)
    }

    fn str_list_of(expr: &Expr) -> Option<Vec<String>> {
        match &expr.kind {
            ExprKind::ListLit(items) => items
                .iter()
                .map(|e| match &e.kind {
                    ExprKind::StrLit(s) => Some(s.clone()),
                    _ => None,
                })
                .collect(),
            _ => None,
        }
    }
}

/// A security-headers policy on a `from http` service (v0.141, ADR 0164): the
/// `security { }` section in the service body. Parsed leniently as a list of
/// `name: value` fields (an unknown one is a checker diagnostic, per the CORS /
/// `@`-annotation precedent) and interpreted through the typed accessors below.
///
/// The closed set is `nosniff` (a `Bool`, default `true` — stamps
/// `X-Content-Type-Options: nosniff`) and `hsts` (a positive `Duration`, opt-in —
/// stamps `Strict-Transport-Security: max-age=…`). Unlike `cors`, the *safe*
/// header is on by default: a `from http` service with no `security { }` still
/// stamps `nosniff`, because a security header you have to remember to switch on
/// is the one you forget (ADR 0164 DECISION A).
#[derive(Debug, Clone)]
pub struct SecurityPolicy {
    /// The `security { }` fields as written, in source order. Field names are
    /// validated against the closed set (`hsts`/`nosniff`) by the checker, not
    /// the parser.
    pub fields: Vec<SecurityField>,
    pub span: Span,
    /// #1786: comments before the closing `}`, an orphaned doc block among
    /// them.
    pub trailing_comments: Vec<Comment>,
    pub trivia: Trivia,
}

/// One `name: value` field inside a `security { }` policy (v0.141).
#[derive(Debug, Clone)]
pub struct SecurityField {
    pub name: Ident,
    pub value: Expr,
    pub span: Span,
    /// #1786: the field's own comments, above it and at the end of its line.
    pub trivia: Trivia,
}

impl SecurityPolicy {
    /// The raw value expression for a field, by name (the last one wins if a
    /// field is repeated — the checker flags the duplicate separately).
    pub fn field(&self, name: &str) -> Option<&Expr> {
        self.fields
            .iter()
            .rev()
            .find(|f| f.name.name == name)
            .map(|f| &f.value)
    }

    /// Whether `X-Content-Type-Options: nosniff` is stamped. Defaults to `true`
    /// (the safe default, ADR 0164 DECISION A); only an explicit `nosniff: false`
    /// opts out. A malformed value has already been reported by the checker; it
    /// falls back to the safe default here.
    pub fn nosniff(&self) -> bool {
        !matches!(
            self.field("nosniff").map(|e| &e.kind),
            Some(ExprKind::BoolLit(false))
        )
    }

    /// The `Strict-Transport-Security` `max-age` in whole seconds, if the author
    /// opted in with an `hsts:` duration; `None` leaves HSTS off (the default —
    /// HSTS pins the browser to HTTPS and is a deliberate opt-in, DECISION A).
    pub fn hsts_max_age_secs(&self) -> Option<i64> {
        match self.field("hsts").map(|e| &e.kind) {
            Some(ExprKind::DurationLit { millis, .. }) => Some(millis / 1_000),
            _ => None,
        }
    }
}

/// A request-body-size policy on a `from http` service (v0.142, ADR 0165): the
/// `limits { }` section in the service body. Parsed leniently as a list of
/// `name: value` fields (an unknown one is a checker diagnostic, per the CORS /
/// `security` / `@`-annotation precedent) and interpreted through the typed
/// accessor below.
///
/// The closed set is `maxBody` — a positive `Int` byte count (there is no byte
/// `Size` literal yet; a `1.mb`-style literal is a named follow-on, the
/// `Duration` playbook). Unlike `security`, this is opt-in: a service with no
/// `limits { }` (and no route `@limit`) has no cap and emits byte-for-byte
/// unchanged output (ADR 0165 DECISION E — the CORS posture).
#[derive(Debug, Clone)]
pub struct LimitsPolicy {
    /// The `limits { }` fields as written, in source order. Field names are
    /// validated against the closed set (`maxBody`) by the checker, not the
    /// parser.
    pub fields: Vec<LimitsField>,
    pub span: Span,
    /// #1786: comments before the closing `}`, an orphaned doc block among
    /// them.
    pub trailing_comments: Vec<Comment>,
    pub trivia: Trivia,
}

/// One `name: value` field inside a `limits { }` policy (v0.142).
#[derive(Debug, Clone)]
pub struct LimitsField {
    pub name: Ident,
    pub value: Expr,
    pub span: Span,
    /// #1786: the field's own comments, above it and at the end of its line.
    pub trivia: Trivia,
}

impl LimitsPolicy {
    /// The raw value expression for a field, by name (the last one wins if a
    /// field is repeated — the checker flags the duplicate separately).
    pub fn field(&self, name: &str) -> Option<&Expr> {
        self.fields
            .iter()
            .rev()
            .find(|f| f.name.name == name)
            .map(|f| &f.value)
    }

    /// The service-wide maximum request-body size in bytes, if the author gave a
    /// positive `maxBody:` `Int` literal; `None` leaves the service without a
    /// default cap. A malformed or non-positive value has already been reported
    /// by the checker; it falls back to `None` here (no cap).
    pub fn max_body(&self) -> Option<i64> {
        match self.field("maxBody").map(|e| &e.kind) {
            Some(ExprKind::IntLit { value, .. }) if *value > 0 => Some(*value),
            _ => None,
        }
    }
}

/// The protocol a service conforms to — declared on the header via
/// `from <protocol>` (v0.44). `Call` is the default (no `from` clause): a
/// contract-mediated internal-RPC surface, not a wire protocol. Multi-endpoint
/// protocols (`Http`, `Cron`) carry no binding — the endpoint lives on each
/// handler; single-binding `Queue` carries its queue name.
#[derive(Debug, Clone)]
pub enum ServiceProtocol {
    /// No `from` clause: the service holds `on call` handlers only.
    Call,
    /// `from http` — many routes; each handler is `on <Method>("route")`.
    Http,
    /// `from cron` — many schedules; each handler is `on schedule("expr")`.
    Cron,
    /// `from queue("name")` — one bound queue; handlers are `on message(...)`.
    Queue { name: String },
    /// `from websocket(in: ClientFrame, out: ServerFrame)` — a held WebSocket
    /// connection (v0.103, real-time track slice 3). `in_type` is the inbound
    /// frame type (client→server, decoded and routed as typed agent messages);
    /// `out_type` is the server→client frame type the held `Connection[out_type]`
    /// carries. The service holds exactly one `on open` handler (edge auth via
    /// `by`, then transfer of the connection to an agent).
    WebSocket { in_type: TypeRef, out_type: TypeRef },
    /// `from Events(E)` or `from Events(E { field: value, .. })`, optionally
    /// followed by `via schema(N)` — a subscriber to event type `E`,
    /// optionally filtered by a structural payload pattern (Events track,
    /// slice 0 spine #936; the pattern is slice 1) and/or the envelope's
    /// `schemaVersion` (slice 4). `Events`, capitalised, is matched as plain
    /// `Ident` text the same way `websocket` is — it names the `Events`
    /// capability directly (every first-party capability is already an
    /// unreserved PascalCase identifier), not a built-in type name, so no
    /// lexer reservation. `pattern` and `schema_dispatch` are independent:
    /// a service may carry either, both, or neither.
    Events {
        event_type: TypeRef,
        pattern: Option<EventPattern>,
        schema_dispatch: Option<SchemaDispatch>,
    },
}

/// An agent declaration (v0.5 §3.6). Agents are state-bearing entities
/// with their own handlers.
#[derive(Debug, Clone)]
pub struct AgentDecl {
    pub name: Ident,
    /// State-migrations S3 (#2248): `agent New replaces Old` — the class
    /// `Old` is renamed to this one, its stored data moving with it.
    pub replaces: Option<Ident>,
    /// `key id: Type` — the identifier-typed value identifying instances.
    pub key_name: Ident,
    pub key_type: TypeRef,
    /// #1788: comments above the `key` line and at its end. A comment on the
    /// agent's `{` line leads the key too.
    pub key_trivia: Trivia,
    /// `store` fields (v0.81, storage track) — each an access-pattern slot of a
    /// declared storage kind (`Cell`/`Map`/…). The successor to the removed
    /// `state { }` record (ADR 0108); every agent declares its state this way.
    pub store_fields: Vec<StoreField>,
    /// Invariants (v0.80 §14) — universally-quantified predicates over the
    /// agent's `store` fields. The phase sits between the fields and the
    /// handlers; each is checked against the state staged by a handler's writes
    /// before it commits.
    pub invariants: Vec<Invariant>,
    /// Step invariants (v0.116 §, testing track slice 4) — named predicates over
    /// the pre-/post-commit state *pair* (`old`/`new`), checked at the commit
    /// boundary beside [`invariants`], from the second commit onward. Widen the
    /// invariant subject from a snapshot to a step (ADR 0144 — one predicate
    /// surface).
    ///
    /// [`invariants`]: AgentDecl::invariants
    pub transitions: Vec<Transition>,
    /// State-migrations S2 (#2224, track §3.3): `migrate from N (old: T) { … }`
    /// steps, each carrying a stored record from lock version `N` to the
    /// current shape. Declared in the phase beside invariants and transitions.
    pub migrations: Vec<MigrateDecl>,
    pub handlers: Vec<Handler>,
    pub documentation: Option<Doc>,
    pub span: Span,
    /// #1756: comments before the closing `}`, an orphaned doc block among
    /// them, so the formatter keeps them.
    pub trailing_comments: Vec<Comment>,
    pub trivia: Trivia,
}

/// A migration step (state-migrations S2, #2224): `migrate from N (old: T) {
/// … }`. `old` names a record type declaring the version-`N` fields the step
/// reads; the body assigns current `store` fields with record-literal
/// semantics (each at most once, order-free, a bare field reads its carried
/// value) and acknowledges removed or uncovered fields with `drop`.
#[derive(Debug, Clone)]
pub struct MigrateDecl {
    /// The `migrate` keyword.
    pub keyword: Span,
    /// The lock version `N` this step reads, and its literal's span.
    pub from: i64,
    pub from_span: Span,
    /// `(old: T)` — absent for a step that reads nothing.
    pub old: Option<MigrateOld>,
    pub items: Vec<MigrateItem>,
    /// #1756-style: comments before the body's closing `}`.
    pub trailing_comments: Vec<Comment>,
    pub documentation: Option<Doc>,
    pub span: Span,
    pub trivia: Trivia,
}

/// A step's `(old: T)` parameter.
#[derive(Debug, Clone)]
pub struct MigrateOld {
    pub name: Ident,
    pub type_ref: TypeRef,
}

/// One item of a migration step's body.
// A step holds a handful of items, so the size gap between `Let`/`Assign` and
// the small `Drop` costs nothing worth boxing every consumer's match for.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum MigrateItem {
    /// `let x = e` — a pure local binding, visible to later items.
    Let(LetStmt),
    /// `f := e` — the current `store` field `f` takes `e`.
    Assign(AssignStmt),
    /// `drop f` — the stored value of `f` is discarded on purpose.
    Drop(MigrateDrop),
}

/// `drop f` inside a migration step.
#[derive(Debug, Clone)]
pub struct MigrateDrop {
    pub keyword: Span,
    pub field: Ident,
    pub span: Span,
    pub trivia: Trivia,
}

/// A `store` field (v0.81, storage track). Each is an access-pattern slot of a
/// declared storage kind: `store <name>: <Kind>[…] [@annotations] [= <init>]`.
/// The kind and its element type are carried as an ordinary [`TypeRef`]
/// (`Cell[Int]`, `Map[K, V]`); the checker restricts which heads are storage
/// kinds. Access-pattern annotations (`@indexed`, …) parse into [`annotations`]
/// (v0.85, ADR 0111); the checker validates them against the closed registry.
///
/// [`annotations`]: StoreField::annotations
#[derive(Debug, Clone)]
pub struct StoreField {
    pub name: Ident,
    /// The storage kind and its element type(s): `Cell[Int]`, `Map[K, V]`. A
    /// dedicated [`StoreKind`] rather than a [`TypeRef`] — storage kinds are not
    /// value types, and the checker dispatches kind-aware operations on the head.
    pub kind: StoreKind,
    /// Storage annotations on the field (v0.85, ADR 0111): `@ttl(5.minutes)`,
    /// `@indexed(by: orderId)`. Parsed in declaration order (after the kind,
    /// before the initialiser); the checker validates names against the closed
    /// registry and gates each to the slice that implements it.
    pub annotations: Vec<Annotation>,
    /// The fresh-key initial value (`= expr`), if given — same disposition as a
    /// `state` field's initialiser (ADRs 0003/0004 carry forward).
    pub init: Option<Expr>,
    pub documentation: Option<Doc>,
    pub span: Span,
    pub trivia: Trivia,
}

/// A storage annotation on a `store` field (v0.85, storage track; ADR 0111):
/// `@<name>(<args>)`. The `name` is matched against the closed registry
/// (`@indexed`/`@ttl`/`@retain`/`@bounded`) by the checker; the grammar accepts
/// any identifier so an unknown name is a checker diagnostic, not a parse error.
/// Arguments are compile-time metadata, restricted to literals (and the `by:`
/// field-name labels of `@indexed`) by the checker per ADR 0111 D4.
#[derive(Debug, Clone)]
pub struct Annotation {
    pub name: Ident,
    pub args: Vec<AnnotationArg>,
    pub span: Span,
}

/// A single annotation argument (v0.85; ADR 0111): an optional `label:` followed
/// by a value expression — `by: orderId` (labelled) or `5.minutes` (positional).
/// The value is parsed as an ordinary [`Expr`] so the duration-literal form
/// (`5.minutes`, landing with the `Duration` slice) needs no special grammar;
/// the checker restricts it to a literal where the annotation is functional.
#[derive(Debug, Clone)]
pub struct AnnotationArg {
    pub label: Option<Ident>,
    pub value: Expr,
    pub span: Span,
}

/// A storage kind applied to its element type(s) (v0.81): `Cell[Int]`,
/// `Map[ReservationId, Reservation]`. The `head` is the kind name (`Cell`,
/// `Map`, `Set`, `Log`, `Queue`, `Cache`); the checker validates it against the
/// closed catalogue. Element types are ordinary [`TypeRef`]s. Refined element
/// types (`Cell[Int where NonNegative]`) ride a later slice (parse_type_ref does
/// not yet accept an inline refinement in type-argument position).
#[derive(Debug, Clone)]
pub struct StoreKind {
    pub head: Ident,
    pub args: Vec<TypeRef>,
    pub span: Span,
}

/// An agent invariant (v0.80 §14). A named predicate over the agent's state
/// fields that must hold of every committed state; a commit that would violate
/// it faults (`InvariantViolation`) before the state is persisted. The
/// predicate references state fields by bare name, mirroring the design-notes
/// worked examples (`status == Paid implies paymentRef.isSome()`).
#[derive(Debug, Clone)]
pub struct Invariant {
    pub name: Ident,
    /// The predicate expression — an ordinary `Bool`-typed expression over the
    /// state fields, plus `implies` and `is`. The parsed-predicate-on-a-
    /// declaration shape mirrors [`ActorRefinement::predicate`].
    pub predicate: Expr,
    pub documentation: Option<Doc>,
    pub span: Span,
    pub trivia: Trivia,
}

/// An agent step invariant (v0.116 §, testing track slice 4). A named predicate
/// over the *pair* of committed states — the pre-commit `old` and the proposed
/// `new`, each the agent's state record — that must hold of every state move; a
/// commit that would violate it faults (`InvariantViolation`) before the state is
/// persisted, exactly as a snapshot [`Invariant`] does. Widens the invariant
/// subject from a snapshot to a step (ADR 0144 — one predicate surface); the
/// predicate reuses the invariant surface (`implies`/`is`/pure methods) with
/// `old`/`new` bound contextually (`old.status is Paid implies new.status is
/// Paid`).
#[derive(Debug, Clone)]
pub struct Transition {
    pub name: Ident,
    /// The predicate expression — an ordinary `Bool`-typed expression over the
    /// `old` and `new` state records, with `implies`/`is` and pure methods,
    /// mirroring [`Invariant`].
    pub predicate: Expr,
    pub documentation: Option<Doc>,
    pub span: Span,
    pub trivia: Trivia,
}

/// A function contract clause (v0.115 §, testing track slice 3). A named
/// predicate on a `fn` signature — a `requires` (precondition) or `ensures`
/// (postcondition). A contract is the invariant predicate attached to a
/// function (ADR 0144 — one predicate surface): the predicate is a pure `Bool`
/// expression over the parameters (`requires`) or the parameters plus `result`
/// (`ensures`), with `implies`/`is` and pure methods, mirroring [`Invariant`].
/// The name rides the failure report and the redundant-test dedup.
#[derive(Debug, Clone)]
pub struct Contract {
    pub name: Ident,
    /// The predicate expression — an ordinary `Bool`-typed expression over the
    /// parameters (and, for an `ensures`, the contextual `result` binding).
    pub predicate: Expr,
    pub span: Span,
}

/// An actor declaration (v0.45 §3.7). An actor is a nominal *contract type*
/// describing an external party at a boundary — not a runnable entity. A
/// handler consumes an actor on its `by` clause; the boundary verifies the
/// declared `auth` scheme and mints a sealed identity (`name.identity`).
#[derive(Debug, Clone)]
pub struct ActorDecl {
    pub name: Ident,
    /// The authentication scheme from `auth = <Scheme>`, stored as the raw
    /// identifier. The checker classifies it: `None`/`Internal`/`Bearer` are
    /// admitted; `Signature` is reserved-and-rejected
    /// (`bynk.actor.scheme_unsupported`); anything else is
    /// `bynk.actor.unknown_scheme`. `None` for the refinement form.
    pub auth: Option<Ident>,
    /// The scheme's keyed config from `auth = Scheme(key = value, …)` (v0.47
    /// `Bearer(secret = "…")`; v0.51 generalised for `Signature(secret, header,
    /// timestamp?, tolerance?)`). Empty for schemes/forms with no config. The
    /// checker validates which keys each scheme requires/allows.
    pub auth_config: Vec<SchemeArg>,
    /// The optional identity type from `, identity = <T>`. Absent ⇒ the
    /// scheme default (`()` for `None`; a sealed `CallerId` for the `Internal`
    /// `on call` channel, `()` for other `Internal` channels).
    pub identity: Option<TypeRef>,
    /// The refinement form `actor Admin = Base where <predicate>` — narrows a
    /// base actor by an authorisation claim (ADR 0091). The predicate is parsed
    /// as a full expression; a static-semantics rule restricts it to the closed
    /// actor-claim catalogue (`hasClaim`/`claimEquals` over a `Bearer` base;
    /// `bynk.actor.refinement_predicate_unsupported` / `…_base_unsupported`).
    pub refinement: Option<ActorRefinement>,
    pub documentation: Option<Doc>,
    pub span: Span,
    pub trivia: Trivia,
    /// #1797: comments above the `auth` entry and at the end of its line. A
    /// comment on the actor's `{` line leads `auth` too. Empty for the
    /// refinement form.
    pub auth_trivia: Trivia,
    /// #1797: comments above the `identity` entry and at the end of its line.
    /// Empty when there is no `identity`.
    pub identity_trivia: Trivia,
    /// #1797: comments before the actor body's closing `}`.
    pub trailing_comments: Vec<Comment>,
}

impl ActorDecl {
    /// The value of a scheme config arg by key, if present (e.g. `secret`,
    /// `header`).
    pub fn scheme_arg(&self, key: &str) -> Option<&SchemeArg> {
        self.auth_config.iter().find(|a| a.key.name == key)
    }
}

/// One `key = value` argument in a scheme config (`Scheme(key = value, …)`).
#[derive(Debug, Clone)]
pub struct SchemeArg {
    pub key: Ident,
    pub value: SchemeArgValue,
    /// Span of the value, for diagnostics.
    pub span: Span,
}

/// A scheme config arg value — a string literal or an integer.
#[derive(Debug, Clone)]
pub enum SchemeArgValue {
    Str(String),
    Int(i64),
}

impl SchemeArgValue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            SchemeArgValue::Str(s) => Some(s),
            SchemeArgValue::Int(_) => None,
        }
    }
    pub fn as_int(&self) -> Option<i64> {
        match self {
            SchemeArgValue::Int(n) => Some(*n),
            SchemeArgValue::Str(_) => None,
        }
    }
}

/// The reserved refinement form `actor Admin = User where <predicate>` (Q3).
/// Parsed in Foundations so the grammar is fixed; admission is a later slice.
#[derive(Debug, Clone)]
pub struct ActorRefinement {
    /// The base actor being refined.
    pub base: Ident,
    /// The `where` predicate. Parsed but not yet checked.
    pub predicate: Expr,
    pub span: Span,
}

/// The `by (<binder>:)? <Actor>` clause on a handler (v0.45; binder optional in
/// v0.50). Names the actor contract the handler consumes; when a `binder` is
/// given, the verified identity binds to it and is read as `binder.identity`.
/// Omitting the binder (`by <Actor>`) declares-and-verifies the contract without
/// capturing the identity — for anonymous or verify-and-discard handlers. Sits
/// after the protocol config and before the parameters.
#[derive(Debug, Clone)]
pub struct ByClause {
    /// The identity binder, if the handler consumes the identity. `None` for the
    /// binder-less `by <Actor>` form. Required when `actors` names more than one
    /// (a sum is resolved by matching on the bound actor).
    pub binder: Option<Ident>,
    /// The actor contract(s) referenced — each a local actor decl or a prelude
    /// actor. A single name is the ordinary single-actor handler; more than one
    /// (`by who: A | B`, v0.52) is an **ordered sum of peer actors** resolved
    /// first-wins, the body matching on the resolved actor. Always non-empty.
    pub actors: Vec<Ident>,
    pub span: Span,
}

impl ByClause {
    /// The first (and, for a single-actor handler, only) actor contract named.
    pub fn primary(&self) -> &Ident {
        &self.actors[0]
    }
    /// Whether this `by` clause names an ordered sum of peer actors (`A | B`).
    pub fn is_sum(&self) -> bool {
        self.actors.len() > 1
    }
}

/// v0.182 (testing-the-boundary Slice A, #664): a call-site actor clause on a
/// test-body `let x <- <service address> by <Actor>(<identity>)`. Distinct from
/// [`ByClause`] (the handler/header form): the *declaration* names which actor
/// may call and binds the verified identity, whereas the *call site* names the
/// actor the case is acting as and supplies the identity value. A unit-identity
/// actor (`Visitor`, and cron/queue's internal actors) carries no `identity`.
#[derive(Debug, Clone)]
pub struct CallSiteActor {
    /// The actor the case acts as — a local actor decl or a prelude actor.
    pub actor: Ident,
    /// The supplied identity value (`"bob"` in `by User("bob")`), or `None` for a
    /// unit-identity actor written `by Visitor` with no argument.
    pub identity: Option<Box<Expr>>,
    /// #2380 (L5): the verified-token claims an `Oidc` actor's handler reads
    /// (`by Workload("id", TokenClaims.of([…]))`), at the in-process tiers.
    pub claims: Option<Box<Expr>>,
    pub span: Span,
}

/// A handler block — `on call(args) -> T given C1, C2 { body }`.
/// Used by both services and agents.
#[derive(Debug, Clone)]
pub struct Handler {
    pub kind: HandlerKind,
    /// Handler-position annotations (v0.140, ADR 0163): `@cache(maxAge: 5.minutes)`
    /// written immediately before `on <METHOD>(…)`. Reuses the [`Annotation`] AST
    /// shared with `store` fields (ADR 0111); the grammar accepts any `@name(args)`
    /// so an unknown name is a project-validation diagnostic, not a parse error. The
    /// first handler-position annotation surface — empty for every handler that
    /// carries none.
    pub annotations: Vec<Annotation>,
    /// For agent handlers, the method-style handler name (e.g.
    /// `on call addItem(...)`). For service handlers, this is None (just
    /// `on call(...)`).
    pub method_name: Option<Ident>,
    /// The `by <binder>: <Actor>` clause (v0.45), if present. Service handlers
    /// only; an absent clause inherits the protocol's default actor.
    pub by_clause: Option<ByClause>,
    pub params: Vec<Param>,
    pub return_type: TypeRef,
    pub given: Vec<CapRef>,
    pub body: Block,
    pub documentation: Option<Doc>,
    pub span: Span,
    pub trivia: Trivia,
}

impl Handler {
    /// The handler's own `@limit(maxBody:)` byte cap, if it carries a well-formed
    /// one (ADR 0165). The checker rejects a malformed `@limit`, so a bad value
    /// reads as no cap here.
    pub fn limit_max_body(&self) -> Option<i64> {
        let ann = self.annotations.iter().find(|a| a.name.name == "limit")?;
        ann.args
            .iter()
            .find_map(|arg| match (&arg.label, &arg.value.kind) {
                (Some(l), ExprKind::IntLit { value, .. }) if l.name == "maxBody" && *value > 0 => {
                    Some(*value)
                }
                _ => None,
            })
    }

    /// The media type a `@raw(contentType:)` handler declares: `Some` marks the
    /// handler's `body: Bytes` as the raw request body (request-side counterpart of
    /// ADR 0143). `"*/*"` accepts any content type. A malformed `@raw` is a checker
    /// error, so a missing or non-literal `contentType` reads as not raw here.
    pub fn raw_content_type(&self) -> Option<&str> {
        let ann = self.annotations.iter().find(|a| a.name.name == "raw")?;
        ann.args
            .iter()
            .find_map(|arg| match (&arg.label, &arg.value.kind) {
                (Some(l), ExprKind::StrLit(s)) if l.name == "contentType" && is_media_type(s) => {
                    Some(s.as_str())
                }
                _ => None,
            })
    }
}

/// Whether `s` is a media type a `@raw(contentType:)` may declare: `type/subtype`
/// of RFC 7230 token characters, or the wildcard `*/*`. Parameters (`; charset=…`)
/// are not part of the declaration.
pub fn is_media_type(s: &str) -> bool {
    fn token(t: &str) -> bool {
        !t.is_empty()
            && t.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
    }
    s == "*/*"
        || s.split_once('/')
            .is_some_and(|(t, sub)| token(t) && token(sub) && t != "*" && sub != "*")
}

/// `Hash` (P8.5, #1516): a `HandlerKind` value is a body-free, span-free
/// discriminant, added so a `DefId`-keyed handler identity could carry it
/// (`bynk-check`'s former `queries::HandlerDefId`, deleted by #1537 — kept
/// here because the derive is harmless, and a rebuild against ADR 0417's
/// shape would need it again). Purely additive; no existing
/// caller matches on hashing behaviour.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum HandlerKind {
    /// `on call(...)` — typed RPC (the only kind in v0.5).
    Call,
    /// `on http METHOD "path"` — external-facing HTTP route (v0.9).
    Http { method: HttpMethod, path: String },
    /// `on cron "expr"` — scheduled task; `expr` is a 5-field cron
    /// expression (v0.10a).
    Cron { expr: String },
    /// `on message(m: T)` — a message off the service's bound queue. The queue
    /// binding lives on the service's `ServiceProtocol::Queue` (v0.44).
    Message,
    /// `on open ...` — the WebSocket upgrade handler (v0.103, real-time track
    /// slice 3). Exactly one per `from websocket` service; carries a mandatory
    /// `by` clause (edge auth) and receives a fresh owned `Connection[out]`.
    Open,
    /// `on close ...` — the WebSocket close handler (v0.106, real-time track slice
    /// 3b-iii). Optional, ≤1 per `from websocket` service; runs when the socket
    /// closes. Like `on open`, edge-authenticated (`by`), with the identity/params
    /// recovered from the socket attachment (set at `on open`). (A `from websocket`
    /// `on message` reuses [`HandlerKind::Message`], disambiguated by the protocol.)
    Close,
    /// `on event(e: E)` — one emission of a `from Events(E)` service's
    /// subscribed event type (Events track, slice 0, spine #936). No
    /// envelope parameter yet (slice 2). `event`, like `message`/`open`/
    /// `close`/`schedule`, is matched by plain ident text at the fixed
    /// position right after `on`, with no lexer reservation — an ordinary
    /// identifier everywhere else in the grammar.
    Event,
    /// `on rejected(why: BoundaryRejection)` — renders the router's own
    /// rejections (`400`, `405`, `413`, `415`) of a `from http` service in the
    /// service's shape, keeping their status (#2341, decision C). At most one
    /// per service; no `by`, no `given`.
    Rejected,
}

/// HTTP methods supported by `on http` handlers (v0.9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

impl HttpMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            HttpMethod::Get => "GET",
            HttpMethod::Post => "POST",
            HttpMethod::Put => "PUT",
            HttpMethod::Patch => "PATCH",
            HttpMethod::Delete => "DELETE",
        }
    }

    pub fn from_ident(s: &str) -> Option<HttpMethod> {
        match s {
            "GET" => Some(HttpMethod::Get),
            "POST" => Some(HttpMethod::Post),
            "PUT" => Some(HttpMethod::Put),
            "PATCH" => Some(HttpMethod::Patch),
            "DELETE" => Some(HttpMethod::Delete),
            _ => None,
        }
    }

    /// True if this method conventionally has no request body.
    pub fn forbids_body(self) -> bool {
        matches!(self, HttpMethod::Get | HttpMethod::Delete)
    }
}

/// Payload shape of an `HttpResult[T]` variant (v0.9 §3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpVariantPayload {
    /// No payload (e.g. `NoContent`, `Unauthorized`).
    None,
    /// Carries a value of the `HttpResult` type parameter `T`.
    Value,
    /// Carries a `String` message (e.g. `BadRequest`, `Conflict`).
    Message,
    /// Carries a `String` target URL, emitted as a `Location` header — the
    /// redirect variants (`Found`, `SeeOther`, `PermanentRedirect`, …).
    Location,
    /// Carries a `Stream[String]`, emitted as an SSE (`text/event-stream`)
    /// streaming body — the `Streaming` (200) variant (v0.101, real-time track
    /// slice 1).
    Streamed,
    /// Carries `(body: Bytes, contentType: String)` — the author-owned raw body
    /// written straight into the response with the declared `content-type` and
    /// **no codec** (the typed-wire guarantee is deliberately off). The `Raw`
    /// (200) variant (v0.111), the first two-argument payload shape, and
    /// `RawCreated` (201, #2403).
    Raw,
    /// Carries `(body: Bytes, contentType: String, etag: String)` — a raw body
    /// with the handler's own entity tag, sent as the response's `ETag` and
    /// validated at run time. The `RawTagged` (200) variant (caching raw
    /// responses), for a body too large to hash or a validator the handler has
    /// for free (a content hash, an object store's `etag`).
    RawTagged,
    /// Carries `(status: ErrorStatus, body: Bytes, contentType: String)` — an
    /// error status the author chooses, with an author-owned body and no codec
    /// (#2341, L4). The `RawError` variant; `status` is one of the vocabulary's
    /// 4xx/5xx variant names ([`error_status`]), resolved at compile time.
    RawError,
}

/// One variant of the built-in `HttpResult[T]` sum (v0.9 §3.3).
#[derive(Debug, Clone, Copy)]
pub struct HttpVariant {
    pub name: &'static str,
    pub payload: HttpVariantPayload,
    pub status: u16,
}

/// All `HttpResult[T]` variants, in declaration order (ascending status). The
/// vocabulary tracks the common, modern HTTP status codes (RFC 9110): success
/// and created/accepted (`Value`), redirects carrying a `Location` URL, and
/// the client/server failures that handlers routinely return (`Message` when
/// an explanation helps the caller, `None` for self-describing statuses).
pub const HTTP_VARIANTS: &[HttpVariant] = &[
    // ── 2xx success ──────────────────────────────────────────────────────
    HttpVariant {
        name: "Ok",
        payload: HttpVariantPayload::Value,
        status: 200,
    },
    // v0.101 (real-time track slice 1): a 200 whose body is a streamed
    // `Stream[String]`, SSE-framed. Status precedes the body, so streaming is
    // 200-only — pre-stream failures are ordinary variants returned instead.
    HttpVariant {
        name: "Streaming",
        payload: HttpVariantPayload::Streamed,
        status: 200,
    },
    // v0.111: a 200 whose body is an author-owned `Bytes` written straight into
    // the response with the declared `content-type` — no codec runs. 200-only,
    // like `Streaming`: it serves service-tier raw bodies (`robots.txt`,
    // `sitemap.xml`, feeds, a QR PNG), not custom-status error pages.
    HttpVariant {
        name: "Raw",
        payload: HttpVariantPayload::Raw,
        status: 200,
    },
    // Caching raw responses: a `Raw` body plus the handler-supplied entity tag
    // its `ETag` carries. A separate variant, so every `Raw(…)` call is unchanged.
    HttpVariant {
        name: "RawTagged",
        payload: HttpVariantPayload::RawTagged,
        status: 200,
    },
    // #2403 (L6): a `Raw` body answered `201` — for an external contract that
    // fixes the bytes of a creation's answer. `Raw`'s payload shape, so every
    // `Raw` check, binding and emit arm covers it; no validator (a 201 answers a
    // write and is not cached).
    HttpVariant {
        name: "RawCreated",
        payload: HttpVariantPayload::Raw,
        status: 201,
    },
    HttpVariant {
        name: "Created",
        payload: HttpVariantPayload::Value,
        status: 201,
    },
    HttpVariant {
        name: "Accepted",
        payload: HttpVariantPayload::Value,
        status: 202,
    },
    HttpVariant {
        name: "NoContent",
        payload: HttpVariantPayload::None,
        status: 204,
    },
    // ── 3xx redirection (carry a `Location` URL) ─────────────────────────
    HttpVariant {
        name: "MovedPermanently",
        payload: HttpVariantPayload::Location,
        status: 301,
    },
    HttpVariant {
        name: "Found",
        payload: HttpVariantPayload::Location,
        status: 302,
    },
    HttpVariant {
        name: "SeeOther",
        payload: HttpVariantPayload::Location,
        status: 303,
    },
    HttpVariant {
        name: "TemporaryRedirect",
        payload: HttpVariantPayload::Location,
        status: 307,
    },
    HttpVariant {
        name: "PermanentRedirect",
        payload: HttpVariantPayload::Location,
        status: 308,
    },
    // ── 4xx client error ─────────────────────────────────────────────────
    HttpVariant {
        name: "BadRequest",
        payload: HttpVariantPayload::Message,
        status: 400,
    },
    HttpVariant {
        name: "Unauthorized",
        payload: HttpVariantPayload::None,
        status: 401,
    },
    HttpVariant {
        name: "Forbidden",
        payload: HttpVariantPayload::None,
        status: 403,
    },
    HttpVariant {
        name: "NotFound",
        payload: HttpVariantPayload::None,
        status: 404,
    },
    HttpVariant {
        name: "MethodNotAllowed",
        payload: HttpVariantPayload::None,
        status: 405,
    },
    HttpVariant {
        name: "NotAcceptable",
        payload: HttpVariantPayload::None,
        status: 406,
    },
    HttpVariant {
        name: "RequestTimeout",
        payload: HttpVariantPayload::None,
        status: 408,
    },
    HttpVariant {
        name: "Conflict",
        payload: HttpVariantPayload::Message,
        status: 409,
    },
    HttpVariant {
        name: "Gone",
        payload: HttpVariantPayload::None,
        status: 410,
    },
    HttpVariant {
        name: "LengthRequired",
        payload: HttpVariantPayload::None,
        status: 411,
    },
    HttpVariant {
        name: "PayloadTooLarge",
        payload: HttpVariantPayload::Message,
        status: 413,
    },
    HttpVariant {
        name: "UnsupportedMediaType",
        payload: HttpVariantPayload::Message,
        status: 415,
    },
    HttpVariant {
        name: "UnprocessableEntity",
        payload: HttpVariantPayload::Message,
        status: 422,
    },
    HttpVariant {
        name: "TooManyRequests",
        payload: HttpVariantPayload::Message,
        status: 429,
    },
    HttpVariant {
        name: "UnavailableForLegalReasons",
        payload: HttpVariantPayload::Message,
        status: 451,
    },
    // ── 5xx server error ─────────────────────────────────────────────────
    HttpVariant {
        name: "ServerError",
        payload: HttpVariantPayload::Message,
        status: 500,
    },
    HttpVariant {
        name: "NotImplemented",
        payload: HttpVariantPayload::Message,
        status: 501,
    },
    HttpVariant {
        name: "BadGateway",
        payload: HttpVariantPayload::Message,
        status: 502,
    },
    HttpVariant {
        name: "ServiceUnavailable",
        payload: HttpVariantPayload::Message,
        status: 503,
    },
    HttpVariant {
        name: "GatewayTimeout",
        payload: HttpVariantPayload::Message,
        status: 504,
    },
    // #2341 (L4): an error status the author chooses, with an author-owned
    // body — the form for an external API's error contract. Its status is
    // its first argument, so the row's own is [`AUTHOR_CHOSEN_STATUS`].
    HttpVariant {
        name: "RawError",
        payload: HttpVariantPayload::RawError,
        status: AUTHOR_CHOSEN_STATUS,
    },
];

/// The `status` of an [`HttpVariant`] whose status is chosen per construction
/// (`RawError`, #2341) rather than fixed by the variant.
pub const AUTHOR_CHOSEN_STATUS: u16 = 0;

/// The status an `ErrorStatus` name stands for in `RawError`'s first argument
/// (#2341, L4): one of the vocabulary's own 4xx/5xx variant names
/// (`Forbidden` → 403). `None` for any other name.
pub fn error_status(name: &str) -> Option<u16> {
    HTTP_VARIANTS
        .iter()
        .find(|v| v.name == name && (400..600).contains(&v.status))
        .map(|v| v.status)
}

/// Find an `HttpResult[T]` variant by name. Returns the variant info or
/// `None` if the name doesn't match.
pub fn http_variant(name: &str) -> Option<HttpVariant> {
    HTTP_VARIANTS.iter().copied().find(|v| v.name == name)
}

/// #2341 (L4): the status a `RawError`'s first argument names — a bare 4xx/5xx
/// variant name (`Forbidden`) or one qualified by `ErrorStatus`
/// (`ErrorStatus.Forbidden`). `None` for any other expression.
pub fn raw_error_status(arg: &Expr) -> Option<u16> {
    match &arg.kind {
        ExprKind::Ident(id) => error_status(&id.name),
        ExprKind::FieldAccess { receiver, field } => match &receiver.kind {
            ExprKind::Ident(r) if r.name == "ErrorStatus" => error_status(&field.name),
            _ => None,
        },
        _ => None,
    }
}

/// Payload shape of a `QueueResult` variant (v0.44). Non-generic — a verdict
/// carries no value; `Retry` carries a `String` reason for the log path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueVariantPayload {
    /// No payload (`Ack`).
    None,
    /// Carries a `String` reason (`Retry`).
    Message,
}

/// One variant of the built-in `QueueResult` sum (v0.44).
#[derive(Debug, Clone, Copy)]
pub struct QueueVariant {
    pub name: &'static str,
    pub payload: QueueVariantPayload,
}

/// All `QueueResult` variants, in declaration order. `Ack` confirms the
/// message; `Retry` redelivers it, carrying a reason for observability.
pub const QUEUE_VARIANTS: &[QueueVariant] = &[
    QueueVariant {
        name: "Ack",
        payload: QueueVariantPayload::None,
    },
    QueueVariant {
        name: "Retry",
        payload: QueueVariantPayload::Message,
    },
];

/// Find a `QueueResult` variant by name.
pub fn queue_variant(name: &str) -> Option<QueueVariant> {
    QUEUE_VARIANTS.iter().copied().find(|v| v.name == name)
}

#[derive(Debug, Clone)]
pub struct TypeDecl {
    pub name: Ident,
    /// `[T, U]` type parameters (v0.157, ADR 0183): empty for a non-generic
    /// type. A generic *record* type (`type Paginated[T] = { … }`) is the only
    /// generic body accepted; the checker rejects type parameters on refined /
    /// opaque / sum bodies. Mirrors [`FnDecl::type_params`].
    pub type_params: Vec<TypeParam>,
    pub body: TypeBody,
    /// Documentation block attached to this declaration (v0.3).
    pub documentation: Option<Doc>,
    pub span: Span,
    pub trivia: Trivia,
}

/// `event Name = { fields }` — a typed fact a context may emit and other
/// contexts' subscriber services may receive (Events track, slice 0, spine
/// #936). Record body only in slice 0 — pattern refinement (subscription
/// side, slice 1) and default-valued fields for additive versioning (slice
/// 3a) both extend a record body, so nothing here forecloses them. An
/// optional `@schema(N)` annotation (slice 3b) asserts the event's current
/// wire schema version, embedded into `env.schemaVersion` at emission — see
/// [`EventDecl::schema_version`]. Legal only inside a `context` —
/// checker-enforced (`bynk.event.outside_context`), not grammar, mirroring
/// how `capability`/`provides` are commons-rejected at the parser while
/// `event` instead follows `messages`' precedent (ADR 0272) of parsing
/// uniformly and letting the checker place it, since unlike
/// `capability`/`provides` an `event` has no meaning to reject early inside
/// an `adapter` either.
#[derive(Debug, Clone)]
pub struct EventDecl {
    pub name: Ident,
    /// Every `@`-annotation attached to this declaration. The parser stays
    /// permissive (zero or more, same as `store` field / `messages`
    /// annotations); the closed registry (today: `@schema` alone) and its
    /// argument shape are a checker concern (`bynk.event.unknown_annotation`
    /// / `bynk.event.bad_schema_version`), not a parse error.
    pub annotations: Vec<Annotation>,
    pub body: RecordBody,
    /// Documentation block attached to this declaration.
    pub documentation: Option<Doc>,
    pub span: Span,
    pub trivia: Trivia,
}

impl EventDecl {
    /// A synthetic `TypeDecl` with this event's name and record body, so an
    /// event registers into the ordinary `types` symbol table and reuses
    /// every existing type-reference/exports/consumes/construction check —
    /// no non-generic type parameters, no separate resolution path. Callers
    /// that need to know a name is specifically an *event* (owner-only
    /// emission, `from Events(E)`/`Events.emit[E]`'s "must be an event, not
    /// just any type" gate) track that separately, alongside this.
    ///
    /// Deliberately lossy: a `TypeDecl` has no `annotations`, so `@schema(N)`
    /// does not survive this conversion. Nothing downstream of this
    /// synthesis needs the event's schema version — only the emitter's own
    /// `Events.emit` lowering does, and it reads [`EventDecl::schema_version`]
    /// directly off the real declaration instead.
    pub fn as_type_decl(&self) -> TypeDecl {
        TypeDecl {
            name: self.name.clone(),
            type_params: Vec::new(),
            body: TypeBody::Record(self.body.clone()),
            documentation: self.documentation.clone(),
            span: self.span,
            trivia: self.trivia.clone(),
        }
    }

    /// This event's declared wire schema version (Events slice 3b, #978):
    /// the positive `Int` literal argument of its sole `@schema(N)`
    /// annotation, or `1` if the annotation is absent — identical to every
    /// event's behaviour before this annotation existed. A malformed
    /// `@schema` (non-positive, non-literal, wrong arity, labelled, or
    /// duplicated) has already been reported by the checker
    /// (`bynk.event.bad_schema_version`); this falls back to `1` rather than
    /// re-deriving that diagnostic.
    pub fn schema_version(&self) -> i64 {
        self.annotations
            .iter()
            .find(|a| a.name.name == "schema")
            .and_then(|a| a.args.first())
            .and_then(|arg| match &arg.value.kind {
                ExprKind::IntLit { value, .. } if *value > 0 => Some(*value),
                _ => None,
            })
            .unwrap_or(1)
    }
}

/// The structural filter on a `from Events(E { field: value, .. })`
/// subscription header (Events track, slice 1, spine #936) — deliver-and-filter:
/// every emission still reaches the fan-out mechanism, and the subscriber's
/// own generated handler evaluates this as a boolean guard before running the
/// body. Deliberately **not** a [`Pattern`] — an event is a plain record, not
/// a sum, so it has no tag for [`Pattern::Variant`] to test; extending the
/// shared `Pattern` enum to fit would touch parser/checker/emitter/fmt/
/// tree-sitter/LSP sites and drag in match-exhaustiveness semantics a
/// delivery filter does not need. This amends
/// [ADR 0286](../decisions/0286-events-pattern-dispatch-deliver-and-filter.md)'s
/// "no bespoke matching engine is introduced for Events" claim; its
/// deliver-and-filter decision is unchanged. No static narrowing: a matching
/// handler body still sees its parameter at its own declared type, never
/// narrowed to a listed field's specific value (deferred — narrowing needs a
/// singleton-variant type the checker does not have, and waits on the
/// refinement-propagation design question `design/bynk-type-system.md`
/// §2.5.4 names as still open).
#[derive(Debug, Clone)]
pub struct EventPattern {
    /// The listed fields, in source order. Never empty — a pattern with no
    /// fields has no shape (`from Events(E)`, no braces, is the pattern-less
    /// form; `from Events(E { })` is a parse error pointing at it).
    pub fields: Vec<EventPatternField>,
    /// The span of the required trailing `..` — every listed field leaves
    /// the rest of the record's fields unconstrained, and that must be
    /// written explicitly rather than implied.
    pub rest_span: Span,
    pub span: Span,
}

/// One `name: value` entry in an [`EventPattern`].
#[derive(Debug, Clone)]
pub struct EventPatternField {
    pub name: Ident,
    pub value: EventPatternValue,
    pub span: Span,
}

/// The value a pattern field is matched against. A closed set, mirroring
/// [`Pattern::Literal`]'s closed literal kinds plus a nullary sum-variant
/// reference — no nested record sub-patterns in v1 (slice 1 filters on
/// top-level fields only).
#[derive(Debug, Clone)]
pub enum EventPatternValue {
    /// An `Int`/`String`/`Bool` literal — matches the field by value equality.
    Literal { value: LiteralValue, span: Span },
    /// A nullary sum-type variant, optionally qualified: `Region.Domestic` or
    /// bare `Domestic` — both resolve against the field's declared sum type.
    /// A variant that carries a payload is rejected (`bynk.event.
    /// pattern_variant_payload`): testing only the tag while ignoring a
    /// payload would silently over-broaden the filter.
    Variant {
        /// `Some(Region)` for the qualified form, `None` for bare.
        type_name: Option<Ident>,
        variant: Ident,
        span: Span,
    },
}

impl EventPattern {
    pub fn span(&self) -> Span {
        self.span
    }
}

impl EventPatternValue {
    pub fn span(&self) -> Span {
        match self {
            EventPatternValue::Literal { span, .. } => *span,
            EventPatternValue::Variant { span, .. } => *span,
        }
    }
}

/// A `via schema(...)` dispatch clause on a `from Events(...)` header
/// (Events track, slice 4, spine #936): filters delivery by the envelope's
/// `schemaVersion`, parallel to [`EventPattern`] but matched against the
/// envelope rather than the payload, and written after the `Events(...)`
/// header's closing `)` rather than inside it. Delivery is still
/// deliver-and-filter (unchanged from slice 1's ADR 0286): the fan-out
/// mechanism delivers every emission to every subscriber regardless, and
/// this becomes one more independently-evaluated runtime guard in the
/// subscriber's own generated handler — no cross-subscriber ambiguity
/// check (two sibling subscribers with the same or overlapping version
/// coverage are both legal, undiagnosed).
#[derive(Debug, Clone)]
pub struct SchemaDispatch {
    pub pattern: SchemaVersionPattern,
    pub span: Span,
}

/// The pattern a `via schema(...)` clause matches `env.schemaVersion`
/// against. A closed set of one variant today — literal only, mirroring
/// `@schema(N)`'s own permissive-parse-then-checker-validate split (a
/// non-positive value is a checker error, not a parse error, for the same
/// diagnostic style). A future slice's range patterns (`via schema(2..)`)
/// are additive to this enum, not a breaking rename of every match site
/// this slice creates.
#[derive(Debug, Clone)]
pub enum SchemaVersionPattern {
    Literal(i64),
}

/// The right-hand side of a `type` declaration. In v0/v0.1 only the
/// `Refined` variant existed; v0.2 adds records and sums; v0.3 adds opaque.
#[derive(Debug, Clone)]
pub enum TypeBody {
    /// Refined base type: `BaseType where refinement`.
    Refined {
        base: BaseType,
        base_span: Span,
        refinement: Option<Refinement>,
    },
    /// Record type: `{ field: T where ..., ... }`.
    Record(RecordBody),
    /// Sum type: pipe-form variants or `enum { ... }` shorthand.
    Sum(SumBody),
    /// Opaque base type: `opaque BaseType (where refinement)?` (v0.3 §3.4).
    /// Identity is nominal; the base type is hidden outside the defining commons.
    Opaque {
        base: BaseType,
        base_span: Span,
        refinement: Option<Refinement>,
    },
}

/// Body of a record-type declaration (v0.2 §3.1).
#[derive(Debug, Clone)]
pub struct RecordBody {
    pub fields: Vec<RecordField>,
    pub span: Span,
    /// #1788: comments before the closing `}`.
    pub trailing_comments: Vec<Comment>,
}

/// One field of a record type declaration. Each field may carry inline
/// refinement, which is enforced at construction time on the field's value.
#[derive(Debug, Clone)]
pub struct RecordField {
    pub name: Ident,
    pub type_ref: TypeRef,
    pub refinement: Option<Refinement>,
    /// v0.11: an optional initial-value expression. Only meaningful on agent
    /// `state` fields (the field's fresh-key value); ignored / rejected on
    /// record-type fields by the checker.
    pub init: Option<Expr>,
    pub span: Span,
    /// #1888: the doc above the field, in either form. Hover, completion and
    /// the documentation page show it.
    pub documentation: Option<Doc>,
    /// #1788: comments above the field and at the end of its line.
    pub trivia: Trivia,
}

/// Body of a sum-type declaration (v0.2 §3.2).
#[derive(Debug, Clone)]
pub struct SumBody {
    pub variants: Vec<Variant>,
    /// v0.154 (ADR 0178): declared error embeddings — `embeds E as V, …` after
    /// the variants. Each says "an `E` value auto-wraps into variant `V`", which
    /// the `?` operator uses to convert a cross-context error without a manual
    /// `.mapErr`. Empty for a sum with no embeddings.
    pub embeds: Vec<EmbedsClause>,
    pub span: Span,
    /// #1794: comments before the closing `}` of an `enum { … }` body, or, in
    /// the pipe form, on their own lines between the last variant and its
    /// `embeds` clause.
    pub trailing_comments: Vec<Comment>,
}

/// One `embeds <source_type> as <variant>` mapping in a sum body (v0.154, ADR
/// 0178). Declares that a value of `source_type` can be auto-wrapped into the
/// named single-payload `variant` of the enclosing sum.
#[derive(Debug, Clone)]
pub struct EmbedsClause {
    pub source_type: TypeRef,
    pub variant: Ident,
    pub span: Span,
}

/// One variant of a sum type. Variants may have payload fields; a
/// payload-less variant is a simple tag.
#[derive(Debug, Clone)]
pub struct Variant {
    pub name: Ident,
    pub payload: Vec<VariantField>,
    pub span: Span,
    /// #1888: the doc above the variant, in either form.
    pub documentation: Option<Doc>,
    /// #1794: comments above the variant and at the end of its line. A comment
    /// on an `enum {` line leads the first variant, and so does one on the `=`
    /// line of a pipe-form sum. The last pipe-form variant's end-of-line
    /// comment is the type's own trailing comment unless an `embeds` clause
    /// follows it.
    pub trivia: Trivia,
}

/// One payload field of a sum variant. Variant payload fields use named
/// declarations like record fields, but do not carry refinement in v0.2.
#[derive(Debug, Clone)]
pub struct VariantField {
    pub name: Ident,
    pub type_ref: TypeRef,
    pub span: Span,
    /// #1888: the doc above the payload field, in either form.
    pub documentation: Option<Doc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum BaseType {
    Int,
    String,
    Bool,
    Float,
    /// `Duration` (v0.86, ADR 0112) — a span of time, a distinct base type
    /// erased to TS `number` carrying milliseconds (the `Clock` unit). Modelled
    /// on `Float`: Bynk-side-only, no implicit `Int` coercion (save the one
    /// sanctioned clock-math mix).
    Duration,
    /// `Instant` (v0.90, ADR 0114) — an absolute point in time, a distinct base
    /// type erased to TS `number` carrying Unix epoch milliseconds (the
    /// `Clock` unit). No literal (minted by `Clock.now()`); arithmetic composes
    /// with `Duration` (`Instant ± Duration -> Instant`, `Instant − Instant ->
    /// Duration`). Supersedes ADR 0112 D4's `Int`↔`Duration` clock-math mix.
    Instant,
    /// `Bytes` (v0.110, ADR 0142) — an immutable finite octet sequence, the
    /// seventh base type. Unlike its neighbours it does **not** erase to TS
    /// `number`: a `Bytes` lowers to a `Uint8Array`. No source literal
    /// (constructed via `Bytes.fromUtf8`/`fromBase64`/`empty`); `==` compares
    /// by content (real emitter codegen, not host `===`); wires as a base64
    /// JSON string; not `Map`-keyable and not orderable.
    Bytes,
}

impl BaseType {
    pub fn name(self) -> &'static str {
        match self {
            BaseType::Int => "Int",
            BaseType::String => "String",
            BaseType::Bool => "Bool",
            BaseType::Float => "Float",
            BaseType::Duration => "Duration",
            BaseType::Instant => "Instant",
            BaseType::Bytes => "Bytes",
        }
    }

    /// The base type written `name`, or `None` for anything else.
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "Int" => BaseType::Int,
            "String" => BaseType::String,
            "Bool" => BaseType::Bool,
            "Float" => BaseType::Float,
            "Duration" => BaseType::Duration,
            "Instant" => BaseType::Instant,
            "Bytes" => BaseType::Bytes,
            _ => return None,
        })
    }
}

/// A `Duration` literal unit (v0.86, ADR 0112) — the closed set of suffixes in a
/// `<int>.<unit>` literal. Each maps to a fixed millisecond factor (`Duration`
/// erases to `Int` milliseconds).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurationUnit {
    Milliseconds,
    Seconds,
    Minutes,
    Hours,
    Days,
}

impl DurationUnit {
    /// Resolve a unit name (`minutes`) to its variant, or `None` if it is not one
    /// of the closed set. Used by the parser to recognise an `<int>.<unit>`
    /// literal; an unrecognised name leaves the expression a field access.
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "milliseconds" => DurationUnit::Milliseconds,
            "seconds" => DurationUnit::Seconds,
            "minutes" => DurationUnit::Minutes,
            "hours" => DurationUnit::Hours,
            "days" => DurationUnit::Days,
            _ => return None,
        })
    }

    /// The unit name as written.
    pub fn name(self) -> &'static str {
        match self {
            DurationUnit::Milliseconds => "milliseconds",
            DurationUnit::Seconds => "seconds",
            DurationUnit::Minutes => "minutes",
            DurationUnit::Hours => "hours",
            DurationUnit::Days => "days",
        }
    }

    /// The unit's value in milliseconds.
    pub fn millis(self) -> i64 {
        match self {
            DurationUnit::Milliseconds => 1,
            DurationUnit::Seconds => 1_000,
            DurationUnit::Minutes => 60_000,
            DurationUnit::Hours => 3_600_000,
            DurationUnit::Days => 86_400_000,
        }
    }
}

/// An integer refinement bound (v0.40, ADR 0073): the parsed value plus the
/// bound's source span (covering a leading `-`). Value-only beyond the span —
/// ints have one canonical printed form, so the formatter stays idempotent
/// without a stored lexeme. The span backs the `InRange`-swap quick-fix.
#[derive(Debug, Clone)]
pub struct IntBound {
    pub value: i64,
    pub span: Span,
}

/// A float refinement bound (v0.21): the parsed value plus the signed source
/// lexeme (for byte-stable emission). v0.40 (ADR 0073): also the source span,
/// for the `InRange`-swap quick-fix.
#[derive(Debug, Clone)]
pub struct FloatBound {
    pub value: f64,
    pub lexeme: String,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Refinement {
    pub predicates: Vec<RefinementPred>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct RefinementPred {
    pub kind: PredKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum PredKind {
    Matches(String),
    InRange(IntBound, IntBound),
    /// `InRange` with float bounds (v0.21) — a separate variant so every
    /// `Int` refinement path stays untouched. Bounds keep their source
    /// lexemes (including any sign) so emitted runtime checks are
    /// byte-stable.
    InRangeF(FloatBound, FloatBound),
    MinLength(i64),
    MaxLength(i64),
    Length(i64),
    NonNegative,
    Positive,
    NonEmpty,
}

impl PredKind {
    pub fn name(&self) -> &'static str {
        match self {
            PredKind::Matches(_) => "Matches",
            PredKind::InRange(..) | PredKind::InRangeF(..) => "InRange",
            PredKind::MinLength(_) => "MinLength",
            PredKind::MaxLength(_) => "MaxLength",
            PredKind::Length(_) => "Length",
            PredKind::NonNegative => "NonNegative",
            PredKind::Positive => "Positive",
            PredKind::NonEmpty => "NonEmpty",
        }
    }
}

/// #1651: the order a refinement's predicates are **checked** in at runtime:
/// every `Matches` after every other predicate, and otherwise source order.
///
/// A `Matches` pattern may take time polynomial in the input's length, and a
/// `MaxLength`/`Length` bound is what caps it (`bynk.types.polynomial_regex_capped`),
/// so the length checks must run first wherever the author wrote them. The order
/// is observable only in which failure a value failing several predicates
/// reports. Every runtime check site (`.of`, the boundary codec, `is`) and the
/// checker's compile-time literal check use this order, so they agree. The
/// canonical form a contract fingerprint hashes sorts its own copy and is
/// unaffected.
pub fn in_check_order<T>(preds: &[T], kind: impl Fn(&T) -> &PredKind) -> Vec<&T> {
    let (regex, rest): (Vec<&T>, Vec<&T>) = preds
        .iter()
        .partition(|p| matches!(kind(p), PredKind::Matches(_)));
    rest.into_iter().chain(regex).collect()
}

/// A function type parameter (v0.20a, `fn name[A, B](…)`). A struct rather
/// than a bare Ident so the ADR-0028 "bound-capable" promise is a later field
/// addition, not a representation change.
#[derive(Debug, Clone)]
pub struct TypeParam {
    pub name: Ident,
    pub span: Span,
}

/// A lambda expression (v0.20a): `(params) => expr` or `(params) => { … }`.
/// `=>` is the value arrow (shared with `match`); param annotations are
/// optional where an expected function type supplies them.
#[derive(Debug, Clone)]
pub struct LambdaExpr {
    pub params: Vec<LambdaParam>,
    pub body: Box<Expr>,
    pub span: Span,
}

/// A lambda parameter. A separate type from [`Param`] because its annotation
/// is optional — `Param.type_ref` stays mandatory at every signature site.
#[derive(Debug, Clone)]
pub struct LambdaParam {
    pub name: Ident,
    pub type_ref: Option<TypeRef>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct FnDecl {
    /// v0.20a: `[A, B]` type parameters; empty for non-generic functions.
    pub type_params: Vec<TypeParam>,
    /// Free function or method (`TypeName.methodName`). See [`FnName`].
    pub name: FnName,
    pub params: Vec<Param>,
    pub return_type: TypeRef,
    /// v0.115: preconditions (`requires <name>: <pred>`), parsed between the
    /// return type and the body. A contract clause is the invariant predicate
    /// attached to a function (ADR 0144 — one predicate surface); `requires`
    /// scopes over the parameters only.
    pub requires: Vec<Contract>,
    /// v0.115: postconditions (`ensures <name>: <pred>`). Scopes over the
    /// parameters *and* `result`, the contextual binding for the return value.
    pub ensures: Vec<Contract>,
    pub body: Block,
    /// True when the first parameter is the special `self` parameter. Only
    /// valid for method declarations.
    pub has_self: bool,
    /// Documentation block attached to this declaration (v0.3).
    pub documentation: Option<Doc>,
    pub span: Span,
    pub trivia: Trivia,
}

/// A function-declaration name: either a free function `f` or a method
/// `T.method` (v0.2 §3.6).
#[derive(Debug, Clone)]
pub enum FnName {
    /// `fn name(...)` — a free function.
    Free(Ident),
    /// `fn TypeName.methodName(...)` — a method attached to a type.
    Method {
        type_name: Ident,
        method_name: Ident,
    },
}

impl FnName {
    /// The function's short name for diagnostics. For methods returns the
    /// method portion only; the type prefix is recovered via `type_name`.
    pub fn ident(&self) -> &Ident {
        match self {
            FnName::Free(id) => id,
            FnName::Method { method_name, .. } => method_name,
        }
    }

    /// For methods, the attached type's identifier; `None` for free fns.
    pub fn type_name(&self) -> Option<&Ident> {
        match self {
            FnName::Free(_) => None,
            FnName::Method { type_name, .. } => Some(type_name),
        }
    }

    /// The displayed full name (e.g., `Money.add` or `parseSku`).
    pub fn display(&self) -> String {
        match self {
            FnName::Free(id) => id.name.clone(),
            FnName::Method {
                type_name,
                method_name,
            } => format!("{}.{}", type_name.name, method_name.name),
        }
    }
}

/// A brace-delimited block of statements ending in a tail expression
/// whose value is the block's value (spec v0.1 §3.1).
#[derive(Debug, Clone)]
pub struct Block {
    pub statements: Vec<Statement>,
    pub tail: Box<Expr>,
    pub span: Span,
    /// Line comments that appear between the last statement (or the
    /// opening brace) and the tail expression. Preserved here because
    /// expressions do not carry trivia in v1.1.
    pub tail_leading_comments: Vec<Comment>,
    /// `true` when the block was written with no explicit tail expression and
    /// the parser synthesised a `()` (unit) tail (v0.146, ADR 0170). The tail
    /// is a real `ExprKind::UnitLit` either way; this flag records that it was
    /// *implicit* so the formatter can omit it (Bynk has no statement
    /// terminator, so a printed `()` would re-attach to the last statement on
    /// re-parse — `x` `()` → `x()`). The parser re-derives the implicit unit
    /// tail, so omitting it is loss-free.
    pub implicit_tail: bool,
}

impl Block {
    /// Whether this block is a synthesised empty unit block — no statements and
    /// an *implicit* `()` tail (v0.146, ADR 0170). This is exactly the shape the
    /// parser inserts for an `if` with no `else` branch, so both the checker
    /// (gating the else-less form to unit) and the formatter (omitting the
    /// synthetic `else { () }`) recognise it here.
    pub fn is_synth_unit(&self) -> bool {
        self.statements.is_empty()
            && self.implicit_tail
            && matches!(self.tail.kind, ExprKind::UnitLit)
    }
}

/// Block-level statement.
#[derive(Debug, Clone)]
pub enum Statement {
    /// `let name (: T)? = expr` — pure binding (v0.1).
    Let(LetStmt),
    /// `let name (: T)? <- expr` — effectful binding (v0.5).
    EffectLet(LetStmt),
    /// `expect expr` — verify a Bool predicate at test runtime (v0.7; renamed
    /// from `assert` in v0.112). Only valid inside test case bodies.
    Expect(ExpectStmt),
    /// `~> expr` — an asynchronous fire-and-forget send (v0.79). The caller does
    /// not await the reply; legal only when the reply is `Effect[()]`. No binder.
    Send(SendStmt),
    /// `do expr` — an effect-performing expression statement (v0.146, ADR 0170).
    /// Runs an `Effect[()]` and discards its (unit) result — the binder-free
    /// sugar for `let _ <- expr` when the awaited value is unit. Legal only in
    /// an effectful body; the operand MUST be `Effect[()]` (a valued reply keeps
    /// the explicit `let _ <- e`, so throwing away a real value stays visible).
    Do(DoStmt),
    /// `name := expr` — a `Cell` store write (v0.81, storage track). The
    /// unconditional write form; `.update(fn)` (a method call) is the
    /// read-modify-write form. ADR 0108.
    Assign(AssignStmt),
    /// `advance Clock by <Duration>` — move the harness clock forward
    /// (#2137, end-to-end testing track §3.4). `advance` is contextual: it
    /// opens a statement only before `<Ident> by`. Only valid in a `case` body.
    Advance(AdvanceStmt),
}

impl Statement {
    pub fn span(&self) -> Span {
        match self {
            Statement::Let(l) | Statement::EffectLet(l) => l.span,
            Statement::Expect(a) => a.span,
            Statement::Send(s) => s.span,
            Statement::Do(d) => d.span,
            Statement::Assign(a) => a.span,
            Statement::Advance(a) => a.span,
        }
    }
}

/// #2137: `advance <Cap> by <amount>` — moves the case's harness clock forward
/// by a `Duration`. `cap` must be `Clock` (the checker refuses any other).
#[derive(Debug, Clone)]
pub struct AdvanceStmt {
    /// The contextual `advance` keyword.
    pub keyword: Span,
    /// The capability whose time moves (`Clock`).
    pub cap: Ident,
    /// The `Duration` to move it by.
    pub amount: Expr,
    pub span: Span,
    pub trivia: Trivia,
}

#[derive(Debug, Clone)]
pub struct ExpectStmt {
    pub value: Expr,
    pub span: Span,
    pub trivia: Trivia,
}

/// `name := expr` — a `Cell` store write (v0.81, storage track). `target` is the
/// `Cell` field being written (a bare name for now; the checker resolves it to a
/// `store` field). `value` is the new value.
#[derive(Debug, Clone)]
pub struct AssignStmt {
    pub target: Ident,
    pub value: Expr,
    pub span: Span,
    pub trivia: Trivia,
}

#[derive(Debug, Clone)]
pub struct LetStmt {
    pub name: Ident,
    pub type_annot: Option<TypeRef>,
    pub value: Expr,
    /// v0.182 (#664): the call-site `by <Actor>(<identity>)` clause on an
    /// `EffectLet` whose value addresses a test service handler. `None` on a pure
    /// `Let` (the `by` is parsed only in the `<-` arm) and on an effect-let with
    /// no principal.
    pub principal: Option<CallSiteActor>,
    /// #2010: set when this binding is the hidden one a `let <pattern> … else …`
    /// statement desugars into. `None` for every binding the author wrote.
    pub let_else: Option<LetElse>,
    pub span: Span,
    pub trivia: Trivia,
}

/// #2010: what remains of a `let <pattern> <- e else …` (or `= e else …`)
/// statement once the parser has desugared it.
///
/// The parser rewrites `let P <- e else E` followed by the rest of the block
/// into a hidden binding, `let __letElse<offset> <- e`, and a `match` on it as
/// the block's tail: `P => { rest of the block }` first, then either
/// `_ => E` (an `else` block) or the `else match` arms. Everything downstream
/// (checker, emitter, editor) sees only that ordinary `match`. This marker,
/// carried on the hidden binding, is what lets the formatter print the sugar
/// back and the checker word its diagnostics in the author's terms.
#[derive(Debug, Clone)]
pub struct LetElse {
    /// Whether the `else` was a block or a `match { arms }`.
    pub form: LetElseForm,
    /// The `else` keyword.
    pub else_span: Span,
}

/// #2010: the two spellings of a `let … else` fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LetElseForm {
    /// `else { … }` — desugared to a trailing `_ => { … }` arm.
    Block,
    /// `else match { arms }` — the arms follow the pattern's arm unchanged.
    Match,
}

/// #2010: the prefix of every hidden `let … else` binding. User identifiers
/// start with a letter (the lexer's `Ident` rule), so no user name can collide
/// with it, and the emitter already treats `__`-prefixed names as its own.
pub const LET_ELSE_BINDER_PREFIX: &str = "__letElse";

impl Expr {
    /// #2010: whether this is the `match` a `let <pattern> … else …` was
    /// desugared into — one whose scrutinee is the hidden binder. It is not an
    /// expression the author wrote, so source-level tools (refactorings,
    /// diagnostics) treat it as the statement it came from.
    pub fn is_let_else_match(&self) -> bool {
        matches!(
            &self.kind,
            ExprKind::Match { discriminant, .. }
                if matches!(&discriminant.kind, ExprKind::Ident(i) if i.name.starts_with(LET_ELSE_BINDER_PREFIX))
        )
    }
}

/// #2010: whether `name` is a binding the compiler generated rather than one
/// the author wrote. Editor surfaces (inlay hints, locals, symbols) skip these.
pub fn is_generated_binding(name: &str) -> bool {
    name.starts_with("__")
}

#[derive(Debug, Clone)]
pub struct SendStmt {
    /// The send target — a recipient call, e.g. `Logger.info(msg)`.
    pub value: Expr,
    pub span: Span,
    pub trivia: Trivia,
}

/// `do expr` — an effect-performing expression statement (v0.146, ADR 0170).
/// `value` is the awaited effect, which MUST be `Effect[()]`.
#[derive(Debug, Clone)]
pub struct DoStmt {
    pub value: Expr,
    pub span: Span,
    pub trivia: Trivia,
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: Ident,
    pub type_ref: TypeRef,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum TypeRef {
    Base(BaseType, Span),
    Named(Ident),
    /// `Result[T, E]` — the built-in generic Result type (v0.1).
    Result(Box<TypeRef>, Box<TypeRef>, Span),
    /// `Option[T]` — the built-in generic Option type (v0.2).
    Option(Box<TypeRef>, Span),
    /// `Effect[T]` — the built-in generic Effect type (v0.5).
    Effect(Box<TypeRef>, Span),
    /// `HttpResult[T]` — the built-in HTTP-result sum (v0.9).
    HttpResult(Box<TypeRef>, Span),
    /// `QueueResult` — the built-in queue verdict sum (`Ack | Retry`),
    /// non-generic; the required return of a queue handler (v0.44).
    QueueResult(Span),
    /// `Asset` — a static file declared by `asset name = "path"` (#1883,
    /// slice 1a, [DECISION H]). Non-generic, like `QueueResult`; its one
    /// field is `.url : String`.
    Asset(Span),
    /// `RequestHeaders` (#2341, decision A) — an HTTP handler's read-only view
    /// of the request's headers, the type of its `headers` parameter.
    /// Non-generic and opaque: `get`, `bearer`, `clientIp`. Never stored or
    /// sent (it has no wire form).
    RequestHeaders(Span),
    /// `TokenClaims` (#2380, L5) — the verified claims of an `Oidc` actor's
    /// token, read through its binder (`who.claims`). Read-only, never stored
    /// or sent.
    TokenClaims(Span),
    /// `BoundaryRejection` (#2341, decision C) — why the router refused a
    /// request before any handler ran; the type of an `on rejected` handler's
    /// parameter. Opaque here (its variants are matched, never built). Never
    /// stored or sent.
    BoundaryRejection(Span),
    /// `List[T]` — the built-in generic immutable list type (v0.20b).
    List(Box<TypeRef>, Span),
    /// `Map[K, V]` — the built-in generic immutable map type (v0.20b).
    /// Keys are confined to value-keyable types
    /// (`bynk.types.unkeyable_map_key`).
    Map(Box<TypeRef>, Box<TypeRef>, Span),
    /// `Query[T]` — the built-in lazy storage-read description (v0.91, ADR 0115).
    /// Nameable in a pure helper's return type; non-storable and non-boundary
    /// (like `Effect`/`Fn`).
    Query(Box<TypeRef>, Span),
    /// `Stream[T]` — the value-over-time primitive (v0.100, real-time track
    /// slice 0). A lazy, pull-shaped sequence produced over time; non-storable
    /// and non-boundary (like `Query`/`Effect`/`Fn`).
    Stream(Box<TypeRef>, Span),
    /// `Connection[F]` — a held WebSocket connection (v0.102, real-time track
    /// slice 2). `F` is the server→client frame type. A `Held` resource:
    /// non-serialisable, non-boundary, and governed by the linearity discipline
    /// (§2.9); storable only in `Cell[Option[Connection]]` / `Map[K, Connection]`.
    Connection(Box<TypeRef>, Span),
    /// `History[Agent]` — a generated, driven call-history of an agent (v0.119,
    /// testing track slice 7, ADR 0155). A test-only generator, legal only in
    /// `for all` binding position inside a `property`; it is not a value type,
    /// so it never resolves in a field/param/return position. The bound subject
    /// behaves as an ordinary `List[Step]`.
    History(Box<TypeRef>, Span),
    /// `ValidationError` — the built-in error type used by refined-type
    /// constructors (v0.1).
    ValidationError(Span),
    /// `JsonError` — the built-in JSON-decode error type (v0.22b). A
    /// uniform record (`kind`/`path`/`message`, all `String`) the codec
    /// maps `BoundaryError` variants and parse failures into.
    JsonError(Span),
    /// `()` — the unit type (v0.5).
    Unit(Span),
    /// `A -> B` / `(A, B) -> C` / `() -> B` — a function type (v0.20a).
    /// Right-associative; effectful iff the return type is `Effect[_]`
    /// (the structural rule). Confined to non-boundary positions
    /// (`bynk.types.function_at_boundary`).
    Fn(Vec<TypeRef>, Box<TypeRef>, Span),
    /// `Name[Arg, …]` — an application of a user-declared generic type
    /// (v0.157, ADR 0183). `name` is a user type name (never a built-in
    /// generic, which each have a dedicated variant above). Arity and the
    /// existence of the referenced type are checked in the resolver.
    App {
        name: Ident,
        args: Vec<TypeRef>,
        span: Span,
    },
}

impl TypeRef {
    pub fn span(&self) -> Span {
        match self {
            TypeRef::Base(_, s) => *s,
            TypeRef::Named(id) => id.span,
            TypeRef::Result(_, _, s) => *s,
            TypeRef::Option(_, s) => *s,
            TypeRef::Effect(_, s) => *s,
            TypeRef::HttpResult(_, s) => *s,
            TypeRef::QueueResult(s) => *s,
            TypeRef::Asset(s) => *s,
            TypeRef::RequestHeaders(s) => *s,
            TypeRef::TokenClaims(s) => *s,
            TypeRef::BoundaryRejection(s) => *s,
            TypeRef::List(_, s) => *s,
            TypeRef::Map(_, _, s) => *s,
            TypeRef::Query(_, s) => *s,
            TypeRef::Stream(_, s) => *s,
            TypeRef::Connection(_, s) => *s,
            TypeRef::History(_, s) => *s,
            TypeRef::ValidationError(s) => *s,
            TypeRef::JsonError(s) => *s,
            TypeRef::Unit(s) => *s,
            TypeRef::Fn(_, _, s) => *s,
            TypeRef::App { span, .. } => *span,
        }
    }
}

/// v0.174 (#592): does the generic record type `name` transitively contain a
/// reference to itself — through any field-type path, including collection and
/// `Option` wrappers, sum-variant payloads, and generic type arguments? Such a
/// type has no finite set of monomorphised boundary codecs: uniform recursion
/// (`Node[T] = { next: Option[Node[T]] }`) would need a self-referential codec
/// chain the per-instantiation model does not yet generate, and polymorphic
/// recursion (`Weird[T] = { next: Option[Weird[List[T]]] }`) an unbounded set of
/// instantiations. Both are rejected at a boundary
/// (`bynk.generics.recursive_generic_at_boundary`).
///
/// Detection is reachability over the type-containment graph: `name` is
/// recursive iff it is reachable from its own body, following every named /
/// applied head and descending into every wrapper, map/result pair, function
/// position, and generic argument. Terminates via the `visited` set.
pub fn generic_record_is_recursive(
    name: &str,
    types: &std::collections::HashMap<String, std::sync::Arc<TypeDecl>>,
) -> bool {
    fn heads(t: &TypeRef, out: &mut Vec<String>) {
        match t {
            TypeRef::Named(id) => out.push(id.name.clone()),
            TypeRef::App {
                name: app_name,
                args,
                ..
            } => {
                out.push(app_name.name.clone());
                for a in args {
                    heads(a, out);
                }
            }
            TypeRef::Option(a, _)
            | TypeRef::List(a, _)
            | TypeRef::Effect(a, _)
            | TypeRef::HttpResult(a, _)
            | TypeRef::Query(a, _)
            | TypeRef::Stream(a, _)
            | TypeRef::Connection(a, _)
            | TypeRef::History(a, _) => heads(a, out),
            TypeRef::Result(a, b, _) | TypeRef::Map(a, b, _) => {
                heads(a, out);
                heads(b, out);
            }
            TypeRef::Fn(ps, r, _) => {
                for p in ps {
                    heads(p, out);
                }
                heads(r, out);
            }
            TypeRef::Base(..)
            | TypeRef::QueueResult(_)
            | TypeRef::Asset(_)
            | TypeRef::RequestHeaders(_)
            | TypeRef::TokenClaims(_)
            | TypeRef::BoundaryRejection(_)
            | TypeRef::ValidationError(_)
            | TypeRef::JsonError(_)
            | TypeRef::Unit(_) => {}
        }
    }
    fn body_heads(decl: &TypeDecl, out: &mut Vec<String>) {
        match &decl.body {
            TypeBody::Record(r) => {
                for f in &r.fields {
                    heads(&f.type_ref, out);
                }
            }
            TypeBody::Sum(s) => {
                for v in &s.variants {
                    for p in &v.payload {
                        heads(&p.type_ref, out);
                    }
                }
            }
            TypeBody::Refined { .. } | TypeBody::Opaque { .. } => {}
        }
    }
    let Some(root) = types.get(name) else {
        return false;
    };
    let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut stack: Vec<String> = Vec::new();
    body_heads(root, &mut stack);
    while let Some(n) = stack.pop() {
        if n == name {
            return true;
        }
        if !visited.insert(n.clone()) {
            continue;
        }
        if let Some(decl) = types.get(&n) {
            body_heads(decl, &mut stack);
        }
    }
    false
}

/// T3.4 (R2.4): a node's identity, independent of position — allocated once,
/// monotonically, per expression the parser constructs (`Parser::alloc_expr_id`
/// in `bynk-syntax/src/parser.rs`). Never derived from a `Span`, so two
/// expressions occupying the same byte range (a synthetic node, a
/// zero-width span) never collide the way a span-keyed side table could.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ExprId(pub u32);

impl ExprId {
    /// Reserved for `Expr` nodes built outside the parser — after checking,
    /// during emission — that are never looked up in a checker-populated
    /// `expr_types`/`expr_ty` table (they are lowered directly, from
    /// already-typed sub-expressions they wrap or splice). A lookup against
    /// this id is a bug: the node was never checked and has no recorded
    /// type of its own.
    pub const SYNTHETIC: ExprId = ExprId(u32::MAX);
}

#[derive(Debug, Clone)]
pub struct Expr {
    pub id: ExprId,
    pub kind: ExprKind,
    pub span: Span,
}

/// Finding #31: `Expr` sets the size of every expression node in the
/// program — `ExprKind::Observation`'s payload and `ExprKind::Is`'s pattern
/// field are boxed specifically to keep it small (176 bytes unboxed, 128
/// boxed, measured on this target). Pinned so the next large variant added
/// to `ExprKind` is a compile error here rather than a silent regression.
/// T3.4: `id: ExprId` adds 4 bytes (padded); the budget is unchanged, so this
/// still fits.
/// T3.4 (R2.4): `id: ExprId` is a deliberate 8-byte increase (128 → 136,
/// alignment-padded from 4), not a silent regression — the ceiling moves
/// with it, once, here, so the next *accidental* growth still trips this
/// assertion rather than hiding under slack headroom.
/// T3.5 (R2.2): `file: FileId` on `Span` is a deliberate increase (136 → 160
/// — `Span` itself grows from 16 to 24 bytes with alignment padding, and
/// `ExprKind`'s largest variant carries more than one `Span`), not a silent
/// regression — the ceiling moves with it, once, here, exactly as T3.4 did
/// for `id: ExprId`.
const _: () = assert!(std::mem::size_of::<Expr>() <= 160);

impl ExprKind {
    /// Construct an `IntLit` for a *synthesized* integer — one the compiler
    /// invents rather than reading from source (a default `1`, a computed bound).
    /// The lexeme is the canonical decimal form (no separators). Source-parsed
    /// literals keep their as-written lexeme instead (v0.142, ADR 0166).
    pub fn int_lit(value: i64) -> ExprKind {
        ExprKind::IntLit {
            value,
            lexeme: value.to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    /// An integer literal (typed `Int`). The lexeme is kept alongside the parsed
    /// value (v0.142, ADR 0166) so formatting is byte-stable: an author's `_`
    /// digit separators (`1_048_576`) survive a round-trip, mirroring the
    /// `FloatLit` treatment. The value is separator-free; emission lowers the
    /// value, so emitted output is unaffected.
    IntLit {
        value: i64,
        lexeme: String,
    },
    /// A float literal (v0.21). The lexeme is kept alongside the parsed
    /// value so emission and formatting are byte-stable (`1e10` must not
    /// normalise to `10000000000`).
    FloatLit {
        value: f64,
        lexeme: String,
    },
    /// A duration literal `<int>.<unit>` (v0.86, ADR 0112): `5.minutes`,
    /// `30.days`. The parser recognises the `IntLit . <unit>` shape and records
    /// the magnitude, the unit, and the resolved milliseconds (the value the
    /// emitter lowers to). Typed `Duration`.
    DurationLit {
        /// The integer magnitude as written (`5` in `5.minutes`).
        value: i64,
        /// The unit name (`minutes`), one of the closed set.
        unit: DurationUnit,
        /// The value in milliseconds — `value * unit factor`.
        millis: i64,
    },
    StrLit(String),
    /// An interpolated string `"… \(expr) …"` (v0.43, ADR 0075). Chunks and
    /// holes alternate. A plain `"…"` with no holes stays [`ExprKind::StrLit`],
    /// so existing code and the emitter/formatter fast-path are untouched.
    InterpStr(Vec<InterpPart>),
    BoolLit(bool),
    Ident(Ident),
    Call {
        name: Ident,
        /// v0.20a: explicit type arguments (`name[T](…)`); empty when absent.
        type_args: Vec<TypeRef>,
        args: Vec<Expr>,
    },
    /// A lambda (v0.20a). See [`LambdaExpr`].
    Lambda(LambdaExpr),
    BinOp(BinOp, Box<Expr>, Box<Expr>),
    UnaryOp(UnaryOp, Box<Expr>),
    Paren(Box<Expr>),
    /// `{ stmts; expr }` — block expression (v0.1).
    Block(Block),
    /// `if cond { then } else { else }` (v0.1).
    If {
        cond: Box<Expr>,
        then_block: Box<Block>,
        else_block: Box<Block>,
    },
    /// `Ok(value)` — Result success constructor (v0.1).
    Ok(Box<Expr>),
    /// `Err(error)` — Result failure constructor (v0.1).
    Err(Box<Expr>),
    /// `expr?` — propagation operator (v0.1).
    Question(Box<Expr>),
    /// `TypeName { field: value, ... }` — record construction (v0.2).
    RecordConstruction {
        type_name: Ident,
        fields: Vec<FieldInit>,
    },
    /// `receiver.field` — field access on a record value (v0.2). v0.3 adds
    /// `.raw` on opaque types within the defining commons.
    FieldAccess {
        receiver: Box<Expr>,
        field: Ident,
    },
    /// `receiver.method(args)` — instance method call (v0.2). The
    /// resolver determines the receiver's type and looks up the method.
    MethodCall {
        receiver: Box<Expr>,
        method: Ident,
        /// v0.22b: explicit type arguments on a qualified static
        /// (`Json.decode[T](…)`); empty when absent. The same-line-`[`
        /// rule applies as for `Call` type application (0039).
        type_args: Vec<TypeRef>,
        args: Vec<Expr>,
    },
    /// `match disc { arm+ }` — pattern matching (v0.2).
    Match {
        discriminant: Box<Expr>,
        arms: Vec<MatchArm>,
    },
    /// `expr is pattern` — pattern test, returns Bool (v0.2).
    ///
    /// `pattern` is boxed (finding #31): `Pattern`'s `Variant` case carries two
    /// `Ident`s plus a `Vec`, inlining it into every `ExprKind` sets the size
    /// of every expression node in the program for the one variant that
    /// tests a pattern.
    Is {
        value: Box<Expr>,
        pattern: Box<Pattern>,
    },
    /// `Some(value)` — Option Some constructor (v0.2).
    Some(Box<Expr>),
    /// `None` — Option None constructor (v0.2).
    None,
    /// `()` — unit literal (v0.5).
    UnitLit,
    /// `TypeName { ...base, field: value, ... }` or `{ ...base, ... }` —
    /// record spread expression (v0.5).
    RecordSpread {
        /// Optional type prefix (`TypeName { ...base }`). Absent for the
        /// bare form used inside `commit`.
        type_name: Option<Ident>,
        /// The base record being spread.
        base: Box<Expr>,
        /// Field overrides (always full `name: value` form — never shorthand).
        overrides: Vec<FieldInit>,
    },
    /// `Effect.pure(value)` — wrap a synchronous value into `Effect[T]`
    /// (v0.5). Recognised in the parser as a special-form.
    EffectPure(Box<Expr>),
    /// `expect expr` — expectation as an expression of type `()` (v0.9.1;
    /// renamed from `assert` in v0.112). Valid only inside test bodies. Evaluates
    /// `expr` (must be Bool); if false, the surrounding test case fails.
    Expect(Box<Expr>),
    /// `Val[T]`, `Val[T](args)` — test-context value construction (v0.9.4).
    /// `args` is empty for the bare form and holds the pin arguments for
    /// `Val[T](...)`. The record-override form `Val[T] { ... }` is not yet
    /// parsed. Valid only inside test bodies; has type `T`.
    Val {
        type_ref: TypeRef,
        args: Vec<Expr>,
    },
    /// `Wire(<String>)` — a raw, pre-validation argument to a `system`-tier
    /// service address (testing-the-boundary Slice C). The inner expression is a
    /// `String` carrying the wire form the boundary will receive *unvalidated* —
    /// a body's JSON text or a path segment — so a case can drive the router with
    /// input the type system forbids and observe the rejection. Legal only at
    /// `system` (there is no wire at `unit`); the router validates it, so no
    /// refined value is ever minted from a `Wire` (ADR 0182 untouched).
    Wire(Box<Expr>),
    /// `[a, b, c]` — list literal (v0.20b). An empty `[]` requires an
    /// expected type (`bynk.types.uninferable_element_type`).
    ListLit(Vec<Expr>),
    /// An observation over a consumed capability's recorded calls (v0.117,
    /// testing track slice 5). The direct subject of an `expect` in a `case`
    /// body — `expect Cap.op called once with <pred>`, `expect Cap.op never
    /// called`, `expect A.op before B.op`. Types as `Bool` (the claim about the
    /// recorded trace), lowered to a boolean over the recorded log.
    /// Boxed (finding #31): at ~160 bytes, `ObservationExpr` inlined here set
    /// the size of every `ExprKind` for the one variant that records a
    /// capability-call observation.
    Observation(Box<ObservationExpr>),
    /// `<call> faults` — the claim that an effectful call **faults** (#1706).
    /// The direct subject of an `expect` in a `case` body —
    /// `expect quote.call("GBP") faults`. The claim's `call` is the call
    /// itself, an `Effect[_]` the claim awaits; the claim types as `Bool` and
    /// holds when awaiting the call throws (a capability fault, an injected
    /// `stub … fails`, an invariant violation) rather than returning a value.
    /// A fault is untyped and uncatchable by the caller, so this is a test's
    /// observation of the fault, not a handler for it: no production code can
    /// write it.
    ///
    /// #1812: the claim may carry a call-site principal (`expect
    /// api.POST(…) by User("bob") faults`), so an identity-carrying handler's
    /// fault path is testable; see [`FaultsExpr`]. Boxed, as `Observation` is,
    /// so the principal does not widen every `ExprKind`.
    Faults(Box<FaultsExpr>),
    /// `trace(Cap.op)` — the bound-trace escape hatch (v0.117, testing track
    /// slice 5). Yields the recorded calls of `Cap.op` as a `List[<CallRecord>]`
    /// (a synthetic record of the operation's parameters), asserted over with the
    /// ordinary value surface. Test-body-only, like [`ExprKind::Val`].
    Trace {
        cap: Ident,
        op: Ident,
    },
}

/// Every directly-nested sub-expression of `e` — the **total** child
/// iterator. The match is exhaustive (no `_` arm), so adding an [`ExprKind`]
/// variant is a compile error here rather than a silently incomplete walk —
/// the trap the checker's three hand-rolled partial walkers each fell into
/// (block statements and match-arm bodies were skipped, so e.g. the `:=`
/// self-reference rule was bypassable through a match arm).
///
/// Exhaustive over [`ExprKind`] *and* over the expressions each variant holds
/// (#1760: match-arm guards were once missed, and every walk built on this one
/// missed them with it).
///
/// Descends one level: block *statements* and the tail, match-arm guards and
/// bodies, lambda bodies, interpolation holes, record-field values, and
/// observation predicates are all children. Callers recurse for a deep walk.
pub fn expr_children(e: &Expr) -> Vec<&Expr> {
    fn block_children<'a>(b: &'a Block, out: &mut Vec<&'a Expr>) {
        for s in &b.statements {
            statement_exprs(s, out);
        }
        out.push(&b.tail);
    }
    let mut out = Vec::new();
    match &e.kind {
        ExprKind::IntLit { .. }
        | ExprKind::FloatLit { .. }
        | ExprKind::DurationLit { .. }
        | ExprKind::StrLit(_)
        | ExprKind::BoolLit(_)
        | ExprKind::Ident(_)
        | ExprKind::None
        | ExprKind::UnitLit
        | ExprKind::Trace { .. } => {}
        ExprKind::InterpStr(parts) => {
            for p in parts {
                if let InterpPart::Hole(h) = p {
                    out.push(h.as_ref());
                }
            }
        }
        ExprKind::Call { args, .. } | ExprKind::Val { args, .. } | ExprKind::ListLit(args) => {
            out.extend(args.iter())
        }
        ExprKind::Wire(inner) => out.push(inner.as_ref()),
        ExprKind::Lambda(l) => out.push(l.body.as_ref()),
        ExprKind::BinOp(_, l, r) => {
            out.push(l.as_ref());
            out.push(r.as_ref());
        }
        ExprKind::UnaryOp(_, inner)
        | ExprKind::Paren(inner)
        | ExprKind::Ok(inner)
        | ExprKind::Err(inner)
        | ExprKind::Question(inner)
        | ExprKind::Some(inner)
        | ExprKind::EffectPure(inner)
        | ExprKind::Expect(inner) => out.push(inner.as_ref()),
        ExprKind::Faults(f) => {
            // #1812: a call-site principal's identity is evaluated first, as
            // an argument to the claimed call — the same order as an
            // effect-let's in `statement_exprs`.
            if let Some(identity) = f.principal.as_ref().and_then(|p| p.identity.as_deref()) {
                out.push(identity);
            }
            if let Some(claims) = f.principal.as_ref().and_then(|p| p.claims.as_deref()) {
                out.push(claims);
            }
            out.push(&f.call);
        }
        ExprKind::Block(b) => block_children(b, &mut out),
        ExprKind::If {
            cond,
            then_block,
            else_block,
        } => {
            out.push(cond.as_ref());
            block_children(then_block, &mut out);
            block_children(else_block, &mut out);
        }
        ExprKind::RecordConstruction { fields, .. } => {
            out.extend(fields.iter().filter_map(|f| f.value.as_ref()));
        }
        ExprKind::FieldAccess { receiver, .. } => out.push(receiver.as_ref()),
        ExprKind::MethodCall { receiver, args, .. } => {
            out.push(receiver.as_ref());
            out.extend(args.iter());
        }
        ExprKind::Match { discriminant, arms } => {
            out.push(discriminant.as_ref());
            for arm in arms {
                // #1760: the guard, in evaluation order before the body. It is
                // an ordinary expression, checked like any other.
                if let Some(guard) = &arm.guard {
                    out.push(guard);
                }
                match &arm.body {
                    MatchBody::Expr(e) => out.push(e),
                    MatchBody::Block(b) => block_children(b, &mut out),
                }
            }
        }
        ExprKind::Is { value, .. } => out.push(value.as_ref()),
        ExprKind::RecordSpread {
            base, overrides, ..
        } => {
            out.push(base.as_ref());
            out.extend(overrides.iter().filter_map(|f| f.value.as_ref()));
        }
        ExprKind::Observation(obs) => match &obs.matcher {
            ObservationMatcher::Called { count, with_pred } => {
                if let Some(c) = count {
                    out.push(c.as_ref());
                }
                if let Some(p) = with_pred {
                    out.push(p.as_ref());
                }
            }
            ObservationMatcher::NeverCalled | ObservationMatcher::Before { .. } => {}
        },
    }
    out
}

/// The expressions directly contained in a statement — the statement half of
/// [`expr_children`]'s total walk. Exhaustive over [`Statement`] for the same
/// reason, and over each statement's expressions: a `let`'s call-site
/// principal identity included.
pub fn statement_exprs<'a>(s: &'a Statement, out: &mut Vec<&'a Expr>) {
    match s {
        Statement::Let(l) | Statement::EffectLet(l) => {
            // #1766 review: a call-site principal's identity (`by User(who)`)
            // is a full expression. It is evaluated first, as an argument to
            // the addressed call.
            if let Some(identity) = l.principal.as_ref().and_then(|p| p.identity.as_deref()) {
                out.push(identity);
            }
            if let Some(claims) = l.principal.as_ref().and_then(|p| p.claims.as_deref()) {
                out.push(claims);
            }
            out.push(&l.value)
        }
        Statement::Expect(a) => out.push(&a.value),
        Statement::Send(snd) => out.push(&snd.value),
        Statement::Do(d) => out.push(&d.value),
        Statement::Assign(a) => out.push(&a.value),
        Statement::Advance(a) => out.push(&a.amount),
    }
}

/// A fault claim, `<call> [by <Actor>(<identity>)] faults` (#1706, #1812).
/// `call` is the effectful call the claim awaits; `principal` is the optional
/// call-site `by` clause, read exactly as on an effect-let (`let r <- call by
/// User("bob")`): it supplies the identity the addressed handler reads.
#[derive(Debug, Clone)]
pub struct FaultsExpr {
    pub call: Expr,
    pub principal: Option<CallSiteActor>,
}

/// An observation of a capability operation's recorded calls (v0.117, testing
/// track slice 5). `cap`/`op` name the seam (`Logger.log`); `matcher` is the
/// claim about the recorded calls.
#[derive(Debug, Clone)]
pub struct ObservationExpr {
    pub cap: Ident,
    pub op: Ident,
    pub matcher: ObservationMatcher,
}

/// The claim an [`ObservationExpr`] makes about a seam's recorded calls (v0.117).
#[derive(Debug, Clone)]
pub enum ObservationMatcher {
    /// `called` [`once` | `<n> times`]? [`with` `<pred>`]?. `count` is `None`
    /// for a bare `called` (at least one); `Some(expr)` is the exact-count claim
    /// (a literal; `once` desugars to `1`). `with_pred` matches a call whose
    /// arguments (in scope by the operation's parameter names) satisfy it.
    Called {
        count: Option<Box<Expr>>,
        with_pred: Option<Box<Expr>>,
    },
    /// `never called` — zero calls.
    NeverCalled,
    /// `before Cap.op` — the first call of the subject precedes the first call
    /// of the named operation (both must have occurred).
    Before { cap: Ident, op: Ident },
}

/// One part of an interpolated string (v0.43, ADR 0075). An
/// [`ExprKind::InterpStr`] holds an alternating run of these.
#[derive(Debug, Clone)]
pub enum InterpPart {
    /// Literal text between holes, with escapes already resolved.
    Chunk(String),
    /// An interpolated expression `\(expr)`. Type-checked by the hole rule
    /// (base scalars only; see the checker) and lowered into a template-
    /// literal `${…}` slot.
    Hole(Box<Expr>),
}

/// One field-initialiser inside a record construction expression:
/// either `name: expr` or the shorthand `name` (which requires a binding
/// of the same name in scope and uses its value).
#[derive(Debug, Clone)]
pub struct FieldInit {
    pub name: Ident,
    /// `None` means shorthand — the field's value is the same-named binding.
    pub value: Option<Expr>,
    pub span: Span,
}

/// One arm of a `match` expression: `pattern => body` or, with a guard,
/// `pattern if guard => body` (guard added in the nested-patterns increment,
/// ADR 0169). A guarded arm matches only when the pattern matches **and** the
/// `Bool` guard evaluates true; it never contributes to exhaustiveness.
#[derive(Debug, Clone)]
pub struct MatchArm {
    pub pattern: Pattern,
    /// Optional `if <Bool-expr>` guard between the pattern and `=>`.
    pub guard: Option<Expr>,
    pub body: MatchBody,
    pub span: Span,
}

/// The right-hand side of a match arm — either a single expression or
/// a block.
#[derive(Debug, Clone)]
pub enum MatchBody {
    Expr(Expr),
    Block(Block),
}

impl MatchBody {
    pub fn span(&self) -> Span {
        match self {
            MatchBody::Expr(e) => e.span,
            MatchBody::Block(b) => b.span,
        }
    }
}

/// A pattern (v0.2 §3.8). Patterns appear in `match` arms and as the
/// right-hand side of the `is` operator.
#[derive(Debug, Clone)]
pub enum Pattern {
    /// `_` — matches any value, no bindings.
    Wildcard(Span),
    /// A lowercase identifier — binds the whole value to `name` and matches
    /// anything (ADR 0169). At the top of a `match` arm it binds the scrutinee
    /// (`n if n > 0 => …`); inside a payload position it binds the field
    /// (`Some(user)`). The uppercase-led counterpart is a nullary [`Pattern::Variant`].
    Binding(Ident),
    /// A literal pattern — `31`, `"english"`, `true` (v0.130 §2.3.4). Matches a
    /// primitive scrutinee (`Int`/`String`/`Bool`) by value equality. The
    /// admitted set mirrors ADR 0001's closed literal set (integers — including
    /// a leading unary minus — strings, and booleans); `Float`/`()` are not
    /// admitted as patterns.
    Literal { value: LiteralValue, span: Span },
    /// `Variant` or `Variant(bindings)` or `TypeName.Variant(bindings)`. Each
    /// payload binding is itself a [`Pattern`] (ADR 0169), so payloads nest:
    /// `Some(Ok(x))`, `Err(PollClosed)`.
    Variant {
        /// Optional qualifier: `TypeName.Variant`.
        type_name: Option<Ident>,
        /// The variant name.
        variant: Ident,
        /// Payload bindings (empty for nullary variants).
        bindings: Vec<PatternBinding>,
        span: Span,
    },
    /// `p 'where' refinement-predicate` — a refinement guard on a pattern
    /// (#472). Matches when `inner` matches *and* the scrutinee satisfies
    /// `predicate` at runtime. v1 admits only `Wildcard` as `inner` (no
    /// binding form yet); refutable — never counts toward exhaustiveness or
    /// as a catch-all arm, the same treatment as an `if` guard (§2.3.4).
    Refined {
        inner: Box<Pattern>,
        predicate: Refinement,
        span: Span,
    },
    /// `p₁ | p₂ | … | pₙ` — an or-pattern (#474 §2.3.4): matches if any
    /// alternative matches. Left-associative `|`, flattened by the parser's
    /// chain fold into one `Vec` — an alternative is always a leaf
    /// (`Wildcard`/`Binding`/`Literal`/`Variant`), never itself an `Or`
    /// (there is no parenthesized-pattern syntax to nest one inside another).
    /// Well-typedness (checked, not parsed): every alternative binds the same
    /// set of names, a name shared across alternatives has the same type
    /// (including refinement) in each, and every alternative matches the same
    /// value type.
    Or(Vec<Pattern>, Span),
}

/// The value carried by a [`Pattern::Literal`]. A closed set (ADR 0001):
/// integer, string, and boolean. Kept distinct from [`ExprKind`] so patterns
/// carry only what they can actually match, and so it is `Eq`/`Hash` for the
/// duplicate-arm check.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LiteralValue {
    Int(i64),
    Str(String),
    Bool(bool),
}

impl LiteralValue {
    /// A human-readable rendering for diagnostics (`31`, `"english"`, `true`).
    pub fn describe(&self) -> String {
        match self {
            LiteralValue::Int(n) => n.to_string(),
            LiteralValue::Str(s) => format!("{s:?}"),
            LiteralValue::Bool(b) => b.to_string(),
        }
    }
}

impl Pattern {
    pub fn span(&self) -> Span {
        match self {
            Pattern::Wildcard(s) => *s,
            Pattern::Binding(id) => id.span,
            Pattern::Literal { span, .. } => *span,
            Pattern::Variant { span, .. } => *span,
            Pattern::Refined { span, .. } => *span,
            Pattern::Or(_, span) => *span,
        }
    }

    /// Every identifier this pattern binds into scope, recursively (`_` and
    /// nullary variants bind nothing). Used by the resolver and the checker to
    /// populate an arm's scope, and by the guard to see the arm's bindings.
    ///
    /// For [`Pattern::Or`] this returns the *first* alternative's names — the
    /// checker separately verifies (#474 Rule 1) that every alternative binds
    /// the same set, so this is a defensive default when that rule is
    /// violated, not a semantic choice among alternatives.
    pub fn bound_names(&self) -> Vec<&Ident> {
        match self {
            Pattern::Wildcard(_) | Pattern::Literal { .. } => Vec::new(),
            Pattern::Binding(id) => vec![id],
            Pattern::Variant { bindings, .. } => bindings
                .iter()
                .flat_map(|b| b.pattern().bound_names())
                .collect(),
            Pattern::Refined { inner, .. } => inner.bound_names(),
            Pattern::Or(alts, _) => alts.first().map(Pattern::bound_names).unwrap_or_default(),
        }
    }

    /// True when this pattern matches every value and binds nothing — a bare
    /// `_`. A [`Pattern::Binding`] also matches everything but *does* bind, so it
    /// is not a pure wildcard.
    pub fn is_wildcard(&self) -> bool {
        matches!(self, Pattern::Wildcard(_))
    }

    /// True when this pattern matches every value (a `_` or a name binding),
    /// i.e. it is irrefutable and covers the position for exhaustiveness. An
    /// [`Pattern::Or`] is irrefutable when any alternative is — `_` in any
    /// position already makes the whole pattern match everything.
    pub fn is_irrefutable(&self) -> bool {
        match self {
            Pattern::Wildcard(_) | Pattern::Binding(_) => true,
            Pattern::Or(alts, _) => alts.iter().any(Pattern::is_irrefutable),
            _ => false,
        }
    }
}

/// A single binding inside a variant pattern. Two surface forms:
/// `pattern` (positional — match the i-th payload field) and
/// `fieldName: pattern` (named — match the named payload field). The matched
/// sub-`pattern` is a full [`Pattern`] (ADR 0169), so a plain `name` is a
/// [`Pattern::Binding`], `_` a [`Pattern::Wildcard`], and `Ok(x)` a nested
/// [`Pattern::Variant`].
#[derive(Debug, Clone)]
pub struct PatternBinding {
    /// Source form: positional or named.
    pub kind: PatternBindingKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum PatternBindingKind {
    /// `pattern` (e.g. `x`, `_`, `Ok(v)`): match the payload field at this position.
    Positional { pattern: Pattern },
    /// `field: pattern`: match the named payload field against `pattern`.
    Named { field: Ident, pattern: Pattern },
}

impl PatternBinding {
    /// The sub-pattern this binding matches its payload field against.
    pub fn pattern(&self) -> &Pattern {
        match &self.kind {
            PatternBindingKind::Positional { pattern } => pattern,
            PatternBindingKind::Named { pattern, .. } => pattern,
        }
    }

    /// True when this binding discards its field (`_` or `field: _`) — a pure
    /// wildcard sub-pattern that binds nothing.
    pub fn is_wildcard(&self) -> bool {
        self.pattern().is_wildcard()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    /// `P implies Q` — logical implication (v0.80). Desugars to `!P || Q`; sits
    /// at the lowest precedence (below `||`). Reads directionally (P → Q).
    Implies,
    Or,
    And,
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    Add,
    Sub,
    Mul,
    Div,
}

impl BinOp {
    pub fn name(self) -> &'static str {
        match self {
            BinOp::Implies => "implies",
            BinOp::Or => "||",
            BinOp::And => "&&",
            BinOp::Eq => "==",
            BinOp::NotEq => "!=",
            BinOp::Lt => "<",
            BinOp::LtEq => "<=",
            BinOp::Gt => ">",
            BinOp::GtEq => ">=",
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
}

impl UnaryOp {
    pub fn name(self) -> &'static str {
        match self {
            UnaryOp::Neg => "-",
            UnaryOp::Not => "!",
        }
    }
}

#[cfg(test)]
mod size_tests {
    use super::*;

    /// Finding #31: boxing `ExprKind::Observation`'s payload and
    /// `ExprKind::Is`'s pattern field took `Expr` from 176 to 128 bytes on
    /// this target (the module-level `const _` assertion is the real pin;
    /// this test just makes the before/after concrete and fails loudly if a
    /// future change silently regresses the win rather than tripping the
    /// `<= 128` ceiling by enough to notice).
    #[test]
    fn expr_is_smaller_than_before_the_boxing() {
        assert!(
            std::mem::size_of::<Expr>() < 176,
            "Expr should be smaller than its pre-#31 size of 176 bytes"
        );
    }
}

#[cfg(test)]
mod expr_children_tests {
    use super::*;

    /// #1760: a match arm's guard is a child, between the discriminant and
    /// the arm's body, in evaluation order.
    #[test]
    fn a_match_arms_guard_is_a_child_in_evaluation_order() {
        let source = "commons d\n\nfn f(n: Int, lim: Int) -> Int {\n  match n {\n    k if k > lim => 1\n    _ => 0\n  }\n}\n";
        let tokens = crate::lexer::tokenize(source).unwrap();
        let units = crate::parser::parse_units(&tokens, source).unwrap();
        let SourceUnit::Commons(c) = &units[0] else {
            panic!("a commons");
        };
        let CommonsItem::Fn(f) = &c.items[0] else {
            panic!("a fn");
        };
        let ExprKind::Match { .. } = &f.body.tail.kind else {
            panic!("a match tail");
        };
        let children: Vec<&str> = expr_children(&f.body.tail)
            .into_iter()
            .map(|e| &source[e.span.start..e.span.end])
            .collect();
        assert_eq!(children, ["n", "k > lim", "1", "0"]);
    }

    /// #1766 review: a `let`'s call-site principal identity is a statement
    /// expression, before the value it addresses.
    #[test]
    fn a_principal_identity_is_a_statement_expression() {
        let source = "suite demo.s {\n  case \"c\" {\n    let who = \"alice\"\n    let item <- api.get() by User(who)\n    expect item\n  }\n}\n";
        let tokens = crate::lexer::tokenize(source).unwrap();
        let units = crate::parser::parse_units(&tokens, source).unwrap();
        let SourceUnit::Suite(t) = &units[0] else {
            panic!("a suite");
        };
        let mut exprs = Vec::new();
        statement_exprs(&t.cases[0].body.statements[1], &mut exprs);
        let texts: Vec<&str> = exprs
            .into_iter()
            .map(|e| &source[e.span.start..e.span.end])
            .collect();
        assert_eq!(texts, ["who", "api.get()"]);
    }
}

/// #1832: [`expr_children`] and [`statement_exprs`] reach every child slot of
/// every expression, so every walk built on them does too.
///
/// The guard is in two parts:
///
/// - **An oracle that cannot fall behind the AST.** [`oracle_children`] and
///   [`oracle_statement`] destructure every field of every `ExprKind` variant
///   and of every expression-bearing struct with no `..`, so adding a variant
///   or a field anywhere an expression can hide is a compile error here until
///   the oracle names the new slot.
/// - **A program that uses every slot.** [`COVERAGE`] is parsed (never
///   checked, so it only has to be syntactically valid), and two tests run
///   over it: every slot the oracle knows is used at least once, and at every
///   node `expr_children`/`statement_exprs` return exactly the oracle's
///   children. A new slot therefore fails the first test until the program
///   uses it, and the second until `expr_children` walks it.
///
/// The hand-rolled walks that cannot be built on `expr_children` (they carry
/// scopes, ownership or position) are tested where they live (#1832's
/// triage: the resolver, structure, linearity, effects and emit walks).
#[cfg(test)]
mod slot_coverage_tests {
    use super::*;
    use std::collections::BTreeSet;

    /// Every slot, used at least once. `p` is a commons for the production
    /// forms; the suite holds the test-only ones (`Val`, `Wire`,
    /// observations, `faults`, `trace`, call-site principals).
    const COVERAGE: &str = r#"commons p

fn f(x: Int) -> Int {
  let a = "s \(x) t"
  let b = g(x)
  let c = (y) => y
  let d = x + 1
  let e = -x
  let h = (x)
  let i = (z) => {
    let q = z
    q
  }
  let j = if x > 0 {
    let q = 1
    q
  } else {
    let w = 2
    w
  }
  let k = Ok(x)
  let l = Err(x)
  let n = r?
  let o = Some(x)
  let u = R { a: x }
  let v = r.a
  let w = r.m(x)
  let y = match x {
    z if z > 0 => z
    0 => expect x
    _ => {
      let s = 1
      s
    }
  }
  let aa = x is Some(v)
  let bb = R { ...r, a: x }
  let cc = Effect.pure(x)
  let dd = [x]
  ~> L.log(x)
  do s.put(x)
  cell := x
  x
}
"#;

    const COVERAGE_SUITE: &str = r#"suite p {
  case "c" {
    let who = "alice"
    let a <- api.get(x) by User(who)
    let b = Val[T](x)
    let c = Wire(x)
    expect L.log called 2 times with msg == x
    expect box.call() faults
    expect api.post(x) by User(who) faults
    let t = trace(L.log)
    expect x
  }
}
"#;

    type Slots<'a> = Vec<(&'static str, &'a Expr)>;

    fn oracle_block<'a>(owner: &'static str, b: &'a Block, out: &mut Slots<'a>) {
        let Block {
            statements,
            tail,
            span: _,
            tail_leading_comments: _,
            implicit_tail: _,
        } = b;
        for s in statements {
            for (slot, e) in oracle_statement(s) {
                out.push((slot, e));
            }
        }
        out.push((owner, tail));
    }

    /// A statement's expressions, each tagged with its slot.
    fn oracle_statement(s: &Statement) -> Slots<'_> {
        fn let_slots<'a>(kind: &'static str, l: &'a LetStmt, out: &mut Slots<'a>) {
            let LetStmt {
                name: _,
                type_annot: _,
                value,
                principal,
                let_else: _,
                span: _,
                trivia: _,
            } = l;
            if let Some(CallSiteActor {
                actor: _,
                identity,
                claims: _,
                span: _,
            }) = principal
                && let Some(identity) = identity
            {
                out.push((
                    if kind == "let" {
                        "let.identity"
                    } else {
                        "let<-.identity"
                    },
                    identity,
                ));
            }
            out.push((
                if kind == "let" {
                    "let.value"
                } else {
                    "let<-.value"
                },
                value,
            ));
        }
        let mut out = Vec::new();
        match s {
            Statement::Let(l) => let_slots("let", l, &mut out),
            Statement::EffectLet(l) => let_slots("let<-", l, &mut out),
            Statement::Expect(ExpectStmt {
                value,
                span: _,
                trivia: _,
            }) => out.push(("expect.value", value)),
            Statement::Send(SendStmt {
                value,
                span: _,
                trivia: _,
            }) => out.push(("send.value", value)),
            Statement::Do(DoStmt {
                value,
                span: _,
                trivia: _,
            }) => out.push(("do.value", value)),
            Statement::Assign(AssignStmt {
                target: _,
                value,
                span: _,
                trivia: _,
            }) => out.push(("assign.value", value)),
            Statement::Advance(AdvanceStmt {
                keyword: _,
                cap: _,
                amount,
                span: _,
                trivia: _,
            }) => out.push(("advance.amount", amount)),
        }
        out
    }

    /// An expression's children, each tagged with its slot.
    fn oracle_children(e: &Expr) -> Slots<'_> {
        let Expr {
            id: _,
            kind,
            span: _,
        } = e;
        let mut out = Vec::new();
        match kind {
            ExprKind::IntLit {
                value: _,
                lexeme: _,
            }
            | ExprKind::FloatLit {
                value: _,
                lexeme: _,
            }
            | ExprKind::DurationLit {
                value: _,
                unit: _,
                millis: _,
            }
            | ExprKind::StrLit(_)
            | ExprKind::BoolLit(_)
            | ExprKind::Ident(_)
            | ExprKind::None
            | ExprKind::UnitLit
            | ExprKind::Trace { cap: _, op: _ } => {}
            ExprKind::InterpStr(parts) => {
                for p in parts {
                    match p {
                        InterpPart::Chunk(_) => {}
                        InterpPart::Hole(h) => out.push(("interp.hole", h.as_ref())),
                    }
                }
            }
            ExprKind::Call {
                name: _,
                type_args: _,
                args,
            } => args.iter().for_each(|a| out.push(("call.arg", a))),
            ExprKind::Lambda(LambdaExpr {
                params: _,
                body,
                span: _,
            }) => out.push(("lambda.body", body)),
            ExprKind::BinOp(_, l, r) => {
                out.push(("binop.left", l));
                out.push(("binop.right", r));
            }
            ExprKind::UnaryOp(_, x) => out.push(("unary.operand", x)),
            ExprKind::Paren(x) => out.push(("paren.inner", x)),
            ExprKind::Block(b) => oracle_block("block.tail", b, &mut out),
            ExprKind::If {
                cond,
                then_block,
                else_block,
            } => {
                out.push(("if.cond", cond));
                oracle_block("if.then.tail", then_block, &mut out);
                oracle_block("if.else.tail", else_block, &mut out);
            }
            ExprKind::Ok(x) => out.push(("ok.inner", x)),
            ExprKind::Err(x) => out.push(("err.inner", x)),
            ExprKind::Question(x) => out.push(("question.inner", x)),
            ExprKind::RecordConstruction {
                type_name: _,
                fields,
            } => {
                for FieldInit {
                    name: _,
                    value,
                    span: _,
                } in fields
                {
                    if let Some(v) = value {
                        out.push(("record.field", v));
                    }
                }
            }
            ExprKind::FieldAccess { receiver, field: _ } => out.push(("field.receiver", receiver)),
            ExprKind::MethodCall {
                receiver,
                method: _,
                type_args: _,
                args,
            } => {
                out.push(("method.receiver", receiver));
                args.iter().for_each(|a| out.push(("method.arg", a)));
            }
            ExprKind::Match { discriminant, arms } => {
                out.push(("match.discriminant", discriminant));
                for MatchArm {
                    pattern: _,
                    guard,
                    body,
                    span: _,
                } in arms
                {
                    if let Some(g) = guard {
                        out.push(("match.guard", g));
                    }
                    match body {
                        MatchBody::Expr(x) => out.push(("match.body", x)),
                        MatchBody::Block(b) => oracle_block("match.block.tail", b, &mut out),
                    }
                }
            }
            ExprKind::Is { value, pattern: _ } => out.push(("is.value", value)),
            ExprKind::Some(x) => out.push(("some.inner", x)),
            ExprKind::RecordSpread {
                type_name: _,
                base,
                overrides,
            } => {
                out.push(("spread.base", base));
                for FieldInit {
                    name: _,
                    value,
                    span: _,
                } in overrides
                {
                    if let Some(v) = value {
                        out.push(("spread.override", v));
                    }
                }
            }
            ExprKind::EffectPure(x) => out.push(("effect_pure.inner", x)),
            ExprKind::Expect(x) => out.push(("expect_expr.inner", x)),
            ExprKind::Val { type_ref: _, args } => {
                args.iter().for_each(|a| out.push(("val.arg", a)))
            }
            ExprKind::Wire(x) => out.push(("wire.inner", x)),
            ExprKind::ListLit(items) => items.iter().for_each(|i| out.push(("list.item", i))),
            ExprKind::Observation(obs) => {
                let ObservationExpr {
                    cap: _,
                    op: _,
                    matcher,
                } = obs.as_ref();
                match matcher {
                    ObservationMatcher::Called { count, with_pred } => {
                        if let Some(c) = count {
                            out.push(("observation.count", c));
                        }
                        if let Some(p) = with_pred {
                            out.push(("observation.with", p));
                        }
                    }
                    ObservationMatcher::NeverCalled
                    | ObservationMatcher::Before { cap: _, op: _ } => {}
                }
            }
            ExprKind::Faults(f) => {
                let FaultsExpr { call, principal } = f.as_ref();
                if let Some(CallSiteActor {
                    actor: _,
                    identity,
                    claims: _,
                    span: _,
                }) = principal
                    && let Some(identity) = identity
                {
                    out.push(("faults.identity", identity));
                }
                out.push(("faults.inner", call));
            }
        }
        out
    }

    /// Every slot [`oracle_children`] and [`oracle_statement`] can name.
    const ALL_SLOTS: &[&str] = &[
        "interp.hole",
        "call.arg",
        "lambda.body",
        "binop.left",
        "binop.right",
        "unary.operand",
        "paren.inner",
        "block.tail",
        "if.cond",
        "if.then.tail",
        "if.else.tail",
        "ok.inner",
        "err.inner",
        "question.inner",
        "record.field",
        "field.receiver",
        "method.receiver",
        "method.arg",
        "match.discriminant",
        "match.guard",
        "match.body",
        "match.block.tail",
        "is.value",
        "some.inner",
        "spread.base",
        "spread.override",
        "effect_pure.inner",
        "expect_expr.inner",
        "val.arg",
        "wire.inner",
        "list.item",
        "observation.count",
        "observation.with",
        "faults.identity",
        "faults.inner",
        "let.value",
        "let<-.value",
        "let<-.identity",
        "expect.value",
        "send.value",
        "do.value",
        "assign.value",
    ];

    /// Every top-level block of the coverage program: fn bodies and case
    /// bodies.
    fn bodies(units: &[SourceUnit]) -> Vec<&Block> {
        let mut out = Vec::new();
        for u in units {
            match u {
                SourceUnit::Commons(c) => {
                    for item in &c.items {
                        if let CommonsItem::Fn(f) = item {
                            out.push(&f.body);
                        }
                    }
                }
                SourceUnit::Suite(t) => out.extend(t.cases.iter().map(|c| &c.body)),
                _ => {}
            }
        }
        out
    }

    fn parse(source: &str) -> Vec<SourceUnit> {
        let tokens = crate::lexer::tokenize(source).expect("lex");
        crate::parser::parse_units(&tokens, source).expect("the coverage program parses")
    }

    /// Visit every expression under `b` through the oracle, depth first.
    fn visit<'a>(b: &'a Block, f: &mut impl FnMut(&'a Expr)) {
        fn go<'a>(e: &'a Expr, f: &mut impl FnMut(&'a Expr)) {
            f(e);
            for (_, c) in oracle_children(e) {
                go(c, f);
            }
        }
        let mut top = Vec::new();
        oracle_block("body.tail", b, &mut top);
        for (_, e) in top {
            go(e, f);
        }
    }

    #[test]
    fn the_coverage_program_uses_every_slot() {
        let mut used = BTreeSet::new();
        for source in [COVERAGE, COVERAGE_SUITE] {
            let units = parse(source);
            for b in bodies(&units) {
                for s in &b.statements {
                    used.extend(oracle_statement(s).into_iter().map(|(slot, _)| slot));
                }
                visit(b, &mut |e| {
                    used.extend(oracle_children(e).into_iter().map(|(slot, _)| slot));
                    if let ExprKind::Block(b) = &e.kind {
                        for s in &b.statements {
                            used.extend(oracle_statement(s).into_iter().map(|(slot, _)| slot));
                        }
                    }
                });
            }
        }
        let missing: Vec<&str> = ALL_SLOTS
            .iter()
            .copied()
            .filter(|s| !used.contains(s))
            .collect();
        let unknown: Vec<&str> = used
            .iter()
            .copied()
            .filter(|s| !ALL_SLOTS.contains(s))
            .collect();
        assert!(
            missing.is_empty() && unknown.is_empty(),
            "slots the coverage program never uses: {missing:?}; slots missing from ALL_SLOTS: {unknown:?}"
        );
    }

    #[test]
    fn expr_children_returns_exactly_the_oracle_s_children_at_every_node() {
        let key = |e: &Expr| (e.span.start, e.span.end, e.id);
        let mut checked = 0;
        for source in [COVERAGE, COVERAGE_SUITE] {
            let units = parse(source);
            for b in bodies(&units) {
                for s in &b.statements {
                    let mut got = Vec::new();
                    statement_exprs(s, &mut got);
                    let got: Vec<_> = got.into_iter().map(key).collect();
                    let want: Vec<_> = oracle_statement(s)
                        .into_iter()
                        .map(|(_, e)| key(e))
                        .collect();
                    assert_eq!(got, want, "statement_exprs at {:?}", s.span());
                }
                visit(b, &mut |e| {
                    let got: Vec<_> = expr_children(e).into_iter().map(key).collect();
                    let want: Vec<_> = oracle_children(e)
                        .into_iter()
                        .map(|(_, c)| key(c))
                        .collect();
                    assert_eq!(
                        got,
                        want,
                        "expr_children of `{}`",
                        &source[e.span.start..e.span.end]
                    );
                    checked += 1;
                });
            }
        }
        assert!(checked > 50, "only {checked} nodes checked");
    }
}

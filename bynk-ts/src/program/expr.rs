#[cfg(doc)]
use super::{TsClassMethod, TsDecl};
use super::{TsParam, TsStmt, TsType, VerbatimOrigin};
use bynk_syntax::span::Span;

/// An expression. Only the shapes `events_fanout.rs` concretely uses
/// (Decision B) — not the reference sketch's full `TsExpr` list (`Arrow`,
/// `Cond`, `TemplateLit`, `Spread` are all unused in the grounding file and
/// deliberately not built here). Arc C slice 3 (#1321, `workers.rs`) adds
/// `Arrow`, `OptionalMember`/`OptionalIndex` (Decision A, gaps 3/4) — this
/// slice's own real, grounded needs.
#[derive(Debug, Clone, PartialEq)]
pub enum TsExpr {
    /// A real identifier — the only content this variant is for. #1539
    /// (review of `design/reviews/2026-08-30-post-restructuring-review.md`
    /// Part 3/Part 5 §1): before this doc existed, `bynk-emit` also smuggled
    /// non-identifier text through it at 15 call sites in `emit.rs` alone —
    /// a quoted string literal, `this.<method>` member access, a complete
    /// `!(<pred>)` unary expression — an untagged `Verbatim`-shaped escape
    /// hatch [`TsStmt::verbatim`]'s own doc already named the concern for
    /// (Q2's "makes the ratchet a compile-time construct, not a grep"), just
    /// with no tag and no probe watching it. Every one of those call sites
    /// now either builds the matching real node (`Lit(TsLit::Str)`, `Member`,
    /// `Unary`) or routes through [`TsExpr::VerbatimExpr`] instead — this
    /// variant no longer holds anything but a genuine identifier.
    Ident(String),
    /// A residual, not-yet-converted expression carrying a [`VerbatimOrigin`]
    /// tag — [`TsStmt::verbatim`]'s own discipline, closed for expressions
    /// too (#1539). Printed exactly as [`TsExpr::Ident`] always was (raw
    /// text, no escaping of its own — the caller is responsible for
    /// producing valid, already-escaped TypeScript, the same contract
    /// `TsStmt::verbatim`'s own text carries), but now visible to
    /// [`crate::verbatim_violations`] and `xtask`'s own `verbatim_origins`/
    /// `verbatim_sites` gated probes, which had nothing to watch at the
    /// expression level before this variant existed. What ends up here after
    /// #1539's own conversion pass: a generic-typed method/constructor
    /// callee (`Call`/`New` have no `type_args` field), a block-bodied IIFE
    /// with no matching `TsArrowBody` shape, and a `pred_condition_and_
    /// message`-style message that is already pre-escaped or unescaped by
    /// contract (a second escaping pass through `TsLit::Str` would corrupt
    /// it) — none of these has a real node to convert to without expanding
    /// this crate's own type algebra well past what #1539 asks for.
    VerbatimExpr(String, VerbatimOrigin),
    /// Expression text that also declares the names it uses (1A.36b-1): the
    /// expression twin of [`TsStmt::raw_using`]. `text` prints exactly as an
    /// [`TsExpr::Ident`]'s does and is bracketed as one is, so a caller whose
    /// expression is *printed nodes* joined into one line (a form no node can
    /// hold) keeps it, and `names` are the identifiers the text spells without
    /// declaring, which `name_closure` counts as uses. The names are taken from
    /// the nodes the text was printed from (`unbound_names_in` over them), never
    /// scanned out of the text.
    RawUsing(String, Vec<String>),
    /// `object.property`.
    Member {
        object: Box<TsExpr>,
        property: String,
    },
    /// `object?.property` — the optional-chaining form of [`TsExpr::Member`].
    /// A distinct variant, not an `optional: bool` flag on `Member` itself
    /// (#1321's own Decision A, gap 4, left the mechanism to the
    /// implementation): a flag would touch every one of `Member`'s
    /// already-many real call sites (`events_fanout.rs`, this crate's own
    /// printer tests) that never need one, where a separate variant touches
    /// none of them — narrower, matching this track's own repeated
    /// "smallest correct scope" judgment. `workers.rs`'s own secret-probe
    /// idiom (`emit_websocket_upgrade`/`emit_http_wrapper`/
    /// `emit_secret_lookup`, three identical real sites) is the only
    /// grounding.
    OptionalMember {
        object: Box<TsExpr>,
        property: String,
    },
    /// `callee<A, B>`: a TypeScript instantiation expression (1A.20, #3721,
    /// Decision A). A generic call is `Call { callee: TypeApp { .. }, args }`
    /// (`f<T>(a)`) and a generic constructor `New { callee: TypeApp { .. }, args }`
    /// (`new C<T>(a)`), so `Call` and `New` stay as they are. Postfix like
    /// [`TsExpr::Member`]: the callee is bracketed under the rule `Call`'s own
    /// callee uses.
    ///
    /// Valid only as the callee of a `Call` or a `New`: `f<T>.x` is TS1477 and `f<>` is a
    /// syntax error, so `type_args` must not be empty and the printer asserts it, as it does
    /// for [`TsExpr::Spread`].
    TypeApp {
        callee: Box<TsExpr>,
        type_args: Vec<TsType>,
    },
    /// `((name = default): ret => { body })()`: an immediately invoked arrow whose one untyped
    /// parameter takes its value from a default, which JS evaluates at call time, before the body
    /// runs (1A.21, #3722). `Json.encodePlain` evaluates its argument this way before the body
    /// raises a flag. A node of its own, rather than a default on [`TsParam`], so the 189 parameter
    /// literals stay as they are. Prints as a call, so it takes the brackets a call takes.
    DefaultIife {
        name: String,
        default: Box<TsExpr>,
        return_type: TsType,
        body: Vec<TsStmt>,
    },
    /// An array printed one item per line, each with a trailing comma and the
    /// closing bracket back at the enclosing depth, wherever it sits (1A.14:
    /// `par`'s `await __bynkSettleAll([…])`). Unlike [`TsExpr::Array`] with
    /// `multiline: true`, which breaks only as a whole statement's initialiser,
    /// this breaks as a call argument too.
    LinesArray(Vec<TsExpr>),
    /// `object[index]`.
    Index {
        object: Box<TsExpr>,
        index: Box<TsExpr>,
    },
    /// `object?.[index]` — the optional-chaining form of [`TsExpr::Index`],
    /// the same real grounding and the same "separate variant, not a flag"
    /// reasoning as [`TsExpr::OptionalMember`].
    OptionalIndex {
        object: Box<TsExpr>,
        index: Box<TsExpr>,
    },
    /// `(params) => body` (`is_async: false`) or `async (params) => body`
    /// (`is_async: true`) — an arrow function, expression- or block-bodied
    /// (see [`TsArrowBody`]). `is_async` added by #1327: `emit_composition_root`'s
    /// own `__eventsDispatch` closure (`async (events: Array<...>) => {...}`)
    /// is the first real `Arrow` site that's async — mirrors
    /// `TsDecl::Function`'s own `is_async` field (#1325), added for the same
    /// reason. `generics`/`return_type` added by #1339: `emit_sum_type`'s
    /// own generic payload-constructor arrows (`<T>(name: T): Sum<T> =>
    /// (...)`) need both — bare generic names only (empty for every
    /// non-generic site, the overwhelming majority), matching
    /// `TsObjectEntry::Method.generics`'s own (#1337) identical convention;
    /// `return_type` mirrors `TsDecl::Function.return_type`'s own existing
    /// `Option<TsType>` shape — a real gap the accepted proposal's own
    /// grounding named `generics` for but missed: every one of this file's
    /// own real generic-payload arrows carries an explicit return-type
    /// annotation the arrow itself owns (`: {name}{params}`), not something
    /// the body's own type alone determines.
    ///
    /// `body`'s type became [`TsArrowBody`] (#1435, Arc E slice 1): from
    /// P7.8 through #1434 this field was a bare `Box<TsExpr>` — expression-
    /// bodied only, "extend narrowly" (the same posture `TsBinaryOp` takes
    /// for its own operator table), since every real site up to and
    /// including #1339's generic payload-constructor arrows had an
    /// expression body. `serialisation.rs`'s `serialise_field_expr_wire`
    /// (`bynk-emit`) is the first real site that doesn't: its `Float`
    /// non-finite guard is a genuine statement-bodied IIFE (`((v: number) =>
    /// { if (!Number.isFinite(v)) throw new Error(...); return v as
    /// JsonValue; })(value)`), not reducible to one expression. Widening the
    /// existing field (`TsArrowBody::Expr(Box<TsExpr>)` |
    /// `TsArrowBody::Block(Vec<TsStmt>)`) rather than adding a second
    /// sibling `TsExpr` variant matches this file's own repeated
    /// "extend the existing variant when every real site still needs the
    /// same node kind, only the body shape differs" precedent
    /// (`TsObjectEntry::Method.inline`, #1337) — every prior `Arrow`
    /// construction site across the workspace wraps its already-correct
    /// expression body in `TsArrowBody::Expr(..)`, a mechanical change with
    /// no behavior difference.
    Arrow {
        params: Vec<TsParam>,
        is_async: bool,
        generics: Vec<String>,
        return_type: Option<TsType>,
        body: Box<TsArrowBody>,
    },
    Call {
        callee: Box<TsExpr>,
        args: Vec<TsExpr>,
    },
    New {
        callee: Box<TsExpr>,
        args: Vec<TsExpr>,
    },
    /// A value object literal, e.g. `{ status: 204 }` — comma-separated.
    /// `multiline: false` (the ordinary case, via [`TsExpr::object`])
    /// always prints on one line, matching [`TsType::Object`]'s own
    /// (semicolon-separated) single-line convention for the *type*-position
    /// shape. `multiline: true` (via [`TsExpr::multiline_object`]) is a
    /// real, grounded gap found implementing Arc C's first slice (#1317):
    /// `events_fanout.rs`'s own `__eventRoutes` table is a top-level
    /// `const` initializer with one entry per line, each with its own
    /// trailing comma, closing brace at the *statement's* own indent —
    /// TypeScript's ordinary multi-line object-literal convention, which
    /// nothing in this crate could represent before this addition. Only
    /// statement/declaration-level renderers (which already carry `depth`)
    /// can render this correctly — `printer/multiline.rs`'s own `render_stmt_level_
    /// expr`, and (#1355) `render_multiline_object_entry`'s own `Prop` arm,
    /// for a `multiline: true` object nested one level inside ANOTHER
    /// multiline object as one of its own entries' values —
    /// `emit_messages_bundle`'s own real doubly-nested `{ locale: { code:
    /// expr, ... }, ... }` table. A `multiline: true` object nested any
    /// OTHER way (an array element, a call argument, a `Prop`'s value
    /// inside a non-multiline object, …) still renders via the ordinary
    /// depth-unaware `render_expr` recursion, which cannot honour
    /// `multiline` — not reachable from any real `bynk-emit` call site
    /// today, but worth knowing before nesting one that way.
    ///
    /// `end_span` (1A.32a) is only read for `multiline: true`: the source span the
    /// closing `}` line maps to, so what follows the last entry belongs to the
    /// declaration the object initialises rather than to the last entry's final
    /// body line. `None` records nothing. Build one with
    /// [`TsExpr::multiline_object_ending_at`].
    Object {
        entries: Vec<TsObjectEntry>,
        multiline: bool,
        end_span: Option<Span>,
    },
    /// An array literal, e.g. `[{ binding: "x", service: "y" }]`.
    /// `multiline: false` (the ordinary case, via [`TsExpr::array`]) always
    /// prints on one line, comma-separated — every real site before this
    /// slice. `multiline: true` (via [`TsExpr::multiline_array`]) is #1325's
    /// own real gap: `emit_test_main`'s own `modules` array (one `{ name,
    /// run }` entry per test, one per line, each with its own trailing
    /// comma, closing `]` at the statement's own indent) — the exact same
    /// shape [`TsExpr::Object`]'s own `multiline` field already represents
    /// for object literals, just for an array. Same reachability boundary as
    /// `Object`'s own `multiline` field (see its own doc, updated by
    /// #1355): `render_stmt_level_expr` and `render_multiline_object_
    /// entry`'s own `Prop` arm both honour it; nested any other way, a
    /// `multiline: true` array falls back to single-line via the ordinary
    /// depth-unaware `render_expr` recursion.
    Array {
        items: Vec<TsExpr>,
        multiline: bool,
    },
    /// `` `text${expr}more text` `` — a template literal. `parts.len()` is
    /// always `exprs.len() + 1` (`parts[0]` before the first substitution,
    /// `parts[i+1]` after `exprs[i]`, …). #1325's own real, first grounding
    /// for this shape (`bynk-ts`'s own module doc named `TemplateLit`
    /// explicitly "unused in the grounding file" until now):
    /// `emit_test_main`'s own `` `${m.name}:` ``/`` `${passed} passed,
    /// ${failed} failed.` `` lines.
    ///
    /// **`parts` are printed verbatim — the printer applies no escaping of
    /// its own.** The same "the field is already a raw-text slot" reasoning
    /// [`TsDecl::Import`]'s own `names` field doc already uses, not a new
    /// pattern: `emit_test_main`'s own two real ✓/✗ substitution lines embed
    /// a literal `✓`/`✗` JS unicode escape as pre-formed ASCII
    /// text (six literal characters, not the actual glyph) — an escaper
    /// mirroring [`TsLit::Str`]'s own (which escapes every `\` it sees)
    /// would double that literal backslash into `\\u2713`, corrupting the
    /// exact byte-golden output this slice must match. Every real part in
    /// `emit_test_main` is static, compiler-authored text (never Bynk user
    /// data), so there is no real content this boundary loses safety on
    /// today — a future caller passing untrusted/dynamic text into `parts`
    /// is responsible for pre-escaping backtick/`` ${ ``/`\` itself before
    /// constructing one.
    TemplateLit {
        parts: Vec<String>,
        exprs: Vec<TsExpr>,
    },
    Await(Box<TsExpr>),
    /// `expr as ty`.
    As {
        expr: Box<TsExpr>,
        ty: TsType,
    },
    /// `expr as const`: a const assertion, which keeps a literal's own type
    /// (`{ tag: "Some" as const }`). It is not an `As` over a type named
    /// `const`: `const` is a keyword here, not a type.
    AsConst(Box<TsExpr>),
    Unary {
        op: TsUnaryOp,
        expr: Box<TsExpr>,
    },
    Binary {
        op: TsBinaryOp,
        left: Box<TsExpr>,
        right: Box<TsExpr>,
    },
    /// `test ? consequent : alternate` — #1323's own real gap: `method ===
    /// "HEAD" ? headResponse(__response) : __response` (once per `GET`
    /// route) and `method === "OPTIONS" ? 204 : 405` (the method-fallthrough
    /// path). The lowest-precedence expression form after `Arrow` — needs
    /// the same parenthesization-rule coverage `Arrow` got in review of
    /// #1322 (`needs_parens_as_operand`/`render_binary_operand`/`As`'s own
    /// local rule), added proactively in this same slice rather than left
    /// for a review round to re-find.
    Conditional {
        test: Box<TsExpr>,
        consequent: Box<TsExpr>,
        alternate: Box<TsExpr>,
    },
    /// [`TsExpr::Conditional`] broken over three lines (1A.38a): the test, then
    /// `? consequent` and `: alternate` each on a line of their own, one level
    /// deeper than the statement that holds it. A separate variant rather than a
    /// `multiline` field, so none of the existing `Conditional` constructions
    /// changes; it takes every precedence and parenthesisation rule `Conditional`
    /// does. Build it with [`TsExpr::multiline_conditional`].
    MultilineConditional {
        test: Box<TsExpr>,
        consequent: Box<TsExpr>,
        alternate: Box<TsExpr>,
    },
    /// An explicit, printer-preserved parenthesization — distinct from every
    /// other variant's own precedence-*derived* parens (`render_operand`/
    /// `render_binary_operand`), which the printer adds or omits based on
    /// what the wrapped expression *is*. `Paren` instead always prints
    /// `(<inner>)` regardless of `inner`'s own shape or precedence. #1323's
    /// own real, narrow need: `workers_entry.rs`'s CORS-preflight guard
    /// wraps its own path-match condition in unconditional parens
    /// (`&& ({cond})`) even when `cond` reduces to a single equality check
    /// with no operator lower-precedence than the outer `&&` — a real case
    /// the precedence-derived rules correctly do *not* parenthesize (they're
    /// answering "is this needed for correctness", not "did the source
    /// always write parens here"). Not a general escape hatch: every other
    /// real site in this file's own content still goes through the ordinary
    /// precedence machinery unchanged.
    Paren(Box<TsExpr>),
    /// `...x`, a spread element (1A.18a, #3719). Legal only as an item of
    /// [`TsExpr::Array`] or an argument of [`TsExpr::Call`] (1A.32b), where it prints
    /// `...` then its operand with no brackets
    /// of its own: a spread operand is an assignment expression, so anything but a
    /// comma expression (which `TsExpr` cannot build) stands there as it is. A new
    /// variant rather than an item type on `Array`, which every existing array
    /// site would then have to change.
    Spread(Box<TsExpr>),
    /// `operand!`, the non-null assertion (1A.23, #3724): the persisted-store arms read a
    /// `modify` callback's current value, typed `V | undefined`, as a `V` once the key is known
    /// present. Postfix like [`TsExpr::Member`]: the operand is bracketed under the rule a
    /// member's object uses.
    NonNull(Box<TsExpr>),
    /// `target = value`, an assignment *expression* (1A.18b, #3719): the one
    /// place a value is assigned and used at once, `(__h[__k] = __h[__k] ?? [])
    /// .push(__u)`. An assignment as a whole statement is
    /// `TsStmtKind::Assign`. The printer brackets it wherever it is an
    /// operand; a builder that wants the brackets for readability, as that
    /// example does, adds an explicit [`TsExpr::Paren`].
    Assign {
        target: Box<TsExpr>,
        value: Box<TsExpr>,
    },
    /// `target += value` (1A.18b, #3719), the list kernel's `average` sum. A
    /// sibling of [`TsExpr::Assign`], used as an expression statement.
    AddAssign {
        target: Box<TsExpr>,
        value: Box<TsExpr>,
    },
    /// `target ??= value` (1A.36b-1): assigns only when the target is `null` or
    /// `undefined`, and yields the target's value. A sibling of [`TsExpr::Assign`]
    /// that a cached constant's accessor is built from, bracketed as it is.
    NullishAssign {
        target: Box<TsExpr>,
        value: Box<TsExpr>,
    },
    Lit(TsLit),
    /// `(a, b, c)` (1A.35a part B): a comma expression, which evaluates each
    /// item in order and yields the last. It prints with its own brackets, so it
    /// is safe in any operand or argument position. Needs at least two items.
    Sequence(Vec<TsExpr>),
    /// `callee?.(args)` (1A.35a part B): an optional call, which yields
    /// `undefined` without calling when `callee` is nullish.
    OptionalCall {
        callee: Box<TsExpr>,
        args: Vec<TsExpr>,
    },
    /// A function expression with a statement body (1A.35a part B), the form
    /// that can be a generator: `async function* (params) { body }`. An arrow
    /// cannot `yield`, so the `Stream` generators need this. It prints as an
    /// operand bracketed, so an immediately invoked one reads
    /// `(function () { … })()`.
    Function {
        is_async: bool,
        is_generator: bool,
        params: Vec<TsParam>,
        return_type: Option<TsType>,
        body: Vec<TsStmt>,
    },
    /// `yield <expr>` (1A.35a part B), legal in a generator [`TsExpr::Function`]'s
    /// body. It binds looser than any operator, so it is bracketed as an operand.
    Yield(Box<TsExpr>),
}

/// [`TsExpr::Arrow`]'s own `body` shape (#1435, Arc E slice 1) — see that
/// field's own doc for why this is a widened field rather than a second
/// `TsExpr` variant. `Expr` is every real site before this slice (and the
/// overwhelming majority after it): the arrow's body is one expression,
/// printed with no surrounding braces. `Block` is `serialisation.rs`'s own
/// `Float` non-finite guard, the first real statement-bodied arrow anywhere
/// in this tree — printed as a real braced block, reusing this crate's
/// own printer's existing compact-statement-list renderer
/// (`render_compact_stmts`, the same one `TsStmtKind::InlineBlock` already
/// shares with `render_branch`'s own same-line `if`/`else`) rather than a
/// third copy of that "one physical line, semicolon-separated" logic. Every
/// real `Block` site today is exactly this one-line IIFE shape (an arrow
/// with no other real use of a genuinely multi-line block body has been
/// found); a future multi-line block-bodied arrow is a real, separate gap
/// this variant does not yet cover.
#[derive(Debug, Clone, PartialEq)]
pub enum TsArrowBody {
    Expr(Box<TsExpr>),
    Block(Vec<TsStmt>),
    /// A genuinely multi-line block body (back-half paydown 1A.12): `{`, one
    /// statement per line one level deeper than the line the arrow starts on,
    /// and `}` back at that line's indent. Unlike [`TsArrowBody::Block`]'s
    /// compact one-line form, it may hold any statement, nested at its real
    /// depth, and it records no source-map checkpoints for them.
    Multiline(Vec<TsStmt>),
    /// A block body printed on one line by flattening (back-half paydown 1A.39a): each statement
    /// is printed at depth 0, its lines trimmed and joined with single spaces, and the whole is
    /// bracketed `{ .. }`. A statement that prints over several lines (a braced `if`) therefore
    /// sits on the line as `if (c) { return x; }`. It is the layout the ICU `select` arm and the
    /// witness decoder have always had, and it records no source-map checkpoints. Unlike
    /// [`TsArrowBody::Block`], which holds statements that each print on one line, it admits any
    /// statement and never falls back to the multi-line form.
    ///
    /// **It trims every printed line**, so a statement that holds a template literal spanning
    /// lines (or any text with significant leading whitespace) would be rewritten. A debug build
    /// asserts that no statement prints one.
    Flat(Vec<TsStmt>),
}

impl TsExpr {
    /// [`TsExpr::RawUsing`]: `text`, with the `names` it uses declared.
    pub fn raw_using(text: impl Into<String>, names: Vec<String>) -> Self {
        TsExpr::RawUsing(text.into(), names)
    }

    /// A `bigint` literal: the digits of `n` (a leading `-` allowed), printed with the `n` suffix.
    pub fn bigint(n: impl std::fmt::Display) -> Self {
        let digits = n.to_string();
        debug_assert!(
            !digits.is_empty()
                && digits
                    .strip_prefix('-')
                    .unwrap_or(&digits)
                    .bytes()
                    .all(|b| b.is_ascii_digit()),
            "a bigint literal holds decimal digits only: {digits:?}"
        );
        TsExpr::Lit(TsLit::BigInt(digits))
    }

    /// Whether this expression must be bracketed to be the object of a member access
    /// (`<expr>.name`, `<expr>(..)`). The printer brackets a binary, a cast, an arrow, a
    /// conditional or an assignment there itself, but not an `await`, a unary operator or a
    /// number literal (`Num` or `Raw`), which would read as `await f().op()` or `1.op()`: those
    /// are the forms that keep a bracket the caller writes. A `Spread` is included
    /// defensively: it is legal only as an array item, so a receiver is never one today.
    pub fn needs_receiver_brackets(&self) -> bool {
        match self {
            TsExpr::Await(_)
            | TsExpr::Unary { .. }
            | TsExpr::Lit(TsLit::Num(_) | TsLit::BigInt(_) | TsLit::Raw(_))
            | TsExpr::Spread(_) => true,
            TsExpr::Lit(TsLit::Str(_) | TsLit::Null | TsLit::Bool(_))
            | TsExpr::Ident(..)
            | TsExpr::VerbatimExpr(..)
            | TsExpr::RawUsing(..)
            | TsExpr::Member { .. }
            | TsExpr::OptionalMember { .. }
            | TsExpr::TypeApp { .. }
            | TsExpr::DefaultIife { .. }
            | TsExpr::LinesArray(..)
            | TsExpr::Index { .. }
            | TsExpr::OptionalIndex { .. }
            | TsExpr::Arrow { .. }
            | TsExpr::Call { .. }
            | TsExpr::New { .. }
            | TsExpr::Object { .. }
            | TsExpr::Array { .. }
            | TsExpr::TemplateLit { .. }
            | TsExpr::As { .. }
            | TsExpr::AsConst(..)
            | TsExpr::Binary { .. }
            | TsExpr::Conditional { .. }
            | TsExpr::MultilineConditional { .. }
            | TsExpr::Paren(..)
            | TsExpr::NonNull(..)
            | TsExpr::Assign { .. }
            | TsExpr::AddAssign { .. }
            | TsExpr::NullishAssign { .. }
            | TsExpr::Sequence(..)
            | TsExpr::OptionalCall { .. }
            | TsExpr::Function { .. }
            | TsExpr::Yield(..) => false,
        }
    }

    /// The ordinary, single-line object literal, `Prop`-only — every entry
    /// is `key: value`. `events_fanout.rs`'s own entries (and this crate's
    /// own printer tests) are all this shape; kept taking a plain
    /// `Vec<(String, TsExpr)>` rather than `Vec<TsObjectEntry>` so none of
    /// those existing call sites needed to change when `TsObjectEntry` was
    /// added (#1321) — see [`TsExpr::object_entries`] for the mixed-entry
    /// form `workers.rs` itself needs (shorthand/spread/method entries).
    pub fn object(entries: Vec<(String, TsExpr)>) -> Self {
        TsExpr::Object {
            entries: entries
                .into_iter()
                .map(|(k, v)| TsObjectEntry::prop(k, v))
                .collect(),
            multiline: false,
            end_span: None,
        }
    }

    /// One entry per line, each with its own trailing comma — see
    /// [`TsExpr::Object`]'s own doc for the real shape and the
    /// depth-awareness this needs at the print site. `Prop`-only, the same
    /// convenience [`TsExpr::object`]'s own doc explains.
    pub fn multiline_object(entries: Vec<(String, TsExpr)>) -> Self {
        TsExpr::Object {
            entries: entries
                .into_iter()
                .map(|(k, v)| TsObjectEntry::prop(k, v))
                .collect(),
            multiline: true,
            end_span: None,
        }
    }

    /// The single-line object literal, taking [`TsObjectEntry`] directly —
    /// for a mixed entry list (shorthand/spread/method alongside `Prop`),
    /// which [`TsExpr::object`]'s own `Vec<(String, TsExpr)>` convenience
    /// can't represent. #1321's own real grounding: `workers.rs`'s local
    /// capability-provider `deps` object mixes shorthand names
    /// (`{ cap1, cap2 }`) with explicit `key: value` entries
    /// (`__exec: exec`) in one literal.
    pub fn object_entries(entries: Vec<TsObjectEntry>) -> Self {
        TsExpr::Object {
            entries,
            multiline: false,
            end_span: None,
        }
    }

    /// [`TsExpr::multiline_object`]'s own `TsObjectEntry` sibling —
    /// #1321's own real grounding: `workers.rs`'s `compose`-returned
    /// surface object is one shorthand-async-`Method` entry per wrapper,
    /// one per line.
    pub fn multiline_object_entries(entries: Vec<TsObjectEntry>) -> Self {
        TsExpr::Object {
            entries,
            multiline: true,
            end_span: None,
        }
    }

    /// [`TsExpr::multiline_object_entries`] whose closing `}` line maps to
    /// `end_span` (1A.32a) — a service object's closing line belongs to the
    /// service, not to its last handler's final body line.
    pub fn multiline_object_ending_at(entries: Vec<TsObjectEntry>, end_span: Span) -> Self {
        TsExpr::Object {
            entries,
            multiline: true,
            end_span: Some(end_span),
        }
    }

    /// A [`TsExpr::MultilineConditional`]: `test`, then `? consequent` and
    /// `: alternate` on their own lines.
    pub fn multiline_conditional(test: TsExpr, consequent: TsExpr, alternate: TsExpr) -> Self {
        TsExpr::MultilineConditional {
            test: Box::new(test),
            consequent: Box::new(consequent),
            alternate: Box::new(alternate),
        }
    }

    /// The ordinary, single-line array literal — every real site before
    /// #1325.
    pub fn array(items: Vec<TsExpr>) -> Self {
        TsExpr::Array {
            items,
            multiline: false,
        }
    }

    /// One entry per line, each with its own trailing comma — see
    /// [`TsExpr::Array`]'s own doc for the real shape and the
    /// depth-awareness this needs at the print site. #1325's own real
    /// grounding: `emit_test_main`'s own `modules` array.
    pub fn multiline_array(items: Vec<TsExpr>) -> Self {
        TsExpr::Array {
            items,
            multiline: true,
        }
    }

    /// A template literal — see [`TsExpr::TemplateLit`]'s own doc for the
    /// real shape and its no-escaping boundary. The sole caller-facing
    /// invariant (`parts.len() == exprs.len() + 1`) is asserted here rather
    /// than left to `render_expr`'s own `parts`-driven loop, which would
    /// otherwise silently drop trailing `exprs` on a malformed tree instead
    /// of failing loudly (review of #1326, finding 1).
    pub fn template_lit(parts: Vec<String>, exprs: Vec<TsExpr>) -> Self {
        debug_assert_eq!(
            parts.len(),
            exprs.len() + 1,
            "TsExpr::template_lit: parts.len() must be exprs.len() + 1 \
             (parts[0] before the first substitution, parts[i+1] after \
             exprs[i], …) — got {} parts and {} exprs",
            parts.len(),
            exprs.len()
        );
        TsExpr::TemplateLit { parts, exprs }
    }
}

/// One entry of a [`TsExpr::Object`] literal. Only `Prop` existed before
/// #1321 (`events_fanout.rs`'s own grounding never needed the other three) —
/// `workers.rs`'s own dominant shape (Decision A, gap 1) is a literal
/// object whose entries are shorthand async methods (every `on call`/`on
/// http`/… wrapper attaches this way), its local capability-`deps` object
/// mixes bare shorthand names with explicit `key: value` pairs, and three
/// real sites spread another object into one (gap 2, folded in here rather
/// than as its own top-level `TsExpr` variant — a spread only ever appears
/// as an object- or array-literal entry in this file, never as a
/// standalone expression, so scoping it to entry position is the narrower
/// correct change).
#[derive(Debug, Clone, PartialEq)]
pub enum TsObjectEntry {
    /// `key: value`, with an optional JSDoc block before it (printed the
    /// way [`TsObjectEntry::Method`]'s `doc` is). Build it with
    /// [`TsObjectEntry::prop`] and [`TsObjectEntry::with_doc`].
    Prop {
        key: String,
        value: TsExpr,
        doc: Option<String>,
    },
    /// `key` alone — object-literal property shorthand, e.g. `{ cap1,
    /// cap2 }`. Distinct from `Prop(name, TsExpr::Ident(name))`, which
    /// prints `name: name`, not the shorthand form real TypeScript
    /// property-shorthand text actually is.
    Shorthand(String),
    /// A shorthand async method entry, e.g. `async foo(a: T) { ... },` —
    /// `workers.rs`'s own dominant shape (Decision A, gap 1): every
    /// wrapper helper (`emit_call_wrapper`, `emit_event_wrapper`, …)
    /// attaches to the returned `compose` surface this way — none of
    /// `workers.rs`'s own real sites annotates a return type, so
    /// `return_type` stayed unneeded until #1323's own real gap:
    /// `workers_entry.rs`'s `export default { fetch, scheduled?, queue? }`
    /// entries all carry one (`: Promise<Response>`/`: Promise<void>`), the
    /// same field [`TsClassMethod::return_type`] already has for the
    /// declaration-position sibling this mirrors.
    ///
    /// `generics` and `doc` (#1337's own real gaps, both found only by the
    /// zero-diff fixture check — not the accepted proposal's own citation,
    /// which searched the project-form fixture corpus only;
    /// `402_generic_instance_method` is single-file form, and while
    /// `65_money_uses_time`/`64_full_time_commons` ARE project form, the
    /// citation's own doc-comment search used the wrong marker, `///`
    /// rather than this language's real `---`-delimited doc block —
    /// both gaps outside what that first pass actually checked):
    ///
    /// - `generics`: a method on a generic type erases to
    ///   `{name}<{generics}>(self: {Type}<{generics}>, …)` — `Box.map<A,
    ///   U>`, the type's own `A` plus the method's own `U`. Bare names
    ///   only, no bounds/defaults — every real generic parameter list this
    ///   tree ever builds is (matching [`crate::printer::print_type`]'s
    ///   own `TsType::Named`-only-name convention for the identical
    ///   reason), so a full `Vec<TsParam>` (with its own unneeded
    ///   `ty`/`optional` fields) would be the wrong shape here.
    /// - `doc`: a JSDoc block immediately preceding the method entry, at
    ///   the same indent — `Timestamp.diff`/`Timestamp.add`
    ///   (`65_money_uses_time`) both carry one. Printed the identical way
    ///   `TsStmtKind::DocComment` is (same escaping, same blank-line
    ///   convention), reusing that one renderer rather than a second copy
    ///   — this field exists because an object-entry method has no
    ///   `TsStmt` slot of its own to hold a preceding statement in.
    Method {
        name: String,
        is_async: bool,
        generics: Vec<String>,
        params: Vec<TsParam>,
        return_type: Option<TsType>,
        doc: Option<String>,
        /// `false` (every method entry landed before #1337): `body` renders
        /// as an ordinary braced, multi-line block. `true`: `body` renders
        /// compactly on the SAME line as the signature — `{ <stmts>; }`,
        /// reusing this crate's own established compact-statement
        /// machinery (`TsStmtKind::InlineBlock`'s own sibling shape). A
        /// real gap #1337's own zero-diff check found:
        /// `emit_forwarded_methods`'s own pre-conversion `writeln!` always
        /// built the WHOLE entry — signature and one-statement body alike —
        /// on one physical line (`"  {method}({params}): {ret} {{ return
        /// …; }},"`), a genuinely different real shape from every other
        /// `Method` entry in this tree, all of which are multi-line.
        inline: bool,
        body: Vec<TsStmt>,
        /// As [`TsClassMethod::span`]: where the entry's first printed line (its
        /// doc comment, if it has one) maps to. `None` records nothing.
        span: Option<Span>,
    },
    /// `...expr` — an object-spread entry (Decision A, gap 2), e.g. `{
    /// ...deps, identity: __caller }`.
    Spread(TsExpr),
}

impl TsObjectEntry {
    /// An undocumented `key: value` entry: the shape of nearly every `Prop`.
    pub fn prop(key: impl Into<String>, value: TsExpr) -> Self {
        TsObjectEntry::Prop {
            key: key.into(),
            value,
            doc: None,
        }
    }

    /// This entry with a JSDoc block before it. Only a `Prop` carries one: a
    /// doc on any other entry is a caller bug, caught in debug builds (in
    /// release it is ignored). See `doc` on [`TsObjectEntry::Prop`].
    #[must_use]
    pub fn with_doc(mut self, text: impl Into<String>) -> Self {
        debug_assert!(
            matches!(self, TsObjectEntry::Prop { .. }),
            "with_doc: only a `Prop` entry carries a doc"
        );
        if let TsObjectEntry::Prop { doc, .. } = &mut self {
            *doc = Some(text.into());
        }
        self
    }

    /// A multi-line, non-generic, undocumented method entry with no span: the
    /// shape of nearly every method entry. Build any other with the variant
    /// directly.
    pub fn method(
        name: impl Into<String>,
        is_async: bool,
        params: Vec<TsParam>,
        return_type: Option<TsType>,
        body: Vec<TsStmt>,
    ) -> Self {
        TsObjectEntry::Method {
            name: name.into(),
            is_async,
            generics: Vec::new(),
            params,
            return_type,
            doc: None,
            inline: false,
            body,
            span: None,
        }
    }

    /// This entry with its body compact on the signature's own line (a no-op for an
    /// entry that is not a method). See `inline` on [`TsObjectEntry::Method`].
    #[must_use]
    pub fn inlined(mut self) -> Self {
        if let TsObjectEntry::Method { inline, .. } = &mut self {
            *inline = true;
        }
        self
    }
}

/// `!x` (`events_fanout.rs`'s `!Array.isArray(subs)`, `!binding`) and
/// `typeof x` (#1321, `workers.rs`'s own secret-probe idiom: `typeof
/// __secret !== "string"`) — the two unary operators real content uses, not
/// the full JS/TS table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TsUnaryOp {
    Not,
    Typeof,
    /// Arithmetic negation, `-x` (1A.3, #3696). Printed with its operand
    /// parenthesised by the same rule as `!`, and a negated negation as
    /// `-(-x)`, never the decrement `--x`.
    Neg,
    /// `void x` (1A.32c-1): evaluates its operand and discards it, as the
    /// idiom for marking a parameter used. Printed with its operand
    /// bracketed by the same rule as `!` and `typeof`.
    Void,
    /// `++x` (1A.35a part B): the prefix increment, which yields the new value.
    PreIncrement,
}

/// `??` (`events_fanout.rs`'s `env ?? {}`), plus three more real operators
/// #1321 (`workers.rs`) grounds: `||`/`&&` (the Bearer-header presence
/// checks, `__authz === null || !__authz.startsWith(...)` /
/// `__authz !== null && __authz.startsWith(...)`) and `===`/`!==`
/// (pervasive throughout the file's own tagged-result and header checks).
/// #1323 (`workers_entry.rs`) grounds one more: `>` (the request-body-
/// ceiling guard's own `Number(__contentLength) > <cap>` — the one real
/// site anywhere in `bynk-emit` that needs a relational, not equality,
/// comparison). Not the
/// full JS/TS operator table (Decision B's own "extend narrowly" posture) —
/// see the printer's own `binary_precedence` (`bynk-ts/src/printer/precedence.rs`,
/// private) for why a nested `Binary` operand's parenthesisation needed to
/// become precedence-aware once more than one operator existed. Arc C, step
/// (11) (#1388) grounds one more: `+` (string concatenation) — the
/// ICU-formatting cluster's own dominant structural pattern, every
/// literal/placeholder segment in a message template joins this way. Real
/// JS/TS precedence (binds tighter than every comparison/logical operator
/// this table already has) and real left-associativity (`"a" + "b" + "c"`
/// needs no parens, the same way a same-operator `||`/`&&` chain already
/// prints flat) both matter here, not just the operator symbol itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TsBinaryOp {
    NullishCoalescing,
    Or,
    And,
    StrictEq,
    StrictNotEq,
    GreaterThan,
    /// `instanceof` — Arc C slice 34 (`tests_emit.rs` slice D, #1403)'s own
    /// real gap: `emit_test_case_function`'s own catch clause
    /// (`e instanceof ExpectationError`) is the first real `instanceof`
    /// anywhere in `bynk-emit`'s own content. Real JS/TS precedence puts it
    /// at the same tier as the relational comparisons (`<`/`>`) — sharing
    /// [`TsBinaryOp::LessThan`]'s own tier, not a new one — rendered as the
    /// keyword `" instanceof "`, textually the same shape as `&&`/`||`
    /// rather than symbol punctuation.
    InstanceOf,
    /// `<` — Arc C slice 33 (`tests_emit.rs` slice C, #1401)'s own real gap:
    /// `emit_stub_class`'s `ReturnsEach` sequence-cursor guard
    /// (`this.__seq_N < <bound>`) is the first real `<` comparison anywhere
    /// in `bynk-emit`'s own content. Same precedence tier as
    /// [`TsBinaryOp::GreaterThan`] (real JS/TS relational operators all
    /// share one level) — added alongside it rather than folding into a
    /// single "relational" variant, matching this enum's own existing
    /// one-variant-per-real-operator convention.
    LessThan,
    Add,
    /// `in` — Arc E slice 5 (`serialisation.rs`, #1443)'s own real gap:
    /// `emit_record_codec`'s per-field default-value prevalidation line
    /// (`"<field>" in obj ? obj["<field>"] : <default>`) needs a real "is
    /// this wire key present at all" test, distinct from `!== undefined`
    /// (Events slice 3a, #972's own Decision D: a wire key present with an
    /// explicit `null`/`{"kind":"None"}` value must NOT fall through to the
    /// default, only a genuinely *absent* key may). Real JS/TS grammar puts
    /// `in` at the exact same `RelationalExpression` precedence tier as
    /// `<`/`>`/`instanceof` (all five — plus `<=`/`>=`, unmodelled here —
    /// share one level in the spec), so it joins [`TsBinaryOp::LessThan`]'s
    /// tier rather than a new one, the same "share, don't multiply, tiers"
    /// convention `InstanceOf`/`LessThan` already set. Rendered as the
    /// keyword `" in "`, the same textual-keyword-operator shape
    /// `InstanceOf` already established (not symbol punctuation).
    In,
    /// `>=` — #1471's own real gap: `pred_condition_and_message`'s
    /// (`bynk-emit`) `NonNegative`/`InRange`/`InRangeF`/`MinLength` arms
    /// each build a real "at least" comparison (`{receiver} >= 0`,
    /// `{receiver} >= {a}`, `{receiver}.length >= {n}`) that this enum had
    /// no home for — [`TsBinaryOp::In`]'s own doc already named this exact
    /// gap ("plus `<=`/`>=`, unmodelled here"). Same `RelationalExpression`
    /// precedence tier as [`TsBinaryOp::GreaterThan`]/[`TsBinaryOp::
    /// LessThan`]/[`TsBinaryOp::InstanceOf`]/[`TsBinaryOp::In`] (real
    /// JS/TS puts all six at one level), rendered `" >= "` — symbol
    /// punctuation, matching `GreaterThan`/`LessThan`'s own convention,
    /// not a keyword like `InstanceOf`/`In`.
    GreaterThanEq,
    /// `<=` — the same gap's other half: `InRange`/`InRangeF`/`MaxLength`'s
    /// own "at most" comparison (`{receiver} <= {b}`, `{receiver}.length
    /// <= {n}`). Same tier and rendering convention as
    /// [`TsBinaryOp::GreaterThanEq`], symmetric with it the way
    /// `GreaterThan`/`LessThan` already are.
    LessThanEq,
    /// `*` — #2955's own real gap: the `Decimal` range check
    /// (`decimal_range_expr`) scales each side to a common scale with
    /// `bigint` arithmetic (`coefficient * 10n ** …`). Multiplicative tier,
    /// one above [`TsBinaryOp::Add`].
    Mul,
    /// `**` — the same gap's other half (`10n ** globalThis.BigInt(scale)`).
    /// Exponentiation tier, one above [`TsBinaryOp::Mul`]. Right-associative
    /// in JS/TS, so the printer's default "same operator nested in itself
    /// parenthesises" rule keeps any chained `**` correct.
    Exp,
    /// `-` (binary) — slice 1A.5 (#3703): Bynk's `Sub`. Shares
    /// [`TsBinaryOp::Add`]'s tier; left-associative, so a right-nested
    /// same-tier operand keeps its brackets (`a - (b - c)`).
    Sub,
    /// `/` — slice 1A.5 (#3703): Bynk's `Div`. Shares [`TsBinaryOp::Mul`]'s
    /// tier; left-associative.
    Div,
}

/// A literal — the three kinds `events_fanout.rs` uses (a string, a number,
/// `null`), plus `Bool` (#1323's own real gap: `workers_entry.rs`'s
/// `CorsPolicy.credentials`/`SecurityPolicy.nosniff` object-literal fields
/// are real booleans, e.g. `credentials: true`, `nosniff: false` — nothing
/// before this slice's own grounding ever built one).
#[derive(Debug, Clone, PartialEq)]
pub enum TsLit {
    Str(String),
    /// Rendered verbatim, as text — TypeScript's own numeric-literal
    /// grammar is not this crate's problem to re-derive; the caller passes
    /// the exact digits it wants printed.
    Num(String),
    /// A `bigint` literal: the decimal digits, with an optional leading `-`, and no suffix. The
    /// printer adds the `n`. Build it with [`TsExpr::bigint`].
    BigInt(String),
    Null,
    Bool(bool),
    /// The complete literal text, printed exactly as given — no quoting, no
    /// escaping, nothing added or removed. #1325's own real, narrow gap:
    /// `emit_test_main`'s own `const PREFIX = "system · ";` embeds
    /// a literal JS unicode escape (six ASCII characters, `·`) as
    /// pre-formed text — [`TsLit::Str`]'s own escaper (which turns every `\`
    /// it sees into `\\`) would double that literal backslash, corrupting
    /// this specific byte-golden output. The same "already a raw-text slot"
    /// boundary [`TsExpr::TemplateLit`]'s own `parts` field documents, just
    /// for a whole literal (quotes included) rather than a template
    /// literal's own static segments. One real site — not a general
    /// escaping bypass for ordinary string content, which stays on
    /// `TsLit::Str`.
    Raw(String),
}

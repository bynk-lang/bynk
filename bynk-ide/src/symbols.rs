//! Symbol lookups for hover and go-to-definition.
//!
//! Single-file lookups walk the parsed AST. Cross-file lookups (v1.1; LSP
//! spec §3.4 cross-file requirement) iterate the project's `.bynk` sources
//! to find a declaration in any unit the user might be referencing — used
//! when the open file lacks the symbol the user clicked on (typically
//! because the name was imported via `uses` or made available via
//! `consumes`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bynk_syntax::ast::*;
use bynk_syntax::lexer::tokenize;
use bynk_syntax::parser::parse_unit_with_recovery;
use bynk_syntax::span::Span;

/// Return the source span of the declaration named `name` in the given
/// source text. Returns `None` if no declaration matches.
///
/// The name of each item comes from [`CommonsItem::name`] rather than a list of
/// arms here: that `match` is exhaustive, so a variant this lookup does not
/// handle cannot compile, and a new one is answered the day it is added. The
/// arms it replaces omitted `Actor`, and a `_ => {}` catch-all swallowed it —
/// so go-to-definition on the `User` in `by u: User` found nothing whenever the
/// index rung above it had not resolved the offset (an unanalysed or mid-edit
/// buffer, a file outside the analysed project).
///
/// Note this deliberately matches a **method** by its bare name — `fn
/// Stored.retitle` answers to `retitle` — because go-to-definition from a bare
/// identifier depends on it. `describe_item` guards on `FnName::Free` instead
/// and the two therefore disagree on what a bare name means, which ADR 0191 D2
/// records as intended rather than as drift.
pub fn find_declaration_span(source: &str, name: &str) -> Option<Span> {
    let tokens = tokenize(source).ok()?;
    let (unit, _errs) = parse_unit_with_recovery(&tokens, source);
    let unit = unit?;
    let items: &[CommonsItem] = match &unit {
        SourceUnit::Commons(c) => &c.items,
        SourceUnit::Context(c) => &c.items,
        SourceUnit::Adapter(a) => &a.items,
        SourceUnit::Suite(_) => &[],
    };
    items
        .iter()
        .filter_map(CommonsItem::name)
        .find(|ident| ident.name == name)
        .map(|ident| ident.span)
}

/// Build a Markdown summary of a named declaration suitable for an LSP
/// hover response. Returns `None` if no declaration matches.
pub fn describe_symbol(source: &str, name: &str) -> Option<String> {
    let tokens = tokenize(source).ok()?;
    let (unit, _errs) = parse_unit_with_recovery(&tokens, source);
    let unit = unit?;
    let items: &[CommonsItem] = match &unit {
        SourceUnit::Commons(c) => &c.items,
        SourceUnit::Context(c) => &c.items,
        SourceUnit::Adapter(a) => &a.items,
        SourceUnit::Suite(_) => &[],
    };
    // S4a (#1966): a derived capability is rendered with its base, which the
    // unit selects in a `consumes U { Base }`.
    if let SourceUnit::Context(ctx) = &unit
        && let Some(c) = items.iter().find_map(|item| match item {
            CommonsItem::Capability(c) if c.name.name == name && c.derived.is_some() => Some(c),
            _ => None,
        })
        && let Some(d) = &c.derived
    {
        let base_unit = ctx.consumes.iter().find_map(|cd| {
            cd.selected
                .as_ref()?
                .iter()
                .any(|s| s.name == d.base.name)
                .then(|| cd.target.joined())
        });
        let base = base_unit
            .as_deref()
            .and_then(|u| crate::completion::embedded_capability(u, &d.base.name));
        return Some(describe_derived_capability(
            c,
            base_unit.as_deref(),
            base.as_ref(),
        ));
    }
    for item in items {
        if let Some(summary) = describe_item(item, name) {
            return Some(summary);
        }
    }
    None
}

/// v0.121 (ADR 0156): the reserved-keyword doc for the token at `offset` in
/// `source`, if the cursor sits on one — matched by source text against
/// `bynk_syntax::keywords::KEYWORDS`, independent of the token's `TokenKind`
/// (unlike the identifier-only lexical hover fallback above). This is hover's
/// floor for the mechanical coverage test: every lowercase-initial keyword
/// gets at least this, even where `describe_symbol` has no richer path for it
/// (e.g. the testing-track clause keywords — `requires`/`ensures`/`suite`/…).
pub fn describe_keyword_at(source: &str, offset: usize) -> Option<&'static str> {
    let tokens = tokenize(source).ok()?;
    let word = tokens
        .iter()
        .find(|t| t.span.start <= offset && offset < t.span.end)
        .map(|t| &source[t.span.start..t.span.end])?;
    bynk_syntax::keywords::KEYWORDS
        .iter()
        .find(|k| k.word == word)
        .map(|k| k.meaning)
}

/// v0.137.0 (ADR 0161): hover for the `key`/`store` *contextual* keywords and
/// the agent state fields they introduce. Both are lexed as `Ident`s (not
/// reserved `KEYWORDS`), and the fields they declare are neither `let`/param
/// locals nor top-level declarations — so neither the keyword fallback nor the
/// `describe_symbol`/locals paths in the hover handler reach them. This closes
/// that gap: for the cursor on the `key`/`store` keyword *or* on the field name
/// it declares, render the field's signature (type, and a `store` field's
/// `@indexed`/`@bounded`/… annotations) followed by the contextual-keyword doc.
///
/// #611 (gap A): a *reference* to a state field inside the agent's body — a
/// bare read (`lastSeq + 1`), a `:=` write target, an invariant subject, a store
/// op's receiver (`items.put(…)`) — renders the same hover as its declaration.
/// State fields are absent from the project index and are not `let`/param
/// locals, so a reference resolved nowhere before this. The hover handler tries
/// the locals path first, so a local shadowing a field name still hovers as the
/// local — matching the checker, which dispatches a store op only on a bare
/// ident that is *not* in the value scope.
///
/// `None` when the cursor is not on an agent's `key`/`store` keyword, its
/// state-field name, or a reference to one within the agent.
pub fn describe_agent_state_at(source: &str, offset: usize) -> Option<String> {
    let tokens = tokenize(source).ok()?;
    let (unit, _errs) = parse_unit_with_recovery(&tokens, source);
    let unit = unit?;
    let items: &[CommonsItem] = match &unit {
        SourceUnit::Commons(c) => &c.items,
        SourceUnit::Context(c) => &c.items,
        SourceUnit::Adapter(a) => &a.items,
        SourceUnit::Suite(_) => &[],
    };
    // State-migrations S3 (#2248): `removed agent Old`, on the `removed`
    // keyword or the class name.
    if let SourceUnit::Context(c) = &unit {
        for r in &c.removed_agents {
            if span_covers(r.keyword, offset) || span_covers(r.name.span, offset) {
                return Some(format!(
                    "```bynk\nremoved agent {}\n```\n\nThe agent class `{}` is gone: its stored \
                     data is deleted at the next deploy, which needs `--confirm-delete \
                     <context>.{}`.",
                    r.name.name, r.name.name, r.name.name
                ));
            }
        }
    }
    for item in items {
        let CommonsItem::Agent(a) = item else {
            continue;
        };
        // State-migrations S3 (#2248): `agent New replaces Old` — on the
        // `replaces` keyword or the old name.
        if let Some(old) = &a.replaces {
            let on_kw = preceding_ident_span(&tokens, source, old.span, "replaces")
                .is_some_and(|s| span_covers(s, offset));
            if on_kw || span_covers(old.span, offset) {
                return Some(format!(
                    "```bynk\nagent {} replaces {}\n```\n\nRenames the agent class `{}` to `{}`: \
                     its stored data and schema history move to `{}`.",
                    a.name.name, old.name, old.name, a.name.name, a.name.name
                ));
            }
        }
        // `key <name>: <type>` — the cursor on the field name, or on the `key`
        // keyword token immediately preceding it.
        let on_key_kw = preceding_ident_span(&tokens, source, a.key_name.span, "key")
            .is_some_and(|s| span_covers(s, offset));
        if on_key_kw || span_covers(a.key_name.span, offset) {
            return Some(key_hover(a));
        }
        // `store <name>: <kind> <annotations>` — the parser sets each field's
        // span to start at its `store` keyword, so the keyword span is derivable
        // without re-scanning.
        for f in &a.store_fields {
            let store_kw = Span {
                file: f.span.file,
                start: f.span.start,
                end: f.span.start + "store".len(),
            };
            let on_store_kw = source.get(store_kw.start..store_kw.end) == Some("store")
                && span_covers(store_kw, offset);
            if on_store_kw || span_covers(f.name.span, offset) {
                return Some(store_field_hover(f));
            }
        }
        // State-migrations S2 (#2224): the contextual `migrate` keyword (its
        // span starts each step) and `drop` (each drop item's keyword span).
        for m in &a.migrations {
            if span_covers(m.keyword, offset)
                && source.get(m.keyword.start..m.keyword.end) == Some("migrate")
            {
                return Some(migrate_hover(m));
            }
            for item in &m.items {
                if let MigrateItem::Drop(d) = item
                    && span_covers(d.keyword, offset)
                {
                    return Some(format!(
                        "```bynk\ndrop {}\n```\n\nDiscards the stored value of `{}` on purpose: \
                         a removed field the step doesn't read, or a changed `Set`, `Log` or \
                         `Cache` field, which starts afresh.",
                        d.field.name, d.field.name
                    ));
                }
            }
        }
        // #611: a reference to `key`/`store` state from within this agent.
        if !in_state_scope(a, offset) {
            continue;
        }
        let Some((name, name_span)) = ident_at(&tokens, source, offset) else {
            continue;
        };
        // State is referenced by **bare** name, so a member of another value
        // (`p.items`) is not a state reference even when the names coincide.
        if is_dot_preceded(source, name_span.start) {
            continue;
        }
        if name == a.key_name.name {
            return Some(key_hover(a));
        }
        if let Some(f) = a.store_fields.iter().find(|f| f.name.name == name) {
            return Some(store_field_hover(f));
        }
    }
    None
}

/// #706, #2135: the reserved call-site principals of a test case — each with
/// what the `system` driver sends for it. Not declarations: `by <name>` names
/// one of these, never an actor.
pub const RESERVED_PRINCIPALS: &[(&str, &str)] = &[
    (
        "Nobody",
        "Sends no credential — no `Authorization` header on a Bearer route, no signature headers on a `Signature` route — so the real seam refuses it (`Rejected(Unauthorized)`). `system` tier only.",
    ),
    (
        "Forged",
        "Signs a `Signature` route's request with a secret the seam does not hold, so the real seam refuses it (`Rejected(Unauthorized)`). `system` tier only.",
    ),
    (
        "Stale",
        "Signs a `Signature` route's request correctly, timestamped `tolerance + 60` seconds in the past, so the real seam refuses it as a replay (`Rejected(Unauthorized)`). Needs an actor with `timestamp` and `tolerance`. `system` tier only.",
    ),
];

/// #2135: hover for a reserved principal (`Nobody`, `Forged`, `Stale`) in a
/// call-site `by` clause: what the driver sends. `None` when the cursor is not
/// on one of those names right after `by`.
pub fn describe_reserved_principal_at(source: &str, offset: usize) -> Option<String> {
    let tokens = tokenize(source).ok()?;
    let (name, span) = ident_at(&tokens, source, offset)?;
    let (_, doc) = RESERVED_PRINCIPALS.iter().find(|(n, _)| *n == name)?;
    preceding_ident_span(&tokens, source, span, "by")
        .map(|_| format!("```bynk\nby {name}\n```\n\n{doc}"))
}

/// #2134: the contextual `env` word's one-line doc, shown when hovering it.
pub const ENV_KEYWORD_DOC: &str = "The variables and secrets a suite's participants see (`env { NAME = \"value\" … }`): on each Worker's `env` at `system`, and what the `Secrets` double answers at `unit`/`integration`. A case's `env` layers over its suite's, name by name.";

/// #2134: hover for a suite's or case's `env { … }` clause — on the `env`
/// word, its doc ([`ENV_KEYWORD_DOC`]); on an entry's name, the entry and
/// where it applies: "variable given to every participant (suite)", or
/// "(case, overrides suite)" for a case entry its suite also gives. `env` is
/// lexed as an identifier, so neither the keyword fallback nor the
/// declaration rungs reach it. `None` when the cursor is on neither.
pub fn describe_env_at(source: &str, offset: usize) -> Option<String> {
    let tokens = tokenize(source).ok()?;
    let (unit, _errs) = parse_unit_with_recovery(&tokens, source);
    let SourceUnit::Suite(suite) = unit? else {
        return None;
    };
    let on_kw = |e: &EnvClause| {
        span_covers(
            Span {
                file: e.span.file,
                start: e.span.start,
                end: e.span.start + "env".len(),
            },
            offset,
        )
    };
    let suite_gives = |name: &str| {
        suite
            .envs
            .iter()
            .flat_map(|e| e.entries.iter())
            .any(|x| x.name.name == name)
    };
    let render = |entry: &EnvEntry, scope: &str| {
        let value = match &entry.value.kind {
            ExprKind::StrLit(v) => format!("{v:?}"),
            _ => "…".to_string(),
        };
        format!(
            "```bynk\n{} = {value}\n```\n\nvariable given to every participant ({scope})",
            entry.name.name
        )
    };
    for e in &suite.envs {
        if on_kw(e) {
            return Some(ENV_KEYWORD_DOC.to_string());
        }
        if let Some(entry) = e.entries.iter().find(|x| span_covers(x.name.span, offset)) {
            return Some(render(entry, "suite"));
        }
    }
    for case in &suite.cases {
        for e in &case.envs {
            if on_kw(e) {
                return Some(ENV_KEYWORD_DOC.to_string());
            }
            if let Some(entry) = e.entries.iter().find(|x| span_covers(x.name.span, offset)) {
                let scope = if suite_gives(&entry.name.name) {
                    "case, overrides suite"
                } else {
                    "case"
                };
                return Some(render(entry, scope));
            }
        }
    }
    None
}

/// #611: true when `offset` sits where an agent's `key`/`store` state is
/// referenceable by bare name — a handler body, or an invariant/transition
/// predicate. Deliberately narrower than the agent's own span: the declaration
/// region names things that are *not* state references, and a store annotation
/// argument (`@indexed(by: id)`) names a field of the **stored value**, which
/// must not masquerade as a same-named `key`/`store` field.
fn in_state_scope(a: &AgentDecl, offset: usize) -> bool {
    a.handlers.iter().any(|h| span_covers(h.body.span, offset))
        || a.invariants
            .iter()
            .any(|i| span_covers(i.predicate.span, offset))
        || a.transitions
            .iter()
            .any(|t| span_covers(t.predicate.span, offset))
}

/// Hover for a `migrate from N` step (state-migrations S2, #2224).
fn migrate_hover(m: &MigrateDecl) -> String {
    let old = m
        .old
        .as_ref()
        .map(|o| format!(" ({}: {})", o.name.name, type_ref_str(&o.type_ref)))
        .unwrap_or_default();
    format!(
        "```bynk\nmigrate from {}{old}\n```\n\nA migration step: carries a stored record from \
         schema version {} to the agent's current shape when it next loads.",
        m.from, m.from
    )
}

/// The hover for an agent's `key` field — its declaration and every reference.
fn key_hover(a: &AgentDecl) -> String {
    let sig = format!("key {}: {}", a.key_name.name, type_ref_str(&a.key_type));
    render_state_hover(&sig, "key")
}

/// The hover for a `store` field — its declaration and every reference.
fn store_field_hover(f: &StoreField) -> String {
    let mut sig = format!("store {}: {}", f.name.name, store_kind_str(&f.kind));
    for ann in &f.annotations {
        sig.push(' ');
        sig.push_str(&bynk_fmt::annotation_to_string(ann));
    }
    render_state_hover(&sig, "store")
}

/// #611: hover for a `store` field's operation — the `<op>` of a
/// `<field>.<op>(…)` call on an agent's `store` field (`items.put(id, item)`).
/// Store operations are checked but never indexed and are not value-receiver
/// methods, so `qualified_callee_at` (name-receivers only) never reaches them
/// and they resolved nowhere. Renders the operation's signature from the
/// enumerable [`bynk_check::store_ops`] registry — generic in the kind's
/// key/value/element type — over the field's declared kind, which grounds it.
///
/// `locals` guards the receiver the way the checker's dispatch does: a store op
/// is a bare ident receiver that is *not* in the value scope, so a local
/// shadowing the field name makes this an ordinary value method, not a store op.
/// `None` when the cursor is not on a store operation of the enclosing agent.
pub fn describe_store_op_at(
    source: &str,
    offset: usize,
    locals: &[bynk_check::locals::LocalBinding],
) -> Option<String> {
    let tokens = tokenize(source).ok()?;
    let (unit, _errs) = parse_unit_with_recovery(&tokens, source);
    let unit = unit?;
    let items: &[CommonsItem] = match &unit {
        SourceUnit::Commons(c) => &c.items,
        SourceUnit::Context(c) => &c.items,
        SourceUnit::Adapter(a) => &a.items,
        SourceUnit::Suite(_) => &[],
    };
    // The cursor must sit on the `<op>` of a `<recv>.<op>` access.
    let (op, op_span) = ident_at(&tokens, source, offset)?;
    let (recv, recv_start) = receiver_segment_at(source, op_span)?;
    // The checker dispatches a store op on a **bare** ident receiver only, so a
    // qualified one is not one: `p.items.contains(…)` is an ordinary value method
    // on a record field that merely shares a store field's name.
    if is_dot_preceded(source, recv_start) {
        return None;
    }
    // A local of the receiver's name shadows the store field (the same
    // by-provenance dispatch) — then this is a value method, not a store op.
    if bynk_check::locals::locals_at(locals, recv_start)
        .iter()
        .any(|b| b.name == recv)
    {
        return None;
    }
    for item in items {
        let CommonsItem::Agent(a) = item else {
            continue;
        };
        if !in_state_scope(a, offset) {
            continue;
        }
        let f = a.store_fields.iter().find(|f| f.name.name == recv)?;
        let sig = bynk_check::store_ops::ops_for(&f.kind.head.name)
            .iter()
            .find(|o| o.name == op)?
            .signature;
        return Some(format!(
            "```bynk\n{sig}\n```\n\nA `{}` store operation on `store {}: {}` — the field's \
             declared kind grounds the operation's type parameters.",
            f.kind.head.name,
            f.name.name,
            store_kind_str(&f.kind),
        ));
    }
    None
}

/// The identifier token covering `offset` — its text and span — if the cursor is
/// on one.
fn ident_at<'a>(
    tokens: &[bynk_syntax::lexer::Token],
    source: &'a str,
    offset: usize,
) -> Option<(&'a str, Span)> {
    tokens
        .iter()
        .find(|t| t.kind == bynk_syntax::lexer::TokenKind::Ident && span_covers(t.span, offset))
        .and_then(|t| Some((source.get(t.span.start..t.span.end)?, t.span)))
}

/// The receiver segment of a `<recv>.<member>` access whose member sits at
/// `member_span`: the identifier run immediately before the dot, and the offset
/// it starts at. `None` when the member is not dot-preceded. Shared by every
/// caller that reads a receiver off the line prefix, so the extraction has one
/// definition rather than a copy per call site — delegates the actual boundary
/// scan to `completion::ident_ending_at`.
fn receiver_segment_at(text: &str, member_span: Span) -> Option<(&str, usize)> {
    let before = text.get(..member_span.start)?.strip_suffix('.')?;
    crate::completion::ident_ending_at(before, before.len())
}

/// True when the identifier starting at `start` is itself the member of a
/// further access (the `items` of `p.items`) rather than a bare name.
pub(crate) fn is_dot_preceded(text: &str, start: usize) -> bool {
    // A `..` is a rest marker (`[x, ..rest]`, `T { f, .. }`), not a member
    // access, so the name after it is a fresh binding.
    text[..start].ends_with('.') && !text[..start].ends_with("..")
}

/// #596: the storage-kind vocabulary a bare receiver is eligible for at
/// completion's `<recv>.` position — `recv_end` is the offset just past the
/// receiver identifier (where its dot sat before
/// `completion::value_receiver_rewrite` dropped it, so `source` here is that
/// rewritten buffer). Mirrors `describe_store_op_at`'s by-provenance receiver
/// check (not shadowed by a local, inside the declaring agent's state scope),
/// but starts from the receiver's own end offset rather than walking back from
/// an operation token, since completion fires before any member name is
/// typed — and the rewrite's postcondition already guarantees a bare,
/// non-dot-qualified name reaches here, so unlike `describe_store_op_at` there
/// is no further `is_dot_preceded` check to make.
///
/// Returns the field's storage-kind head (`"Map"`, `"Cache"`, `"Set"`,
/// `"Cell"`, `"Log"`) and, for a `Map`, whether its value type is a held
/// `Connection` (v0.158, ADR 0184: `.entries`/`.keys`/`.values` are refused on
/// one) — enough for the completion layer to look up each kind's registry
/// without re-parsing.
pub fn store_field_kind_at(
    source: &str,
    recv_end: usize,
    locals: &[bynk_check::locals::LocalBinding],
) -> Option<(String, bool)> {
    let (recv, recv_start) = crate::completion::ident_ending_at(source, recv_end)?;
    if bynk_check::locals::locals_at(locals, recv_start)
        .iter()
        .any(|b| b.name == recv)
    {
        return None;
    }
    let tokens = tokenize(source).ok()?;
    let (unit, _errs) = parse_unit_with_recovery(&tokens, source);
    let unit = unit?;
    let items: &[CommonsItem] = match &unit {
        SourceUnit::Commons(c) => &c.items,
        SourceUnit::Context(c) => &c.items,
        SourceUnit::Adapter(a) => &a.items,
        SourceUnit::Suite(_) => &[],
    };
    for item in items {
        let CommonsItem::Agent(a) = item else {
            continue;
        };
        if !in_state_scope(a, recv_end) {
            continue;
        }
        if let Some(f) = a.store_fields.iter().find(|f| f.name.name == recv) {
            let held = f.kind.head.name == "Map"
                && f.kind.args.len() == 2
                && matches!(f.kind.args[1], TypeRef::Connection(..));
            return Some((f.kind.head.name.clone(), held));
        }
    }
    None
}

/// v0.140 (ADR 0163): hover for a handler-position annotation (`@cache`). Handler
/// annotations are not symbols and declare no local, so they miss both the
/// `describe_symbol` and locals paths — this closes the gap. For the cursor
/// anywhere within a handler's `@cache( … )` annotation, render the formatted
/// annotation followed by a prose description of `@cache` and its fields. `None`
/// when the cursor is not inside a handler annotation.
pub fn describe_handler_annotation_at(source: &str, offset: usize) -> Option<String> {
    let tokens = tokenize(source).ok()?;
    let (unit, _errs) = parse_unit_with_recovery(&tokens, source);
    let unit = unit?;
    let items: &[CommonsItem] = match &unit {
        SourceUnit::Commons(c) => &c.items,
        SourceUnit::Context(c) => &c.items,
        SourceUnit::Adapter(a) => &a.items,
        SourceUnit::Suite(_) => &[],
    };
    for item in items {
        let (handlers, service_cap): (&[Handler], Option<i64>) = match item {
            CommonsItem::Service(s) => (&s.handlers, s.limits.as_ref().and_then(|l| l.max_body())),
            CommonsItem::Agent(a) => (&a.handlers, None),
            _ => continue,
        };
        for h in handlers {
            for ann in &h.annotations {
                if span_covers(ann.span, offset) {
                    return Some(render_handler_annotation_hover(ann, h, service_cap));
                }
            }
        }
    }
    None
}

/// #2137 (ADR 0156): hover for the contextual `advance` keyword of an
/// `advance Clock by <Duration>` statement. `None` anywhere else, so an
/// ordinary identifier named `advance` keeps its own hover.
pub fn describe_advance_at(source: &str, offset: usize) -> Option<String> {
    let tokens = tokenize(source).ok()?;
    let idx = tokens
        .iter()
        .position(|t| t.span.start <= offset && offset < t.span.end)?;
    let text = |i: usize| tokens.get(i).map(|t| &source[t.span.start..t.span.end]);
    if text(idx)? != "advance"
        || tokens.get(idx + 1)?.kind != bynk_syntax::lexer::TokenKind::Ident
        || tokens.get(idx + 2)?.kind != bynk_syntax::lexer::TokenKind::By
    {
        return None;
    }
    Some(
        "```bynk\nadvance Clock by <Duration>\n```\n\nMoves the test harness's clock forward. Each case's clock starts at the epoch; at `unit` and `integration` the `Clock` double reads it, and at `system` every participant's first-party `Clock` does. It governs `Clock` only: JWT expiry, the `Signature` tolerance and Durable Object alarms keep wall-clock time."
            .to_string(),
    )
}

/// Constrained generics slice 1 (ADR 0156): hover for the contextual `derives`
/// keyword of a type declaration's `derives Ord` clause. `None` anywhere else,
/// so an ordinary identifier named `derives` keeps its own hover.
pub fn describe_derives_at(source: &str, offset: usize) -> Option<String> {
    let tokens = tokenize(source).ok()?;
    let (unit, _errs) = parse_unit_with_recovery(&tokens, source);
    let items: &[CommonsItem] = match unit.as_ref()? {
        SourceUnit::Commons(c) => &c.items,
        SourceUnit::Context(c) => &c.items,
        SourceUnit::Adapter(a) => &a.items,
        SourceUnit::Suite(_) => return None,
    };
    items.iter().find_map(|item| match item {
        CommonsItem::Type(t) if t.derives_kw.is_some_and(|k| k.start <= offset && offset < k.end) => {
            Some(
                "```bynk\ntype Priority = enum { Low, High } derives Ord\n```\n\nOpts the type into a constraint. `derives Ord` orders the type by declaration order: a record compares field by field, then a sum by variant, so reordering the declaration changes the order. Every field must itself be orderable. `Eq`, `Key` and `Codable` are automatic and are not derived."
                    .to_string(),
            )
        }
        _ => None,
    })
}

/// The doc for a constraint name (`Eq`, `Ord`), shown on hover wherever one
/// appears in a bound or a `derives` clause.
fn constraint_doc(name: &str) -> Option<&'static str> {
    match name {
        "Eq" => Some(
            "```bynk\nconstraint Eq\n```\n\nValues of the type compare with `==` and `!=`. Declare it on a type parameter (`T: Eq`) to compare its values inside the function. Every type that can be compared is `Eq` automatically.",
        ),
        "Ord" => Some(
            "```bynk\nconstraint Ord\n```\n\nValues of the type are ordered: they compare with `<`, `<=`, `>` and `>=`, and can be sorted. Declare it on a type parameter (`T: Ord`) to order its values inside the function; `Ord` implies `Eq`. A type opts in with `derives Ord`, which orders it by declaration order.",
        ),
        _ => None,
    }
}

/// Constrained generics slice 1: hover for a constraint name in a bound
/// (`T: Ord`, `where A: Eq`) or a `derives` clause. `None` for any other
/// identifier, so a type that happens to be named `Ord` keeps its own hover.
pub fn describe_constraint_at(source: &str, offset: usize) -> Option<String> {
    let span = constraint_token_spans(source)
        .into_iter()
        .find(|s| s.start <= offset && offset < s.end)?;
    constraint_doc(&source[span.start..span.end]).map(str::to_string)
}

/// #2640 (ADR 0156): hover for the contextual `par` keyword of a `par { … }`
/// binding block. `None` anywhere else, so an ordinary identifier named `par`
/// keeps its own hover.
pub fn describe_par_at(source: &str, offset: usize) -> Option<String> {
    let tokens = tokenize(source).ok()?;
    let idx = tokens
        .iter()
        .position(|t| t.span.start <= offset && offset < t.span.end)?;
    let text = |i: usize| tokens.get(i).map(|t| &source[t.span.start..t.span.end]);
    if text(idx)? != "par" || tokens.get(idx + 1)?.kind != bynk_syntax::lexer::TokenKind::LBrace {
        return None;
    }
    Some(
        "```bynk\npar {\n  let a <- first()\n  let b <- second()\n}\n```\n\nIssues every effect in the block, in source order, before awaiting any of them, and completes when all have settled. The names bound inside are in scope after the closing brace; no binding may mention a name a sibling binds. If any effect faults, the block waits for the rest, then raises the first fault in source order. Holds only `let x <- e` and `do e`."
            .to_string(),
    )
}

/// #2618: a hover for the contextual `private` modifier under the cursor:
/// an identifier spelled `private` immediately before a `fn`, `type`,
/// `messages`, `asset`, `embed` or `const` keyword, the one position the parser reads
/// it as the modifier. `None` anywhere else, where `private` is an ordinary
/// name.
pub fn describe_private_at(source: &str, offset: usize) -> Option<String> {
    use bynk_syntax::lexer::TokenKind;
    let tokens = tokenize(source).ok()?;
    let idx = tokens
        .iter()
        .position(|t| t.span.start <= offset && offset < t.span.end)?;
    let t = tokens.get(idx)?;
    if t.kind != TokenKind::Ident || &source[t.span.start..t.span.end] != "private" {
        return None;
    }
    let next = tokens.get(idx + 1)?.kind;
    if !matches!(
        next,
        TokenKind::Fn
            | TokenKind::Type
            | TokenKind::Messages
            | TokenKind::Asset
            | TokenKind::Embed
            | TokenKind::Const
    ) {
        return None;
    }
    let doc = bynk_syntax::keywords::CONTEXTUAL_KEYWORDS
        .iter()
        .find(|k| k.word == "private")
        .map(|k| k.meaning)
        .unwrap_or_default();
    Some(format!("```bynk\nprivate\n```\n\n{doc}"))
}

/// #2138: the doc `run.id` shows on hover and in completion.
pub const RUN_ID_DOC: &str = "**`run.id`** — this test run's identity, a `String`. In process it is always `\"local\"`; each `bynk test --against` run gets a fresh one, so a key built from it (`\"order-\\(run.id)\"`) doesn't collide with an earlier run's data on a shared deployment.";

/// #2138: whether the `run` identifier starting at `run_start` in `source`
/// is the test run's `run` — inside a suite's `case` body, with no binder of
/// its own in scope there. #2175: the scope is the checker's: a `let` earlier
/// in any enclosing block, a lambda parameter, a `match` arm's pattern, or an
/// `is <pattern>` test's bindings all shadow it. #2181: an `is` test binds
/// where the checker says it does (`bynk_check::narrowing`): in an `if`'s
/// `then` block when the condition holds, in its `else` block when it fails
/// (`if !(v is Some(run)) { … } else { run.id }`), and on the right of `&&`
/// and `implies` (`v is Some(run) && run.id == "x"`).
pub fn is_test_run_receiver(source: &str, run_start: usize) -> bool {
    let Ok(tokens) = tokenize(source) else {
        return false;
    };
    let (unit, _errs) = parse_unit_with_recovery(&tokens, source);
    let Some(SourceUnit::Suite(suite)) = unit else {
        return false;
    };
    let Some(case) = suite.cases.iter().find(|c| span_covers(c.span, run_start)) else {
        return false;
    };
    run_bound_in_block(&case.body, run_start, &mut Vec::new()) != Some(true)
}

/// Whether the `run` at `at` is bound by a binder in scope, if `at` is in
/// `block`; `bound` holds the names bound around it.
fn run_bound_in_block(block: &Block, at: usize, bound: &mut Vec<String>) -> Option<bool> {
    let mark = bound.len();
    let mut found = None;
    for stmt in &block.statements {
        let mut exprs = Vec::new();
        statement_exprs(stmt, &mut exprs);
        found = exprs
            .into_iter()
            .find_map(|e| run_bound_in_expr(e, at, bound));
        if found.is_some() {
            break;
        }
        // A `let`'s own value doesn't see its name; what follows does.
        if let Statement::Let(l) | Statement::EffectLet(l) = stmt {
            bound.push(l.name.name.clone());
        }
        // #2640: a `par` block's names enter scope after its closing brace.
        if let Statement::Par(p) = stmt {
            bound.extend(p.statements.iter().filter_map(|s| match s {
                Statement::EffectLet(l) => Some(l.name.name.clone()),
                _ => None,
            }));
        }
    }
    let found = found.or_else(|| run_bound_in_expr(&block.tail, at, bound));
    bound.truncate(mark);
    found
}

fn run_bound_in_expr(e: &Expr, at: usize, bound: &mut Vec<String>) -> Option<bool> {
    if !span_covers(e.span, at) {
        return None;
    }
    let mark = bound.len();
    let found = match &e.kind {
        ExprKind::Ident(id) if id.span.start == at && id.name == "run" => {
            Some(bound.iter().any(|n| n == "run"))
        }
        ExprKind::Block(b) => run_bound_in_block(b, at, bound),
        ExprKind::Lambda(l) => {
            bound.extend(l.params.iter().map(|p| p.name.name.clone()));
            run_bound_in_expr(&l.body, at, bound)
        }
        ExprKind::If {
            cond,
            then_block,
            else_block,
        } => run_bound_in_expr(cond, at, bound)
            .or_else(|| {
                let inner = bound.len();
                bound.extend(is_bound_names(cond, true));
                let r = run_bound_in_block(then_block, at, bound);
                bound.truncate(inner);
                r
            })
            .or_else(|| {
                let inner = bound.len();
                bound.extend(is_bound_names(cond, false));
                let r = run_bound_in_block(else_block, at, bound);
                bound.truncate(inner);
                r
            }),
        // The left side's `is` bindings reach the right side.
        ExprKind::BinOp(BinOp::And | BinOp::Implies, lhs, rhs) => run_bound_in_expr(lhs, at, bound)
            .or_else(|| {
                bound.extend(is_bound_names(lhs, true));
                run_bound_in_expr(rhs, at, bound)
            }),
        ExprKind::Match { discriminant, arms } => run_bound_in_expr(discriminant, at, bound)
            .or_else(|| {
                arms.iter()
                    .find(|a| span_covers(a.span, at))
                    .and_then(|arm| {
                        bound.extend(
                            arm.pattern
                                .bound_names()
                                .into_iter()
                                .map(|id| id.name.clone()),
                        );
                        arm.guard
                            .as_ref()
                            .and_then(|g| run_bound_in_expr(g, at, bound))
                            .or_else(|| match &arm.body {
                                MatchBody::Expr(body) => run_bound_in_expr(body, at, bound),
                                MatchBody::Block(b) => run_bound_in_block(b, at, bound),
                            })
                    })
            }),
        _ => expr_children(e)
            .into_iter()
            .find_map(|c| run_bound_in_expr(c, at, bound)),
    };
    bound.truncate(mark);
    found
}

/// The names `cond`'s `is <pattern>` tests bind when it evaluates to
/// `when_true`, by the checker's own rule.
fn is_bound_names(cond: &Expr, when_true: bool) -> Vec<String> {
    bynk_check::narrowing::matched_is_tests(cond, when_true)
        .into_iter()
        .filter_map(|test| match &test.kind {
            ExprKind::Is { pattern, .. } => Some(pattern.bound_names()),
            _ => None,
        })
        .flatten()
        .map(|id| id.name.clone())
        .collect()
}

/// #2138 (ADR 0156): hover for `run.id` in a `case` body — on either the
/// `run` or the `id`. `None` anywhere else, and where the case binds its own
/// `run`, so an ordinary value keeps its own hover.
pub fn describe_run_id_at(source: &str, offset: usize) -> Option<String> {
    // A character scan, not the token stream: `run.id` is often inside a
    // string interpolation (`"order-\(run.id)"`), which lexes as one token.
    let is_ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let offset = offset.min(source.len());
    let start = source[..offset]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_ident(*c))
        .last()
        .map_or(offset, |(i, _)| i);
    let end = source[start..]
        .char_indices()
        .find(|(_, c)| !is_ident(*c))
        .map_or(source.len(), |(i, _)| start + i);
    let ends_ident = |rest: &str| !rest.starts_with(is_ident);
    let run = match &source[start..end] {
        "run" if source[end..].strip_prefix(".id").is_some_and(ends_ident) => start,
        "id" if source[..start].ends_with("run.") => start - "run.".len(),
        _ => return None,
    };
    if source[..run].ends_with(|c: char| is_ident(c) || c == '.') {
        return None; // `x.run.id` or `myrun.id`: not the test run
    }
    is_test_run_receiver(source, run).then(|| RUN_ID_DOC.to_string())
}

/// #2136 (ADR 0156): hover for the `schedule` / `message` address name in a
/// `system`-tier case (`sweeper.schedule(…)`, `deliveries.message(m)`), which
/// runs the handler through the participant Worker's own `scheduled` / `queue`
/// entry point rather than in process. `None` anywhere else, including the
/// same address at `unit` or `integration`, and a `.schedule` / `.message`
/// whose receiver is not a `cron` / `queue` service of the suite's target (a
/// record field named `message`, say). The target's declaration is looked up
/// in `source` and `others` (the project's other files).
pub fn describe_system_job_address_at<'s>(
    source: &'s str,
    offset: usize,
    others: impl IntoIterator<Item = &'s str>,
) -> Option<String> {
    let is_ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let start = source[..offset.min(source.len())]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_ident(*c))
        .last()
        .map_or(offset, |(i, _)| i);
    let end = source[start..]
        .char_indices()
        .find(|(_, c)| !is_ident(*c))
        .map_or(source.len(), |(i, _)| start + i);
    let name = &source[start..end];
    if !source[..start].ends_with('.') || !matches!(name, "schedule" | "message") {
        return None;
    }
    let tokens = tokenize(source).ok()?;
    let (unit, _errs) = parse_unit_with_recovery(&tokens, source);
    let SourceUnit::Suite(suite) = unit? else {
        return None;
    };
    let case = suite.cases.iter().find(|c| span_covers(c.span, start))?;
    if case.tier.or(suite.tier) != Some(TestTier::System) {
        return None;
    }
    // The receiver: the identifier right before the `.`.
    let before = &source[..start - 1];
    let receiver = &before[before
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_ident(*c))
        .last()
        .map_or(before.len(), |(i, _)| i)..];
    let target = suite.target.joined();
    let is_job_service = std::iter::once(source).chain(others).any(|src| {
        let Ok(tokens) = tokenize(src) else {
            return false;
        };
        let (unit, _errs) = parse_unit_with_recovery(&tokens, src);
        let Some(SourceUnit::Context(ctx)) = unit else {
            return false;
        };
        ctx.name.joined() == target
            && ctx.items.iter().any(|item| {
                matches!(item, CommonsItem::Service(svc)
                if svc.name.name == receiver
                    && match name {
                        "schedule" => matches!(svc.protocol, ServiceProtocol::Cron),
                        _ => matches!(svc.protocol, ServiceProtocol::Queue { .. }),
                    })
            })
    });
    if !is_job_service {
        return None;
    }
    Some(if name == "schedule" {
        "**`svc.schedule(\"<expr>\", at)`** at `system` — runs the cron handler through the Worker's `scheduled` entry, with `{ cron, scheduledTime: at }`, and yields the handler's own `Result`.".to_string()
    } else {
        "**`q.message(m)`** at `system` — delivers `m` through the Worker's `queue` entry, after a JSON round trip, and yields `Ack` or `Retry(reason)` as the consumer acknowledged or retried it.".to_string()
    })
}

/// v0.140 (ADR 0163): the spans to classify as `decorator` semantic tokens — each
/// handler annotation's `@name` (the `@` through the name) and its argument labels
/// (`maxAge:`, `scope:`). Parsed from `source`; empty when it carries no handler
/// annotations. Feeds the semantic-tokens producer, which is otherwise a
/// parse-free index read, so the parse lives here beside the hover parse.
pub fn handler_annotation_token_spans(source: &str) -> Vec<Span> {
    let Ok(tokens) = tokenize(source) else {
        return Vec::new();
    };
    let (unit, _errs) = parse_unit_with_recovery(&tokens, source);
    let Some(unit) = unit else {
        return Vec::new();
    };
    let items: &[CommonsItem] = match &unit {
        SourceUnit::Commons(c) => &c.items,
        SourceUnit::Context(c) => &c.items,
        SourceUnit::Adapter(a) => &a.items,
        SourceUnit::Suite(_) => &[],
    };
    let mut spans = Vec::new();
    for item in items {
        let handlers: &[Handler] = match item {
            CommonsItem::Service(s) => &s.handlers,
            CommonsItem::Agent(a) => &a.handlers,
            _ => continue,
        };
        for h in handlers {
            for ann in &h.annotations {
                // The `@name` — from the annotation's leading `@` through its name.
                spans.push(Span {
                    file: ann.span.file,
                    start: ann.span.start,
                    end: ann.name.span.end,
                });
                // Each argument label (`maxAge:`, `scope:`).
                for arg in &ann.args {
                    if let Some(label) = &arg.label {
                        spans.push(label.span);
                    }
                }
            }
        }
    }
    spans
}

/// #2618: the span of every `private` modifier on the unit's declarations,
/// for the `keyword` semantic token. `private` is contextual, so an editor
/// grammar that matches keywords by spelling cannot tell the modifier from a
/// binding named `private`; the parse can.
pub fn private_modifier_token_spans(source: &str) -> Vec<Span> {
    let Ok(tokens) = tokenize(source) else {
        return Vec::new();
    };
    let (unit, _errs) = parse_unit_with_recovery(&tokens, source);
    let Some(unit) = unit else {
        return Vec::new();
    };
    let items: &[CommonsItem] = match &unit {
        SourceUnit::Commons(c) => &c.items,
        SourceUnit::Context(c) => &c.items,
        SourceUnit::Adapter(a) => &a.items,
        SourceUnit::Suite(_) => &[],
    };
    items.iter().filter_map(CommonsItem::private_span).collect()
}

/// Constrained generics slice 0: the span of every constraint name in a bound
/// — a type parameter's (`T: Eq` on a `fn`, `type` or capability operation)
/// and a method's `where A: Eq` entries — so the editor can classify them as
/// `interface` tokens.
pub fn constraint_token_spans(source: &str) -> Vec<Span> {
    let Ok(tokens) = tokenize(source) else {
        return Vec::new();
    };
    let (unit, _errs) = parse_unit_with_recovery(&tokens, source);
    let Some(unit) = unit else {
        return Vec::new();
    };
    let items: &[CommonsItem] = match &unit {
        SourceUnit::Commons(c) => &c.items,
        SourceUnit::Context(c) => &c.items,
        SourceUnit::Adapter(a) => &a.items,
        SourceUnit::Suite(_) => &[],
    };
    let mut spans = Vec::new();
    fn params(tps: &[TypeParam], spans: &mut Vec<Span>) {
        for tp in tps {
            spans.extend(tp.bounds.iter().map(|b| b.span));
        }
    }
    for item in items {
        match item {
            CommonsItem::Fn(f) => params(&f.type_params, &mut spans),
            CommonsItem::Type(t) => {
                params(&t.type_params, &mut spans);
                // Constrained generics slice 1: `derives Ord` names constraints too.
                spans.extend(t.derives.iter().map(|d| d.span));
            }
            CommonsItem::Capability(c) => {
                for op in &c.ops {
                    params(&op.type_params, &mut spans);
                }
            }
            _ => {}
        }
        if let CommonsItem::Fn(f) = item {
            for wb in &f.where_bounds {
                spans.extend(wb.bounds.iter().map(|b| b.span));
            }
        }
    }
    spans
}

/// The `Cache-Control` value a well-formed `@cache` emits — the same reading
/// `bynk_lower::lower_route_cache_ir` makes, restated here because the IDE does
/// not depend on the lowering crate. `None` when `maxAge` is missing or not a
/// whole-second `Duration` literal (the checker flags those).
fn cache_control_of(ann: &Annotation) -> Option<String> {
    let mut max_age_secs = None;
    let mut scope = "private";
    let mut immutable = false;
    for arg in &ann.args {
        match (arg.label.as_ref().map(|l| l.name.as_str()), &arg.value.kind) {
            (Some("maxAge"), ExprKind::DurationLit { millis, .. })
                if *millis > 0 && millis % 1000 == 0 =>
            {
                max_age_secs = Some(millis / 1000);
            }
            (Some("scope"), ExprKind::Ident(id)) if id.name == "public" => scope = "public",
            (Some("immutable"), ExprKind::BoolLit(b)) => immutable = *b,
            _ => {}
        }
    }
    let secs = max_age_secs?;
    let suffix = if immutable { ", immutable" } else { "" };
    Some(format!("{scope}, max-age={secs}{suffix}"))
}

/// The formatted annotation in a code block, plus a prose description for the
/// closed handler-annotation set. `@cache`, `@limit` (v0.142) and `@raw` carry prose; any
/// other name (a typo the checker will flag) still hovers as its formatted form so
/// the surface is never silent.
fn render_handler_annotation_hover(
    ann: &Annotation,
    handler: &Handler,
    service_cap: Option<i64>,
) -> String {
    let sig = bynk_fmt::annotation_to_string(ann);
    if ann.name.name == "cache" {
        let emitted = match cache_control_of(ann) {
            Some(header) => format!("\n\nEmits `Cache-Control: {header}`."),
            None => String::new(),
        };
        return format!(
            "```bynk\n{sig}\n```\n\n\
             **`@cache`** — cache this `GET` read. A `GET` returning `Ok` (or a `Raw` body \
             of at most 1 MiB) already carries a synthesised weak `ETag`, and `RawTagged` \
             carries the handler's own; a matching `If-None-Match` is answered \
             `304 Not Modified`. `@cache` adds a `Cache-Control` freshness window on top.\n\n\
             - **`maxAge`** — the freshness window, a `Duration` (e.g. `5.minutes`), lowered to \
             `Cache-Control: max-age`.\n\
             - **`scope`** — `public` or `private` (default `private`; a shared cache stores the \
             response only when `public`).\n\
             - **`immutable`** — `true` promises the response at this URL never changes, so \
             caches do not revalidate it within `maxAge` (default `false`).{emitted}"
        );
    }
    // v0.142 (ADR 0165): `@limit` caps the request body size on a write route.
    if ann.name.name == "limit" {
        return format!(
            "```bynk\n{sig}\n```\n\n\
             **`@limit`** — cap the request body size on this `POST`/`PUT`/`PATCH` route. A \
             request whose body exceeds the `maxBody` byte ceiling is answered `413 Payload Too \
             Large`, synthesised before the body is read.\n\n\
             - **`maxBody`** — the maximum request body size in bytes, a positive `Int`."
        );
    }
    // `@raw` takes the request body as the octets sent, under a declared media type.
    if ann.name.name == "raw" {
        let cap = match handler.limit_max_body().or(service_cap) {
            Some(bytes) => format!("{bytes} bytes (from `@limit` or the service `limits`)"),
            None => "none — a `@raw` route needs one (`bynk.http.raw_body_unbounded`)".to_string(),
        };
        let content_type = match handler.raw_content_type() {
            Some("*/*") => "any".to_string(),
            Some(ct) => format!("`{ct}`"),
            None => "—".to_string(),
        };
        return format!(
            "```bynk\n{sig}\n```\n\n\
             **`@raw`** — take this route's `body: Bytes` as the raw request body: the octets as \
             sent, with no JSON and no base64. Any other `Content-Type` is answered `415 \
             Unsupported Media Type`.\n\n\
             - **`contentType`** — the media type accepted: {content_type}.\n\
             - **Effective body cap** — {cap}."
        );
    }
    format!("```bynk\n{sig}\n```")
}

/// True when `offset` falls within `span` (half-open, as hover offsets are).
fn span_covers(span: Span, offset: usize) -> bool {
    span.start <= offset && offset < span.end
}

/// The span of the token immediately preceding the token that begins at
/// `name_span`, if that preceding token's source text is exactly `kw` — used to
/// locate a contextual keyword (`key`) that the AST records only by its effect,
/// not with a span of its own.
fn preceding_ident_span(
    tokens: &[bynk_syntax::lexer::Token],
    source: &str,
    name_span: Span,
    kw: &str,
) -> Option<Span> {
    let idx = tokens
        .iter()
        .position(|t| t.span.start == name_span.start)?;
    let prev = tokens.get(idx.checked_sub(1)?)?;
    (source.get(prev.span.start..prev.span.end) == Some(kw)).then_some(prev.span)
}

/// A code-block field signature followed by the contextual keyword's one-line
/// doc from [`bynk_syntax::keywords::CONTEXTUAL_KEYWORDS`].
fn render_state_hover(sig: &str, contextual_kw: &str) -> String {
    let doc = bynk_syntax::keywords::CONTEXTUAL_KEYWORDS
        .iter()
        .find(|k| k.word == contextual_kw)
        .map(|k| k.meaning)
        .unwrap_or_default();
    format!("```bynk\n{sig}\n```\n\n{doc}")
}

/// v0.122 (editor-currency slice 1): a hover summary for `self` under the
/// cursor — `self: <Type>`. `self` is a reserved keyword (never an `Ident`, so
/// it does not flow through `locals_nav`), but a `self` *use* is a typed
/// expression, so its type is in `expr_types` at the token's span. For a method
/// the type is the receiver's name; for an agent handler the checker gives
/// `self` a synthetic record type `__<Agent>Self` (to resolve `self.<key>`),
/// which is un-synthesised here to `<Agent>`. `None` when the cursor is not on
/// the `self` keyword or its type is unknown (no `expr_types` entry for it —
/// on a broken buffer ADR 0094's partial types usually still carry one; when
/// they don't, this degrades to the keyword doc).
pub fn describe_self_at(
    text: &str,
    offset: usize,
    expr_types: &[(Span, bynk_check::checker::TyId)],
    tys: &bynk_check::checker::Types,
) -> Option<String> {
    let tokens = tokenize(text).ok()?;
    let on_self = tokens.iter().any(|t| {
        t.span.start <= offset && offset < t.span.end && &text[t.span.start..t.span.end] == "self"
    });
    if !on_self {
        return None;
    }
    let ty = bynk_check::expr_types::type_at_offset(expr_types, offset, tys)?;
    let display = ty.display(tys);
    let name = display
        .strip_prefix("__")
        .and_then(|s| s.strip_suffix("Self"))
        .unwrap_or(&display);
    Some(format!("```bynk\nself: {name}\n```"))
}

/// #2683, #2711: hover on the `within` of `e.within(d)` or the `race` of
/// `a.race(b)`, where the receiver is an `Effect`: its signature and what it
/// does and does not do. The kernel methods have no declaration to read a doc
/// from, so these are written here; the receiver's type keeps a user type's own
/// `within` or `race` method from being described as them.
pub fn describe_effect_method_at(
    text: &str,
    offset: usize,
    expr_types: &[(Span, bynk_check::checker::TyId)],
    tys: &bynk_check::checker::Types,
) -> Option<String> {
    let tokens = tokenize(text).ok()?;
    let tok = tokens.iter().find(|t| {
        t.span.start <= offset
            && offset < t.span.end
            && matches!(&text[t.span.start..t.span.end], "within" | "race")
    })?;
    let word = &text[tok.span.start..tok.span.end];
    let before = text[..tok.span.start].trim_end();
    let after = text[tok.span.end..].trim_start();
    if !before.ends_with('.') || !after.starts_with('(') {
        return None;
    }
    let dot = before.len() - 1;
    let recv = bynk_check::expr_types::type_at_offset(expr_types, dot.checked_sub(1)?, tys)?;
    if !matches!(&*tys.get(recv), bynk_check::checker::Ty::Effect(_)) {
        return None;
    }
    if word == "race" {
        return Some(
            "```bynk\nrace(other: Effect[T]) -> Effect[T]\n```\n\n\
             Takes whichever of the two effects succeeds first. A side that faults does not \
             win while the other may still succeed; if both fault, the first fault in source \
             order is raised, with the other attached as `suppressed`. A same-instant tie goes \
             to the receiver.\n\n\
             **The loser is not cancelled.** It is handed to the platform's `waitUntil` and \
             runs to completion, so its side effects still happen: use `race` for idempotent \
             work such as a read, not a write. Use `FetchOptions.timeout` when work must \
             actually stop.\n\n\
             Only in a service handler's body (lambdas included): an agent handler or a \
             function has no execution context to hand the loser to."
                .to_string(),
        );
    }
    Some(
        "```bynk\nwithin(d: Duration) -> Effect[Result[T, TimedOut]]\n```\n\n\
         Bounds how long the effect may take. If it settles within `d` the result is `Ok`; \
         if `d` passes first it is `Err(TimedOut { after: d })` and the handler moves on.\n\n\
         **The effect is not cancelled.** It is handed to the platform's `waitUntil` and may \
         still complete after the handler has returned, so its side effects can still happen. \
         Use `FetchOptions.timeout` when the work must actually stop.\n\n\
         Only in a service handler's body (lambdas included): an agent handler or a function \
         has no execution context to hand the effect to."
            .to_string(),
    )
}

/// v0.123 (editor-currency slice 2, DECISION B): if the identifier at
/// `ident_span` is the member of an `Upper.member` name-receiver access
/// (`Clock.now`, `Email.of`), return the full `Recv.member` callee for
/// [`crate::signature_help::resolve_label`] to resolve to its signature — the
/// same resolution completion and signature help perform, no new index.
/// `None` for a bare identifier or a lowercase (value-receiver) method, which
/// `resolve_label` does not handle.
pub fn qualified_callee_at(text: &str, ident_span: Span) -> Option<String> {
    let (recv, _) = receiver_segment_at(text, ident_span)?;
    if !recv.chars().next()?.is_uppercase() {
        return None;
    }
    let member = text.get(ident_span.start..ident_span.end)?;
    Some(format!("{recv}.{member}"))
}

/// Describe a symbol declared in the embedded first-party sources — the `bynk`
/// and `bynk.cloudflare` adapters and the `bynk.list`/`bynk.map`/`bynk.string`
/// stdlib. Hover and completion-doc resolution otherwise walk only the project's
/// files (`walk_bynk_files`), so stdlib/surface symbols had no surfaced signature
/// or doc; this is the fallback after the project scan. Any `---` doc block on a
/// first-party declaration rides along (via `describe_fn`/`describe_type`/…),
/// once the sources carry one.
pub fn describe_firstparty_symbol(name: &str) -> Option<String> {
    // The single first-party source list (`bynk-check::firstparty`), so a new
    // first-party commons is hoverable without a second edit here (#901).
    bynk_check::firstparty::FIRSTPARTY_SOURCES
        .iter()
        .find_map(|(unit, src)| {
            describe_symbol(src, name).map(|d| d + &firstparty_notes(unit, name))
        })
}

/// Standard-library S3 (#1963): what hover adds about a first-party item's
/// stability: that it is experimental (no compatibility promise, and an
/// `[unstable] allow` to use), and that an enum is extensible (a `match` on it
/// needs a wildcard). Empty for a stable, closed item.
pub fn firstparty_notes(unit: &str, name: &str) -> String {
    let mut out = String::new();
    let path = format!("{unit}.{name}");
    if bynk_check::firstparty::stability_of(&path)
        == Some(bynk_check::firstparty::Stability::Experimental)
    {
        out.push_str(&format!(
            "\n\n**Experimental**: it may change or disappear in any release. To use it, add `\"{path}\"` to `[unstable] allow` in `bynk.toml`."
        ));
    }
    if bynk_check::firstparty::is_extensible(unit, name) {
        out.push_str(
            "\n\n**Extensible**: it may gain variants in a minor release, so a `match` on it needs a wildcard arm.",
        );
    }
    out
}

/// Slice 6b: the `(unit name, name span)` of every `uses`/`consumes` target in
/// the source — the clickable ranges for document links. The link's target file
/// is resolved by the handler through the unit→source map (ADR 0095); this only
/// finds the spans, so it works on the live buffer regardless of the map.
pub fn unit_reference_spans(source: &str) -> Vec<(String, Span)> {
    let Ok(tokens) = tokenize(source) else {
        return Vec::new();
    };
    let (Some(unit), _) = parse_unit_with_recovery(&tokens, source) else {
        return Vec::new();
    };
    let mut out: Vec<(String, Span)> = Vec::new();
    // A suite links its target (the unit under test) plus any `uses` clauses,
    // mirroring the `uses`/`consumes` links on the other unit kinds (#609).
    let (uses, consumes): (&[UsesDecl], &[ConsumesDecl]) = match &unit {
        SourceUnit::Commons(c) => (&c.uses, &[]),
        SourceUnit::Context(c) => (&c.uses, &c.consumes),
        SourceUnit::Adapter(a) => (&a.uses, &a.consumes),
        SourceUnit::Suite(s) => {
            out.push((s.target.joined(), s.target.span));
            (&s.uses, &[])
        }
    };
    for u in uses {
        out.push((u.target.joined(), u.target.span));
    }
    for c in consumes {
        out.push((c.target.joined(), c.target.span));
    }
    out
}

/// #1885: the qualified unit name under `offset`, when the cursor sits on the
/// file's own unit-header name or on a `uses`/`consumes` (or a suite's) target
/// path — the positions where hover shows a unit's module doc.
///
/// The match is on the name's byte range (`start..=end`), never on `Span`
/// equality, so it holds for a buffer re-parsed with `FileId::UNKNOWN`. Every
/// segment of a dotted path answers with the whole name: the cursor on
/// `model` in `uses compat.model` names `compat.model`, not a `model`.
pub fn unit_name_at(source: &str, offset: usize) -> Option<String> {
    let own = own_declaration_name(source);
    own.into_iter()
        .chain(unit_reference_spans(source))
        .find(|(_, span)| span.start <= offset && offset <= span.end)
        .map(|(name, _)| name)
}

/// #1885: hover Markdown for the unit `name` — a fenced `<kind> <name>` line,
/// followed by its module doc when one of its files carries one.
///
/// `sources` are the texts to search: every file of the project the caller
/// can see (a multi-file context's doc may live in a sibling of the file the
/// cursor is in). A file that declares `name` as a `commons`, `context` or
/// `adapter` contributes; `suite` files never do, since a suite's own doc is
/// not its target's. At most one file of a unit carries a module doc
/// (`bynk.project.duplicate_module_doc`); should a broken project have more,
/// the first one in `sources` order is shown. `None` when no source declares
/// `name`.
pub fn describe_unit<'s>(sources: impl IntoIterator<Item = &'s str>, name: &str) -> Option<String> {
    let mut kind: Option<&'static str> = None;
    let mut doc: Option<String> = None;
    for source in sources {
        let Ok(tokens) = tokenize(source) else {
            continue;
        };
        let (Some(unit), _) = parse_unit_with_recovery(&tokens, source) else {
            continue;
        };
        if matches!(unit, SourceUnit::Suite(_)) || unit.name().joined() != name {
            continue;
        }
        kind.get_or_insert(unit.kind_name());
        if doc.is_none() {
            doc = unit.module_doc().map(str::to_string);
        }
        if doc.is_some() {
            break;
        }
    }
    let kind = kind?;
    let mut out = format!("```bynk\n{kind} {name}\n```\n");
    if let Some(doc) = doc {
        out.push('\n');
        out.push_str(&doc);
        out.push('\n');
    }
    Some(out)
}

/// #1885: the module-doc hover for the unit name under `offset` in `text` (see
/// [`unit_name_at`]), resolved against `text` itself, then `others` (the
/// project's other files), then the embedded first-party sources (so
/// `consumes bynk.cloudflare` still names its unit). `None` when the cursor is
/// not on a unit name, or no source declares it.
pub fn describe_unit_at<'s>(
    text: &'s str,
    offset: usize,
    others: impl IntoIterator<Item = &'s str>,
) -> Option<String> {
    let name = unit_name_at(text, offset)?;
    let firstparty = bynk_check::firstparty::FIRSTPARTY_SOURCES
        .iter()
        .map(|(_, src)| *src);
    describe_unit(std::iter::once(text).chain(others).chain(firstparty), &name)
}

/// #848: one intra-doc-link candidate scanned from doc-comment text — a
/// `[Name]` shortcut, a `` [`Name`] `` code-span-wrapped shortcut, or a
/// `[text][Name]` full reference. `span` is the byte range (into the text the
/// scanner was given) of the whole construct to rewrite when `name` resolves;
/// `display` is the link text to keep (`Name` for a shortcut, the code span
/// including backticks for the code-span form, `text` for a full reference).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocLinkCandidate {
    pub name: String,
    pub span: std::ops::Range<usize>,
    pub display: String,
}

/// #848: is `s` shaped like a doc-link candidate name — a bare identifier, or
/// identifiers joined by `.` (an `Owner.member` or `unit.path.Name`
/// reference)? Every segment must be an identifier, so ordinary bracketed
/// prose (`[note]`, `[sic]`, `[e.g.]`) is never a candidate.
fn is_doc_link_name(s: &str) -> bool {
    fn is_ident(part: &str) -> bool {
        let mut chars = part.chars();
        matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    }
    s.split('.').all(is_ident)
}

/// #848: fenced-code-block byte ranges (paired ` ``` ` lines only — doc
/// comments are free text, not guaranteed well-formed Markdown, so a `~~~`
/// fence or an unterminated one just gets a best-effort range to end of text)
/// and the set of `[label]: destination`-shaped reference-definition labels,
/// both found in `text` line by line. [`scan_doc_link_candidates`] excludes
/// both: fenced content is never scanned for candidates, and a label with an
/// author-defined reference keeps its ordinary Markdown meaning instead of
/// becoming an intra-doc link.
fn doc_link_prepass(text: &str) -> (Vec<(usize, usize)>, std::collections::HashSet<String>) {
    let mut fenced = Vec::new();
    let mut defined = std::collections::HashSet::new();
    let mut in_fence = false;
    let mut fence_start = 0usize;
    let mut pos = 0usize;
    loop {
        let line_end = text[pos..]
            .find('\n')
            .map(|i| pos + i + 1)
            .unwrap_or(text.len());
        let line = &text[pos..line_end];
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            if in_fence {
                fenced.push((fence_start, line_end));
                in_fence = false;
            } else {
                fence_start = pos;
                in_fence = true;
            }
        } else if !in_fence {
            // A reference definition (`[label]: url`) — CommonMark allows up
            // to 3 leading spaces.
            let indent = line.len() - trimmed.len();
            if indent <= 3
                && let Some(rest) = trimmed.strip_prefix('[')
                && let Some(close) = rest.find(']')
            {
                let label = &rest[..close];
                if let Some(after_colon) = rest[close + 1..].strip_prefix(':')
                    && !label.is_empty()
                    && !after_colon.trim().is_empty()
                {
                    defined.insert(label.to_string());
                }
            }
        }
        if line_end >= text.len() {
            break;
        }
        pos = line_end;
    }
    if in_fence {
        fenced.push((fence_start, text.len()));
    }
    (fenced, defined)
}

/// #848: scans the bracket construct starting at `text[start]` (which must be
/// `[`) for one of the recognised intra-doc-link forms. `None` when the
/// bracket isn't well-formed, is an explicit-URL link (`[text](url)`), is a
/// collapsed reference (`[text][]` — out of scope), names something that
/// isn't identifier-shaped, or names an author-defined reference label.
fn scan_bracket_candidate(
    text: &str,
    start: usize,
    defined: &std::collections::HashSet<String>,
) -> Option<DocLinkCandidate> {
    let bytes = text.as_bytes();
    debug_assert_eq!(bytes[start], b'[');
    // `` [`Name`] `` — a code-span-wrapped shortcut.
    if bytes.get(start + 1) == Some(&b'`') {
        let inner_start = start + 2;
        let close_tick = text[inner_start..].find('`')? + inner_start;
        if bytes.get(close_tick + 1) != Some(&b']') {
            return None;
        }
        let name = &text[inner_start..close_tick];
        if !is_doc_link_name(name) || defined.contains(name) {
            return None;
        }
        return Some(DocLinkCandidate {
            name: name.to_string(),
            span: start..close_tick + 2,
            display: text[start + 1..close_tick + 1].to_string(),
        });
    }
    let inner_start = start + 1;
    let close = text[inner_start..].find(']')? + inner_start;
    let inner = &text[inner_start..close];
    let after = close + 1;
    match bytes.get(after) {
        // `[text](url)` — an explicit URL, unchanged Markdown.
        Some(b'(') => None,
        // A reference-definition line reached mid-scan (pass 1 already
        // excludes true line-start ones) — never a candidate.
        Some(b':') => None,
        // `[text][label]` — a full reference.
        Some(b'[') => {
            let label_start = after + 1;
            let label_close = text[label_start..].find(']')? + label_start;
            let label = &text[label_start..label_close];
            // `[text][]` (collapsed reference) is out of scope this increment.
            if label.is_empty() || !is_doc_link_name(label) || defined.contains(label) {
                return None;
            }
            Some(DocLinkCandidate {
                name: label.to_string(),
                span: start..label_close + 1,
                display: inner.to_string(),
            })
        }
        // `[Name]` — a shortcut.
        _ => {
            if !is_doc_link_name(inner) || defined.contains(inner) {
                return None;
            }
            Some(DocLinkCandidate {
                name: inner.to_string(),
                span: start..after,
                display: inner.to_string(),
            })
        }
    }
}

/// #848: every intra-doc-link candidate in `text`, in order. `[text](url)`,
/// an author-defined `[label]: url` reference, fenced-code-block content, and
/// non-identifier bracket content (`[note]`) are excluded — see
/// `scan_bracket_candidate` and `doc_link_prepass` (private below).
pub fn scan_doc_link_candidates(text: &str) -> Vec<DocLinkCandidate> {
    let (fenced, defined) = doc_link_prepass(text);
    let in_fenced = |i: usize| fenced.iter().any(|&(s, e)| i >= s && i < e);
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] != b'[' || in_fenced(i) {
            i += 1;
            continue;
        }
        match scan_bracket_candidate(text, i, &defined) {
            Some(cand) => {
                i = cand.span.end;
                out.push(cand);
            }
            None => i += 1,
        }
    }
    out
}

/// #848: the `(candidate name, absolute source span)` of every intra-doc-link
/// candidate inside every `DocBlock` token in `source` — the clickable
/// ranges for doc-comment document links, mirroring [`unit_reference_spans`]'s
/// role for `uses`/`consumes` targets. Spans are computed against the raw,
/// unstripped doc-block body (`bynk_syntax::lexer::doc_block_body_range`),
/// not the common-indent-stripped `doc_block_content` text — that stripping
/// is not offset-preserving, so a span-based caller must avoid it. Only
/// tokenizes (not a full parse), so links still surface even when the rest of
/// the file has a parse error elsewhere, as long as tokenization succeeds.
pub fn doc_link_spans(source: &str) -> Vec<(String, Span)> {
    doc_link_spans_attached(source)
        .into_iter()
        .map(|l| (l.name, l.span))
        .collect()
}

/// #2454: one intra-doc-link candidate with the declaration its doc comment
/// is attached to. `attached` is the byte offset of that declaration's name
/// (the first identifier after the doc comment) — the key a resolver matches
/// against an index definition site — or `None` when no identifier follows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocLinkSpan {
    pub name: String,
    pub span: Span,
    pub attached: Option<usize>,
}

/// #2454: [`doc_link_spans`] plus the declaration each doc block precedes.
pub fn doc_link_spans_attached(source: &str) -> Vec<DocLinkSpan> {
    let Ok(tokens) = tokenize(source) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (i, t) in tokens.iter().enumerate() {
        // #1888: a `--|` doc line links too, one line at a time.
        let range = match t.kind {
            bynk_syntax::lexer::TokenKind::DocBlock => {
                bynk_syntax::lexer::doc_block_body_range(source, t.span)
            }
            bynk_syntax::lexer::TokenKind::DocLine => {
                Some(bynk_syntax::lexer::doc_line_text_range(source, t.span))
            }
            _ => None,
        };
        let Some(range) = range else {
            continue;
        };
        // #2569: a method `fn T.m(..)` is defined (and indexed) at `m`, so
        // that is where its doc attaches — the panel's offset is its entry
        // span's, which is the name.
        let attached = tokens[i + 1..]
            .iter()
            .position(|n| n.kind == bynk_syntax::lexer::TokenKind::Ident)
            .map(|p| {
                let rest = &tokens[i + 1 + p..];
                match rest {
                    [_, dot, name, ..]
                        if dot.kind == bynk_syntax::lexer::TokenKind::Dot
                            && name.kind == bynk_syntax::lexer::TokenKind::Ident
                            && p > 0
                            && tokens[i + p].kind == bynk_syntax::lexer::TokenKind::Fn =>
                    {
                        name.span.start
                    }
                    _ => rest[0].span.start,
                }
            });
        for cand in scan_doc_link_candidates(&source[range.clone()]) {
            out.push(DocLinkSpan {
                name: cand.name,
                span: Span::new(range.start + cand.span.start, range.start + cand.span.end),
                attached,
            });
        }
    }
    out
}

/// #2454: the unit whose scope a file's doc links resolve in — its own
/// declared name, or a suite's *target* unit (a suite declares no unit of
/// its own, so [`own_declaration_name`] is `None` for it).
pub fn doc_link_unit(source: &str) -> Option<String> {
    let tokens = tokenize(source).ok()?;
    let (Some(unit), _) = parse_unit_with_recovery(&tokens, source) else {
        return None;
    };
    Some(unit.name().joined())
}

/// #2458: where a doc comment's cursor sits inside an unfinished `[…` link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocLinkCompletionContext {
    /// The `Owner` of an `[Owner.` prefix, when the cursor is past a dot.
    pub owner: Option<String>,
    /// The identifier text typed so far (after the `[` or the last `.`).
    pub partial: String,
    /// Offset the completion replaces from (`cursor - partial.len()`).
    pub replace_start: usize,
    /// Offset the completion replaces to: the cursor, extended over any
    /// identifier characters that follow it (`[Rat|eView]`), so accepting an
    /// item replaces the whole word rather than duplicating its tail.
    pub replace_end: usize,
    /// Whether a `]` already follows the replaced word, so the item need not add one.
    pub closed: bool,
    /// The byte offset of the name of the declaration this doc block is
    /// attached to (as `DocLinkSpan::attached`), when one follows.
    pub attached: Option<usize>,
}

/// #2458: the doc-link completion context at `offset`, or `None` when the
/// cursor is not inside a `---` / `--|` doc comment (or is in a fenced code
/// block, or after a `[` already closed or broken by a non-identifier
/// character). This is the strict gate for the server-wide `[` trigger: code
/// outside doc ranges never yields a context. Tokenizes only; `None` if the
/// source does not tokenize.
pub fn doc_link_context(source: &str, offset: usize) -> Option<DocLinkCompletionContext> {
    let tokens = tokenize(source).ok()?;
    let (range, attached) = tokens.iter().enumerate().find_map(|(i, t)| {
        let range = match t.kind {
            bynk_syntax::lexer::TokenKind::DocBlock => {
                bynk_syntax::lexer::doc_block_body_range(source, t.span)
            }
            bynk_syntax::lexer::TokenKind::DocLine => {
                Some(bynk_syntax::lexer::doc_line_text_range(source, t.span))
            }
            _ => None,
        }?;
        (range.start <= offset && offset <= range.end).then(|| {
            let attached = tokens[i + 1..]
                .iter()
                .find(|n| n.kind == bynk_syntax::lexer::TokenKind::Ident)
                .map(|n| n.span.start);
            (range, attached)
        })
    })?;
    let body = &source[range.start..range.end];
    let (fenced, _) = doc_link_prepass(body);
    let rel = offset - range.start;
    if fenced.iter().any(|&(s, e)| rel >= s && rel < e) {
        return None;
    }
    let before = &body[..rel];
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let open = before[line_start..].rfind('[')? + line_start;
    let typed = &before[open + 1..];
    if !typed
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '.')
    {
        return None;
    }
    let (owner, partial) = match typed.rfind('.') {
        Some(i) if i > 0 => (Some(typed[..i].to_string()), &typed[i + 1..]),
        Some(_) => return None,
        None => (None, typed),
    };
    let tail = source[offset..range.end]
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(range.end - offset);
    let replace_end = offset + tail;
    Some(DocLinkCompletionContext {
        owner,
        partial: partial.to_string(),
        replace_start: offset - partial.len(),
        replace_end,
        closed: source[replace_end..].starts_with(']'),
        attached,
    })
}

/// #302: the source's own declared qualified name and its span — the
/// rewrite target when the file backing this unit is renamed. `None` for a
/// `suite` (its `SourceUnit::name()` is its *target*'s name, not one of its
/// own; nothing else addresses a suite by name) or on a parse bail.
pub fn own_declaration_name(source: &str) -> Option<(String, Span)> {
    let tokens = tokenize(source).ok()?;
    let (Some(unit), _) = parse_unit_with_recovery(&tokens, source) else {
        return None;
    };
    if matches!(unit, SourceUnit::Suite(_)) {
        return None;
    }
    let name = unit.name();
    Some((name.joined(), name.span))
}

fn describe_item(item: &CommonsItem, name: &str) -> Option<String> {
    match item {
        CommonsItem::Type(t) if t.name.name == name => Some(describe_type(t)),
        // v0.166 (#616): a bare key names a *free* function. A method's identity
        // is its compound `"Type.method"` key (below); matching one by its bare
        // method name answered with whichever type declared it first, so
        // `g.bump()` and even `fn Gauge.bump`'s own declaration rendered
        // `Counter.bump`. `signature_help::resolve_label` guards its free-fn path
        // the same way.
        CommonsItem::Fn(f) if matches!(f.name, FnName::Free(_)) && f.name.ident().name == name => {
            Some(describe_fn(f))
        }
        CommonsItem::Capability(c) if c.name.name == name => Some(describe_capability(c)),
        CommonsItem::Service(s) if s.name.name == name => Some(describe_service(s)),
        CommonsItem::Agent(a) if a.name.name == name => Some(describe_agent(a)),
        CommonsItem::Provider(p) if p.provider_name.name == name => Some(describe_provider(p)),
        // v0.166 (#616): an actor, keyed by its plain name — the `Actor` index
        // kind ADR 0190 filed as the clearest evidence that the renderer, not the
        // ladder, is where these were missing. `by u: User` resolved here and
        // rendered nothing.
        CommonsItem::Actor(a) if a.name.name == name => Some(describe_actor(a)),
        // message-bundles slice 1 (#859): a messages block, keyed by its tag —
        // the same top-level, plain-name convention as Capability/Service/
        // Agent above.
        CommonsItem::Messages(m) if m.tag == name => Some(describe_messages(m)),
        // #1883: an asset's hover and docs.
        CommonsItem::Asset(a) if a.name.name == name => Some(describe_asset(a)),
        // #2387, slice 1: an embed's hover and docs.
        CommonsItem::Embed(e) if e.name.name == name => Some(describe_embed(e)),
        // #2617: a constant's hover and docs.
        CommonsItem::Const(c) if c.name.name == name => Some(describe_const(c)),
        // #972 (Events slice 3a): an event, keyed by its plain name — the same
        // top-level convention as the arms above. Reuses `describe_type` via
        // the synthetic `TypeDecl` `as_type_decl` already builds for every
        // other event-as-type consumer (exports/consumes/construction).
        CommonsItem::Event(e) if e.name.name == name => Some(describe_event(e)),
        // #611 (gap B): a record field, keyed `"Type.field"` by the index — the
        // checker records construction labels and field accesses as `Field` refs,
        // so hover resolves the key but had no arm to render it and fell through
        // to the locals path, which name-matches in scope (a `title:` label bound
        // to a same-named handler param). Top-level names carry no `.`, so the
        // compound key can only match here.
        CommonsItem::Type(t) => {
            let (owner, field) = name.rsplit_once('.')?;
            if t.name.name != owner {
                return None;
            }
            let TypeBody::Record(r) = &t.body else {
                return None;
            };
            r.fields
                .iter()
                .find(|f| f.name.name == field)
                .map(|f| describe_record_field(t, f))
        }
        // #972 (Events slice 3a): an event field, keyed `"Event.field"` —
        // mirrors the `CommonsItem::Type` arm above via the same synthetic
        // `TypeDecl`, so a defaulted event field's hover also renders its
        // `= <expr>`.
        CommonsItem::Event(e) => {
            let (owner, field) = name.rsplit_once('.')?;
            if e.name.name != owner {
                return None;
            }
            let synthetic = e.as_type_decl();
            e.body
                .fields
                .iter()
                .find(|f| f.name.name == field)
                .map(|f| describe_record_field(&synthetic, f))
        }
        // v0.166 (#616): a method, keyed `"Type.method"` (ADR 0069). `display()`
        // renders exactly that key, so the compound name matches the one method
        // it names — the type prefix is what disambiguates `Counter.bump` from
        // `Gauge.bump`.
        CommonsItem::Fn(f) => (f.name.display() == name).then(|| describe_fn(f)),
        // v0.166 (#616): a capability operation, keyed `"Cap.op"` (ADR 0069).
        CommonsItem::Capability(c) => {
            let (owner, op) = name.rsplit_once('.')?;
            if c.name.name != owner {
                return None;
            }
            c.ops
                .iter()
                .find(|o| o.name.name == op)
                .map(|o| describe_capability_op(c, o))
        }
        // #304: an agent handler, keyed `"Agent.handler"` — the checker
        // records a dispatch call (`agentInstance.handler(...)`) as this
        // compound key, mirroring the method/field/op convention above.
        CommonsItem::Agent(a) => {
            let (owner, handler) = name.rsplit_once('.')?;
            if a.name.name != owner {
                return None;
            }
            a.handlers
                .iter()
                .find(|h| h.method_name.as_ref().is_some_and(|n| n.name == handler))
                .map(|h| describe_agent_handler(a, h, handler))
        }
        _ => None,
    }
}

/// v0.166 (#616): an actor as declared — the `auth` scheme with its config, the
/// `identity` type, or the refinement form's base and claim predicate.
/// Mirrors `bynk-fmt`'s `format_actor`, as [`describe_agent`] mirrors an agent.
///
/// #847: `pub(crate)` so the documentation-view aggregator ([`crate::documentation`])
/// renders each declaration through the *same* signature+doc assembly hover uses,
/// rather than a parallel renderer that could drift from it.
pub(crate) fn describe_actor(a: &ActorDecl) -> String {
    let mut out = String::from("```bynk\n");
    match &a.refinement {
        // `actor Admin = User where hasClaim("admin")` (ADR 0091).
        Some(r) => out.push_str(&format!(
            "actor {} = {} where {}",
            a.name.name,
            r.base.name,
            bynk_fmt::expr_to_string(&r.predicate)
        )),
        None => {
            // An absent `auth` is the `None` scheme, which is how it parses.
            let auth = a.auth.as_ref().map_or("None", |i| i.name.as_str());
            out.push_str(&format!("actor {} {{ auth = {auth}", a.name.name));
            if !a.auth_config.is_empty() {
                let args: Vec<String> = a
                    .auth_config
                    .iter()
                    .map(|arg| match &arg.value {
                        // The parser resolves escapes at lex time, so the stored
                        // value is *unescaped* — re-escape it through the
                        // formatter's own escaper, or a `"` in the config renders
                        // as invalid Bynk inside the fence below.
                        SchemeArgValue::Str(s) => {
                            format!("{} = \"{}\"", arg.key.name, bynk_fmt::escape_string(s))
                        }
                        SchemeArgValue::Int(n) => format!("{} = {n}", arg.key.name),
                    })
                    .collect();
                out.push_str(&format!("({})", args.join(", ")));
            }
            if let Some(id) = &a.identity {
                out.push_str(&format!(", identity = {}", type_ref_str(id)));
            }
            out.push_str(" }");
        }
    }
    out.push_str("\n```\n");
    if let Some(doc) = &a.documentation {
        out.push('\n');
        out.push_str(doc);
        out.push('\n');
    }
    out
}

/// #926: an op's own `[T, …]` type parameters, rendered for hover — `""` when
/// the op is non-generic. Shared by [`describe_capability_op`] and
/// [`describe_capability`] so the two signature renderings can't drift.
fn capability_op_type_params_str(op: &CapabilityOp) -> String {
    if op.type_params.is_empty() {
        return String::new();
    }
    let names: Vec<&str> = op
        .type_params
        .iter()
        .map(|tp| tp.name.name.as_str())
        .collect();
    format!("[{}]", names.join(", "))
}

/// v0.166 (#616): a capability operation as declared, attributed to the
/// capability that owns it. Mirrors how [`describe_capability`] renders the same
/// op within the capability body, as [`describe_record_field`] does for a field.
pub(crate) fn describe_capability_op(c: &CapabilityDecl, op: &CapabilityOp) -> String {
    let params: Vec<String> = op
        .params
        .iter()
        .map(|p| format!("{}: {}", p.name.name, type_ref_str(&p.type_ref)))
        .collect();
    let mut out = format!(
        "```bynk\nfn {}{}({}) -> {}\n```\n\nAn operation of capability `{}`.\n",
        op.name.name,
        capability_op_type_params_str(op),
        params.join(", "),
        type_ref_str(&op.return_type),
        c.name.name
    );
    if let Some(doc) = &op.documentation {
        out.push('\n');
        out.push_str(doc);
        out.push('\n');
    }
    out
}

/// #304: an agent handler as declared, attributed to the agent that owns it —
/// its dispatch name, params, and return type. Mirrors [`describe_capability_op`].
pub(crate) fn describe_agent_handler(a: &AgentDecl, h: &Handler, handler_name: &str) -> String {
    let params: Vec<String> = h
        .params
        .iter()
        .map(|p| format!("{}: {}", p.name.name, type_ref_str(&p.type_ref)))
        .collect();
    let mut out = format!(
        "```bynk\nfn {}({}) -> {}\n```\n\nA handler of agent `{}`.\n",
        handler_name,
        params.join(", "),
        type_ref_str(&h.return_type),
        a.name.name
    );
    if let Some(doc) = &h.documentation {
        out.push('\n');
        out.push_str(doc);
        out.push('\n');
    }
    out
}

/// #847: a service handler as declared, attributed to the service that owns it —
/// its route/protocol (`on GET("/x")`, `on call`, …) with the typed params and
/// return type. The documentation-view counterpart to [`describe_agent_handler`]:
/// a service handler has no compound index key (its route, not a dispatch name,
/// identifies it — see `bynk-lsp/src/sequence_request.rs`), so hover never
/// describes one on its own; the doc page is the first surface that renders each
/// individually, and it does so through the same fenced-signature + doc-prose
/// shape every other `describe_*` uses.
pub(crate) fn describe_service_handler(s: &ServiceDecl, h: &Handler) -> String {
    let params: Vec<String> = h
        .params
        .iter()
        .map(|p| format!("{}: {}", p.name.name, type_ref_str(&p.type_ref)))
        .collect();
    let mut out = format!(
        "```bynk\n{}({}) -> {}\n```\n\nA handler of service `{}`.\n",
        handler_line(h),
        params.join(", "),
        type_ref_str(&h.return_type),
        s.name.name
    );
    if let Some(doc) = &h.documentation {
        out.push('\n');
        out.push_str(doc);
        out.push('\n');
    }
    out
}

/// #611: a record field as declared — its type and any `where` refinement —
/// attributed to the record that owns it. Mirrors how [`describe_type`] renders
/// the same field within the record body.
///
/// #1888: a documented field shows its own doc in place of the generic
/// "A field of `T`." line.
pub(crate) fn describe_record_field(t: &TypeDecl, f: &RecordField) -> String {
    let mut sig = format!("{}: {}", f.name.name, type_ref_str(&f.type_ref));
    if let Some(r) = &f.refinement {
        sig.push_str(&format!(" where {}", bynk_fmt::refinement_to_string(r)));
    }
    // #972 (Events slice 3a): render a field default the same way the
    // formatter does — `= <expr>` — so a subscriber-visible default is not
    // silently dropped from hover.
    if let Some(init) = &f.init {
        sig.push_str(&format!(" = {}", bynk_fmt::expr_to_string(init)));
    }
    format!(
        "```bynk\n{sig}\n```\n\n{}",
        member_prose(&f.documentation, || format!(
            "A field of `{}`.",
            t.name.name
        ))
    )
}

/// #1888: a member's doc, trailing whitespace trimmed, or `fallback` when it
/// has none.
fn member_prose(doc: &Option<Doc>, fallback: impl FnOnce() -> String) -> String {
    match doc {
        Some(d) => d.trim_end().to_string(),
        None => fallback(),
    }
}

/// #1888: a sum variant as declared, `| Name(field: Type, …)`, attributed to
/// its sum unless it carries its own doc, which shows in its place. A payload
/// field's doc follows, one line of the list per documented field.
pub(crate) fn describe_variant(t: &TypeDecl, v: &Variant) -> String {
    let mut sig = format!("| {}", v.name.name);
    if !v.payload.is_empty() {
        let parts: Vec<String> = v
            .payload
            .iter()
            .map(|p| format!("{}: {}", p.name.name, type_ref_str(&p.type_ref)))
            .collect();
        sig.push_str(&format!("({})", parts.join(", ")));
    }
    let mut out = format!(
        "```bynk\n{sig}\n```\n\n{}",
        member_prose(&v.documentation, || format!(
            "A variant of `{}`.",
            t.name.name
        ))
    );
    for p in &v.payload {
        if let Some(doc) = &p.documentation {
            out.push_str(&format!("\n\n`{}`: {}", p.name.name, doc.trim_end()));
        }
    }
    out
}

/// #1888: one payload field of a sum variant, attributed to its variant
/// unless it carries its own doc.
pub(crate) fn describe_variant_field(t: &TypeDecl, v: &Variant, p: &VariantField) -> String {
    format!(
        "```bynk\n{}: {}\n```\n\n{}",
        p.name.name,
        type_ref_str(&p.type_ref),
        member_prose(&p.documentation, || format!(
            "A field of `{}.{}`.",
            t.name.name, v.name.name
        ))
    )
}

/// #1888: every sum variant declared in `source`'s unit, with its sum.
fn with_sum_variants<R>(
    source: &str,
    mut f: impl FnMut(&TypeDecl, &Variant) -> Option<R>,
) -> Option<R> {
    let tokens = tokenize(source).ok()?;
    let (unit, _errs) = parse_unit_with_recovery(&tokens, source);
    let items: &[CommonsItem] = match unit.as_ref()? {
        SourceUnit::Commons(c) => &c.items,
        SourceUnit::Context(c) => &c.items,
        SourceUnit::Adapter(a) => &a.items,
        SourceUnit::Suite(_) => &[],
    };
    for item in items {
        if let CommonsItem::Type(t) = item
            && let TypeBody::Sum(s) = &t.body
        {
            for v in &s.variants {
                if let Some(r) = f(t, v) {
                    return Some(r);
                }
            }
        }
    }
    None
}

/// #1888: the hover for a sum variant named by a qualified key,
/// `"Shape.Circle"`, declared in `source`; `None` when no sum there has it.
pub fn describe_variant_key(source: &str, key: &str) -> Option<String> {
    let (owner, name) = key.rsplit_once('.')?;
    with_sum_variants(source, |t, v| {
        (t.name.name == owner && v.name.name == name).then(|| describe_variant(t, v))
    })
}

/// #1888: the hover for a sum variant named bare, as a pattern or a
/// constructor names it (`Circle(r)`), when exactly one sum in `source`
/// declares a variant of that name; `None` otherwise, so an ambiguous name
/// answers nothing rather than the wrong variant.
pub fn describe_variant_named(source: &str, name: &str) -> Option<String> {
    let mut found: Vec<String> = Vec::new();
    with_sum_variants::<()>(source, |t, v| {
        if v.name.name == name {
            found.push(describe_variant(t, v));
        }
        None
    });
    (found.len() == 1).then(|| found.remove(0))
}

pub(crate) fn describe_type(t: &TypeDecl) -> String {
    let mut out = String::new();
    out.push_str("```bynk\n");
    // v0.157 (ADR 0183): render `[A, B]` type parameters on a generic type.
    let params = if t.type_params.is_empty() {
        String::new()
    } else {
        let names: Vec<String> = t
            .type_params
            .iter()
            .map(|tp| bound_str(&tp.name.name, &tp.bounds))
            .collect();
        format!("[{}]", names.join(", "))
    };
    // #2618: the unit-private modifier, as written.
    if t.private.is_some() {
        out.push_str("private ");
    }
    out.push_str(&format!("type {}{} = ", t.name.name, params));
    match &t.body {
        // v0.123 (slice 2): render the refined/opaque `where` predicate (was
        // collapsed to the bare base) via the formatter's own renderer.
        TypeBody::Refined {
            base, refinement, ..
        } => {
            out.push_str(base.name());
            if let Some(r) = refinement {
                out.push_str(&format!(" where {}", bynk_fmt::refinement_to_string(r)));
            }
        }
        TypeBody::Opaque {
            base, refinement, ..
        } => {
            out.push_str(&format!("opaque {}", base.name()));
            if let Some(r) = refinement {
                out.push_str(&format!(" where {}", bynk_fmt::refinement_to_string(r)));
            }
        }
        // Record fields, one per line (was collapsed to `record`).
        TypeBody::Record(r) => {
            if r.fields.is_empty() {
                out.push_str("{}");
            } else {
                out.push_str("{\n");
                for f in &r.fields {
                    out.push_str(&format!("\t{}: {}", f.name.name, type_ref_str(&f.type_ref)));
                    if let Some(r) = &f.refinement {
                        out.push_str(&format!(" where {}", bynk_fmt::refinement_to_string(r)));
                    }
                    // #972 (Events slice 3a): an event field's default,
                    // reachable here via `EventDecl::as_type_decl`'s
                    // synthetic `TypeDecl`.
                    if let Some(init) = &f.init {
                        out.push_str(&format!(" = {}", bynk_fmt::expr_to_string(init)));
                    }
                    out.push_str(",\n");
                }
                out.push('}');
            }
        }
        // Sum variants, with payloads (was collapsed to `sum`).
        TypeBody::Sum(s) => {
            out.push_str("enum {\n");
            for v in &s.variants {
                out.push_str(&format!("\t{}", v.name.name));
                if !v.payload.is_empty() {
                    let parts: Vec<String> = v
                        .payload
                        .iter()
                        .map(|p| format!("{}: {}", p.name.name, type_ref_str(&p.type_ref)))
                        .collect();
                    out.push_str(&format!("({})", parts.join(", ")));
                }
                out.push_str(",\n");
            }
            out.push('}');
        }
    }
    // Constrained generics slice 1: the `derives` clause, after the body.
    if !t.derives.is_empty() {
        let names: Vec<&str> = t.derives.iter().map(|d| d.name.as_str()).collect();
        out.push_str(&format!(" derives {}", names.join(" + ")));
    }
    out.push_str("\n```\n");
    if let Some(doc) = &t.documentation {
        out.push('\n');
        out.push_str(doc);
        out.push('\n');
    }
    out
}

/// Events slice 3b (#978): additively render an event's `@schema(N)` (or any
/// future event annotation) into its hover, the same rendering
/// `store_field_hover` already uses. `describe_type`'s signature is shared
/// with every ordinary type's hover, so this post-processes its output for
/// the one synthetic-`TypeDecl` caller that has annotations to show, rather
/// than widening that signature. A no-op when the event declares none —
/// absent-annotation hover stays byte-identical to before this slice.
fn describe_event(e: &EventDecl) -> String {
    let base = describe_type(&e.as_type_decl());
    if e.annotations.is_empty() {
        return base;
    }
    let annotations: String = e
        .annotations
        .iter()
        .map(|a| format!(" {}", bynk_fmt::annotation_to_string(a)))
        .collect();
    let from = format!("type {} = ", e.name.name);
    let to = format!("type {}{annotations} = ", e.name.name);
    base.replacen(&from, &to, 1)
}

/// `[T: Eq, U]`, or empty for a non-generic function: the declared bounds, so
/// hover shows what a caller must satisfy (constrained generics slice 0).
pub(crate) fn fn_type_params_str(f: &FnDecl) -> String {
    if f.type_params.is_empty() {
        return String::new();
    }
    let ps: Vec<String> = f
        .type_params
        .iter()
        .map(|tp| bound_str(&tp.name.name, &tp.bounds))
        .collect();
    format!("[{}]", ps.join(", "))
}

/// ` where A: Eq`, or empty without a `where` clause.
pub(crate) fn fn_where_str(f: &FnDecl) -> String {
    if f.where_bounds.is_empty() {
        return String::new();
    }
    let ps: Vec<String> = f
        .where_bounds
        .iter()
        .map(|wb| bound_str(&wb.param.name, &wb.bounds))
        .collect();
    format!(" where {}", ps.join(", "))
}

fn bound_str(name: &str, bounds: &[Ident]) -> String {
    if bounds.is_empty() {
        return name.to_string();
    }
    let names: Vec<&str> = bounds.iter().map(|b| b.name.as_str()).collect();
    format!("{name}: {}", names.join(" + "))
}

pub(crate) fn describe_fn(f: &FnDecl) -> String {
    let mut out = String::new();
    out.push_str("```bynk\n");
    // #2618: the unit-private modifier, as written.
    if f.private.is_some() {
        out.push_str("private ");
    }
    out.push_str("fn ");
    out.push_str(&f.name.display());
    out.push_str(&fn_type_params_str(f));
    out.push('(');
    let mut parts: Vec<String> = Vec::new();
    if f.has_self {
        parts.push("self".into());
    }
    for p in &f.params {
        parts.push(format!("{}: {}", p.name.name, type_ref_str(&p.type_ref)));
    }
    out.push_str(&parts.join(", "));
    out.push_str(") -> ");
    out.push_str(&type_ref_str(&f.return_type));
    out.push_str(&fn_where_str(f));
    // v0.123 (slice 2): the contract clauses (v0.115), beneath the signature —
    // rendered through the formatter's own predicate renderer.
    for c in &f.requires {
        out.push_str(&format!(
            "\n\trequires {}: {}",
            c.name.name,
            bynk_fmt::expr_to_string(&c.predicate)
        ));
    }
    for c in &f.ensures {
        out.push_str(&format!(
            "\n\tensures {}: {}",
            c.name.name,
            bynk_fmt::expr_to_string(&c.predicate)
        ));
    }
    out.push_str("\n```\n");
    if let Some(doc) = &f.documentation {
        out.push('\n');
        out.push_str(doc);
        out.push('\n');
    }
    out
}

pub(crate) fn describe_capability(c: &CapabilityDecl) -> String {
    let mut out = String::new();
    // S4a (#1966): a derived capability has no body of its own to render.
    if c.derived.is_some() {
        return describe_derived_capability(c, None, None);
    }
    out.push_str("```bynk\ncapability ");
    out.push_str(&c.name.name);
    out.push_str(" {\n");
    for op in &c.ops {
        out.push_str("\tfn ");
        out.push_str(&op.name.name);
        out.push_str(&capability_op_type_params_str(op));
        out.push('(');
        let parts: Vec<String> = op
            .params
            .iter()
            .map(|p| format!("{}: {}", p.name.name, type_ref_str(&p.type_ref)))
            .collect();
        out.push_str(&parts.join(", "));
        out.push_str(") -> ");
        out.push_str(&type_ref_str(&op.return_type));
        out.push('\n');
    }
    out.push_str("}\n```\n");
    if let Some(doc) = &c.documentation {
        out.push('\n');
        out.push_str(doc);
        out.push('\n');
    }
    out
}

/// S4a (#1966): a derived capability (`capability Sessions = Kv`): its
/// declaration, that it is a distinct instance of its base (in `base_unit`,
/// when known), and the base's operations, when its declaration is at hand.
pub(crate) fn describe_derived_capability(
    c: &CapabilityDecl,
    base_unit: Option<&str>,
    base: Option<&CapabilityDecl>,
) -> String {
    let Some(d) = &c.derived else {
        return describe_capability(c);
    };
    let mut out = format!("```bynk\ncapability {} = {}", c.name.name, d.base.name);
    for a in &d.annotations {
        out.push_str(" @");
        out.push_str(&a.name.name);
    }
    let from = base_unit.map_or(String::new(), |u| format!(" (`{u}`)"));
    // L7 (#2414): an `EdgeCache` instance owns no storage: it switches the
    // Worker's edge cache on.
    let what = if d.base.name == "EdgeCache" {
        "this context's Worker's edge cache"
    } else {
        "its own storage"
    };
    out.push_str(&format!(
        "\n```\n\nA distinct instance of `{}`{from}: {what}, with `{}`'s operations",
        d.base.name, d.base.name
    ));
    match base {
        Some(b) if !b.ops.is_empty() => {
            out.push_str(":\n\n```bynk\n");
            for op in &b.ops {
                let params: Vec<String> = op
                    .params
                    .iter()
                    .map(|p| format!("{}: {}", p.name.name, type_ref_str(&p.type_ref)))
                    .collect();
                out.push_str(&format!(
                    "fn {}{}({}) -> {}\n",
                    op.name.name,
                    capability_op_type_params_str(op),
                    params.join(", "),
                    type_ref_str(&op.return_type)
                ));
            }
            out.push_str("```\n");
        }
        _ => out.push_str(".\n"),
    }
    // P3 (#2342): a rate limiter's emitted stanza, and how it counts.
    if d.base.name == "RateLimit" {
        let number = |name: &str| {
            d.annotations
                .iter()
                .find(|a| a.name.name == name)
                .and_then(|a| a.args.first())
                .and_then(|arg| match &arg.value.kind {
                    ExprKind::IntLit { value, .. } => Some(*value),
                    ExprKind::DurationLit { millis, .. } => Some(millis / 1000),
                    _ => None,
                })
        };
        if let (Some(limit), Some(period)) = (number("limit"), number("period")) {
            out.push_str(&format!(
                "\nEmits `[[ratelimits]]` with `simple = {{ limit = {limit}, period = {period} }}`: {limit} calls per key every {period} seconds. Counting is approximate (per Cloudflare location, eventually consistent).\n"
            ));
        }
    }
    // P2 (#2578): a queue producer's emitted stanza, and the queue it names.
    if d.base.name == "Queue"
        && let Some(queue) = d
            .annotations
            .iter()
            .find(|a| a.name.name == "name")
            .and_then(|a| a.args.first())
            .and_then(|arg| match &arg.value.kind {
                ExprKind::StrLit(s) => Some(s.clone()),
                _ => None,
            })
    {
        out.push_str(&format!(
            "\nEmits `[[queues.producers]]` sending to queue `{queue}` (environment-qualified by `bynk deploy`). Bodies are JSON text, normally `Json.encode(value)`.\n"
        ));
    }
    // P3b (#2582): an Analytics Engine dataset, named by deploy, not source.
    if d.base.name == "Analytics" {
        out.push_str("\nEmits `[[analytics_engine_datasets]]`: the dataset is named for the context and the capability in snake case (`shop.cart`'s `Signups` writes to `shop_cart_signups`), environment-qualified by `bynk deploy`, and created on the first write.\n");
    }
    // P4a (#2602): a D1 database, named by deploy and minted on first deploy.
    if d.base.name == "D1" {
        out.push_str("\nEmits `[[d1_databases]]`: the database is named for the context and the capability (`shop.cart`'s `Orders` is `shop-cart-orders`), environment-qualified, created by `bynk deploy`, which records its id.\n");
    }
    if d.base.name == "EdgeCache" {
        out.push_str("\nEmits `[cache] enabled = true`: a `@cache` route's `2xx` is cached at Cloudflare's edge, and every other `GET` or `HEAD` answer is `no-store`.\n");
    }
    if let Some(doc) = &c.documentation {
        out.push('\n');
        out.push_str(doc);
        out.push('\n');
    }
    out
}

/// message-bundles slice 1 (#859): a message bundle's tag, its annotations
/// (`@reference`, unresolved cardinality shown as-is — that's a checker
/// concern, not hover's), and its declared codes.
pub(crate) fn describe_messages(m: &MessagesDecl) -> String {
    let mut out = String::new();
    out.push_str("```bynk\n");
    // #2618: the unit-private modifier, as written.
    if m.private.is_some() {
        out.push_str("private ");
    }
    out.push_str("messages ");
    out.push('"');
    out.push_str(&m.tag);
    out.push('"');
    for ann in &m.annotations {
        out.push(' ');
        out.push_str(&bynk_fmt::annotation_to_string(ann));
    }
    out.push_str(" {\n");
    for entry in &m.entries {
        // message-bundles slice 3 (#878): name any non-plain (plural/
        // select/number/date) placeholders inline, so an author can tell
        // at a glance which codes carry ICU dispatch without expanding the
        // (elided) template text itself.
        let icu_kinds: Vec<String> = bynk_check::icu::template_format_kinds(&entry.template)
            .into_iter()
            .map(|(name, kind)| (name.to_string(), kind.as_str()))
            .filter(|(_, kind)| *kind != "plain")
            .map(|(name, kind)| format!("{name}: {kind}"))
            .collect();
        if icu_kinds.is_empty() {
            out.push_str(&format!("\t\"{}\" => …\n", entry.code));
        } else {
            out.push_str(&format!(
                "\t\"{}\" => …  // {}\n",
                entry.code,
                icu_kinds.join(", ")
            ));
        }
    }
    out.push_str("}\n```\n");
    if let Some(doc) = &m.documentation {
        out.push('\n');
        out.push_str(doc);
        out.push('\n');
    }
    out
}

/// #1883, slice 1a: an `asset` declaration's markdown — the declaration
/// itself, the rule that it is public, then its doc.
/// #2387, slice 1: an embed's hover and docs. An embed is private: its file's
/// text is compiled into the code as a `String`, never served.
pub(crate) fn describe_embed(e: &EmbedDecl) -> String {
    let mut out = format!(
        "```bynk\n{}embed {} = \"{}\"\n```\n\nPrivate: the file's text, compiled into the code as a `String`; never served.\n",
        if e.private.is_some() { "private " } else { "" },
        e.name.name,
        bynk_fmt::escape_string(&e.path)
    );
    if let Some(doc) = &e.documentation {
        out.push('\n');
        out.push_str(doc);
        out.push('\n');
    }
    out
}

/// #2617: a constant's hover and docs — the declaration as written (its type
/// and initialiser), then its doc.
pub(crate) fn describe_const(c: &ConstDecl) -> String {
    let mut out = format!(
        "```bynk\n{}const {}: {} = {}\n```\n",
        // The unit-private modifier, as written.
        if c.private.is_some() { "private " } else { "" },
        c.name.name,
        type_ref_str(&c.ty),
        bynk_fmt::expr_to_string(&c.init)
    );
    if let Some(doc) = &c.documentation {
        out.push('\n');
        out.push_str(doc);
        out.push('\n');
    }
    out
}

pub(crate) fn describe_asset(a: &AssetDecl) -> String {
    let at =
        a.at.as_ref()
            .map(|(p, _)| format!(" at \"{}\"", bynk_fmt::escape_string(p)))
            .unwrap_or_default();
    // #1883: where it is served. The editor lists the `assets/` folder but
    // never reads its bytes, so a content-hashed URL shows its shape with
    // `<hash>` in place of the content hash — from the same function the
    // build uses, so the two cannot drift.
    let served = match &a.at {
        Some((p, _)) => format!("Served at `{p}`."),
        None => format!(
            "Served at `{}` (content-hashed, cached as immutable).",
            bynk_check::assets::served_path_with_hash(
                &bynk_project::assets::normalise_asset_path(&a.path),
                "<hash>"
            )
        ),
    };
    let mut out = format!(
        "```bynk\n{}asset {} = \"{}\"{at}\n```\n\n{served} Public: served by the platform, never by the Worker.\n",
        if a.private.is_some() { "private " } else { "" },
        a.name.name,
        bynk_fmt::escape_string(&a.path)
    );
    if let Some(doc) = &a.documentation {
        out.push('\n');
        out.push_str(doc);
        out.push('\n');
    }
    out
}

/// v0.123 (slice 2): the `from <protocol>` header suffix for a service, or the
/// empty string for a plain `on call` service.
fn service_protocol_suffix(p: &ServiceProtocol) -> String {
    match p {
        ServiceProtocol::Call => String::new(),
        ServiceProtocol::Http => " from http".to_string(),
        ServiceProtocol::Cron => " from cron".to_string(),
        ServiceProtocol::Queue { name } => format!(" from queue(\"{name}\")"),
        ServiceProtocol::WebSocket { .. } => " from websocket".to_string(),
        // Events track slice 1 (spine #936): a subscription pattern is not
        // rendered in this short suffix, matching how WebSocket's in/out
        // types are also omitted here — this is a one-line summary label,
        // not a full re-render of the header.
        ServiceProtocol::Events { event_type, .. } => {
            format!(" from Events({})", type_ref_str(event_type))
        }
    }
}

/// v0.131: a one-line summary of a `cors { }` policy for hover — the origins
/// (always present), then `credentials`/`maxAge` when set.
fn cors_summary(cors: &CorsPolicy) -> String {
    let mut parts = vec![format!("origins: {:?}", cors.origins())];
    if cors.credentials() {
        parts.push("credentials: true".to_string());
    }
    if let Some(secs) = cors.max_age_secs() {
        parts.push(format!("maxAge: {secs}s"));
    }
    parts.join(", ")
}

/// v0.141: a one-line summary of a `security { }` policy for hover — `nosniff`
/// (default on, shown when off) and `hsts` (when opted in).
fn security_summary(security: &SecurityPolicy) -> String {
    let mut parts = Vec::new();
    if !security.nosniff() {
        parts.push("nosniff: false".to_string());
    }
    if let Some(secs) = security.hsts_max_age_secs() {
        parts.push(format!("hsts: {secs}s"));
    }
    if parts.is_empty() {
        // The default posture (nosniff on, no HSTS) with an empty block.
        "nosniff".to_string()
    } else {
        parts.join(", ")
    }
}

/// v0.142 (ADR 0165): a one-line summary of a `limits { }` policy for hover — the
/// `maxBody` byte ceiling when set.
fn limits_summary(limits: &LimitsPolicy) -> String {
    match limits.max_body() {
        Some(bytes) => format!("maxBody: {bytes} bytes"),
        None => "maxBody".to_string(),
    }
}

/// v0.123 (slice 2): the `on …` line for a handler — its route/protocol shape.
pub(crate) fn handler_line(h: &Handler) -> String {
    match &h.kind {
        HandlerKind::Call => "on call".to_string(),
        HandlerKind::Http { method, path } => format!("on {}(\"{}\")", method.as_str(), path),
        HandlerKind::Cron { expr } => format!("on schedule(\"{expr}\")"),
        HandlerKind::Message => "on message".to_string(),
        HandlerKind::Open => "on open".to_string(),
        HandlerKind::Close => "on close".to_string(),
        HandlerKind::Event => "on event".to_string(),
        HandlerKind::Rejected => "on rejected".to_string(),
    }
}

pub(crate) fn describe_service(s: &ServiceDecl) -> String {
    // v0.123 (slice 2): the protocol header and a line per route (was a bare
    // handler count).
    let mut out = format!(
        "```bynk\nservice {}{} {{\n",
        s.name.name,
        service_protocol_suffix(&s.protocol)
    );
    // v0.131: the CORS policy, if any, renders as a `cors { … }` header line
    // summarising the origins (the load-bearing field).
    if let Some(cors) = &s.cors {
        out.push_str(&format!("\tcors {{ {} }}\n", cors_summary(cors)));
    }
    // v0.141: the security-headers policy, if declared, renders similarly.
    if let Some(security) = &s.security {
        out.push_str(&format!(
            "\tsecurity {{ {} }}\n",
            security_summary(security)
        ));
    }
    // v0.142 (ADR 0165): the request-limits policy, if declared, renders similarly.
    if let Some(limits) = &s.limits {
        out.push_str(&format!("\tlimits {{ {} }}\n", limits_summary(limits)));
    }
    for h in &s.handlers {
        out.push_str(&format!("\t{}\n", handler_line(h)));
    }
    out.push_str("}\n```\n");
    if let Some(doc) = &s.documentation {
        out.push('\n');
        out.push_str(doc);
        out.push('\n');
    }
    out
}

/// v0.123 (slice 2): a store field's kind — `Cell[Int]`, `Map[K, V]`, or a bare
/// head with no type args.
fn store_kind_str(k: &StoreKind) -> String {
    if k.args.is_empty() {
        k.head.name.clone()
    } else {
        let args: Vec<String> = k.args.iter().map(type_ref_str).collect();
        format!("{}[{}]", k.head.name, args.join(", "))
    }
}

pub(crate) fn describe_agent(a: &AgentDecl) -> String {
    // v0.123 (slice 2): the store fields plus the `invariant`/`transition` step
    // invariants (v0.116), was a bare store-field count.
    let mut out = format!(
        "```bynk\nagent {} {{\n\tkey {}: {}\n",
        a.name.name,
        a.key_name.name,
        type_ref_str(&a.key_type),
    );
    for f in &a.store_fields {
        out.push_str(&format!(
            "\tstore {}: {}\n",
            f.name.name,
            store_kind_str(&f.kind)
        ));
    }
    for inv in &a.invariants {
        out.push_str(&format!(
            "\tinvariant {}: {}\n",
            inv.name.name,
            bynk_fmt::expr_to_string(&inv.predicate)
        ));
    }
    for tr in &a.transitions {
        out.push_str(&format!(
            "\ttransition {}: {}\n",
            tr.name.name,
            bynk_fmt::expr_to_string(&tr.predicate)
        ));
    }
    out.push_str("}\n```\n");
    if let Some(doc) = &a.documentation {
        out.push('\n');
        out.push_str(doc);
        out.push('\n');
    }
    out
}

pub(crate) fn describe_provider(p: &ProviderDecl) -> String {
    let mut out = format!(
        "```bynk\nprovides {} = {}\n```\n",
        p.capability.name, p.provider_name.name
    );
    if let Some(doc) = &p.documentation {
        out.push('\n');
        out.push_str(doc);
        out.push('\n');
    }
    out
}

/// A cross-file declaration lookup result: the path of the file containing
/// the declaration, the declaration's source span, and the full source
/// text of that file (returned because callers need it to convert the
/// span to an LSP range and to build hover content).
pub struct CrossFileSymbol {
    pub path: PathBuf,
    pub span: Span,
    pub source: String,
}

/// The project's files, in the same deterministic order
/// `bynk_project::discover_bynk_files` produces (sorted by path) —
/// `HashMap` iteration order is unspecified, and "the first hit wins" only
/// means something if that order is stable and reproducible.
fn sorted_paths(files: &HashMap<PathBuf, String>) -> Vec<&PathBuf> {
    let mut paths: Vec<&PathBuf> = files.keys().collect();
    paths.sort();
    paths
}

/// Find `name`'s declaration in any project file other than `current_path`.
/// Content-ownership track (#1086) slice 1: `files` is a pre-read
/// `(path, content)` map (the caller's overlay-then-disk sweep) rather than
/// bare paths this function used to read from disk itself. Returns the
/// first hit, in path-sorted order (see `sorted_paths`); `None` if the name
/// is not found anywhere in the project.
///
/// Caller is responsible for trying the open file's local symbol table
/// first; this function intentionally skips `current_path` so the local
/// path remains the fast path.
pub fn find_declaration_cross_file(
    files: &HashMap<PathBuf, String>,
    current_path: &Path,
    name: &str,
) -> Option<CrossFileSymbol> {
    for path in sorted_paths(files) {
        if path.as_path() == current_path {
            continue;
        }
        let source = &files[path];
        if let Some(span) = find_declaration_span(source, name) {
            return Some(CrossFileSymbol {
                path: path.clone(),
                span,
                source: source.clone(),
            });
        }
    }
    None
}

/// Markdown hover content for `name` from any project file other than
/// `current_path`, plus the path of the file that contributed it. See
/// `find_declaration_cross_file`'s doc for the content-ownership-track
/// shape of `files`. Returns `None` if the name is not declared anywhere in
/// the project.
pub fn describe_symbol_cross_file(
    files: &HashMap<PathBuf, String>,
    current_path: &Path,
    name: &str,
) -> Option<(PathBuf, String)> {
    for path in sorted_paths(files) {
        if path.as_path() == current_path {
            continue;
        }
        let source = &files[path];
        if let Some(desc) = describe_symbol(source, name) {
            return Some((path.clone(), desc));
        }
    }
    None
}

pub fn type_ref_str(t: &TypeRef) -> String {
    match t {
        // v0.20a: function types render in Bynk surface syntax.
        TypeRef::Fn(params, ret, _) => {
            let lhs = match params.len() {
                0 => "()".to_string(),
                1 if !matches!(params[0], TypeRef::Fn(..)) => type_ref_str(&params[0]),
                _ => format!(
                    "({})",
                    params
                        .iter()
                        .map(type_ref_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            };
            format!("{lhs} -> {}", type_ref_str(ret))
        }
        TypeRef::Base(b, _) => b.name().to_string(),
        TypeRef::Named(id) => id.name.clone(),
        TypeRef::Result(a, b, _) => format!("Result[{}, {}]", type_ref_str(a), type_ref_str(b)),
        TypeRef::Option(t, _) => format!("Option[{}]", type_ref_str(t)),
        TypeRef::Effect(t, _) => format!("Effect[{}]", type_ref_str(t)),
        TypeRef::HttpResult(t, _) => format!("HttpResult[{}]", type_ref_str(t)),
        TypeRef::QueueResult(_) => "QueueResult".to_string(),
        TypeRef::Asset(_) => "Asset".to_string(),
        TypeRef::RequestHeaders(_) => "RequestHeaders".to_string(),
        TypeRef::TokenClaims(_) => "TokenClaims".to_string(),
        TypeRef::BoundaryRejection(_) => "BoundaryRejection".to_string(),
        // v0.20b: the built-in collection types.
        TypeRef::List(t, _) => format!("List[{}]", type_ref_str(t)),
        TypeRef::Set(t, _) => format!("Set[{}]", type_ref_str(t)),
        TypeRef::Query(t, _) => format!("Query[{}]", type_ref_str(t)),
        TypeRef::Stream(t, _) => format!("Stream[{}]", type_ref_str(t)),
        TypeRef::Connection(t, _) => format!("Connection[{}]", type_ref_str(t)),
        TypeRef::History(t, _) => format!("History[{}]", type_ref_str(t)),
        TypeRef::Map(k, v, _) => format!("Map[{}, {}]", type_ref_str(k), type_ref_str(v)),
        TypeRef::ValidationError(_) => "ValidationError".to_string(),
        TypeRef::JsonError(_) => "JsonError".to_string(),
        TypeRef::Unit(_) => "()".to_string(),
        // v0.157 (ADR 0183): a user generic-type application, as written.
        TypeRef::App { name, args, .. } => format!(
            "{}[{}]",
            name.name,
            args.iter().map(type_ref_str).collect::<Vec<_>>().join(", ")
        ),
    }
}

#[cfg(test)]
mod tests {
    /// #2683: hover on `within` names its signature and the abandoned-effect
    /// caveat, and only when the receiver is an `Effect`.
    #[test]
    fn within_hovers_on_an_effect_receiver_only() {
        use bynk_check::checker::{Ty, Types};
        use bynk_syntax::ast::BaseType;
        let tys = Types::new();
        let int = tys.intern(Ty::Base(BaseType::Int));
        let effect = tys.intern(Ty::Effect(int));
        let src = "let r <- p.quote(1).within(2.seconds)\n";
        let dot = src.find(".within").unwrap();
        let at = src.find("within").unwrap() + 1;
        let on_effect = [(Span::new(8, dot), effect)];
        let hover = describe_effect_method_at(src, at, &on_effect, &tys).expect("within hovers");
        assert!(
            hover.contains("within(d: Duration) -> Effect[Result[T, TimedOut]]"),
            "{hover}"
        );
        assert!(hover.contains("not cancelled"), "{hover}");
        assert!(hover.contains("waitUntil"), "{hover}");
        // A receiver that is not an `Effect` is a user's own `within`.
        let on_int = [(Span::new(8, dot), int)];
        assert!(describe_effect_method_at(src, at, &on_int, &tys).is_none());
        // Elsewhere on the line is not `within`.
        assert!(describe_effect_method_at(src, 0, &on_effect, &tys).is_none());

        // #2711: `race` hovers the same way, with the loser caveat.
        let src = "let r <- p.get(1).race(p.get(2))\n";
        let dot = src.find(".race").unwrap();
        let at = src.find("race").unwrap() + 1;
        let on_effect = [(Span::new(8, dot), effect)];
        let hover = describe_effect_method_at(src, at, &on_effect, &tys).expect("race hovers");
        assert!(
            hover.contains("race(other: Effect[T]) -> Effect[T]"),
            "{hover}"
        );
        assert!(hover.contains("loser is not cancelled"), "{hover}");
        assert!(hover.contains("idempotent"), "{hover}");
        let on_int = [(Span::new(8, dot), int)];
        assert!(describe_effect_method_at(src, at, &on_int, &tys).is_none());
    }

    /// A constant's hover shows its `private` modifier as written, and the
    /// modifier itself hovers before `const` as before the other kinds.
    #[test]
    fn private_const_hover_shows_the_modifier() {
        let src = "commons m\n\nprivate const MaxRetries: Int = 3\n\nconst Shown: Int = 1\n";
        let tokens = tokenize(src).unwrap();
        let (Some(SourceUnit::Commons(c)), _) = parse_unit_with_recovery(&tokens, src) else {
            panic!("a commons");
        };
        let hidden = describe_item(&c.items[0], "MaxRetries").unwrap();
        assert!(
            hidden.starts_with("```bynk\nprivate const MaxRetries: Int = 3\n"),
            "{hidden}"
        );
        let shown = describe_item(&c.items[1], "Shown").unwrap();
        assert!(shown.starts_with("```bynk\nconst Shown"), "{shown}");
        assert!(describe_private_at(src, src.find("private").unwrap()).is_some());
    }

    /// #2640: `par` hovers only as the head of a `par { … }` block.
    #[test]
    fn par_hover_is_for_the_block_keyword_only() {
        let block =
            "fn f() -> Effect[Int] {\n  par {\n    let a <- g()\n  }\n  Effect.pure(a)\n}\n";
        let at = block.find("par").unwrap();
        assert!(describe_par_at(block, at).unwrap().contains("par {"));
        let ident = "fn f(par: Int) -> Int {\n  par\n}\n";
        assert!(describe_par_at(ident, ident.find("par").unwrap()).is_none());
        assert!(describe_par_at(ident, ident.rfind("par").unwrap()).is_none());
    }

    use super::*;

    /// #2136: the `system` cron/queue hover answers only when the receiver is
    /// a `cron` / `queue` service of the suite's target, not a record field
    /// named `message`, and not at a lower tier.
    #[test]
    fn system_job_hover_needs_a_job_service_receiver() {
        const CTX: &str = "context cr.jobs\n\ntype Envelope = { message: String }\n\nservice sweeper from cron {\n  on schedule(\"*/5 * * * *\") () -> Effect[Result[(), String]] {\n    Ok(())\n  }\n}\n\nservice deliveries from queue(\"d\") {\n  on message(e: Envelope) -> Effect[QueueResult] {\n    Ack\n  }\n}\n";
        let suite = |tier: &str| {
            format!(
                "suite cr.jobs{tier} {{\n  case \"c\" {{\n    let e = Envelope {{ message: \"hi\" }}\n    let m = e.message\n    let r <- deliveries.message(e)\n    let s <- sweeper.schedule(\"*/5 * * * *\")\n    expect m == \"hi\"\n  }}\n}}\n"
            )
        };
        let at = |src: &str, needle: &str| {
            src.find(needle).expect("needle is in the suite") + needle.len() - "message".len()
        };
        let system = suite(" as system");
        let field = at(&system, "e.message");
        assert!(describe_system_job_address_at(&system, field, [CTX]).is_none());
        let queue = at(&system, "deliveries.message");
        let hover =
            describe_system_job_address_at(&system, queue, [CTX]).expect("queue address hovers");
        assert!(hover.contains("`queue` entry"), "{hover}");
        let cron = system.find("sweeper.schedule").expect("cron address") + "sweeper.".len();
        let hover =
            describe_system_job_address_at(&system, cron, [CTX]).expect("cron address hovers");
        assert!(hover.contains("`scheduled` entry"), "{hover}");
        let unit = suite("");
        let queue = at(&unit, "deliveries.message");
        assert!(describe_system_job_address_at(&unit, queue, [CTX]).is_none());
    }

    /// #2138: `run.id` hovers in a case body, on either half, but not where
    /// the case binds its own `run`, nor outside a suite.
    #[test]
    fn run_id_hovers_only_as_the_test_run() {
        let src = "suite rid.orders {\n  case \"a\" {\n    expect run.id == \"local\"\n  }\n  case \"b\" {\n    let run = Wrapper { id: \"x\" }\n    expect run.id == \"x\"\n  }\n}\n";
        let first = src.find("run.id").expect("first run.id");
        assert!(describe_run_id_at(src, first).is_some());
        assert!(describe_run_id_at(src, first + "run.".len()).is_some());
        let shadowed = src.rfind("run.id").expect("second run.id");
        assert!(describe_run_id_at(src, shadowed).is_none());
        let interp = "suite rid.orders as system {\n  case \"a\" {\n    let r <- api.POST(\"/o/:id\", \"o-\\(run.id)\") by Visitor\n    expect r is Ok(true)\n  }\n}\n";
        assert!(describe_run_id_at(interp, interp.find("run.id").expect("run.id")).is_some());
        let ctx = "context rid.orders\n\nfn f(run: Wrapper) -> String {\n  run.id\n}\n";
        assert!(describe_run_id_at(ctx, ctx.find("run.id").expect("run.id")).is_none());
    }

    /// #2175: the shadowing check follows the checker's scopes — a `let`
    /// in a nested block, a lambda parameter, a `match` pattern and an
    /// `if … is` binding each shadow `run`; outside their scope it is the
    /// test run again.
    #[test]
    fn run_id_hover_respects_nested_binders() {
        let src = "suite rid.orders {\n  case \"a\" {\n    let w = Wrapper { id: \"x\" }\n    let a = if true {\n      let run = w\n      run.id\n    } else { \"\" }\n    let f = (run: Wrapper) => run.id\n    let m = match Some(w) { Some(run) => run.id, None => \"\" }\n    let i = if Some(w) is Some(run) { run.id } else { \"\" }\n    let j = Some(w) is Some(run) && run.id == \"x\"\n    let k = if !(Some(w) is Some(run)) { \"\" } else { run.id }\n    expect run.id == \"local\"\n  }\n}\n";
        let starts: Vec<usize> = src.match_indices("run.id").map(|(i, _)| i).collect();
        assert_eq!(starts.len(), 7, "{src}");
        for &shadowed in &starts[..6] {
            assert!(
                describe_run_id_at(src, shadowed).is_none(),
                "shadowed at {shadowed}: {}",
                &src[shadowed.saturating_sub(30)..shadowed + 6]
            );
        }
        assert!(describe_run_id_at(src, starts[6]).is_some(), "the test run");
    }

    /// #2135: hover on a reserved principal after `by` says what it sends;
    /// the same name elsewhere is not one.
    #[test]
    fn hover_describes_reserved_principals() {
        let src = "suite pay.hooks as system {\n\tcase \"c\" {\n\t\tlet r <- webhooks.POST(\"/h\", \"x\") by Forged\n\t\tlet Stale = 1\n\t\texpect r is Rejected(Unauthorized)\n\t}\n}\n";
        let forged = describe_reserved_principal_at(src, src.find("Forged").unwrap()).unwrap();
        assert!(
            forged.contains("by Forged") && forged.contains("does not hold"),
            "{forged}"
        );
        assert!(describe_reserved_principal_at(src, src.find("Stale").unwrap()).is_none());
    }

    /// #2134: hover on an `env` clause's word and entry names says where the
    /// entry applies.
    #[test]
    fn hover_describes_env_entries() {
        let src = "suite shop.pay {\n\tenv {\n\t\tA = \"x\"\n\t}\n\n\tcase \"c\" {\n\t\tenv { A = \"y\"\n\t\t\tB = \"z\" }\n\t\texpect true\n\t}\n}\n";
        let at = |needle: &str| src.find(needle).unwrap();
        assert_eq!(
            describe_env_at(src, at("env")).as_deref(),
            Some(ENV_KEYWORD_DOC)
        );
        let suite_a = describe_env_at(src, at("A = \"x\"")).unwrap();
        assert!(suite_a.contains("A = \"x\""), "{suite_a}");
        assert!(suite_a.ends_with("(suite)"), "{suite_a}");
        let case_a = describe_env_at(src, at("A = \"y\"")).unwrap();
        assert!(case_a.ends_with("(case, overrides suite)"), "{case_a}");
        let case_b = describe_env_at(src, at("B = ")).unwrap();
        assert!(case_b.ends_with("(case)"), "{case_b}");
        assert!(describe_env_at(src, at("expect")).is_none());
    }

    /// S3 (#1963): hover on a first-party extensible enum says a `match` on it
    /// needs a wildcard; a closed one, and a stable one, say nothing extra.
    #[test]
    fn hover_marks_an_extensible_first_party_enum() {
        let fetch_error = describe_firstparty_symbol("FetchError").expect("FetchError hovers");
        assert!(fetch_error.contains("**Extensible**"), "{fetch_error}");
        assert!(
            !fetch_error.contains("**Experimental**"),
            "nothing ships experimental"
        );
        assert!(
            describe_firstparty_symbol("Method")
                .unwrap()
                .contains("**Extensible**")
        );
        let closed = describe_firstparty_symbol("Request").expect("Request hovers");
        assert!(
            !closed.contains("**Extensible**"),
            "a closed first-party type is not marked extensible"
        );
        assert_eq!(
            firstparty_notes("app.user", "FetchError"),
            "",
            "a user's own enum"
        );
    }

    /// Content-ownership track (#1086) slice 1: `find_declaration_cross_file`/
    /// `describe_symbol_cross_file` take a pre-read `(path, content)` map
    /// rather than bare paths they used to read from disk themselves, so the
    /// tests below build that map directly — a synthetic root plus a
    /// `(relative_path, contents)` list, no real filesystem I/O needed at all.
    fn project_files(root: &Path, files: &[(&str, &str)]) -> HashMap<PathBuf, String> {
        files
            .iter()
            .map(|(rel, contents)| (root.join(rel), (*contents).to_string()))
            .collect()
    }

    #[test]
    fn receiver_segment_survives_a_multibyte_char_before_the_receiver() {
        // A multi-byte non-identifier char before the receiver used to make the
        // `i + 1` byte offset land mid-codepoint → panic on slice (#715),
        // reached from hover on a `recv.member` access inside a string literal.
        let text = "\"€p.items"; // member `items` sits after the dot
        let member_start = text.rfind("items").unwrap();
        let (recv, start) =
            receiver_segment_at(text, Span::new(member_start, text.len())).expect("receiver");
        assert_eq!(recv, "p");
        assert_eq!(&text[start..start + recv.len()], "p");
    }

    #[test]
    fn cross_file_definition_resolves_into_sibling_file() {
        let root = PathBuf::from("/synthetic/cross_file_definition");
        let files = project_files(
            &root,
            &[
                (
                    "a.bynk",
                    "commons demo.a\n\ntype Foo = Int where Positive\n",
                ),
                (
                    "b.bynk",
                    "commons demo.b\n\nuses demo.a\n\ntype Bar = Int where NonNegative\n",
                ),
            ],
        );
        let current = root.join("b.bynk");
        let found = find_declaration_cross_file(&files, &current, "Foo")
            .expect("Foo should resolve into a.bynk");
        let expected = root.join("a.bynk");
        assert_eq!(found.path, expected);
        assert!(
            found.source.contains("type Foo = Int where Positive"),
            "source returned does not contain Foo declaration"
        );
    }

    #[test]
    fn cross_file_definition_skips_current_file() {
        let root = PathBuf::from("/synthetic/cross_file_skip_current");
        let files = project_files(
            &root,
            &[(
                "only.bynk",
                "commons demo.only\n\ntype Foo = Int where Positive\n",
            )],
        );
        let current = root.join("only.bynk");
        // The only file containing Foo is current; cross-file must skip it.
        assert!(find_declaration_cross_file(&files, &current, "Foo").is_none());
    }

    #[test]
    fn cross_file_hover_returns_markdown_summary() {
        let root = PathBuf::from("/synthetic/cross_file_hover");
        let files = project_files(
            &root,
            &[
                (
                    "money.bynk",
                    "commons demo.money\n\n\
                     ---\n\
                     Amount in minor units of currency.\n\
                     ---\n\
                     type Money = Int where NonNegative\n",
                ),
                (
                    "orders.bynk",
                    "commons demo.orders\n\nuses demo.money\n\ntype OrderId = Int where Positive\n",
                ),
            ],
        );
        let current = root.join("orders.bynk");
        let (other_path, desc) = describe_symbol_cross_file(&files, &current, "Money")
            .expect("Money should produce hover content");
        assert_eq!(other_path, root.join("money.bynk"));
        assert!(desc.contains("type Money"));
        assert!(
            desc.contains("Amount in minor units"),
            "hover should include the doc block"
        );
    }

    #[test]
    fn cross_file_returns_none_for_unknown_name() {
        let root = PathBuf::from("/synthetic/cross_file_none");
        let files = project_files(
            &root,
            &[(
                "a.bynk",
                "commons demo.a\n\ntype Foo = Int where Positive\n",
            )],
        );
        let current = root.join("a.bynk");
        assert!(find_declaration_cross_file(&files, &current, "DoesNotExist").is_none());
        assert!(describe_symbol_cross_file(&files, &current, "DoesNotExist").is_none());
    }

    #[test]
    fn first_party_symbols_describe_their_signature_and_doc() {
        // Slice 9: stdlib/surface symbols live in the embedded sources, not the
        // project — the hover/completion-doc fallback finds them there, signature
        // and `---` doc block alike.
        let reverse = describe_firstparty_symbol("reverse").expect("`bynk.list.reverse` described");
        assert!(
            reverse.contains("reverse") && reverse.contains("List"),
            "{reverse}"
        );
        assert!(
            reverse.contains("reverse order"),
            "doc block surfaced: {reverse}"
        );
        // The `bynk` adapter surface too (a capability, exercising the adapter path).
        let clock = describe_firstparty_symbol("Clock").expect("`bynk`-surface `Clock`");
        assert!(
            clock.contains("wall-clock"),
            "capability doc surfaced: {clock}"
        );
        // #901: the whole locale surface hovered as nothing before
        // `bynk.locale`/`bynk.locale.types` reached the shared source list. A
        // type from the leaf, and a fn from the value-level commons.
        let locale_tag = describe_firstparty_symbol("LocaleTag")
            .expect("`bynk.locale.types.LocaleTag` described");
        assert!(
            locale_tag.contains("LocaleTag") && locale_tag.contains("Matches"),
            "{locale_tag}"
        );
        let render = describe_firstparty_symbol("render").expect("`bynk.locale.render` described");
        assert!(
            render.contains("render") && render.contains("LocaleTag"),
            "{render}"
        );
        let with_text =
            describe_firstparty_symbol("withText").expect("`bynk.locale.withText` described");
        assert!(with_text.contains("withText"), "{with_text}");
        // A name in no first-party source yields nothing (the fallback no-ops).
        assert!(describe_firstparty_symbol("DoesNotExist").is_none());
    }

    #[test]
    fn unit_reference_spans_finds_uses_and_consumes_targets() {
        // Slice 6b: the clickable ranges for document links — `uses`/`consumes`
        // unit names, with spans covering the name (resolution is the handler's).
        let src = "context app.main\n  uses billing.charge\n  consumes platform.time\n";
        let spans = unit_reference_spans(src);
        let names: Vec<&str> = spans.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"billing.charge"), "{names:?}");
        assert!(names.contains(&"platform.time"), "{names:?}");
        // The span covers exactly the unit name (so the link underlines it).
        let (_, span) = spans.iter().find(|(n, _)| n == "billing.charge").unwrap();
        assert_eq!(&src[span.start..span.end], "billing.charge");
    }

    #[test]
    fn unit_reference_spans_links_the_suite_target() {
        // #609: the `suite <target>` header links to the unit under test, and any
        // `uses` clauses the fragment brings in link like the other unit kinds.
        let src = "suite todos\n  uses billing.charge\n";
        let spans = unit_reference_spans(src);
        let names: Vec<&str> = spans.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"todos"), "{names:?}");
        assert!(names.contains(&"billing.charge"), "{names:?}");
        // The span covers exactly the target name (so the link underlines it).
        let (_, span) = spans.iter().find(|(n, _)| n == "todos").unwrap();
        assert_eq!(&src[span.start..span.end], "todos");
    }

    #[test]
    fn doc_link_context_is_some_only_inside_a_doc_range() {
        let src = "---\nSee [Rat\n---\nfn f() -> Int { [1, 2] }\n";
        let at = src.find("Rat").unwrap() + 3;
        let ctx = doc_link_context(src, at).expect("in doc");
        assert_eq!(ctx.partial, "Rat");
        assert_eq!(ctx.owner, None);
        assert_eq!(ctx.replace_start, at - 3);
        assert!(!ctx.closed);
        assert_eq!(ctx.attached, Some(src.find("f()").unwrap()));
        let code = src.find("[1").unwrap() + 1;
        assert_eq!(doc_link_context(src, code), None);
    }

    #[test]
    fn doc_link_context_splits_an_owner_prefix_and_sees_a_closer() {
        let src = "---\nSee [RateView.re]\n---\nfn f() -> Int { 1 }\n";
        let at = src.find(".re").unwrap() + 3;
        let ctx = doc_link_context(src, at).unwrap();
        assert_eq!(ctx.owner.as_deref(), Some("RateView"));
        assert_eq!(ctx.partial, "re");
        assert!(ctx.closed);
    }

    #[test]
    fn doc_link_context_replaces_the_rest_of_a_word_under_the_cursor() {
        let src = "---\nSee [RateView] here\n---\nfn f() -> Int { 1 }\n";
        let at = src.find("Rat").unwrap() + 3;
        let ctx = doc_link_context(src, at).unwrap();
        assert_eq!(ctx.partial, "Rat");
        assert_eq!(&src[ctx.replace_start..ctx.replace_end], "RateView");
        assert!(ctx.closed);
    }

    #[test]
    fn doc_link_context_is_none_in_a_fenced_block_and_after_a_bare_dot() {
        let fenced = "---\n```\nlet v = xs[\n```\n---\nfn f() -> Int { 1 }\n";
        let at = fenced.find("xs[").unwrap() + 3;
        assert_eq!(doc_link_context(fenced, at), None);
        let dot = "---\nSee [.foo\n---\nfn f() -> Int { 1 }\n";
        let at = dot.find(".foo").unwrap() + 4;
        assert_eq!(doc_link_context(dot, at), None);
    }

    #[test]
    fn doc_link_context_is_none_after_a_closed_or_broken_bracket() {
        let src = "---\nSee [Done] and x\n---\nfn f() -> Int { 1 }\n";
        let at = src.find(" and").unwrap() + 4;
        assert_eq!(doc_link_context(src, at), None);
    }

    #[test]
    fn doc_link_context_works_on_a_doc_line() {
        let src = "--| See [Li\nfn f() -> Int { 1 }\n";
        let at = src.find("Li").unwrap() + 2;
        assert_eq!(doc_link_context(src, at).unwrap().partial, "Li");
    }

    #[test]
    fn scan_doc_link_candidates_finds_a_bare_shortcut() {
        let cands = scan_doc_link_candidates("See [Limiter] for details.");
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0].name, "Limiter");
        assert_eq!(cands[0].display, "Limiter");
        assert_eq!(
            &"See [Limiter] for details."[cands[0].span.clone()],
            "[Limiter]"
        );
    }

    #[test]
    fn scan_doc_link_candidates_finds_a_dotted_shortcut() {
        let cands = scan_doc_link_candidates("See [RateView.remaining].");
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0].name, "RateView.remaining");
    }

    #[test]
    fn scan_doc_link_candidates_finds_a_code_span_shortcut() {
        let text = "The [`RateView.remaining`] field is never negative.";
        let cands = scan_doc_link_candidates(text);
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0].name, "RateView.remaining");
        assert_eq!(cands[0].display, "`RateView.remaining`");
        assert_eq!(&text[cands[0].span.clone()], "[`RateView.remaining`]");
    }

    #[test]
    fn scan_doc_link_candidates_finds_a_full_reference() {
        let cands = scan_doc_link_candidates("See [the limiter][Limiter] for details.");
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0].name, "Limiter");
        assert_eq!(cands[0].display, "the limiter");
    }

    #[test]
    fn scan_doc_link_candidates_leaves_an_explicit_url_untouched() {
        let cands = scan_doc_link_candidates("See [Limiter](https://example.com).");
        assert!(cands.is_empty(), "{cands:?}");
    }

    #[test]
    fn scan_doc_link_candidates_leaves_an_author_defined_reference_untouched() {
        let text = "See [foo] for details.\n\n[foo]: https://example.com\n";
        let cands = scan_doc_link_candidates(text);
        assert!(cands.is_empty(), "{cands:?}");
    }

    #[test]
    fn scan_doc_link_candidates_ignores_non_identifier_bracket_content() {
        let cands = scan_doc_link_candidates("See [see note] and [1] below.");
        assert!(cands.is_empty(), "{cands:?}");
    }

    #[test]
    fn scan_doc_link_candidates_ignores_fenced_code_block_content() {
        let text = "Example:\n```\n[Bracket, Syntax]\n[label]: not-a-real-def\n```\nSee [Limiter].";
        let cands = scan_doc_link_candidates(text);
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0].name, "Limiter");
    }

    #[test]
    fn scan_doc_link_candidates_does_not_recognise_the_collapsed_reference_form() {
        // `[label][]` (CommonMark's "collapsed reference") is deliberately out
        // of scope this increment — use the shortcut form `[label]` instead.
        let cands = scan_doc_link_candidates("See [Limiter][].");
        assert!(cands.is_empty(), "{cands:?}");
    }

    #[test]
    fn doc_link_spans_reports_absolute_offsets_in_an_indented_doc_block() {
        let src = "context app.main\n  ---\n  See [Limiter] here.\n  ---\n  service api {}\n";
        let spans = doc_link_spans(src);
        assert_eq!(spans.len(), 1);
        let (name, span) = &spans[0];
        assert_eq!(name, "Limiter");
        assert_eq!(&src[span.start..span.end], "[Limiter]");
    }

    /// #1888: a `--|` doc line links too, at its absolute offset.
    #[test]
    fn doc_link_spans_finds_links_in_doc_lines() {
        let src = "--- Wraps a [Foo].\n--- And a [Bar].\ntype Baz = Int\n";
        let spans = doc_link_spans(src);
        let names: Vec<&str> = spans.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["Foo", "Bar"]);
        assert_eq!(&src[spans[1].1.start..spans[1].1.end], "[Bar]");
    }

    #[test]
    fn doc_link_spans_finds_links_across_multiple_doc_blocks() {
        let src = "---\nSee [Foo].\n---\ntype Foo = Int\n\n---\nSee [Bar].\n---\ntype Bar = Int\n";
        let spans = doc_link_spans(src);
        let names: Vec<&str> = spans.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["Foo", "Bar"]);
    }

    #[test]
    fn own_declaration_name_finds_context_and_commons_names() {
        // #302: the rename target when a unit's own file moves.
        let src = "context app.main\n  uses billing.charge\n";
        let (name, span) = own_declaration_name(src).expect("context has its own name");
        assert_eq!(name, "app.main");
        assert_eq!(&src[span.start..span.end], "app.main");

        let src = "commons app.util\n\ntype Foo = Int where Positive\n";
        let (name, _) = own_declaration_name(src).expect("commons has its own name");
        assert_eq!(name, "app.util");
    }

    #[test]
    fn own_declaration_name_none_for_suites() {
        // A suite's `SourceUnit::name()` is its *target*'s name, not its own —
        // nothing `uses`/`consumes` a suite by name, so renaming its file
        // needs no declaration rewrite.
        let src = "suite todos\n";
        assert!(own_declaration_name(src).is_none());
    }

    // v0.123 (slice 2): hover renders the real shape of each declaration —
    // record fields, sum variants, the refined `where`, the opaque base.
    #[test]
    fn describe_type_renders_fields_variants_and_refinements() {
        let record = describe_symbol(
            "commons demo.m\n\ntype Order = {\n  id: OrderId,\n  total: Money,\n}\n",
            "Order",
        )
        .unwrap();
        assert!(record.contains("type Order = {"), "{record}");
        assert!(record.contains("id: OrderId"), "{record}");
        assert!(record.contains("total: Money"), "{record}");

        let sum = describe_symbol(
            "commons demo.m\n\ntype Status = enum { Pending, Shipped }\n",
            "Status",
        )
        .unwrap();
        assert!(sum.contains("enum {"), "{sum}");
        assert!(sum.contains("Pending") && sum.contains("Shipped"), "{sum}");

        let refined = describe_symbol(
            "commons demo.m\n\ntype Email = String where NonEmpty\n",
            "Email",
        )
        .unwrap();
        assert!(
            refined.contains("type Email = String where NonEmpty"),
            "{refined}"
        );

        let opaque =
            describe_symbol("commons demo.m\n\ntype Token = opaque String\n", "Token").unwrap();
        assert!(opaque.contains("type Token = opaque String"), "{opaque}");
    }

    // v0.123 (slice 2): a function's `requires`/`ensures` contracts render
    // beneath its signature.
    #[test]
    fn describe_fn_renders_contracts() {
        let src = "commons demo.m\n\nfn discount(p: Int, pct: Int) -> Int\n  requires p_nonneg: p >= 0\n  ensures never_negative: result >= 0\n{\n  p\n}\n";
        let out = describe_symbol(src, "discount").unwrap();
        assert!(
            out.contains("fn discount(p: Int, pct: Int) -> Int"),
            "{out}"
        );
        assert!(out.contains("requires p_nonneg: p >= 0"), "{out}");
        assert!(out.contains("ensures never_negative: result >= 0"), "{out}");
    }

    // Constrained generics slice 0: hover shows a generic's declared bounds and
    // a method's `where` clause.
    #[test]
    fn describe_fn_renders_bounds_and_where() {
        let src = "commons demo.m\n\ntype Box[A] = { value: A }\n\nfn same[T: Eq, U](a: T, b: T, u: U) -> Bool { a == b }\n\nfn Box.has(self, x: A) -> Bool where A: Eq { self.value == x }\n";
        let out = describe_symbol(src, "same").unwrap();
        assert!(
            out.contains("fn same[T: Eq, U](a: T, b: T, u: U) -> Bool"),
            "{out}"
        );
        let method = describe_symbol(src, "Box.has").unwrap();
        assert!(
            method.contains("fn Box.has(self, x: A) -> Bool where A: Eq"),
            "{method}"
        );
        let ty =
            describe_symbol("commons demo.m\n\ntype Pair[T: Eq] = { a: T }\n", "Pair").unwrap();
        assert!(ty.contains("type Pair[T: Eq] = "), "{ty}");
    }

    // Constrained generics slice 0: every constraint name in a bound is a token
    // span — a `fn`'s, a `type`'s and a method's `where` entries — and a bare
    // parameter has none.
    #[test]
    fn constraint_token_spans_cover_every_bound() {
        let src = "commons demo.m\n\ntype P[T: Eq] = { a: T }\n\nfn f[T: Eq + Eq, U](a: T, u: U) -> T { a }\n\nfn P.g(self) -> Int where T: Eq { 0 }\n";
        let names: Vec<&str> = constraint_token_spans(src)
            .into_iter()
            .map(|s| &src[s.start..s.end])
            .collect();
        assert_eq!(names, ["Eq", "Eq", "Eq", "Eq"]);
        assert!(constraint_token_spans("commons m\n\nfn f[T](a: T) -> T { a }\n").is_empty());
    }

    // Constrained generics slice 1: a type's hover shows its `derives` clause,
    // a generic's signature its `Ord` bounds.
    #[test]
    fn describe_renders_derives_and_ord_bounds() {
        let src = "commons demo.m\n\ntype Priority = enum { Low, High } derives Ord\n\ntype Row = { a: Int } derives Ord\n\nfn top[T: Ord](xs: List[T]) -> Option[T] { None }\n\nfn both[T: Eq + Ord](a: T) -> T { a }\n\nfn Box.big(self, x: A) -> Bool where A: Ord { true }\n";
        let p = describe_symbol(src, "Priority").unwrap();
        assert!(p.contains("} derives Ord\n```"), "{p}");
        assert!(
            describe_symbol(src, "Row")
                .unwrap()
                .contains("} derives Ord")
        );
        assert!(
            describe_symbol(src, "top")
                .unwrap()
                .contains("fn top[T: Ord](xs: List[T]) -> Option[T]")
        );
        assert!(
            describe_symbol(src, "both")
                .unwrap()
                .contains("fn both[T: Eq + Ord](a: T)")
        );
        assert!(
            describe_symbol(src, "Box.big")
                .unwrap()
                .contains("-> Bool where A: Ord")
        );
        // No clause, no `derives`.
        let plain = describe_symbol("commons demo.m\n\ntype R = { a: Int }\n", "R").unwrap();
        assert!(!plain.contains("derives"), "{plain}");
    }

    // Constrained generics slice 1: `derives` hovers as a keyword only in a
    // type's clause; a name that merely reads `derives` does not.
    #[test]
    fn describe_derives_at_is_the_clause_only() {
        let src =
            "commons demo.m\n\ntype P = Int derives Ord\n\nfn f(derives: Int) -> Int { derives }\n";
        let at = src.find("derives").unwrap();
        let doc = describe_derives_at(src, at).expect("hover on `derives`");
        assert!(
            doc.contains("derives Ord") && doc.contains("declaration order"),
            "{doc}"
        );
        assert!(describe_derives_at(src, at + 2).is_some());
        assert!(describe_derives_at(src, src.find("Ord").unwrap()).is_none());
        assert!(describe_derives_at(src, src.rfind("derives").unwrap()).is_none());
        assert!(describe_derives_at(src, src.find("(derives").unwrap() + 1).is_none());
    }

    // Constrained generics slice 1: hover on a constraint name in a bound or a
    // `derives` clause explains it; a lookalike elsewhere does not.
    #[test]
    fn describe_constraint_at_explains_bounds_and_derives() {
        let src = "commons demo.m\n\ntype P = Int derives Ord\n\nfn f[T: Eq + Ord](a: T) -> T { a }\n\nfn Box.g(self, x: A) -> Bool where A: Ord { true }\n";
        let eq = describe_constraint_at(src, src.find("Eq").unwrap()).unwrap();
        assert!(eq.contains("constraint Eq") && eq.contains("=="), "{eq}");
        for (i, _) in src.match_indices("Ord") {
            let ord = describe_constraint_at(src, i + 1).expect("Ord hover");
            assert!(
                ord.contains("constraint Ord") && ord.contains("implies `Eq`"),
                "{ord}"
            );
        }
        assert!(describe_constraint_at(src, src.find("Int").unwrap()).is_none());
        assert!(describe_constraint_at(src, src.find("fn f").unwrap()).is_none());
    }

    // Constrained generics slice 1: `derives Ord` names are constraint tokens.
    #[test]
    fn constraint_token_spans_cover_derives() {
        let src =
            "commons demo.m\n\ntype P = Int derives Ord\n\ntype Q[T: Ord] = { a: T } derives Ord\n";
        let names: Vec<&str> = constraint_token_spans(src)
            .into_iter()
            .map(|s| &src[s.start..s.end])
            .collect();
        assert_eq!(names, ["Ord", "Ord", "Ord"]);
        let first = constraint_token_spans(src)[0];
        assert!(
            src[..first.start].ends_with("derives "),
            "derives clause is the first span"
        );
    }

    // v0.123 (slice 2): a service renders its protocol header and route lines.
    #[test]
    fn describe_service_renders_routes() {
        let src = "context demo.app\n\nservice greeter {\n  on call(name: String) -> Effect[String] {\n    Effect.pure(name)\n  }\n}\n";
        let out = describe_symbol(src, "greeter").unwrap();
        assert!(out.contains("service greeter {"), "{out}");
        assert!(out.contains("on call"), "{out}");
    }

    // v0.123 (slice 2): an agent renders its store fields and the
    // `invariant`/`transition` step invariants.
    #[test]
    fn describe_agent_renders_store_and_invariants() {
        let src = "context demo.app\n\nagent Counter {\n  key id: String\n  store count: Cell[Int] = 0\n  invariant non_negative: count >= 0\n  transition monotonic: new.count >= old.count\n  on call bump() -> Effect[Result[(), String]] {\n    Ok(())\n  }\n}\n";
        let out = describe_symbol(src, "Counter").unwrap();
        assert!(out.contains("agent Counter {"), "{out}");
        assert!(out.contains("key id: String"), "{out}");
        assert!(out.contains("store count: Cell[Int]"), "{out}");
        assert!(out.contains("invariant non_negative: count >= 0"), "{out}");
        assert!(
            out.contains("transition monotonic: new.count >= old.count"),
            "{out}"
        );
    }

    // v0.123 (slice 2, DECISION B): the `Recv.member` detection that feeds
    // capability-op call-site hover through `resolve_label`.
    #[test]
    fn qualified_callee_detects_upper_receiver_only() {
        let text = "  let t = Clock.now()";
        let now = text.find("now").unwrap();
        assert_eq!(
            qualified_callee_at(text, Span::new(now, now + 3)).as_deref(),
            Some("Clock.now")
        );
        // A lowercase (value) receiver is not our case — resolve_label can't
        // resolve it anyway.
        let text2 = "  xs.fold(0)";
        let fold = text2.find("fold").unwrap();
        assert!(qualified_callee_at(text2, Span::new(fold, fold + 4)).is_none());
        // A bare identifier (no receiver) → None.
        let text3 = "  total";
        assert!(qualified_callee_at(text3, Span::new(2, 7)).is_none());
    }

    // v0.137.0 (ADR 0161): hover for the `key`/`store` contextual keywords and
    // the agent state fields they introduce — on the keyword or on the field
    // name alike, with a `store` field's annotations rendered.
    #[test]
    fn describe_agent_state_covers_key_store_keywords_and_fields() {
        let src = "context demo.app\n\nagent Sessions {\n  key id: String\n  store items: Map[String, Int] @indexed( by: id ) @bounded( 10000 )\n  on call read() -> Effect[Int] {\n    Effect.pure(0)\n  }\n}\n";

        // The `key` keyword and the key field name both render `key id: String`
        // plus the contextual-keyword doc.
        let at_key_kw = src.find("key id").unwrap();
        let key_kw = describe_agent_state_at(src, at_key_kw).expect("hover on `key`");
        assert!(key_kw.contains("key id: String"), "{key_kw}");
        assert!(key_kw.contains("identity field"), "doc line: {key_kw}");
        let at_key_name = src.find("id: String").unwrap();
        let key_name = describe_agent_state_at(src, at_key_name).expect("hover on the key field");
        assert_eq!(key_kw, key_name, "keyword and name hover match");

        // The `store` keyword and the store field name both render the field
        // signature — kind and annotations — plus the doc.
        let at_store_kw = src.find("store items").unwrap();
        let store_kw = describe_agent_state_at(src, at_store_kw).expect("hover on `store`");
        assert!(
            store_kw.contains("store items: Map[String, Int]"),
            "{store_kw}"
        );
        assert!(
            store_kw.contains("@indexed(by: id)"),
            "annotation: {store_kw}"
        );
        assert!(
            store_kw.contains("@bounded(10000)"),
            "annotation: {store_kw}"
        );
        assert!(
            store_kw.contains("persisted agent-state"),
            "doc line: {store_kw}"
        );
        let at_store_name = src.find("items:").unwrap();
        let store_name =
            describe_agent_state_at(src, at_store_name).expect("hover on the store field");
        assert_eq!(store_kw, store_name, "keyword and name hover match");

        // Not on a `key`/`store` keyword or state-field name → no hover (the
        // agent name, and the store kind, both fall through to other paths).
        assert!(describe_agent_state_at(src, src.find("Sessions").unwrap()).is_none());
        assert!(describe_agent_state_at(src, src.find("Map").unwrap()).is_none());
        // The word `id` inside `by: id` is an annotation argument, not the key
        // field's declaration — it must not masquerade as the key field.
        assert!(describe_agent_state_at(src, src.find("by: id").unwrap() + 4).is_none());
    }

    /// #611 (gap A): the reference half of the test above. Hover on a `key`/
    /// `store` field *use* inside the agent body — the case that resolved
    /// nowhere: state fields are absent from the project index and are not
    /// `let`/param locals, so every earlier hover path misses them.
    const TODOS: &str = "context demo.todos\n\
        \n\
        agent Todos {\n\
        \x20 key owner: String\n\
        \n\
        \x20 store items:   Map[String, Int]\n\
        \x20 store lastSeq: Cell[Int]\n\
        \n\
        \x20 invariant nonneg: lastSeq >= 0\n\
        \n\
        \x20 on call add(n: Int) -> Effect[()] {\n\
        \x20   let next = lastSeq + 1\n\
        \x20   let _ <- items.put(owner, next)\n\
        \x20   lastSeq := next\n\
        \x20   Effect.pure(())\n\
        \x20 }\n\
        }\n";

    #[test]
    fn describe_agent_state_covers_references_in_handler_bodies() {
        let src = TODOS;
        let at = |needle: &str| src.find(needle).expect("needle is in the fixture");

        // Every reference renders exactly what the declaration renders.
        let store_decl = describe_agent_state_at(src, at("store lastSeq")).unwrap();
        assert!(
            store_decl.contains("store lastSeq: Cell[Int]"),
            "{store_decl}"
        );
        for (what, needle) in [
            ("a bare read", "lastSeq + 1"),
            ("a `:=` write target", "lastSeq := next"),
            ("an invariant subject", "lastSeq >= 0"),
        ] {
            let hover = describe_agent_state_at(src, at(needle))
                .unwrap_or_else(|| panic!("no hover on {what}"));
            assert_eq!(hover, store_decl, "{what} hovers as its declaration");
        }
        // A store op's receiver — the `items` half of `items.put(…)`.
        let recv = describe_agent_state_at(src, at("items.put")).expect("hover on the receiver");
        assert_eq!(
            recv,
            describe_agent_state_at(src, at("store items")).unwrap()
        );

        // The `key` field is referenceable the same way.
        let key_decl = describe_agent_state_at(src, at("key owner")).unwrap();
        let key_ref = describe_agent_state_at(src, at("owner, next")).expect("hover on `owner`");
        assert_eq!(key_ref, key_decl);
        assert!(key_ref.contains("key owner: String"), "{key_ref}");

        // A name that is not state, and a state-shaped name outside the agent's
        // reference scope, both fall through to the other hover paths.
        assert!(describe_agent_state_at(src, at("next = lastSeq")).is_none());
        assert!(describe_agent_state_at(src, at("Effect.pure")).is_none());
    }

    /// #611 (gap C): hover on a `store` field's operation renders the registry
    /// signature over the field's declared kind.
    #[test]
    fn describe_store_op_renders_the_operation_signature() {
        let src = TODOS;
        let at_put = src
            .find("put(owner")
            .expect("the store op is in the fixture");
        let put = describe_store_op_at(src, at_put, &[]).expect("hover on `items.put`");
        assert!(put.contains("put(key: K, value: V) -> Effect[()]"), "{put}");
        // The field's declared kind grounds `K`/`V`, so it rides along.
        assert!(put.contains("store items: Map[String, Int]"), "{put}");

        // A local shadowing the receiver makes this an ordinary value method,
        // not a store op — mirroring the checker's by-provenance dispatch.
        let shadow = [bynk_check::locals::LocalBinding {
            name: "items".into(),
            def_span: Span::new(0, 5),
            kind: bynk_check::locals::LocalKind::Let,
            ty: "Map[String, Int]".into(),
            scope: Span::new(0, src.len()),
        }];
        assert!(describe_store_op_at(src, at_put, &shadow).is_none());

        // An operation the kind does not have, a receiver that is not a store
        // field, and a non-member identifier all fall through.
        let cell_put = src.replace("items.put(owner, next)", "lastSeq.put(owner, next)");
        assert!(
            describe_store_op_at(&cell_put, cell_put.find("put(owner").unwrap(), &[]).is_none(),
            "a `Cell` has no `put`"
        );
        assert!(describe_store_op_at(src, src.find("pure(()").unwrap(), &[]).is_none());
        assert!(describe_store_op_at(src, src.find("next = lastSeq").unwrap(), &[]).is_none());
    }

    /// A store op binds a **bare** ident receiver. A *qualified* receiver
    /// (`p.items.put(…)`) is an ordinary value method on a record field that
    /// merely shares a store field's name — the same class of confidently-wrong
    /// hover as gap B, and invisible to the index (which does not cover value
    /// methods), so nothing upstream would catch it.
    #[test]
    fn a_qualified_receiver_is_not_a_store_op_or_a_state_reference() {
        let src = "context demo.todos\n\
            \n\
            type Inner = { put: Int }\n\
            type Payload = { items: Inner, lastSeq: Int }\n\
            \n\
            agent Todos {\n\
            \x20 key owner: String\n\
            \x20 store items:   Map[String, Int]\n\
            \x20 store lastSeq: Cell[Int]\n\
            \n\
            \x20 on call add(p: Payload) -> Effect[()] {\n\
            \x20   let a = p.items.put\n\
            \x20   let b = p.lastSeq\n\
            \x20   Effect.pure(())\n\
            \x20 }\n\
            }\n";
        // `p.items.put` — the `put` is a field of `Inner`, not the store op.
        let at_put = src.find("put\n").expect("the qualified member");
        assert!(describe_store_op_at(src, at_put, &[]).is_none());
        // …and the `items` in `p.items` is a field of `Payload`, not the store
        // field — the same root cause, in the state-reference pass.
        let at_items = src.find("p.items").unwrap() + "p.".len();
        assert!(describe_agent_state_at(src, at_items).is_none());
        let at_seq = src.find("p.lastSeq").unwrap() + "p.".len();
        assert!(describe_agent_state_at(src, at_seq).is_none());

        // The bare forms in the same body still resolve.
        let bare = src.find("store items").unwrap();
        assert!(describe_agent_state_at(src, bare).is_some());
    }

    /// #611 (gap B): the index resolves a record-construction field label / field
    /// access to a `Field` key (`"Stored.title"`); hover must render it rather
    /// than fall through to the locals path, which name-matches in scope and
    /// bound `title:` to a same-named handler param.
    #[test]
    fn describe_symbol_renders_a_resolved_record_field() {
        let src = "context demo.todos\n\n\
            type Title = String where NonEmpty\n\n\
            type Stored = {\n\
            \x20 seq:   Int where NonNegative,\n\
            \x20 title: Title,\n\
            }\n";
        let title = describe_symbol(src, "Stored.title").expect("hover on `Stored.title`");
        assert!(title.contains("title: Title"), "{title}");
        assert!(title.contains("A field of `Stored`"), "{title}");
        // A field refinement rides along, as it does in the record body.
        let seq = describe_symbol(src, "Stored.seq").expect("hover on `Stored.seq`");
        assert!(seq.contains("seq: Int where NonNegative"), "{seq}");

        // The bare type name still renders the type, not a field.
        assert!(
            describe_symbol(src, "Stored")
                .unwrap()
                .contains("type Stored")
        );
        // An unknown field, an unknown owner, and a non-record owner yield none.
        assert!(describe_symbol(src, "Stored.nope").is_none());
        assert!(describe_symbol(src, "Nope.title").is_none());
        assert!(describe_symbol(src, "Title.title").is_none());
    }

    // #972 (Events slice 3a): an event's own hover and its per-field hover
    // both render a declared default, mirroring an ordinary record's hover
    // (above) — this is the arm that was entirely missing before this slice
    // (no `CommonsItem::Event` case at all), not just a formatting gap.
    #[test]
    fn describe_symbol_renders_an_event_and_its_defaulted_field() {
        let src = "context demo.orders\n\n\
            type Region = enum { Domestic, International }\n\n\
            event PaymentConfirmed = {\n\
            \x20 orderId: String,\n\
            \x20 region: Region = Region.Domestic,\n\
            }\n";
        let whole = describe_symbol(src, "PaymentConfirmed").expect("hover on the event itself");
        assert!(whole.contains("type PaymentConfirmed = {"), "{whole}");
        assert!(whole.contains("orderId: String"), "{whole}");
        assert!(
            whole.contains("region: Region = Region.Domestic"),
            "{whole}"
        );

        let field = describe_symbol(src, "PaymentConfirmed.region")
            .expect("hover on `PaymentConfirmed.region`");
        assert!(
            field.contains("region: Region = Region.Domestic"),
            "{field}"
        );
        assert!(field.contains("A field of `PaymentConfirmed`"), "{field}");

        // The non-defaulted field renders with no trailing `= ...`.
        let order_id = describe_symbol(src, "PaymentConfirmed.orderId")
            .expect("hover on `PaymentConfirmed.orderId`");
        assert!(order_id.contains("orderId: String"), "{order_id}");
        assert!(!order_id.contains('='), "{order_id}");

        assert!(describe_symbol(src, "PaymentConfirmed.nope").is_none());
        assert!(describe_symbol(src, "Nope.region").is_none());
    }

    // Events slice 3b (#978): a declared `@schema(N)` renders in the
    // event's own hover, additively — an event with none (the test above)
    // stays byte-identical.
    #[test]
    fn describe_symbol_renders_an_events_schema_annotation() {
        let src = "context demo.orders\n\n\
            event PaymentConfirmed @schema(2) = {\n\
            \x20 orderId: String,\n\
            }\n";
        let whole = describe_symbol(src, "PaymentConfirmed").expect("hover on the event itself");
        assert!(
            whole.contains("type PaymentConfirmed @schema(2) = {"),
            "{whole}"
        );
        assert!(whole.contains("orderId: String"), "{whole}");
    }

    /// v0.166 (#616): the actor arm — both declaration forms. The reference-offset
    /// fixture in `hover_references.rs` covers the `Bearer` form against real
    /// analysis output; the schemes without config and ADR 0091's refinement form
    /// are declared by no example project, so they are pinned here.
    #[test]
    fn describe_symbol_renders_an_actor_in_both_forms() {
        let src = "context demo.auth\n\n\
            type UserId = String where NonEmpty\n\n\
            ---\n\
            A signed-in user.\n\
            ---\n\
            actor User { auth = Bearer(secret = \"AUTH_JWT_SECRET\"), identity = UserId }\n\n\
            actor Public { auth = None }\n\n\
            actor Worker { auth = Internal }\n\n\
            actor Admin = User where hasClaim(\"admin\")\n";

        let user = describe_symbol(src, "User").expect("hover on `User`");
        assert!(
            user.contains(
                "actor User { auth = Bearer(secret = \"AUTH_JWT_SECRET\"), identity = UserId }"
            ),
            "{user}"
        );
        // The doc block rides along, as it does for every other declaration.
        assert!(user.contains("A signed-in user."), "{user}");

        // A scheme with no config and no identity renders neither.
        let public = describe_symbol(src, "Public").expect("hover on `Public`");
        assert!(
            public.contains("actor Public { auth = None }") && !public.contains("identity"),
            "{public}"
        );
        assert!(
            describe_symbol(src, "Worker")
                .unwrap()
                .contains("actor Worker { auth = Internal }")
        );

        // ADR 0091's refinement form renders its base and claim predicate.
        let admin = describe_symbol(src, "Admin").expect("hover on `Admin`");
        assert!(
            admin.contains("actor Admin = User where hasClaim(\"admin\")"),
            "{admin}"
        );

        assert!(describe_symbol(src, "Nobody").is_none());
    }

    /// v0.166 (#616, review): the actor arm claims to mirror `bynk-fmt`'s
    /// `format_actor`, so it must escape a scheme-config string the same way.
    /// `SchemeArgValue::Str` holds the value *unescaped* — the parser resolves
    /// `\"`/`\\`/`\n`/`\t` at lex time — so rendering it raw put invalid Bynk
    /// inside a ```bynk fence. Pinned against the formatter's own output rather
    /// than a hand-written expectation: a copy would agree only until one moved.
    #[test]
    fn describe_symbol_escapes_an_actors_scheme_config() {
        let src = "context demo.auth\n\n\
            actor User { auth = Bearer(secret = \"a\\\"b\\\\c\") }\n";
        let hover = describe_symbol(src, "User").expect("hover on `User`");
        assert!(
            hover.contains("actor User { auth = Bearer(secret = \"a\\\"b\\\\c\") }"),
            "the config value must round-trip escaped:\n{hover}"
        );

        // The fenced declaration is exactly what the formatter emits for it.
        let formatted = bynk_fmt::format_source(src, &bynk_fmt::FormatOptions::default())
            .expect("the fixture formats");
        let actor_line = formatted
            .lines()
            .find(|l| l.starts_with("actor User"))
            .expect("the actor line");
        assert!(
            hover.contains(actor_line),
            "hover:\n{hover}\nfmt: {actor_line}"
        );
    }

    /// v0.166 (#616): the capability-op arm, keyed `"Cap.op"` (ADR 0069) —
    /// attributed to its owner, as a field is to its record.
    #[test]
    fn describe_symbol_renders_a_capability_operation() {
        let src = "context demo.svc\n\n\
            capability Logger {\n\
            \x20 ---\n\
            \x20 Record a line.\n\
            \x20 ---\n\
            \x20 fn info(message: String) -> Effect[()]\n\
            }\n\n\
            capability Clock {\n\
            \x20 fn now() -> Effect[Int]\n\
            }\n";

        let info = describe_symbol(src, "Logger.info").expect("hover on `Logger.info`");
        assert!(
            info.contains("fn info(message: String) -> Effect[()]"),
            "{info}"
        );
        assert!(
            info.contains("An operation of capability `Logger`"),
            "{info}"
        );
        assert!(info.contains("Record a line."), "{info}");

        // A no-arg op on another capability — the owner is what disambiguates.
        let now = describe_symbol(src, "Clock.now").expect("hover on `Clock.now`");
        assert!(now.contains("fn now() -> Effect[Int]"), "{now}");

        // The bare capability name still renders the capability itself.
        assert!(
            describe_symbol(src, "Logger")
                .unwrap()
                .contains("capability Logger")
        );
        // An unknown op, and an op read against the wrong owner, yield none.
        assert!(describe_symbol(src, "Logger.nope").is_none());
        assert!(describe_symbol(src, "Clock.info").is_none());
    }

    #[test]
    fn describe_symbol_renders_a_messages_bundle() {
        // message-bundles slice 1 (#859): describe_item's Messages arm — a
        // silent hover gap before this slice (the wildcard fell through to
        // "no hover" for a construct that didn't exist yet).
        let src = "commons app.bundle\n\n\
            ---\n\
            The English reference bundle.\n\
            ---\n\
            messages \"en\" @reference {\n\
            \x20 \"greeting\" => \"Hello, {name}!\"\n\
            \x20 \"farewell\" => \"Bye\"\n\
            }\n";

        let info = describe_symbol(src, "en").expect("hover on the `en` messages tag");
        assert!(info.contains("messages \"en\" @reference {"), "{info}");
        assert!(info.contains("\"greeting\" => …"), "{info}");
        assert!(info.contains("\"farewell\" => …"), "{info}");
        assert!(info.contains("The English reference bundle."), "{info}");
    }

    /// v0.166 (#616, ADR 0191 D2): a bare key names a *free* function. Matching a
    /// method on its bare name answered with whichever type declared it first —
    /// `Gauge.bump`'s own declaration rendered `Counter.bump` — and silently
    /// outranked the index's real answer.
    #[test]
    fn describe_symbol_keys_methods_by_their_compound_name_only() {
        let src = "context demo.shop\n\n\
            type Counter = { count: Int }\n\
            type Gauge = { level: Int }\n\n\
            fn Counter.bump(self) -> Counter { Counter { count: self.count + 1 } }\n\n\
            fn Gauge.bump(self) -> Gauge { Gauge { level: self.level + 1 } }\n\n\
            fn free(n: Int) -> Int { n }\n";

        // The type prefix is the identity: each compound key renders its own.
        let counter = describe_symbol(src, "Counter.bump").expect("hover on `Counter.bump`");
        assert!(
            counter.contains("fn Counter.bump(self) -> Counter") && !counter.contains("Gauge"),
            "{counter}"
        );
        let gauge = describe_symbol(src, "Gauge.bump").expect("hover on `Gauge.bump`");
        assert!(
            gauge.contains("fn Gauge.bump(self) -> Gauge") && !gauge.contains("Counter"),
            "{gauge}"
        );

        // A bare `bump` names no method: it is a guess between the two, and the
        // index's compound key is what resolves them.
        assert!(describe_symbol(src, "bump").is_none());
        // A free function is still keyed by its bare name.
        assert!(
            describe_symbol(src, "free")
                .unwrap()
                .contains("fn free(n: Int) -> Int")
        );
        assert!(describe_symbol(src, "Counter.nope").is_none());
    }

    // v0.122 (slice 1): `self` hover renders its receiver/agent type, reading
    // the type from `expr_types` and un-synthesising the agent-self record.
    #[test]
    fn describe_self_renders_receiver_and_unwraps_agent() {
        use bynk_check::checker::{NamedKind, Ty, Types};
        let text = "self";
        let tys = &Types::new();
        let span = Span::new(0, 4);
        // A method receiver — a plain named type renders verbatim.
        let account = vec![(
            span,
            tys.intern(Ty::Named {
                name: "Account".into(),
                kind: NamedKind::Record,
                args: Vec::new(),
            }),
        )];
        assert_eq!(
            describe_self_at(text, 0, &account, tys).as_deref(),
            Some("```bynk\nself: Account\n```")
        );
        // An agent handler — the synthetic `__CounterSelf` record un-synthesises
        // to the agent name.
        let agent = vec![(
            span,
            tys.intern(Ty::Named {
                name: "__CounterSelf".into(),
                kind: NamedKind::Record,
                args: Vec::new(),
            }),
        )];
        assert_eq!(
            describe_self_at(text, 0, &agent, tys).as_deref(),
            Some("```bynk\nself: Counter\n```")
        );
        // Not on the `self` keyword — a different token yields nothing, even
        // when a type sits at the offset.
        let other = "total";
        assert!(
            describe_self_at(
                other,
                0,
                &[(
                    Span::new(0, 5),
                    tys.intern(Ty::Named {
                        name: "Int".into(),
                        kind: NamedKind::Record,
                        args: Vec::new(),
                    }),
                )],
                tys
            )
            .is_none()
        );
    }

    const CACHE_SVC: &str = "context api\nservice api from http {\n  @cache(maxAge: 5.minutes, scope: public)\n  on GET(\"/x\") () -> Effect[HttpResult[String]] by v: Visitor {\n    Ok(\"y\")\n  }\n}\n";

    #[test]
    fn an_edge_cache_hovers_with_its_stanza() {
        // L7 (#2414): what declaring the instance switches on.
        let src =
            "context a\nconsumes bynk.cloudflare { EdgeCache }\ncapability Edge = EdgeCache\n";
        let tokens = tokenize(src).unwrap();
        let (unit, _) = parse_unit_with_recovery(&tokens, src);
        let Some(SourceUnit::Context(ctx)) = unit else {
            panic!("a context")
        };
        let decl = ctx
            .items
            .iter()
            .find_map(|i| match i {
                CommonsItem::Capability(c) => Some(c),
                _ => None,
            })
            .unwrap();
        let hover = describe_derived_capability(decl, Some("bynk.cloudflare"), None);
        assert!(hover.contains("Emits `[cache] enabled = true`"), "{hover}");
        assert!(!hover.contains("its own storage"), "{hover}");
    }

    #[test]
    fn a_queue_producer_hovers_with_its_stanza() {
        // P2 (#2578): the queue it sends to, as the stanza names it.
        let src = "context a\nconsumes bynk.cloudflare { Queue }\ncapability OrderEvents = Queue @name(\"order-events\")\n";
        let tokens = tokenize(src).unwrap();
        let (unit, _) = parse_unit_with_recovery(&tokens, src);
        let Some(SourceUnit::Context(ctx)) = unit else {
            panic!("a context")
        };
        let decl = ctx
            .items
            .iter()
            .find_map(|i| match i {
                CommonsItem::Capability(c) => Some(c),
                _ => None,
            })
            .unwrap();
        let hover = describe_derived_capability(decl, Some("bynk.cloudflare"), None);
        assert!(
            hover.contains("Emits `[[queues.producers]]` sending to queue `order-events`"),
            "{hover}"
        );
    }

    #[test]
    fn an_analytics_dataset_hovers_with_its_stanza() {
        // P3b (#2582): how the dataset is named, since the source never says.
        let src =
            "context a\nconsumes bynk.cloudflare { Analytics }\ncapability Signups = Analytics\n";
        let tokens = tokenize(src).unwrap();
        let (unit, _) = parse_unit_with_recovery(&tokens, src);
        let Some(SourceUnit::Context(ctx)) = unit else {
            panic!("a context")
        };
        let decl = ctx
            .items
            .iter()
            .find_map(|i| match i {
                CommonsItem::Capability(c) => Some(c),
                _ => None,
            })
            .unwrap();
        let hover = describe_derived_capability(decl, Some("bynk.cloudflare"), None);
        assert!(
            hover.contains("Emits `[[analytics_engine_datasets]]`")
                && hover.contains("shop_cart_signups"),
            "{hover}"
        );
    }

    #[test]
    fn a_rate_limiter_hovers_with_its_stanza() {
        // P3 (#2342): the emitted limit and window, and the counting caveat.
        let src = "context a\nconsumes bynk.cloudflare { RateLimit }\ncapability LoginLimit = RateLimit @limit(20) @period(1.minutes)\n";
        let tokens = tokenize(src).unwrap();
        let (unit, _) = parse_unit_with_recovery(&tokens, src);
        let Some(SourceUnit::Context(ctx)) = unit else {
            panic!("a context")
        };
        let decl = ctx
            .items
            .iter()
            .find_map(|i| match i {
                CommonsItem::Capability(c) => Some(c),
                _ => None,
            })
            .unwrap();
        let hover = describe_derived_capability(decl, Some("bynk.cloudflare"), None);
        assert!(
            hover.contains(
                "`simple = { limit = 20, period = 60 }`: 20 calls per key every 60 seconds"
            ),
            "{hover}"
        );
        assert!(hover.contains("approximate"), "{hover}");
    }

    #[test]
    fn hover_on_cache_annotation_describes_it() {
        // Offset on the `cache` name token.
        let offset = CACHE_SVC.find("cache").unwrap() + 1;
        let hover = describe_handler_annotation_at(CACHE_SVC, offset).expect("hovers @cache");
        assert!(hover.contains("`@cache`"), "names the annotation: {hover}");
        assert!(hover.contains("maxAge"), "documents maxAge: {hover}");
        assert!(hover.contains("scope"), "documents scope: {hover}");
        assert!(hover.contains("immutable"), "documents immutable: {hover}");
        assert!(
            hover.contains("Emits `Cache-Control: public, max-age=300`."),
            "shows the emitted header: {hover}"
        );
        // Off the annotation (on the `Ok` body) — no annotation hover.
        let ok_offset = CACHE_SVC.find("Ok(").unwrap() + 1;
        assert!(describe_handler_annotation_at(CACHE_SVC, ok_offset).is_none());
    }

    #[test]
    fn hover_on_immutable_cache_shows_the_directive() {
        let svc = "context api\nservice api from http {\n  @cache(maxAge: 365.days, scope: public, immutable: true)\n  on GET(\"/x\") () -> Effect[HttpResult[String]] by v: Visitor {\n    Ok(\"y\")\n  }\n}\n";
        let hover = describe_handler_annotation_at(svc, svc.find("cache").unwrap() + 1)
            .expect("hovers @cache");
        assert!(
            hover.contains("Emits `Cache-Control: public, max-age=31536000, immutable`."),
            "{hover}"
        );
    }

    #[test]
    fn hover_on_raw_annotation_shows_content_type_and_effective_cap() {
        let svc = "context api\nservice api from http {\n  limits { maxBody: 1000 }\n  @raw(contentType: \"image/png\")\n  on POST(\"/a\") (body: Bytes) -> Effect[HttpResult[String]] by v: Visitor {\n    Ok(\"y\")\n  }\n  @raw(contentType: \"*/*\")\n  @limit(maxBody: 50)\n  on POST(\"/b\") (body: Bytes) -> Effect[HttpResult[String]] by v: Visitor {\n    Ok(\"y\")\n  }\n}\n";
        let first =
            describe_handler_annotation_at(svc, svc.find("raw").unwrap() + 1).expect("hovers @raw");
        assert!(first.contains("`image/png`"), "names the type: {first}");
        assert!(first.contains("1000 bytes"), "service cap: {first}");
        let second = describe_handler_annotation_at(svc, svc.rfind("raw").unwrap() + 1)
            .expect("hovers @raw");
        assert!(second.contains("any"), "wildcard: {second}");
        assert!(second.contains("50 bytes"), "the route's own cap: {second}");
    }

    #[test]
    fn annotation_token_spans_cover_name_and_labels() {
        let spans = handler_annotation_token_spans(CACHE_SVC);
        // `@cache` + the two argument labels `maxAge`/`scope`.
        assert_eq!(spans.len(), 3, "{spans:?}");
        let texts: Vec<&str> = spans.iter().map(|s| &CACHE_SVC[s.start..s.end]).collect();
        assert_eq!(texts, ["@cache", "maxAge", "scope"]);
        // A service with no annotations yields nothing.
        let plain = "context api\nservice api from http {\n  on GET(\"/x\") () -> Effect[HttpResult[String]] by v: Visitor { Ok(\"y\") }\n}\n";
        assert!(handler_annotation_token_spans(plain).is_empty());
    }

    #[test]
    fn scan_doc_link_candidates_accepts_a_multi_segment_unit_path() {
        let cands = scan_doc_link_candidates("See [a.b.Name] and [e.g.] and [x. y].");
        let names: Vec<_> = cands.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["a.b.Name"]);
    }

    #[test]
    fn doc_link_spans_attached_reports_the_next_identifier() {
        let src = "--- the [x] param\nfn f(x: Int) -> Int { x }\n";
        let links = doc_link_spans_attached(src);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].attached, Some(src.find("f(").unwrap()));
    }

    #[test]
    fn doc_link_spans_attached_puts_a_method_at_its_name() {
        let src = "--- the [x] param\nfn Box.plus(self, x: Int) -> Int { x }\n";
        let links = doc_link_spans_attached(src);
        assert_eq!(links[0].attached, Some(src.find("plus").unwrap()));
    }

    #[test]
    fn doc_link_unit_names_a_suites_target() {
        let src = "suite window\n";
        assert_eq!(own_declaration_name(src), None);
        assert_eq!(doc_link_unit(src).as_deref(), Some("window"));
    }
}

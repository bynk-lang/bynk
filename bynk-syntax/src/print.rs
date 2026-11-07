//! The expression/pattern/statement printer: renders a parsed AST fragment back to Bynk
//! source text.
//!
//! Moved here from `bynk-fmt` (crate-hygiene A3, `design/tracks/crate-hygiene.md` §5) so the
//! checker can quote an expression in a diagnostic without depending on the formatter. The
//! formatter and the checker both call it from here; the text is unchanged byte for byte.

use crate::ast::*;
use crate::span::Span;

thread_local! {
    /// The source being formatted, so a string literal can be re-emitted as it
    /// was written (#2619). Set by `render_units` (in `bynk-fmt`) for the duration of a render.
    static WRITTEN_SOURCE: std::cell::RefCell<Option<std::rc::Rc<str>>> =
        const { std::cell::RefCell::new(None) };
}

/// Scope guard installing `WRITTEN_SOURCE`.
pub struct WrittenForms(Option<std::rc::Rc<str>>);

impl WrittenForms {
    pub fn enter(source: &str) -> Self {
        let new = Some(std::rc::Rc::<str>::from(source));
        Self(WRITTEN_SOURCE.with(|cell| cell.replace(new)))
    }
}

impl Drop for WrittenForms {
    fn drop(&mut self) {
        let previous = self.0.take();
        WRITTEN_SOURCE.with(|cell| *cell.borrow_mut() = previous);
    }
}

/// #2619: a string literal's written form, when the source holds one that is
/// worth preserving: a raw or multi-line literal, or one using a `\u{…}`
/// escape. It is the source text at `e.span`, kept only if it decodes to
/// `value`, so a synthesised or mis-spanned literal falls back to the canonical
/// rendering. Formatting must not turn `r"\d+"` into `"\\d+"`.
fn written_string_form(e: &Expr, value: Option<&str>) -> Option<String> {
    written_literal(e.span, value)
}

/// [`written_string_form`] for a literal at `span` anywhere (an expression, a
/// `Matches` argument, a message template).
fn written_literal(span: Span, value: Option<&str>) -> Option<String> {
    WRITTEN_SOURCE.with(|cell| {
        let source = cell.borrow();
        let text = source.as_deref()?.get(span.range())?;
        let special = text.starts_with("r\"")
            || text.starts_with("r#")
            || text.starts_with("\"\"\"")
            || text.contains("\\u{");
        if !special {
            return None;
        }
        // A multi-line literal is re-emitted at column zero, with the closing
        // delimiter's indentation removed from every line: the value is
        // unchanged (that indentation is stripped anyway), the formatter's
        // re-indent pass then sets the current depth for all of it, and the
        // result no longer depends on how the author happened to indent it.
        let text = if text.starts_with("\"\"\"") || text.starts_with("r\"\"\"") {
            dedent_multiline(text)
        } else {
            text.to_string()
        };
        match value {
            Some(value) => {
                let decoded = crate::lexer::decode_string_literal(&text, span).ok()?;
                (decoded == value).then_some(text)
            }
            // An interpolated multi-line string is kept whole only when it is
            // multi-line (its holes would otherwise be reformatted).
            None => (text.starts_with("\"\"\"") || text.contains("\\u{")).then_some(text),
        }
    })
}

/// Strip the closing delimiter's indentation from every line of a multi-line
/// literal's text, so it sits at column zero (see [`written_string_form`]).
fn dedent_multiline(text: &str) -> String {
    let last = text.rsplit('\n').next().unwrap_or("");
    let indent = &last[..last.len() - last.trim_start_matches([' ', '\t']).len()];
    let mut out = String::with_capacity(text.len());
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        if i == 0 {
            // The opening line keeps a language tag (`"""sql`, #2958) as
            // written, minus any trailing spaces after it.
            let (line, cr) = line.strip_suffix('\r').map_or((line, ""), |l| (l, "\r"));
            out.push_str(line.trim_end_matches([' ', '\t']));
            out.push_str(cr);
        } else if let Some(rest) = line.strip_prefix(indent) {
            out.push_str(rest);
        } else if line.trim().is_empty() {
            // a blank line shorter than the indentation is empty
        } else {
            out.push_str(line);
        }
    }
    out
}

/// v0.182 (#664): render a call-site actor clause — `by User("bob")` or the
/// unit-identity `by Visitor`.
pub fn call_site_actor_src(p: &CallSiteActor) -> String {
    match (&p.identity, &p.claims) {
        // #2380 (L5): an `Oidc` principal may carry the token's claims too.
        (Some(id), Some(claims)) => format!(
            "by {}({}, {})",
            p.actor.name,
            expr_with_prec(id, 0),
            expr_with_prec(claims, 0)
        ),
        (Some(id), None) => format!("by {}({})", p.actor.name, expr_with_prec(id, 0)),
        (None, _) => format!("by {}", p.actor.name),
    }
}

pub fn type_ref_to_string(t: &TypeRef) -> String {
    match t {
        TypeRef::Base(b, _) => b.name().to_string(),
        TypeRef::Named(id) => id.name.clone(),
        TypeRef::Result(a, b, _) => format!(
            "Result[{}, {}]",
            type_ref_to_string(a),
            type_ref_to_string(b)
        ),
        TypeRef::Option(t, _) => format!("Option[{}]", type_ref_to_string(t)),
        TypeRef::Effect(t, _) => format!("Effect[{}]", type_ref_to_string(t)),
        TypeRef::HttpResult(t, _) => format!("HttpResult[{}]", type_ref_to_string(t)),
        TypeRef::QueueResult(_) => "QueueResult".to_string(),
        TypeRef::Asset(_) => "Asset".to_string(),
        TypeRef::RequestHeaders(_) => "RequestHeaders".to_string(),
        TypeRef::RequestQuery(_) => "RequestQuery".to_string(),
        TypeRef::TokenClaims(_) => "TokenClaims".to_string(),
        TypeRef::BoundaryRejection(_) => "BoundaryRejection".to_string(),
        TypeRef::List(t, _) => format!("List[{}]", type_ref_to_string(t)),
        TypeRef::Set(t, _) => format!("Set[{}]", type_ref_to_string(t)),
        TypeRef::Query(t, _) => format!("Query[{}]", type_ref_to_string(t)),
        TypeRef::Stream(t, _) => format!("Stream[{}]", type_ref_to_string(t)),
        TypeRef::Connection(t, _) => format!("Connection[{}]", type_ref_to_string(t)),
        TypeRef::History(t, _) => format!("History[{}]", type_ref_to_string(t)),
        TypeRef::Map(k, v, _) => {
            format!("Map[{}, {}]", type_ref_to_string(k), type_ref_to_string(v))
        }
        TypeRef::ValidationError(_) => "ValidationError".to_string(),
        TypeRef::JsonError(_) => "JsonError".to_string(),
        TypeRef::Unit(_) => "()".to_string(),
        // v0.157 (ADR 0183): a user generic-type application, as written.
        TypeRef::App { name, args, .. } => format!(
            "{}[{}]",
            name.name,
            args.iter()
                .map(type_ref_to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        TypeRef::Fn(params, ret, _) => {
            let lhs = match params.len() {
                0 => "()".to_string(),
                1 if !matches!(params[0], TypeRef::Fn(..)) => type_ref_to_string(&params[0]),
                _ => format!(
                    "({})",
                    params
                        .iter()
                        .map(type_ref_to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            };
            format!("{lhs} -> {}", type_ref_to_string(ret))
        }
    }
}

pub fn refinement_to_string(r: &Refinement) -> String {
    let mut s = String::new();
    for (i, p) in r.predicates.iter().enumerate() {
        if i > 0 {
            s.push_str(" && ");
        }
        s.push_str(&pred_to_string(p));
    }
    s
}

/// A literal as written if it has a special form, else canonically escaped.
pub fn literal_text(span: Span, value: &str) -> String {
    written_literal(span, Some(value)).unwrap_or_else(|| format!("\"{}\"", escape_string(value)))
}

/// The span of a `Matches(…)` predicate's string argument.
pub fn matches_argument_span(p: &RefinementPred) -> Span {
    WRITTEN_SOURCE.with(|cell| {
        let source = cell.borrow();
        let Some(text) = source.as_deref().and_then(|s| s.get(p.span.range())) else {
            return p.span;
        };
        let (Some(open), Some(close)) = (text.find('('), text.rfind(')')) else {
            return p.span;
        };
        let inner = &text[open + 1..close];
        let lead = inner.len() - inner.trim_start().len();
        let start = p.span.start + open + 1 + lead;
        Span::new_in(p.span.file, start, start + inner.trim().len())
    })
}

fn pred_to_string(p: &RefinementPred) -> String {
    let arg = |n: i64| p.arg_text.clone().unwrap_or_else(|| n.to_string());
    match &p.kind {
        PredKind::Matches(re) => format!("Matches({})", literal_text(matches_argument_span(p), re)),
        PredKind::InRange(a, b) => format!("InRange({}, {})", a.lexeme, b.lexeme),
        PredKind::InRangeF(a, b) => format!("InRange({}, {})", a.lexeme, b.lexeme),
        PredKind::InRangeD(a, b) => format!("InRange({}, {})", a.lexeme, b.lexeme),
        PredKind::MinLength(n) => format!("MinLength({})", arg(*n)),
        PredKind::MaxLength(n) => format!("MaxLength({})", arg(*n)),
        PredKind::Length(n) => format!("Length({})", arg(*n)),
        PredKind::NonNegative => "NonNegative".to_string(),
        PredKind::Positive => "Positive".to_string(),
        PredKind::NonEmpty => "NonEmpty".to_string(),
        PredKind::IsoDate => "IsoDate".to_string(),
    }
}

pub fn escape_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            // #2619: a CR or NUL cannot sit raw in a literal, nor can other control
            // characters; they print as `\u{…}`. Anything else (a ZWJ, an
            // RLM, a no-break space) is written as it was.
            '\r' => out.push_str("\\r"),
            '\0' => out.push_str("\\0"),
            c if c.is_control() => {
                out.push_str(&format!("\\u{{{:X}}}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

pub fn expr_to_string(e: &Expr) -> String {
    expr_with_prec(e, 0)
}

// Operator precedences (smaller = binds looser):
//   1: || 2: && 3: == != 4: < <= > >= 5: + - 6: * / 7: unary ! - 8: postfix . () ?
pub fn binop_prec(op: BinOp) -> u8 {
    match op {
        // v0.80: `implies` is the lowest-precedence binary operator (below `||`).
        BinOp::Implies => 0,
        BinOp::Or => 1,
        BinOp::And => 2,
        BinOp::Eq | BinOp::NotEq => 3,
        BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq => 4,
        BinOp::Add | BinOp::Sub => 5,
        BinOp::Mul | BinOp::Div => 6,
    }
}

pub fn expr_with_prec(e: &Expr, parent_prec: u8) -> String {
    match &e.kind {
        // v0.142 (ADR 0166): the stored lexeme verbatim — formatting must not
        // normalise away the author's `_` digit separators.
        ExprKind::IntLit { lexeme, .. } => lexeme.clone(),
        // v0.21: the stored lexeme verbatim — formatting must not normalise.
        ExprKind::FloatLit { lexeme, .. } => lexeme.clone(),
        ExprKind::DecimalLit { lexeme, .. } => lexeme.clone(),
        // v0.86 (ADR 0112): a duration literal `<value>.<unit>`.
        ExprKind::DurationLit { value, unit, .. } => format!("{value}.{}", unit.name()),
        ExprKind::StrLit(s) => {
            written_string_form(e, Some(s)).unwrap_or_else(|| format!("\"{}\"", escape_string(s)))
        }
        // v0.43: re-emit the interpolated string — chunks re-escaped, each
        // hole as `\(expr)`. Re-escaping a chunk's literal `\` to `\\` keeps a
        // source `\\(` (an escaped `\(`) round-tripping as text, not a hole.
        ExprKind::InterpStr(parts) => {
            if let Some(written) = written_string_form(e, None) {
                return written;
            }
            let mut out = String::from("\"");
            for part in parts {
                match part {
                    InterpPart::Chunk(text) => out.push_str(&escape_string(text)),
                    InterpPart::Hole(hole) => {
                        out.push_str(&format!("\\({})", expr_with_prec(hole, 0)));
                    }
                }
            }
            out.push('"');
            out
        }
        ExprKind::BoolLit(b) => b.to_string(),
        ExprKind::UnitLit => "()".to_string(),
        ExprKind::Ident(id) => id.name.clone(),
        ExprKind::ListLit(elems) => format!(
            "[{}]",
            elems
                .iter()
                .map(expr_to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        ExprKind::Call {
            name,
            type_args,
            args,
        } => {
            let targs = if type_args.is_empty() {
                String::new()
            } else {
                format!(
                    "[{}]",
                    type_args
                        .iter()
                        .map(type_ref_to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            let parts: Vec<String> = args.iter().map(|a| expr_with_prec(a, 0)).collect();
            format!("{}{}({})", name.name, targs, parts.join(", "))
        }
        ExprKind::BinOp(op, l, r) => {
            let prec = binop_prec(*op);
            let inner = format!(
                "{} {} {}",
                expr_with_prec(l, prec),
                op.name(),
                expr_with_prec(r, prec + 1)
            );
            if prec < parent_prec {
                format!("({inner})")
            } else {
                inner
            }
        }
        ExprKind::UnaryOp(op, inner) => {
            // Unary binds tightly (prec 7).
            let s = format!("{}{}", op.name(), expr_with_prec(inner, 7));
            if parent_prec > 7 { format!("({s})") } else { s }
        }
        ExprKind::Paren(inner) => format!("({})", expr_with_prec(inner, 0)),
        // v0.20a: a lambda prints as `(params) => body`.
        ExprKind::Lambda(lambda) => {
            let params: Vec<String> = lambda.params.iter().map(lambda_param_src).collect();
            let body = match lambda_body_src(lambda) {
                LambdaBodySrc::Block(b) => format_block_oneline(b),
                LambdaBodySrc::Expr(e) => match &e.kind {
                    ExprKind::Block(b) => format_block_oneline(b),
                    _ => expr_with_prec(e, 0),
                },
            };
            format!("({}) => {}", params.join(", "), body)
        }
        ExprKind::Block(b) => format_block_oneline(b),
        ExprKind::If {
            cond,
            then_block,
            else_block,
        } => {
            // v0.146 (ADR 0170): an `if` with no `else` carries a synthesised
            // unit else-branch — omit it so the else-less form round-trips.
            if else_block.is_synth_unit() {
                format!(
                    "if {} {}",
                    expr_with_prec(cond, 0),
                    format_block_oneline(then_block),
                )
            } else if let Some(chained) = else_if_tail(else_block) {
                format!(
                    "if {} {} else {}",
                    expr_with_prec(cond, 0),
                    format_block_oneline(then_block),
                    expr_with_prec(chained, 0),
                )
            } else {
                format!(
                    "if {} {} else {}",
                    expr_with_prec(cond, 0),
                    format_block_oneline(then_block),
                    format_block_oneline(else_block),
                )
            }
        }
        ExprKind::Ok(v) => format!("Ok({})", expr_with_prec(v, 0)),
        ExprKind::Err(v) => format!("Err({})", expr_with_prec(v, 0)),
        ExprKind::Some(v) => format!("Some({})", expr_with_prec(v, 0)),
        ExprKind::None => "None".to_string(),
        ExprKind::Question(v) => format!("{}?", expr_with_prec(v, 8)),
        ExprKind::RecordConstruction { type_name, fields } => {
            let parts: Vec<String> = fields
                .iter()
                .map(|f| match &f.value {
                    Some(v) => format!("{}: {}", f.name.name, expr_with_prec(v, 0)),
                    None => f.name.name.clone(),
                })
                .collect();
            if parts.is_empty() {
                format!("{} {{}}", type_name.name)
            } else {
                format!("{} {{ {} }}", type_name.name, parts.join(", "))
            }
        }
        ExprKind::FieldAccess { receiver, field } => {
            format!("{}.{}", expr_with_prec(receiver, 8), field.name)
        }
        ExprKind::MethodCall {
            receiver,
            method,
            type_args,
            args,
        } => {
            let targs = if type_args.is_empty() {
                String::new()
            } else {
                format!(
                    "[{}]",
                    type_args
                        .iter()
                        .map(type_ref_to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            let parts: Vec<String> = args.iter().map(|a| expr_with_prec(a, 0)).collect();
            format!(
                "{}.{}{targs}({})",
                expr_with_prec(receiver, 8),
                method.name,
                parts.join(", ")
            )
        }
        ExprKind::Match { discriminant, arms } => {
            let mut out = String::new();
            out.push_str("match ");
            out.push_str(&expr_with_prec(discriminant, 0));
            out.push_str(" {\n");
            for arm in arms {
                out.push('\t');
                out.push_str(&pattern_to_string(&arm.pattern));
                if let Some(guard) = &arm.guard {
                    out.push_str(" if ");
                    out.push_str(&expr_with_prec(guard, 0));
                }
                out.push_str(" => ");
                match &arm.body {
                    MatchBody::Expr(e) => out.push_str(&expr_with_prec(e, 0)),
                    MatchBody::Block(b) => out.push_str(&format_block_oneline(b)),
                }
                out.push_str(",\n");
            }
            out.push('}');
            out
        }
        ExprKind::Is { value, pattern } => {
            format!(
                "{} is {}",
                expr_with_prec(value, 4),
                is_pattern_src(pattern)
            )
        }
        ExprKind::RecordSpread {
            type_name,
            base,
            overrides,
        } => {
            let mut parts = vec![format!("...{}", expr_with_prec(base, 0))];
            for f in overrides {
                if let Some(v) = &f.value {
                    parts.push(format!("{}: {}", f.name.name, expr_with_prec(v, 0)));
                } else {
                    parts.push(f.name.name.clone());
                }
            }
            let body = parts.join(", ");
            match type_name {
                Some(tn) => format!("{} {{ {} }}", tn.name, body),
                None => format!("{{ {} }}", body),
            }
        }
        ExprKind::EffectPure(v) => format!("Effect.pure({})", expr_with_prec(v, 0)),
        ExprKind::Expect(v) => format!("expect {}", expr_with_prec(v, 0)),
        ExprKind::Val { type_ref, args } => {
            let t = type_ref_to_string(type_ref);
            if args.is_empty() {
                format!("Val[{t}]")
            } else {
                let a = args
                    .iter()
                    .map(|x| expr_with_prec(x, 0))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("Val[{t}]({a})")
            }
        }
        ExprKind::Wire(inner) => format!("Wire({})", expr_with_prec(inner, 0)),
        ExprKind::Trace { cap, op } => format!("trace({}.{})", cap.name, op.name),
        // #1706: the fault claim — the call, then the contextual `faults`.
        // #1812: the call-site principal sits between the call and `faults`,
        // in the same position as on an effect-let.
        ExprKind::Faults(f) => match &f.principal {
            Some(p) => format!(
                "{} {} faults",
                expr_with_prec(&f.call, 0),
                call_site_actor_src(p)
            ),
            None => format!("{} faults", expr_with_prec(&f.call, 0)),
        },
        ExprKind::Observation(o) => {
            let subject = format!("{}.{}", o.cap.name, o.op.name);
            match &o.matcher {
                ObservationMatcher::NeverCalled => format!("{subject} never called"),
                ObservationMatcher::Before { cap, op } => {
                    format!("{subject} before {}.{}", cap.name, op.name)
                }
                ObservationMatcher::Called { count, with_pred } => {
                    let mut s = format!("{subject} called");
                    if let Some(c) = count {
                        if matches!(c.kind, ExprKind::IntLit { value: 1, .. }) {
                            s.push_str(" once");
                        } else {
                            s.push_str(&format!(" {} times", expr_with_prec(c, 0)));
                        }
                    }
                    if let Some(p) = with_pred {
                        s.push_str(&format!(" with {}", expr_with_prec(p, 0)));
                    }
                    s
                }
            }
        }
    }
}

/// #2891: the `if` an `else if` desugars to. The parser wraps it in a
/// brace-less block with the same span as the `if` itself, which tells it apart
/// from an explicit `else { if … }` (whose block spans its braces).
pub fn else_if_tail(b: &Block) -> Option<&Expr> {
    (b.statements.is_empty()
        && b.tail_leading_comments.is_empty()
        && !b.implicit_tail
        && matches!(b.tail.kind, ExprKind::If { .. })
        && b.span == b.tail.span)
        .then_some(&*b.tail)
}

pub fn pattern_to_string(p: &Pattern) -> String {
    match p {
        Pattern::Wildcard(_) => "_".to_string(),
        // ADR 0169: a bare name binding renders as its identifier.
        Pattern::Binding(id) => id.name.clone(),
        // v0.130: literal patterns render as their source literal.
        Pattern::Literal { value, text, .. } => match value {
            LiteralValue::Int(n) => text.clone().unwrap_or_else(|| n.to_string()),
            LiteralValue::Str(s) => format!("\"{}\"", escape_string(s)),
            LiteralValue::Bool(b) => b.to_string(),
        },
        // #2699: `lo..hi`, `lo..`, `..hi`, with no spaces.
        Pattern::Range(r) => r.describe_source(),
        // #472: `p where predicate` — the inner pattern, then the predicate
        // list rendered the same way a `type X = Base where P` refinement is.
        Pattern::Refined {
            inner, predicate, ..
        } => format!(
            "{} where {}",
            pattern_to_string(inner),
            refinement_to_string(predicate)
        ),
        Pattern::Variant {
            type_name,
            variant,
            bindings,
            ..
        } => {
            let name_part = match type_name {
                Some(t) => format!("{}.{}", t.name, variant.name),
                None => variant.name.clone(),
            };
            if bindings.is_empty() {
                name_part
            } else {
                // ADR 0169: each payload binding is a full sub-pattern.
                let parts: Vec<String> = bindings
                    .iter()
                    .map(|b| match &b.kind {
                        PatternBindingKind::Positional { pattern } => pattern_to_string(pattern),
                        PatternBindingKind::Named { field, pattern } => {
                            format!("{}: {}", field.name, pattern_to_string(pattern))
                        }
                    })
                    .collect();
                format!("{}({})", name_part, parts.join(", "))
            }
        }
        // #2641: a record pattern renders like a record literal; the trailing
        // `..` is part of the syntax, not an optional marker.
        Pattern::Record {
            type_name, fields, ..
        } => {
            let mut parts: Vec<String> = fields
                .iter()
                .map(|(name, sub)| match sub {
                    Some(p) => format!("{}: {}", name.name, pattern_to_string(p)),
                    None => name.name.clone(),
                })
                .collect();
            parts.push("..".to_string());
            format!("{} {{ {} }}", type_name.name, parts.join(", "))
        }
        // #2680: a list pattern renders like a list literal, with the rest
        // marker last.
        Pattern::List { elems, rest, .. } => {
            let mut parts: Vec<String> = elems.iter().map(pattern_to_string).collect();
            if let Some(r) = rest {
                parts.push(match &r.binder {
                    Some(b) => format!("..{}", b.name),
                    None => "..".to_string(),
                });
            }
            format!("[{}]", parts.join(", "))
        }
        // #474: an or-pattern renders as its alternatives joined by `|`.
        Pattern::Or(alts, _) => alts
            .iter()
            .map(pattern_to_string)
            .collect::<Vec<_>>()
            .join(" | "),
    }
}

/// #2641: a pattern after `is`. A record pattern there must be parenthesised —
/// its `{` would otherwise read as the block that follows the condition — so a
/// record, or an or-pattern with a record alternative, keeps the parentheses
/// the parser dropped as transparent grouping.
pub fn is_pattern_src(p: &Pattern) -> String {
    let src = pattern_to_string(p);
    let has_record = match p {
        Pattern::Record { .. } => true,
        Pattern::Or(alts, _) => alts.iter().any(|a| matches!(a, Pattern::Record { .. })),
        _ => false,
    };
    if has_record { format!("({src})") } else { src }
}

/// #981: whether a block's `()` tail should be omitted rather than printed.
///
/// A `()` tail — whether the parser synthesised it ([`Block::implicit_tail`])
/// or the user wrote it out explicitly — is exactly the block's default
/// value, so dropping it is loss-free: the parser re-derives the same
/// implicit unit tail either way (v0.7 / v0.146, ADR 0170).
///
/// Omitting it is not just an idempotency nicety, it is required for
/// correctness whenever anything precedes the tail (a statement, a `case`'s
/// `stub` clause): Bynk has no statement terminator, so a printed `()`
/// immediately after a preceding line's last token re-attaches to it as a
/// zero-arg call on re-parse (`x` / `()` → `x()`) rather than staying two
/// separate constructs. #735 only special-cased the *implicit*-tail shape;
/// #981 found the identical corruption for an *explicit* `()` tail (e.g. the
/// last statement of a `match` arm's block), which is exactly as dangerous
/// once anything comes before it. So this covers both, structurally, rather
/// than special-casing another syntactic position.
pub fn omit_unit_tail(b: &Block) -> bool {
    matches!(b.tail.kind, ExprKind::UnitLit) && b.tail_leading_comments.is_empty()
}

fn format_block_oneline(b: &Block) -> String {
    if b.statements.is_empty() {
        // v0.146 (ADR 0170): an empty block with a synthesised `()` tail prints
        // as `{}` — printing `{ () }` would not round-trip against the parser's
        // implicit-tail synthesis.
        if b.implicit_tail {
            "{}".to_string()
        } else {
            format!("{{ {} }}", expr_with_prec(&b.tail, 0))
        }
    } else {
        // Multi-line block — render with newlines and tab indentation.
        let mut out = String::from("{\n");
        block_body_lines(b, &mut out);
        out.push('}');
        out
    }
}

/// The string form of [`Formatter::format_block_body`]: each statement and
/// the tail on its own tab-indented line, a `let … else` printed back as
/// written (#2010).
fn block_body_lines(b: &Block, out: &mut String) {
    for stmt in &b.statements {
        if let Some((l, effectful, le)) = let_else_binding(stmt)
            && let ExprKind::Match { arms, .. } = &b.tail.kind
            && let Some(MatchBody::Block(rest)) = arms.first().map(|a| &a.body)
        {
            out.push('\t');
            out.push_str(&let_else_to_string(l, effectful, le, arms));
            out.push('\n');
            block_body_lines(rest, out);
            return;
        }
        out.push('\t');
        out.push_str(&stmt_to_string(stmt));
        out.push('\n');
    }
    if !omit_unit_tail(b) {
        out.push('\t');
        out.push_str(&expr_with_prec(&b.tail, 0));
        out.push('\n');
    }
}

/// #2712: a lambda's body as the author wrote it.
pub enum LambdaBodySrc<'a> {
    Expr(&'a Expr),
    Block(&'a Block),
}

/// #2712: the body a lambda with pattern parameters was written with. The
/// parser wraps it, one level per pattern parameter (first outermost), in
/// `{ let __letElse = __lambdaArg; match __letElse { P => { <rest> } } }`;
/// this peels those back off. A lambda with no pattern parameter is untouched.
pub fn lambda_body_src(lambda: &LambdaExpr) -> LambdaBodySrc<'_> {
    let patterns = lambda.params.iter().filter(|p| p.pattern.is_some()).count();
    if patterns == 0 {
        return LambdaBodySrc::Expr(&lambda.body);
    }
    let ExprKind::Block(block) = &lambda.body.kind else {
        return LambdaBodySrc::Expr(&lambda.body);
    };
    let mut block: &Block = block;
    for _ in 0..patterns {
        let ExprKind::Match { arms, .. } = &block.tail.kind else {
            return LambdaBodySrc::Expr(&lambda.body);
        };
        let Some(MatchBody::Block(rest)) = arms.first().map(|a| &a.body) else {
            return LambdaBodySrc::Expr(&lambda.body);
        };
        block = rest;
    }
    if lambda.block_body {
        LambdaBodySrc::Block(block)
    } else {
        LambdaBodySrc::Expr(&block.tail)
    }
}

/// #2010: the hidden binding a `let … else` desugars into, if `s` is one —
/// with whether it was `<-` and the sugar's own marker.
pub fn let_else_binding(s: &Statement) -> Option<(&LetStmt, bool, &LetElse)> {
    match s {
        Statement::Let(l) => l.let_else.as_ref().map(|le| (l, false, le)),
        Statement::EffectLet(l) => l.let_else.as_ref().map(|le| (l, true, le)),
        _ => None,
    }
}

/// #2010: a `let … else` pattern as source. After `let`, a bare name is a
/// binding, not a pattern (`let Missing | Late = p` would re-parse as a
/// binding named `Missing`), so a pattern whose text opens with a name not
/// followed by `(`, `.` or `{` keeps the parentheses its author had to write.
/// `None`, `true` and `false` are keywords, not names, so they need none.
pub fn let_pattern_src(p: &Pattern) -> String {
    let src = pattern_to_string(p);
    let name_len = src
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(src.len());
    let opens_with_name = src.starts_with(|c: char| c.is_ascii_alphabetic())
        && !matches!(&src[..name_len], "None" | "true" | "false");
    let then = src[name_len..].chars().next();
    // #2654: `Name {` opens a record pattern, so it needs no parentheses.
    let opens_record = src[name_len..].trim_start().starts_with('{');
    if opens_with_name && !opens_record && !matches!(then, Some('(' | '.')) {
        format!("({src})")
    } else {
        src
    }
}

/// #2010: the single-line string form of a `let … else` (see
/// [`Formatter::format_let_else`]).
fn let_else_to_string(l: &LetStmt, effectful: bool, le: &LetElse, arms: &[MatchArm]) -> String {
    let mut out = format!(
        "let {} {} {}",
        let_pattern_src(&arms[0].pattern),
        if effectful { "<-" } else { "=" },
        expr_with_prec(&l.value, 0)
    );
    if let Some(p) = &l.principal {
        out.push_str(&format!(" {}", call_site_actor_src(p)));
    }
    match (le.form, arms.get(1).map(|a| &a.body)) {
        (LetElseForm::Plain, _) => {}
        (LetElseForm::Block, Some(MatchBody::Block(otherwise))) => {
            out.push_str(&format!(" else {}", format_block_oneline(otherwise)));
        }
        _ => {
            let rendered: Vec<String> = arms[1..]
                .iter()
                .map(|a| {
                    let body = match &a.body {
                        MatchBody::Expr(e) => expr_with_prec(e, 0),
                        MatchBody::Block(b) => format_block_oneline(b),
                    };
                    let guard = a
                        .guard
                        .as_ref()
                        .map(|g| format!(" if {}", expr_with_prec(g, 0)))
                        .unwrap_or_default();
                    format!("{}{guard} => {body}", pattern_to_string(&a.pattern))
                })
                .collect();
            out.push_str(&format!(" else match {{ {} }}", rendered.join(", ")));
        }
    }
    out
}

fn stmt_to_string(s: &Statement) -> String {
    match s {
        Statement::Let(l) => {
            let mut out = format!("let {}", l.name.name);
            if let Some(t) = &l.type_annot {
                out.push_str(&format!(": {}", type_ref_to_string(t)));
            }
            out.push_str(&format!(" = {}", expr_with_prec(&l.value, 0)));
            out
        }
        Statement::EffectLet(l) => {
            let mut out = format!("let {}", l.name.name);
            if let Some(t) = &l.type_annot {
                out.push_str(&format!(": {}", type_ref_to_string(t)));
            }
            out.push_str(&format!(" <- {}", expr_with_prec(&l.value, 0)));
            if let Some(p) = &l.principal {
                out.push_str(&format!(" {}", call_site_actor_src(p)));
            }
            out
        }
        Statement::Expect(a) => format!(
            "expect {}{}",
            expr_with_prec(&a.value, 0),
            a.snapshot.as_ref().map_or(String::new(), |c| format!(
                " matches snapshot \"{}\"",
                escape_string(&c.name)
            ))
        ),
        Statement::Send(s) => format!("~> {}", expr_with_prec(&s.value, 0)),
        Statement::Do(d) => format!("do {}", expr_with_prec(&d.value, 0)),
        Statement::Assign(a) => format!("{} := {}", a.target.name, expr_with_prec(&a.value, 0)),
        Statement::Advance(a) => {
            format!("advance {} by {}", a.cap.name, expr_with_prec(&a.amount, 0))
        }
        Statement::Par(p) => {
            let body: Vec<String> = p.statements.iter().map(stmt_to_string).collect();
            format!("par {{ {} }}", body.join(" "))
        }
    }
}

/// #2712: a lambda parameter as source — the author's record pattern when it
/// has one (its hidden binder never prints), else the name and any annotation.
pub fn lambda_param_src(p: &LambdaParam) -> String {
    let name = if p.is_wildcard() { "_" } else { &p.name.name };
    match (&p.pattern, &p.type_ref) {
        (Some(pattern), _) => pattern_to_string(pattern),
        (None, Some(tr)) => format!("{name}: {}", type_ref_to_string(tr)),
        (None, None) => name.to_string(),
    }
}

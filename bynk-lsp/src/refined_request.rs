//! #2470: the refined-type explorer — `bynk/refinedTypeModel`, and the hover
//! link that opens it.
//!
//! Hovering a refined type shows its base type and its predicates in readable
//! form, with a "Try values…" link (a client-side command, offered only to a
//! client that says it runs it — see [`client_supports_explorer_command`]).
//! The link opens a panel: a few example values, and a box that checks a
//! candidate against the predicate and names every clause that fails.
//!
//! **Where the check runs (DECISION A).** In the extension host, over the same
//! condition the emitter lowers for `.of` — [`bynk_emit::emitter::pred_condition_and_message`],
//! printed to a JavaScript expression over `value`. The server does not run
//! anything; it ships each clause's condition as text. No project code is ever
//! evaluated: the only source is a closed set of eight predicate forms the
//! compiler itself generates (the sole user-written part, a `Matches` pattern,
//! sits inside a string literal), and the client evaluates it in a fresh
//! `node:vm` context with a time limit. A `bynk eval` driver would instead
//! compile and run the project's output, which is the thing to avoid.
//!
//! **Examples (DECISION B).** Drawn here, deterministically, from the domain
//! the test runtime's `for all` generator uses (`bynk-emit`'s `int_bounds`,
//! `float_bounds` and `str_min`): the boundaries first, then draws from the
//! same PRNG as `__bynkRng` under a fixed [`SEED`], so the list is identical on
//! every open. A `Matches` type has no generator (`bynk.val.needs_pin`); its
//! drawn strings are kept only if they match the pattern, usually none.

use std::collections::HashSet;

use bynk_syntax::ast::{
    BaseType, CommonsItem, PredKind, Refinement, SourceUnit, TypeBody, TypeDecl,
};
use bynk_syntax::lexer::tokenize;
use bynk_syntax::parser::parse_unit_with_recovery;
use tower_lsp::lsp_types::{Position, TextDocumentIdentifier};

/// The client-side command the hover link invokes, with `[uri, position]`.
/// The VS Code extension registers it (`src/refinedExplorer.ts` in bynk-lang/vscode-bynk).
pub const EXPLORE_COMMAND: &str = "bynk.exploreRefinedType";

/// The `ClientCapabilities.experimental` key a client sets to `true` to say it
/// implements [`EXPLORE_COMMAND`], and so wants the hover link.
pub const CLIENT_CAPABILITY: &str = "refinedTypeExplorerCommand";

/// The fixed PRNG seed behind the examples: the same list on every open.
pub const SEED: u32 = 0x2470;

/// How many examples the panel lists at most.
const MAX_EXAMPLES: usize = 6;

/// The longest string drawn as an example: a `MinLength` beyond this lists none.
const MAX_EXAMPLE_LEN: i64 = 256;

/// The `bynk/refinedTypeModel` request payload — a text-document position.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefinedParams {
    pub text_document: TextDocumentIdentifier,
    pub position: Position,
}

/// One predicate of the refinement.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Clause {
    /// As written: `InRange(1, 100)`.
    pub source: String,
    /// In words: `1 ≤ value ≤ 100`.
    pub description: String,
    /// The emitter's own condition over `value`, as JavaScript: true when the
    /// clause holds.
    pub check: String,
    /// The emitter's failure message, as `.of` reports it.
    pub message: String,
}

/// The `bynk/refinedTypeModel` response.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefinedModel {
    pub name: String,
    /// `Int`, `Float` or `String` — how the panel reads a candidate.
    pub base: &'static str,
    pub opaque: bool,
    /// `type Age = Int where InRange(0, 150)`.
    pub declaration: String,
    /// In the order the runtime checks them (a `Matches` last, #1651).
    pub clauses: Vec<Clause>,
    /// Candidate input texts: a string example is its own text.
    pub examples: Vec<String>,
    /// Why `examples` is short or empty, when it is.
    pub examples_note: Option<String>,
    pub seed: u32,
}

/// The `type` declaration called `name` in `source`, whichever unit kind holds it.
pub fn find_type_decl(source: &str, name: &str) -> Option<TypeDecl> {
    let tokens = tokenize(source).ok()?;
    let (unit, _errs) = parse_unit_with_recovery(&tokens, source);
    let items: &[CommonsItem] = match unit.as_ref()? {
        SourceUnit::Commons(c) => &c.items,
        SourceUnit::Context(c) => &c.items,
        SourceUnit::Adapter(a) => &a.items,
        SourceUnit::Suite(_) => &[],
    };
    items.iter().find_map(|item| match item {
        CommonsItem::Type(t) if t.name.name == name => Some(t.clone()),
        _ => None,
    })
}

/// The explorer model for a declaration, or `None` unless it is a refined (or
/// opaque) `Int`, `Float` or `String` that carries at least one predicate.
/// `with_examples` is false on the hover path, which reads only the clauses:
/// drawing examples there would be work done on every hover for nothing.
pub fn model_for(decl: &TypeDecl, with_examples: bool) -> Option<RefinedModel> {
    let (base, refinement, opaque) = match &decl.body {
        TypeBody::Refined {
            base,
            refinement: Some(r),
            ..
        } => (*base, r, false),
        TypeBody::Opaque {
            base,
            refinement: Some(r),
            ..
        } => (*base, r, true),
        _ => return None,
    };
    let base_name = match base {
        BaseType::Int => "Int",
        BaseType::Float => "Float",
        BaseType::String => "String",
        _ => return None,
    };
    if refinement.predicates.is_empty() {
        return None;
    }
    let clauses = bynk_syntax::ast::in_check_order(&refinement.predicates, |p| &p.kind)
        .into_iter()
        .map(|p| clause(&p.kind))
        .collect();
    let (examples, examples_note) = if with_examples {
        examples(base, refinement)
    } else {
        (Vec::new(), None)
    };
    Some(RefinedModel {
        name: decl.name.name.clone(),
        base: base_name,
        opaque,
        declaration: format!(
            "type {} = {}{} where {}",
            decl.name.name,
            if opaque { "opaque " } else { "" },
            base_name,
            bynk_fmt::refinement_to_string(refinement)
        ),
        clauses,
        examples,
        examples_note,
        seed: SEED,
    })
}

fn clause(kind: &PredKind) -> Clause {
    let (cond, message) = bynk_emit::emitter::pred_condition_and_message(kind, "value");
    let description = match kind {
        PredKind::NonNegative => "value ≥ 0".to_string(),
        PredKind::Positive => "value > 0".to_string(),
        PredKind::InRange(a, b) => format!("{} ≤ value ≤ {}", a.value, b.value),
        PredKind::InRangeF(a, b) => format!("{} ≤ value ≤ {}", a.lexeme, b.lexeme),
        PredKind::NonEmpty => "length ≥ 1".to_string(),
        PredKind::MinLength(n) => format!("length ≥ {n}"),
        PredKind::MaxLength(n) => format!("length ≤ {n}"),
        PredKind::Length(n) => format!("length = {n}"),
        PredKind::Matches(re) => format!("the whole string matches /{re}/"),
    };
    Clause {
        source: bynk_fmt::refinement_to_string(&Refinement {
            predicates: vec![bynk_syntax::ast::RefinementPred {
                kind: kind.clone(),
                span: bynk_syntax::span::Span::default(),
            }],
            span: bynk_syntax::span::Span::default(),
        }),
        description,
        check: bynk_ts::print_expr(&cond),
        message,
    }
}

/// The hover text for a refined type: its base and each clause in words.
pub fn hover_section(model: &RefinedModel) -> String {
    let mut out = format!(
        "**{}** over `{}` — a value is valid when:\n",
        if model.opaque {
            "Opaque refined type"
        } else {
            "Refined type"
        },
        model.base
    );
    for c in &model.clauses {
        out.push_str(&format!("\n- `{}` — {}", c.source, c.description));
    }
    out
}

/// The hover link: a `command:` URI carrying `[uri, position]`, percent-encoded.
pub fn hover_link(uri: &str, position: Position) -> String {
    let args = serde_json::json!([uri, { "line": position.line, "character": position.character }]);
    let mut enc = String::new();
    for b in args.to_string().bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            enc.push(b as char);
        } else {
            enc.push_str(&format!("%{b:02X}"));
        }
    }
    format!("[Try values…](command:{EXPLORE_COMMAND}?{enc})")
}

/// Whether the client advertised `experimental.refinedTypeExplorerCommand: true`.
pub fn client_supports_explorer_command(experimental: Option<&serde_json::Value>) -> bool {
    experimental
        .and_then(|v| v.get(CLIENT_CAPABILITY))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

// ---------------------------------------------------------------------
// Examples: the same PRNG as the test runtime's `__bynkRng`, fixed seed.
// ---------------------------------------------------------------------

struct Rng(u32);

impl Rng {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x6d2b_79f5);
        let mut t = self.0;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        f64::from(t ^ (t >> 14)) / 4_294_967_296.0
    }

    fn int(&mut self, lo: i128, hi: i128) -> i128 {
        if hi <= lo {
            return lo;
        }
        let span = hi - lo + 1;
        let mut r: i128 = 0;
        let mut bound = span;
        while bound > 0 {
            r = (r << 32) | (self.next() * 4_294_967_296.0).floor() as i128;
            bound >>= 32;
        }
        lo + r.rem_euclid(span)
    }

    fn string(&mut self, len: usize) -> String {
        (0..len)
            .map(|_| (b'a' + (self.next() * 26.0).floor() as u8) as char)
            .collect()
    }
}

fn examples(base: BaseType, r: &Refinement) -> (Vec<String>, Option<String>) {
    let mut rng = Rng(SEED);
    let mut seen = HashSet::new();
    let mut out: Vec<String> = Vec::new();
    let mut push = |s: String, out: &mut Vec<String>| {
        if out.len() < MAX_EXAMPLES && seen.insert(s.clone()) {
            out.push(s);
        }
    };
    let preds = r.predicates.iter().map(|p| &p.kind);
    match base {
        BaseType::Int => {
            let (mut lower, mut upper): (Option<i64>, Option<i64>) = (None, None);
            for k in preds {
                let (l, u) = match k {
                    PredKind::Positive => (Some(1), None),
                    PredKind::NonNegative => (Some(0), None),
                    PredKind::InRange(a, b) => (Some(a.value), Some(b.value)),
                    _ => continue,
                };
                lower = lower.max(l);
                upper = match (upper, u) {
                    (Some(x), Some(y)) => Some(x.min(y)),
                    (x, y) => x.or(y),
                };
            }
            let lo = lower.unwrap_or_else(|| (-1000).min(upper.unwrap_or(1000)));
            let hi = upper.unwrap_or_else(|| 1000.max(lo));
            if lo > hi {
                return (out, Some("No integer satisfies every clause.".to_string()));
            }
            push(lo.to_string(), &mut out);
            push(hi.to_string(), &mut out);
            for _ in 0..64 {
                if out.len() >= MAX_EXAMPLES {
                    break;
                }
                push(
                    rng.int(i128::from(lo), i128::from(hi)).to_string(),
                    &mut out,
                );
            }
            (out, None)
        }
        BaseType::Float => {
            let (mut lower, mut upper): (Option<f64>, Option<f64>) = (None, None);
            let mut positive = false;
            for k in preds {
                let (l, u) = match k {
                    PredKind::Positive => {
                        positive = true;
                        (Some(0.0), None)
                    }
                    PredKind::NonNegative => (Some(0.0), None),
                    PredKind::InRangeF(a, b) => (Some(a.value), Some(b.value)),
                    _ => continue,
                };
                lower = match (lower, l) {
                    (Some(x), Some(y)) => Some(x.max(y)),
                    (x, y) => x.or(y),
                };
                upper = match (upper, u) {
                    (Some(x), Some(y)) => Some(x.min(y)),
                    (x, y) => x.or(y),
                };
            }
            let lo = lower.unwrap_or_else(|| (-1000.0f64).min(upper.unwrap_or(1000.0)));
            let hi = upper.unwrap_or_else(|| 1000.0f64.max(lo));
            // `Positive` is `value > 0`: zero is its (exclusive) bound, not a value.
            let admits = |v: f64| !positive || v > 0.0;
            if lo > hi || !admits(hi) {
                return (out, Some("No number satisfies every clause.".to_string()));
            }
            for v in [lo, hi] {
                if admits(v) {
                    push(format!("{v}"), &mut out);
                }
            }
            for _ in 0..64 {
                if out.len() >= MAX_EXAMPLES {
                    break;
                }
                let v = lo + rng.next() * (hi - lo);
                let v = ((v * 100.0).round() / 100.0).clamp(lo, hi);
                if admits(v) {
                    push(format!("{v}"), &mut out);
                }
            }
            (out, None)
        }
        BaseType::String => {
            let (mut min, mut max): (i64, Option<i64>) = (0, None);
            let mut pattern: Option<&str> = None;
            for k in preds {
                match k {
                    PredKind::NonEmpty => min = min.max(1),
                    PredKind::MinLength(n) => min = min.max(*n),
                    PredKind::MaxLength(n) => max = Some(max.map_or(*n, |m| m.min(*n))),
                    PredKind::Length(n) => {
                        min = min.max(*n);
                        max = Some(max.map_or(*n, |m| m.min(*n)));
                    }
                    PredKind::Matches(p) => pattern = Some(p),
                    _ => {}
                }
            }
            let hi = max.unwrap_or(min + 8).min(min + 8);
            if min > hi {
                return (out, Some("No string satisfies every clause.".to_string()));
            }
            if min > MAX_EXAMPLE_LEN {
                return (
                    out,
                    Some(format!(
                        "Every valid string is longer than {MAX_EXAMPLE_LEN} characters, too long to list."
                    )),
                );
            }
            let re = pattern.and_then(|p| regress::Regex::new(&format!("^(?:{p})$")).ok());
            let ok = |s: &str| re.as_ref().is_none_or(|re| re.find(s).is_some());
            let mut try_push = |len: i64, out: &mut Vec<String>, rng: &mut Rng| {
                let s = rng.string(len as usize);
                if ok(&s) {
                    push(s, out);
                }
            };
            try_push(min, &mut out, &mut rng);
            try_push(hi, &mut out, &mut rng);
            for _ in 0..64 {
                if out.len() >= MAX_EXAMPLES {
                    break;
                }
                let len = min + (rng.next() * ((hi - min + 1) as f64)).floor() as i64;
                try_push(len, &mut out, &mut rng);
            }
            let note = (pattern.is_some() && out.is_empty()).then(|| {
                "`Val[T]` has no generator for a `Matches` type (`bynk.val.needs_pin`); type a candidate below.".to_string()
            });
            (out, note)
        }
        _ => (out, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(src: &str, name: &str) -> RefinedModel {
        let decl = find_type_decl(src, name).expect("declaration");
        model_for(&decl, true).expect("model")
    }

    fn checks(m: &RefinedModel) -> Vec<&str> {
        m.clauses.iter().map(|c| c.check.as_str()).collect()
    }

    #[test]
    fn int_range_lists_boundaries_first_and_is_stable() {
        let src = "commons demo\n\ntype Quantity = Int where InRange(1, 100)\n";
        let m = model(src, "Quantity");
        assert_eq!(m.base, "Int");
        assert_eq!(m.examples[..2], ["1".to_string(), "100".to_string()]);
        assert_eq!(
            m,
            model(src, "Quantity"),
            "a fixed seed: the same list on every open"
        );
        assert!(m.examples.len() > 2);
        assert!(
            m.examples
                .iter()
                .all(|e| (1..=100).contains(&e.parse::<i64>().unwrap()))
        );
        assert_eq!(m.clauses[0].description, "1 ≤ value ≤ 100");
        assert_eq!(m.declaration, "type Quantity = Int where InRange(1, 100)");
    }

    #[test]
    fn every_int_predicate_form() {
        let src = "commons demo\n\ntype A = Int where NonNegative\ntype B = Int where Positive\ntype C = Int where InRange(-5, 5)\n";
        let a = model(src, "A");
        assert_eq!(checks(&a), ["value >= 0"]);
        assert_eq!(a.clauses[0].message, "must be non-negative");
        assert_eq!(a.clauses[0].description, "value ≥ 0");
        assert!(a.examples.iter().all(|e| e.parse::<i64>().unwrap() >= 0));
        let b = model(src, "B");
        assert_eq!(checks(&b), ["value > 0"]);
        assert_eq!(b.clauses[0].description, "value > 0");
        assert!(b.examples.iter().all(|e| e.parse::<i64>().unwrap() > 0));
        let c = model(src, "C");
        assert_eq!(checks(&c), ["value >= -5 && value <= 5"]);
        assert_eq!(c.clauses[0].message, "must be in range [-5, 5]");
        assert_eq!(c.examples[..2], ["-5".to_string(), "5".to_string()]);
    }

    #[test]
    fn float_range_and_sign_predicates() {
        let src = "commons demo\n\ntype P = Float where InRange(0.5, 2.5)\ntype N = Float where NonNegative\ntype Q = Float where Positive\n";
        let p = model(src, "P");
        assert_eq!(p.base, "Float");
        assert_eq!(p.clauses[0].description, "0.5 ≤ value ≤ 2.5");
        assert_eq!(p.clauses[0].message, "must be in range [0.5, 2.5]");
        assert_eq!(p.examples[..2], ["0.5".to_string(), "2.5".to_string()]);
        assert!(
            p.examples
                .iter()
                .all(|e| (0.5..=2.5).contains(&e.parse::<f64>().unwrap()))
        );
        assert!(
            model(src, "N")
                .examples
                .iter()
                .all(|e| e.parse::<f64>().unwrap() >= 0.0)
        );
        assert!(
            model(src, "Q")
                .examples
                .iter()
                .all(|e| e.parse::<f64>().unwrap() >= 1.0)
        );
    }

    #[test]
    fn positive_float_combines_with_a_small_range() {
        let src = "commons demo\n\ntype P = Float where Positive && InRange(0.1, 0.5)\ntype Z = Float where Positive && InRange(-1.0, 0.0)\ntype Tiny = Float where Positive && InRange(0.0, 0.005)\n";
        let p = model(src, "P");
        assert!(!p.examples.is_empty(), "{:?}", p.examples_note);
        assert!(
            p.examples
                .iter()
                .all(|e| (0.1..=0.5).contains(&e.parse::<f64>().unwrap()))
        );
        assert!(model(src, "Z").examples.is_empty());
        assert!(model(src, "Z").examples_note.is_some());
        let t = model(src, "Tiny");
        assert!(
            t.examples.iter().all(|e| e.parse::<f64>().unwrap() > 0.0),
            "{:?}",
            t.examples
        );
    }

    #[test]
    fn a_huge_min_length_draws_nothing_and_says_so() {
        let src = "commons demo\n\ntype Big = String where MinLength(100000000)\n";
        let m = model(src, "Big");
        assert!(m.examples.is_empty());
        assert!(
            m.examples_note
                .as_deref()
                .unwrap()
                .contains("too long to list")
        );
    }

    #[test]
    fn the_hover_path_draws_no_examples() {
        let src = "commons demo\n\ntype Q = Int where InRange(1, 100)\n";
        let decl = find_type_decl(src, "Q").unwrap();
        let m = model_for(&decl, false).unwrap();
        assert!(m.examples.is_empty() && m.examples_note.is_none());
        assert_eq!(m.clauses.len(), 1);
    }

    #[test]
    fn every_string_predicate_form() {
        let src = "commons demo\n\ntype E = String where NonEmpty\ntype Lo = String where MinLength(3)\ntype Hi = String where MaxLength(2)\ntype Ex = String where Length(4)\ntype Both = String where MinLength(3) && MaxLength(5)\n";
        let e = model(src, "E");
        assert_eq!(checks(&e), ["value.length > 0"]);
        assert_eq!(e.clauses[0].description, "length ≥ 1");
        assert!(e.examples.iter().all(|s| !s.is_empty()));
        let lo = model(src, "Lo");
        assert_eq!(checks(&lo), ["value.length >= 3"]);
        assert!(lo.examples.iter().all(|s| s.len() >= 3));
        let hi = model(src, "Hi");
        assert_eq!(checks(&hi), ["value.length <= 2"]);
        assert!(hi.examples.iter().all(|s| s.len() <= 2));
        assert!(
            hi.examples.contains(&String::new()),
            "the empty string is in MaxLength's domain"
        );
        let ex = model(src, "Ex");
        assert_eq!(checks(&ex), ["value.length === 4"]);
        assert_eq!(ex.clauses[0].message, "length must be exactly 4");
        assert!(ex.examples.iter().all(|s| s.len() == 4));
        let both = model(src, "Both");
        assert_eq!(both.clauses.len(), 2);
        assert!(both.examples.iter().all(|s| (3..=5).contains(&s.len())));
    }

    #[test]
    fn matches_is_checked_last_and_keeps_only_matching_examples() {
        let src = "commons demo\n\ntype Code = String where Matches(\"[a-z]+\") && MaxLength(4)\ntype Pin = String where Matches(\"[0-9]{4}\")\n";
        let m = model(src, "Code");
        assert_eq!(m.clauses[0].source, "MaxLength(4)", "length first, #1651");
        assert!(m.clauses[1].check.contains("globalThis.RegExp"));
        assert!(m.clauses[1].check.contains("^(?:"));
        assert_eq!(
            m.clauses[1].description,
            "the whole string matches /[a-z]+/"
        );
        assert!(
            m.examples
                .iter()
                .all(|s| s.chars().all(|c| c.is_ascii_lowercase())
                    && s.len() <= 4
                    && !s.is_empty())
        );
        let pin = model(src, "Pin");
        assert!(pin.examples.is_empty());
        assert!(pin.examples_note.as_deref().unwrap().contains("needs_pin"));
    }

    #[test]
    fn a_pattern_stays_inside_its_string_literal() {
        let src = "commons demo\n\ntype T = String where Matches(\"a\\\\\\\"); process.exit(1); (\\\"\")\n";
        let m = model(src, "T");
        let check = &m.clauses[0].check;
        // The only quote characters left unescaped are the literal's own.
        let body = &check[check.find("\"^(?:\"").unwrap()..];
        assert!(body.contains("\\\""), "{check}");
    }

    #[test]
    fn non_refined_and_unrefined_types_have_no_model() {
        let src = "commons demo\n\ntype Plain = Int\ntype Rec = { a: Int }\ntype Flag = Bool\n";
        for n in ["Plain", "Rec", "Flag"] {
            assert!(
                model_for(&find_type_decl(src, n).unwrap(), true).is_none(),
                "{n}"
            );
        }
    }

    #[test]
    fn opaque_with_a_predicate_is_modelled() {
        let src = "commons demo\n\ntype Id = opaque Int where Positive\n";
        let decl = find_type_decl(src, "Id");
        let m = decl.as_ref().and_then(|d| model_for(d, true));
        assert!(m.as_ref().is_some_and(|m| m.opaque), "{decl:?}");
        assert_eq!(
            m.unwrap().declaration,
            "type Id = opaque Int where Positive"
        );
    }

    #[test]
    fn the_hover_link_is_a_percent_encoded_command_uri() {
        let link = hover_link("file:///a/b.bynk", Position::new(3, 7));
        assert!(
            link.starts_with("[Try values…](command:bynk.exploreRefinedType?%5B%22file"),
            "{link}"
        );
        let query = link.split_once('?').unwrap().1;
        assert!(!query.contains(' ') && !query.contains('"'), "{link}");
    }
}

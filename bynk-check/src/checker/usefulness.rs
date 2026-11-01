//! Pattern coverage by usefulness (patterns track, slice 3, #2665).
//!
//! One engine answers both coverage questions, following Maranget's
//! "Warnings for pattern matching" over a matrix of the earlier unguarded
//! arms' patterns:
//!
//! - **exhaustiveness** asks which values no row matches ([`Coverage::uncovered`]),
//!   each reported as a concrete pattern an author can paste as an arm;
//! - **reachability** asks whether an arm's pattern is *useful*, that is,
//!   matches some value no earlier row does ([`Coverage::is_useful`]).
//!
//! Patterns are first lowered to a small internal form (`Pat`) over
//! [`Constructor`]s. Everything type-specific lives in [`Space`], the
//! constructor set of one column's type, so a later pattern form (list
//! lengths, integer ranges) adds a `Constructor` and teaches `Space` to split
//! it, rather than adding a second algorithm.
//!
//! Guards and refinement predicates are not understood: a guarded arm is never
//! a row, and a refined pattern as a *prior* matches nothing (its predicate may
//! fail). A refined pattern as the arm under test is tested as its inner
//! pattern. Or-patterns are expanded into rows at any depth.
//!
//! The search is bounded by [`STEP_BUDGET`]. When it runs out the answers
//! degrade soundly: [`Coverage::uncovered`] reports [`TooComplex`] and
//! [`Coverage::is_useful`] answers "useful" (so nothing is reported).

use super::expressions::{extensible_enum, literal_base_of};
use super::*;

/// Specialisation steps one coverage question may take before it gives up.
/// Several times what the corpus needs (its heaviest match takes about 23,000).
pub(super) const STEP_BUDGET: usize = 200_000;

/// The step budget ran out; the answer is unknown.
#[derive(Debug)]
pub(super) struct TooComplex;

/// A way of building a value of a type. The extension point: list patterns add
/// a length, range patterns an interval.
#[derive(Clone, PartialEq)]
enum Constructor {
    /// The `i`th declared variant of a sum, `Result` or `Option`.
    Variant(usize),
    /// A record's one constructor; its columns are all its fields in declared
    /// order.
    Record,
    Bool(bool),
    /// An `Int` or `String` literal. No finite set of these is complete.
    Literal(LiteralValue),
}

/// The constructors of one column's type.
enum Space {
    Variants {
        variants: Vec<VariantInfo>,
        /// An extensible enum's name: it has one more constructor, a variant
        /// added in a later release, that no pattern can name.
        future: Option<String>,
    },
    Record {
        name: String,
        fields: Vec<(String, TyId)>,
    },
    Bool,
    /// `Int`, `String` and anything without patterns of its own: only a
    /// wildcard completes it.
    Infinite,
}

impl Space {
    /// The declared constructors, in the order witnesses list them; `None`
    /// when no finite set completes the type.
    fn constructors(&self) -> Option<Vec<Constructor>> {
        match self {
            Space::Variants { variants, .. } => {
                Some((0..variants.len()).map(Constructor::Variant).collect())
            }
            Space::Record { .. } => Some(vec![Constructor::Record]),
            Space::Bool => Some(vec![Constructor::Bool(true), Constructor::Bool(false)]),
            Space::Infinite => None,
        }
    }

    /// The column types a constructor's arguments occupy.
    fn arg_tys(&self, c: &Constructor) -> Vec<TyId> {
        match (self, c) {
            (Space::Variants { variants, .. }, Constructor::Variant(i)) => {
                variants[*i].payload.iter().map(|(_, t)| *t).collect()
            }
            (Space::Record { fields, .. }, Constructor::Record) => {
                fields.iter().map(|(_, t)| *t).collect()
            }
            _ => Vec::new(),
        }
    }

    /// The witness for a constructor applied to `args`.
    fn witness(&self, c: &Constructor, args: Vec<Wit>) -> Wit {
        match (self, c) {
            (Space::Variants { variants, .. }, Constructor::Variant(i)) => Wit::Variant {
                name: variants[*i].name.clone(),
                args,
            },
            (Space::Record { name, fields }, Constructor::Record) => Wit::Record {
                name: name.clone(),
                fields: fields.iter().map(|(n, _)| n.clone()).zip(args).collect(),
            },
            (_, Constructor::Bool(b)) => Wit::Lit(b.to_string()),
            (_, Constructor::Literal(v)) => Wit::Lit(v.describe()),
            _ => Wit::Wild,
        }
    }
}

/// A lowered pattern.
#[derive(Clone)]
enum Pat {
    /// Matches nothing: a refined prior, or a pattern that did not lower.
    Never,
    Wild,
    Or(Vec<Pat>),
    Ctor(Constructor, Vec<Pat>),
}

/// A value shape no row matches, to be shown as a pattern.
#[derive(Clone)]
enum Wit {
    Wild,
    /// The variant an extensible enum may gain; shown as the wildcard that
    /// covers it, but tagged so the caller can word it differently.
    Future(String),
    Lit(String),
    Variant {
        name: String,
        args: Vec<Wit>,
    },
    Record {
        name: String,
        fields: Vec<(String, Wit)>,
    },
}

/// Opens and closes the witness for an extensible enum's future variant
/// (standard-library S3, #1963): `\u{1}FetchError\u{2}`. Control characters,
/// so no real variant or pattern text can collide with it.
pub(super) const FUTURE_VARIANT_OPEN: char = '\u{1}';
pub(super) const FUTURE_VARIANT_CLOSE: char = '\u{2}';

impl Wit {
    fn render(&self) -> String {
        match self {
            Wit::Wild => "_".to_string(),
            Wit::Future(name) => format!("{FUTURE_VARIANT_OPEN}{name}{FUTURE_VARIANT_CLOSE}"),
            Wit::Lit(s) => s.clone(),
            Wit::Variant { name, args } if args.is_empty() => name.clone(),
            Wit::Variant { name, args } => format!(
                "{name}({})",
                args.iter().map(Wit::render).collect::<Vec<_>>().join(", ")
            ),
            Wit::Record { name, fields } => {
                let mut parts: Vec<String> = fields
                    .iter()
                    .filter(|(_, w)| !matches!(w, Wit::Wild))
                    .map(|(n, w)| format!("{n}: {}", w.render()))
                    .collect();
                parts.push("..".to_string());
                format!("{name} {{ {} }}", parts.join(", "))
            }
        }
    }
}

type Row = Vec<Pat>;

/// The matrix of earlier unguarded arms over a scrutinee of one type.
pub(super) struct Coverage {
    ty: TyId,
    /// Each row with whether its arm type-checked. Exhaustiveness counts every
    /// row, as an ill-typed arm still names what it meant to cover; reachability
    /// only the well-typed ones, so a mistaken arm cannot make a correct one look
    /// redundant.
    rows: Vec<(Row, bool)>,
}

impl Coverage {
    pub(super) fn new(ty: TyId) -> Self {
        Coverage {
            ty,
            rows: Vec::new(),
        }
    }

    /// Adds an unguarded arm's pattern as a row.
    pub(super) fn push(&mut self, pat: &Pattern, well_typed: bool, ctx: &Ctx) {
        let lowered = Lowering { ctx }.lower(pat, self.ty, true);
        let well_typed = well_typed && lowered.is_some();
        self.rows
            .push((vec![lowered.unwrap_or(Pat::Never)], well_typed));
    }

    /// Whether `pat` matches some value no earlier well-typed row does. `true`
    /// when it cannot be told (the pattern did not lower, or the budget ran out).
    pub(super) fn is_useful(&self, pat: &Pattern, ctx: &Ctx) -> bool {
        let Some(q) = (Lowering { ctx }).lower(pat, self.ty, false) else {
            return true;
        };
        let rows = self
            .rows
            .iter()
            .filter(|(_, ok)| *ok)
            .map(|(r, _)| r.clone())
            .collect();
        let mut engine = Engine { ctx, steps: 0 };
        let useful = engine.useful(rows, vec![q], vec![self.ty]);
        useful.unwrap_or(true)
    }

    /// The patterns of the values no row matches, as the author would write
    /// them (empty when the rows are exhaustive), or [`TooComplex`].
    pub(super) fn uncovered(&self, ctx: &Ctx) -> Result<Vec<String>, TooComplex> {
        let rows = self.rows.iter().map(|(r, _)| r.clone()).collect();
        let mut engine = Engine { ctx, steps: 0 };
        let witnesses = engine.missing(rows, vec![self.ty], true);
        Ok(witnesses?
            .into_iter()
            .map(|mut w| w.remove(0).render())
            .collect())
    }
}

/// Turns checked AST patterns into [`Pat`]s at a column type.
struct Lowering<'c, 'a> {
    ctx: &'c Ctx<'a>,
}

impl Lowering<'_, '_> {
    /// `prior` is whether the pattern is a row of the matrix (a refined
    /// pattern then matches nothing) or the arm under test (it is tested as
    /// its inner pattern). `None` when it does not fit `ty`.
    fn lower(&self, pat: &Pattern, ty: TyId, prior: bool) -> Option<Pat> {
        match pat {
            Pattern::Wildcard(_) | Pattern::Binding(_) => Some(Pat::Wild),
            Pattern::Refined { inner, .. } => {
                if prior {
                    Some(Pat::Never)
                } else {
                    self.lower(inner, ty, prior)
                }
            }
            Pattern::Or(alts, _) => alts
                .iter()
                .map(|a| self.lower(a, ty, prior))
                .collect::<Option<Vec<_>>>()
                .map(Pat::Or),
            Pattern::Literal { value, .. } => match (space_of(ty, self.ctx), value) {
                (Space::Bool, LiteralValue::Bool(b)) => {
                    Some(Pat::Ctor(Constructor::Bool(*b), Vec::new()))
                }
                (Space::Infinite, LiteralValue::Int(_) | LiteralValue::Str(_)) => {
                    Some(Pat::Ctor(Constructor::Literal(value.clone()), Vec::new()))
                }
                _ => None,
            },
            Pattern::Variant {
                variant, bindings, ..
            } => {
                let Space::Variants { variants, .. } = space_of(ty, self.ctx) else {
                    return None;
                };
                let i = variants.iter().position(|v| v.name == variant.name)?;
                let info = &variants[i];
                // A payload that does not fit the variant (wrong arity, an
                // unknown field) is a type error reported where the pattern
                // is checked; the arm still names this variant, so it is
                // taken to cover it rather than also reported as missing.
                let mut args = vec![None; info.payload.len()];
                if bindings
                    .iter()
                    .any(|b| matches!(b.kind, PatternBindingKind::Named { .. }))
                {
                    for b in bindings {
                        if let PatternBindingKind::Named { field, pattern } = &b.kind
                            && let Some(at) =
                                info.payload.iter().position(|(n, _)| n == &field.name)
                        {
                            args[at] = Some(pattern);
                        }
                    }
                } else if bindings.len() == args.len() {
                    for (slot, b) in args.iter_mut().zip(bindings) {
                        *slot = Some(b.pattern());
                    }
                }
                let args = args
                    .into_iter()
                    .zip(&info.payload)
                    .map(|(p, (_, fty))| match p {
                        Some(p) => self.lower(p, *fty, prior),
                        None => Some(Pat::Wild),
                    })
                    .collect::<Option<Vec<_>>>()?;
                Some(Pat::Ctor(Constructor::Variant(i), args))
            }
            Pattern::Record {
                type_name, fields, ..
            } => {
                let Space::Record { name, fields: decl } = space_of(ty, self.ctx) else {
                    return None;
                };
                if name != type_name.name {
                    return None;
                }
                let mut args = vec![Pat::Wild; decl.len()];
                for (n, sub) in fields {
                    let at = decl.iter().position(|(d, _)| *d == n.name)?;
                    if let Some(sub) = sub {
                        args[at] = self.lower(sub, decl[at].1, prior)?;
                    }
                }
                Some(Pat::Ctor(Constructor::Record, args))
            }
        }
    }
}

/// The constructor space of `ty`. A refined type uses its base.
fn space_of(ty: TyId, ctx: &Ctx) -> Space {
    let tys = ctx.tys;
    if let Some(variants) = variants_of(ty, &ctx.input.types, tys) {
        return Space::Variants {
            variants,
            future: extensible_enum(ty, ctx),
        };
    }
    if let Ty::Named {
        name,
        kind: NamedKind::Record,
        args,
    } = &*tys.get(ty)
        && let Some(decl) = ctx.input.types.get(name)
        && let TypeBody::Record(r) = &decl.body
    {
        let fields = r
            .fields
            .iter()
            .map(|f| {
                instantiate_field_ty(decl, args, &f.type_ref, &ctx.input.types, tys)
                    .map(|t| (f.name.name.clone(), t))
            })
            .collect::<Option<Vec<_>>>();
        if let Some(fields) = fields {
            return Space::Record {
                name: name.clone(),
                fields,
            };
        }
    }
    match literal_base_of(ty, tys) {
        Some(BaseType::Bool) => Space::Bool,
        _ => Space::Infinite,
    }
}

struct Engine<'c, 'a> {
    ctx: &'c Ctx<'a>,
    steps: usize,
}

impl Engine<'_, '_> {
    fn tick(&mut self, cost: usize) -> Result<(), TooComplex> {
        self.steps += cost.max(1);
        if self.steps > STEP_BUDGET {
            Err(TooComplex)
        } else {
            Ok(())
        }
    }

    /// Replaces every row whose first pattern is an or-pattern by one row per
    /// alternative, and drops rows that begin with `Never`.
    fn expand(rows: Vec<Row>) -> Vec<Row> {
        fn push(out: &mut Vec<Row>, head: Pat, tail: &[Pat]) {
            match head {
                Pat::Never => {}
                Pat::Or(alts) => {
                    for alt in alts {
                        push(out, alt, tail);
                    }
                }
                head => {
                    let mut row = Vec::with_capacity(tail.len() + 1);
                    row.push(head);
                    row.extend_from_slice(tail);
                    out.push(row);
                }
            }
        }
        let mut out = Vec::new();
        for row in rows {
            let (head, tail) = row.split_first().expect("a row has a column");
            push(&mut out, head.clone(), tail);
        }
        out
    }

    /// The rows left once the first column is known to be built by `c`, with
    /// `c`'s `arity` arguments in front of the remaining columns.
    fn specialize(rows: &[Row], c: &Constructor, arity: usize) -> Vec<Row> {
        rows.iter()
            .filter_map(|row| match &row[0] {
                Pat::Wild => {
                    let mut r = vec![Pat::Wild; arity];
                    r.extend_from_slice(&row[1..]);
                    Some(r)
                }
                Pat::Ctor(rc, args) if rc == c => {
                    let mut r = args.clone();
                    r.extend_from_slice(&row[1..]);
                    Some(r)
                }
                _ => None,
            })
            .collect()
    }

    /// The rows whose first pattern matches any value, without that column.
    fn default_rows(rows: &[Row]) -> Vec<Row> {
        rows.iter()
            .filter(|row| matches!(row[0], Pat::Wild))
            .map(|row| row[1..].to_vec())
            .collect()
    }

    fn present(rows: &[Row], c: &Constructor) -> bool {
        rows.iter()
            .any(|row| matches!(&row[0], Pat::Ctor(rc, _) if rc == c))
    }

    /// Whether some value of the column types, matched by `q`, is matched by
    /// no row.
    fn useful(&mut self, rows: Vec<Row>, q: Row, tys: Vec<TyId>) -> Result<bool, TooComplex> {
        self.tick(rows.len())?;
        let Some((head, rest)) = q.split_first() else {
            return Ok(rows.is_empty());
        };
        match head {
            Pat::Never => Ok(false),
            Pat::Or(alts) => {
                for alt in alts {
                    let mut q = vec![alt.clone()];
                    q.extend_from_slice(rest);
                    if self.useful(rows.clone(), q, tys.clone())? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Pat::Ctor(c, args) => {
                let space = space_of(tys[0], self.ctx);
                let rows = Self::specialize(&Self::expand(rows), c, args.len());
                let mut q = args.clone();
                q.extend_from_slice(rest);
                let mut tys_after = space.arg_tys(c);
                tys_after.extend_from_slice(&tys[1..]);
                self.useful(rows, q, tys_after)
            }
            Pat::Wild => {
                let rows = Self::expand(rows);
                let space = space_of(tys[0], self.ctx);
                let complete = match (&space, space.constructors()) {
                    (
                        Space::Variants {
                            future: Some(_), ..
                        },
                        _,
                    )
                    | (_, None) => None,
                    (_, Some(cs)) => cs.iter().all(|c| Self::present(&rows, c)).then_some(cs),
                };
                match complete {
                    Some(cs) => {
                        for c in cs {
                            let arg_tys = space.arg_tys(&c);
                            let mut q = vec![Pat::Wild; arg_tys.len()];
                            q.extend_from_slice(rest);
                            let mut tys_after = arg_tys.clone();
                            tys_after.extend_from_slice(&tys[1..]);
                            let spec = Self::specialize(&rows, &c, arg_tys.len());
                            if self.useful(spec, q, tys_after)? {
                                return Ok(true);
                            }
                        }
                        Ok(false)
                    }
                    None => {
                        self.useful(Self::default_rows(&rows), rest.to_vec(), tys[1..].to_vec())
                    }
                }
            }
        }
    }

    /// Every vector of values over `tys` that no row matches, as witnesses.
    fn missing(
        &mut self,
        rows: Vec<Row>,
        tys: Vec<TyId>,
        top: bool,
    ) -> Result<Vec<Vec<Wit>>, TooComplex> {
        self.tick(rows.len())?;
        if tys.is_empty() {
            return Ok(if rows.is_empty() {
                vec![Vec::new()]
            } else {
                Vec::new()
            });
        }
        let rows = Self::expand(rows);
        let space = space_of(tys[0], self.ctx);
        let rest_tys = &tys[1..];
        let wilds = |n: usize| vec![Wit::Wild; n];
        let with_rest = |head: Wit, tail: Vec<Wit>| {
            let mut w = vec![head];
            w.extend(tail);
            w
        };
        let Some(ctors) = space.constructors() else {
            // Unbounded: only a wildcard row completes it.
            let ws = self.missing(Self::default_rows(&rows), rest_tys.to_vec(), false)?;
            return Ok(ws.into_iter().map(|w| with_rest(Wit::Wild, w)).collect());
        };
        let any_present = ctors.iter().any(|c| Self::present(&rows, c));
        if rows.is_empty() && !top {
            // Below the first column nothing constrains the rest: one witness.
            return Ok(vec![vec![Wit::Wild; tys.len()]]);
        }
        if rows.is_empty() {
            // Nothing is covered: name each way to build a value once, with
            // wildcard arguments, rather than enumerating their products.
            let mut out = Vec::new();
            for c in &ctors {
                let args = wilds(space.arg_tys(c).len());
                let mut w = vec![space.witness(c, args)];
                w.extend(wilds(rest_tys.len()));
                out.push(w);
            }
            if let Space::Variants {
                future: Some(name), ..
            } = &space
            {
                let mut w = vec![Wit::Future(name.clone())];
                w.extend(wilds(rest_tys.len()));
                out.push(w);
            }
            return Ok(out);
        }
        if !any_present {
            // Every row is a wildcard here: the column adds nothing to say.
            let ws = self.missing(Self::default_rows(&rows), rest_tys.to_vec(), false)?;
            return Ok(ws.into_iter().map(|w| with_rest(Wit::Wild, w)).collect());
        }
        // Some constructors are named. Recurse into each of those; an unnamed
        // one is uncovered wherever the wildcard rows leave the rest uncovered.
        let mut absent_tail: Option<Vec<Vec<Wit>>> = None;
        let mut out = Vec::new();
        for c in &ctors {
            let arg_tys = space.arg_tys(c);
            if Self::present(&rows, c) {
                let mut sub_tys = arg_tys.clone();
                sub_tys.extend_from_slice(rest_tys);
                let spec = Self::specialize(&rows, c, arg_tys.len());
                for mut w in self.missing(spec, sub_tys, false)? {
                    let tail = w.split_off(arg_tys.len());
                    out.push(with_rest(space.witness(c, w), tail));
                }
            } else {
                if absent_tail.is_none() {
                    absent_tail =
                        Some(self.missing(Self::default_rows(&rows), rest_tys.to_vec(), false)?);
                }
                for tail in absent_tail.as_ref().into_iter().flatten() {
                    out.push(with_rest(
                        space.witness(c, wilds(arg_tys.len())),
                        tail.clone(),
                    ));
                }
            }
        }
        if let Space::Variants {
            future: Some(name), ..
        } = &space
        {
            if absent_tail.is_none() {
                absent_tail =
                    Some(self.missing(Self::default_rows(&rows), rest_tys.to_vec(), false)?);
            }
            for tail in absent_tail.into_iter().flatten() {
                out.push(with_rest(Wit::Future(name.clone()), tail));
            }
        }
        Ok(out)
    }
}

//! P4c (#2628): the checker's half of `bynk.cloudflare`'s `D1`.
//!
//! - **Literal-only SQL.** `all`/`first`/`run`'s SQL argument, and every
//!   `SqlStatement`'s `sql`, must be a string literal (`bynk.types.sql_not_literal`),
//!   so data reaches a statement only through its bound `SqlValue` parameters.
//! - **`SqlStatement` stays in-process.** A decoded statement would carry SQL
//!   that was never a literal, so the type is non-storable and non-boundary
//!   (`bynk.types.sql_statement_at_boundary`), and no `Json` codec target.
//! - **`Json.decodeSql[T]`'s row shape** (`bynk.types.sql_row_shape`): `T` is a
//!   record of scalar columns, or a `List` of one.

use super::*;

/// The first-party record a `D1.batch` statement is.
pub(crate) const SQL_STATEMENT: &str = "SqlStatement";

/// The `D1` operations whose first argument is SQL text.
const SQL_TEXT_OPS: &[&str] = &["all", "first", "run"];

/// Whether `name`, in the project's type table, is `bynk.cloudflare`'s own
/// `SqlStatement`, found by its declaration's file as the crypto key types are.
pub(crate) fn is_sql_statement_type(name: &str, types: &HashMap<String, Arc<TypeDecl>>) -> bool {
    name == SQL_STATEMENT
        && types.get(name).is_some_and(|decl| {
            crate::firstparty::unit(crate::firstparty::CLOUDFLARE_UNIT).is_some_and(|unit| {
                decl.span.file == crate::project_model::firstparty_file_id(unit)
            })
        })
}

/// #3070: one literal SQL statement of a derived `D1` instance, as the project
/// phase prepares it against the migrations' schema.
#[derive(Debug, Clone)]
pub struct D1SqlSite {
    /// The capability the call is on (`Orders`), whose instance names the database.
    pub cap: String,
    /// The method (`all`, `first`, `run`, `select`, `firstAs`).
    pub op: String,
    /// The statement as SQLite sees it; for `select`/`firstAs` the composed
    /// `SELECT <columns> <tail>`.
    pub text: String,
    /// The literal's span: where every diagnostic is reported.
    pub span: Span,
    /// For `select`/`firstAs`: the row type and its field names (the columns the
    /// compiler wrote), so a missing one is reported against the type.
    pub row: Option<(String, Vec<String>)>,
    /// The number of values passed, when the list is a list literal.
    pub passed: Option<usize>,
}

/// The length of a list-literal argument; `None` when the list is a name, a
/// call or anything whose length is not fixed by the text.
fn literal_list_len(e: Option<&Expr>, ctx: &Ctx) -> Option<usize> {
    match &e?.kind {
        ExprKind::ListLit(items, ..) => Some(items.len()),
        // #3108: a `let` bound to a list literal in scope.
        ExprKind::Ident(id) => ctx.let_list_len(&id.name),
        _ => None,
    }
}

/// #3065: the text of a SQL argument that is fixed at compile time: a string
/// literal, or a bare reference to a `String` constant whose value, following
/// constant-to-constant references, is a plain string literal. A constant's
/// initialiser is a closed static set (`consts::static_error`: no operators,
/// calls or interpolation), so such a name can carry no run-time input, and the
/// checker judges the very text the call sends. `None` for anything else: a
/// parameter, a `let` binding (locals shadow a constant of the same name), a
/// field, a call, or a constant not declared exactly `String`.
pub(crate) fn sql_literal_text(e: &Expr, ctx: &Ctx) -> Option<String> {
    match &e.kind {
        ExprKind::StrLit(s) => Some(s.clone()),
        ExprKind::Ident(id) if ctx.lookup(id.name.as_str()).is_none() => {
            let mut name = id.name.as_str();
            // A chain longer than the constants in scope is a cycle, already
            // `bynk.resolve.const_cycle`; the bound keeps this total.
            for _ in 0..=ctx.input.consts.len() {
                let c = ctx.input.consts.get(name)?;
                if !matches!(c.ty, TypeRef::Base(BaseType::String, _)) {
                    return None;
                }
                match &c.init.kind {
                    ExprKind::StrLit(s) => return Some(s.clone()),
                    ExprKind::Ident(next) => name = next.name.as_str(),
                    _ => return None,
                }
            }
            None
        }
        _ => None,
    }
}

fn sql_not_literal(span: Span, what: &str) -> CompileError {
    CompileError::new(
        "bynk.types.sql_not_literal",
        span,
        format!("{what} must be a string literal or a `String` constant bound to one"),
    )
    .with_note(
        "pass values as `?1`, `?2`, … parameters in `params`, which are bound, never \
         spliced into the text; SQL built at run time could be injected. A `const` \
         `String` bound to a literal is accepted, since its value is fixed at compile time",
    )
}

/// A call to a derived instance of `D1`: its SQL argument must be a literal.
/// Keyed on the resolved base, not the method name, so a user capability
/// with an `all` operation is untouched.
pub(crate) fn check_sql_argument(
    derived_from: Option<&(String, String)>,
    cap: &str,
    method: &str,
    args: &[Expr],
    ctx: &mut Ctx,
) {
    let Some((unit, base)) = derived_from else {
        return;
    };
    if unit != crate::firstparty::CLOUDFLARE_UNIT || base != "D1" || !SQL_TEXT_OPS.contains(&method)
    {
        return;
    }
    if let Some(sql) = args.first() {
        if let Some(text) = sql_literal_text(sql, ctx) {
            ctx.callees.insert(
                sql.id,
                Callee::D1Sql(D1SqlSite {
                    cap: cap.to_string(),
                    op: method.to_string(),
                    text,
                    span: sql.span,
                    row: None,
                    passed: literal_list_len(args.get(1), ctx),
                }),
            );
        } else {
            ctx.errors.push(sql_not_literal(
                sql.span,
                &format!("the SQL of `{cap}.{method}`"),
            ));
        }
    }
}

/// A `SqlStatement { … }` construction, or a spread over one that overrides
/// `sql`: the field's value must be a literal. A shorthand `sql` field reads a
/// binding, so it is not one. A spread that keeps `sql` copies a statement
/// that was itself built from a literal.
pub(crate) fn check_statement_fields(ty: TyId, fields: &[FieldInit], ctx: &mut Ctx) {
    let is_statement = matches!(
        &*ctx.tys.get(ty),
        Ty::Named { name, kind: NamedKind::Record, .. } if is_sql_statement_type(name, &ctx.input.types)
    );
    if !is_statement {
        return;
    }
    for f in fields.iter().filter(|f| f.name.name == "sql") {
        let literal = f
            .value
            .as_ref()
            .is_some_and(|v| sql_literal_text(v, ctx).is_some());
        if !literal {
            let span = f.value.as_ref().map_or(f.name.span, |v| v.span);
            ctx.errors
                .push(sql_not_literal(span, "a `SqlStatement`'s `sql`"));
        }
    }
}

/// The diagnostic for a `SqlStatement` in a storable or boundary position.
pub(crate) fn statement_at_boundary(span: Span, what: &str) -> CompileError {
    CompileError::new(
        "bynk.types.sql_statement_at_boundary",
        span,
        format!(
            "a `SqlStatement` cannot appear in {what} — a statement received or restored from outside would carry SQL that was never a literal"
        ),
    )
    .with_note("send or store the values, and build the statement where it runs")
}

/// Whether a `Json` codec target is, or holds, a `SqlStatement`. A user type
/// cannot hold one (its declaration is refused), so only the built-in
/// containers need looking through.
pub(crate) fn carries_statement(
    t: TyId,
    types: &HashMap<String, Arc<TypeDecl>>,
    tys: &Types,
) -> bool {
    match &*tys.get(t) {
        Ty::Named { name, args, .. } => {
            is_sql_statement_type(name, types)
                || args.iter().any(|a| carries_statement(*a, types, tys))
        }
        Ty::Result(a, b) | Ty::Map(a, b) => {
            carries_statement(*a, types, tys) || carries_statement(*b, types, tys)
        }
        Ty::Option(a) | Ty::List(a) | Ty::Set(a) => carries_statement(*a, types, tys),
        _ => false,
    }
}

/// The typed reads of a derived `D1` instance, which the compiler lowers over
/// `all`/`first` (#2865); they are not declared operations of the capability.
pub(crate) fn is_typed_read(derived_from: Option<&(String, String)>, method: &str) -> bool {
    matches!(derived_from, Some((unit, base)) if unit == crate::firstparty::CLOUDFLARE_UNIT && base == "D1")
        && matches!(method, "select" | "firstAs")
}

/// The column list the compiler writes for a row record: each field's name as
/// a quoted identifier (an embedded `"` doubled), in declaration order.
pub fn select_columns(row: &str, types: &HashMap<String, Arc<TypeDecl>>) -> String {
    match types.get(row).map(|d| &d.body) {
        Some(TypeBody::Record(record)) => record
            .fields
            .iter()
            .map(|f| format!("\"{}\"", f.name.name.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(", "),
        _ => String::new(),
    }
}

/// Whether a literal tail fails to start at `FROM` (case-insensitive, after
/// whitespace): the compiler writes `SELECT` and the column list ahead of it.
fn begins_without_from(tail: &str) -> bool {
    let t = tail.trim_start();
    !t.get(..4).is_some_and(|p| p.eq_ignore_ascii_case("from"))
        || t[4..]
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
}

/// Whether a literal tail itself begins with the `SELECT` keyword.
fn begins_with_select(tail: &str) -> bool {
    let t = tail.trim_start();
    t.get(..6).is_some_and(|p| p.eq_ignore_ascii_case("select"))
        && !t[6..]
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
}

/// `D1.select[T](tail, params)` / `D1.firstAs[T](tail, params)` on a derived
/// instance (#2865): `T` is a row record, the tail a string literal from
/// `FROM` on. Records the lowering (`Callee::D1Select`) and types the call
/// `Effect[Result[List[T], SqlReadError]]` (`firstAs`: `Option[T]`).
pub(crate) fn check_typed_read(
    cap: &str,
    method: &Ident,
    type_args: &[TypeRef],
    args: &[Expr],
    span: Span,
    expr_id: ExprId,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    let name = format!("{cap}.{}", method.name);
    let bail = |ctx: &mut Ctx| {
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        None
    };
    if type_args.len() != 1 {
        ctx.errors.push(CompileError::new(
            "bynk.generics.type_arg_mismatch",
            span,
            format!(
                "`{name}` takes exactly one type argument (the row type), got {}",
                type_args.len()
            ),
        ));
        return bail(ctx);
    }
    if args.len() != 2 {
        ctx.errors.push(CompileError::new(
            "bynk.capability.op_arity",
            span,
            format!(
                "capability operation `{name}` expects 2 argument(s), but {} were given",
                args.len()
            ),
        ));
        return bail(ctx);
    }
    let Some(t) = resolve_expr_type_ref(&type_args[0], ctx) else {
        return bail(ctx);
    };
    // The first-party `SqlValue` / `SqlReadError`, which the consumer sees
    // through `bynk.cloudflare`'s transparent exports.
    let named = |n: &str, ctx: &Ctx| {
        ctx.input.types.contains_key(n).then(|| {
            tys.intern(Ty::Named {
                name: n.to_string(),
                kind: NamedKind::Sum,
                args: Vec::new(),
            })
        })
    };
    let (Some(value_ty), Some(read_err)) = (named("SqlValue", ctx), named("SqlReadError", ctx))
    else {
        ctx.errors.push(CompileError::new(
            "bynk.capability.unknown_operation",
            span,
            format!("`{name}` needs `bynk.cloudflare`'s `SqlValue` and `SqlReadError` in scope"),
        ));
        return bail(ctx);
    };
    let mut ok = true;
    let as_list = matches!(&*tys.get(t), Ty::List(_));
    let shape = if as_list {
        Err(format!(
            "`{}` is not a row record; `{}` already returns a `List` of rows",
            t.display(tys),
            method.name
        ))
    } else {
        check_row_shape(t, &ctx.input.types, tys).and_then(|()| match &*tys.get(t) {
            Ty::Named { name, .. } if select_columns(name, &ctx.input.types).is_empty() => {
                Err(format!("`{name}` has no fields to select"))
            }
            _ => Ok(()),
        })
    };
    if let Err(why) = shape {
        ctx.errors.push(
            CompileError::new(
                "bynk.types.sql_row_shape",
                type_args[0].span(),
                format!("`{name}` reads SQL rows, but {why}"),
            )
            .with_note(
                "the row is a record whose fields are `Int`, `Float`, `String`, `Bool` or \
                 `Bytes` (or a refined or opaque type over one, or an `Option` of either)",
            ),
        );
        ok = false;
    }
    let mut sql = None;
    let tail_text = sql_literal_text(&args[0], ctx);
    match tail_text.as_ref() {
        Some(tail) => {
            // A constant tail is read at compile time (the composed SQL is
            // recorded below); still type the name so hover and references
            // see it.
            if matches!(args[0].kind, ExprKind::Ident(_)) {
                let _ = type_of(&args[0], None, ctx);
            }
            if begins_with_select(tail) {
                ctx.errors.push(
                    CompileError::new(
                        "bynk.types.select_tail_has_select",
                        args[0].span,
                        format!("the SQL of `{name}` must start at `FROM`, not `SELECT`"),
                    )
                    .with_note(
                        "the compiler writes `SELECT` and the column list from the row type; \
                         use `all` and `Json.decodeSql` to spell the columns yourself",
                    ),
                );
                ok = false;
            } else if begins_without_from(tail) {
                ctx.errors.push(
                    CompileError::new(
                        "bynk.types.select_tail_has_select",
                        args[0].span,
                        format!("the SQL of `{name}` must start at `FROM`"),
                    )
                    .with_note(
                        "the compiler writes `SELECT` and the column list from the row type, \
                         so the literal continues from `FROM`; use `all` and `Json.decodeSql` \
                         for a `WITH` or parenthesised query",
                    ),
                );
                ok = false;
            } else if ok && let Ty::Named { name: row, .. } = &*tys.get(t) {
                let cols = select_columns(row, &ctx.input.types);
                sql = Some(format!("SELECT {cols} {}", tail.trim_start()));
            }
        }
        None => {
            ctx.errors.push(sql_not_literal(
                args[0].span,
                &format!("the SQL of `{cap}.{}`", method.name),
            ));
            // The tail is not an ordinary argument here; still type it for hover.
            let _ = type_of(&args[0], None, ctx);
            ok = false;
        }
    }
    let params_ty = tys.intern(Ty::List(value_ty));
    if let Some(actual) = type_of(&args[1], Some(params_ty), ctx)
        && !compatible(actual, params_ty, tys)
    {
        ctx.errors.push(CompileError::new(
            "bynk.types.argument_mismatch",
            args[1].span,
            format!(
                "argument 2 to capability `{name}` has type `{}`, but parameter expects `{}`",
                actual.display(tys),
                params_ty.display(tys)
            ),
        ));
    }
    let first = method.name == "firstAs";
    let sql = sql.filter(|_| ok)?;
    if let Ty::Named { name: row, .. } = &*tys.get(t)
        && let Some(TypeBody::Record(record)) = ctx.input.types.get(row).map(|d| &d.body)
    {
        ctx.callees.insert(
            args[0].id,
            Callee::D1Sql(D1SqlSite {
                cap: cap.to_string(),
                op: method.name.clone(),
                text: sql.clone(),
                span: args[0].span,
                row: Some((
                    row.clone(),
                    record.fields.iter().map(|f| f.name.name.clone()).collect(),
                )),
                passed: literal_list_len(args.get(1), ctx),
            }),
        );
    }
    ctx.callees.insert(
        expr_id,
        Callee::D1Select {
            cap: cap.to_string(),
            row: t,
            sql,
            first,
        },
    );
    let rows = if first {
        tys.intern(Ty::Option(t))
    } else {
        tys.intern(Ty::List(t))
    };
    Some(tys.intern(Ty::Effect(tys.intern(Ty::Result(rows, read_err)))))
}

/// `Json.decodeSql[T]`: `T` must be a row record, or a `List` of one. `Err`
/// carries the reason, for `bynk.types.sql_row_shape`.
pub(crate) fn check_row_shape(
    t: TyId,
    types: &HashMap<String, Arc<TypeDecl>>,
    tys: &Types,
) -> Result<(), String> {
    let row = match &*tys.get(t) {
        Ty::List(r) => *r,
        _ => t,
    };
    let (name, args) = match &*tys.get(row) {
        Ty::Named {
            name,
            kind: NamedKind::Record,
            args,
        } => (name.clone(), args.clone()),
        _ => {
            return Err(format!(
                "`{}` is not a row record or a `List` of one",
                row.display(tys)
            ));
        }
    };
    if !args.is_empty() {
        return Err(format!("`{name}` is generic; a row record is not"));
    }
    let Some(TypeBody::Record(record)) = types.get(&name).map(|d| &d.body) else {
        return Err(format!("`{name}` is not a row record"));
    };
    for f in &record.fields {
        if !is_column(&f.type_ref, types) {
            return Err(format!(
                "field `{}` of `{name}` is not a column type",
                f.name.name
            ));
        }
    }
    Ok(())
}

/// A type a SQLite column decodes to: a scalar (`Bytes` from base64), a
/// refined or opaque type over one, or an `Option` of either.
fn is_column(r: &TypeRef, types: &HashMap<String, Arc<TypeDecl>>) -> bool {
    match r {
        TypeRef::Option(inner, _) => {
            !matches!(**inner, TypeRef::Option(..)) && is_column(inner, types)
        }
        TypeRef::Base(b, _) => is_column_base(*b),
        TypeRef::Named(id) => types.get(&id.name).is_some_and(|d| match &d.body {
            TypeBody::Refined { base, .. } | TypeBody::Opaque { base, .. } => is_column_base(*base),
            _ => false,
        }),
        _ => false,
    }
}

fn is_column_base(b: BaseType) -> bool {
    !matches!(b, BaseType::Duration | BaseType::Instant)
}

/// The names of `T`'s row fields that decode as `Bool` (directly, through an
/// `Option`, or under a refined or opaque type), for `Json.decodeSql`'s pass
/// over SQLite's `0`/`1`. `T` has passed `check_row_shape`.
pub fn row_bool_fields(row: &str, types: &HashMap<String, Arc<TypeDecl>>) -> Vec<String> {
    fn is_bool(r: &TypeRef, types: &HashMap<String, Arc<TypeDecl>>) -> bool {
        match r {
            TypeRef::Option(inner, _) => is_bool(inner, types),
            TypeRef::Base(b, _) => *b == BaseType::Bool,
            TypeRef::Named(id) => types.get(&id.name).is_some_and(|d| match &d.body {
                TypeBody::Refined { base, .. } | TypeBody::Opaque { base, .. } => {
                    *base == BaseType::Bool
                }
                _ => false,
            }),
            _ => false,
        }
    }
    match types.get(row).map(|d| &d.body) {
        Some(TypeBody::Record(record)) => record
            .fields
            .iter()
            .filter(|f| is_bool(&f.type_ref, types))
            .map(|f| f.name.name.clone())
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tail_tests {
    use super::*;

    #[test]
    fn select_prefix_is_boundary_safe() {
        assert!(begins_with_select("select * from t"));
        assert!(begins_with_select("  SELECT x"));
        assert!(!begins_with_select("selected_x from t"));
        assert!(!begins_with_select("éé"));
        assert!(!begins_with_select("sélect"));
    }

    #[test]
    fn tail_must_start_at_from() {
        assert!(!begins_without_from("FROM t"));
        assert!(!begins_without_from("  from t"));
        assert!(begins_without_from("WITH x AS (SELECT 1) SELECT * FROM x"));
        assert!(begins_without_from("(SELECT 1)"));
        assert!(begins_without_from("fromage"));
        assert!(begins_without_from("éé"));
    }
}

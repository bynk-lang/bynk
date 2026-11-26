//! #4105: the checker's half of `Analytics.writeValue[T]`.
//!
//! A record type `T` is the data point's schema: the compiler computes the
//! field-to-column mapping here, once, and the emitter lowers the call over
//! the untyped `write(DataPoint)` from it. Text-like fields map to `blob1…`,
//! numeric fields to `double1…`, both in declaration order; at most one
//! `@index` `String` field maps to `index1`. Past a column family's twenty, an
//! `@index` that is not a `String`, or a field type with no column is a
//! compile error naming the field.

use bynk_syntax::ast::RecordField;

use super::*;

/// A platform limit of `DataPoint` (see `bynk.cloudflare`): at most this many
/// blob columns, and as many double columns.
const MAX_COLUMNS: usize = 20;

/// How a field's value becomes its column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnKind {
    /// A `String` or a refined or opaque `String`: written as it is.
    Text,
    /// A payload-less enum: written as the variant's name.
    Variant,
    /// A `Bool`: written as `"true"` or `"false"`.
    Flag,
    /// An `Int`, `Float`, `Duration` (milliseconds) or `Instant` (epoch
    /// milliseconds), or a refined or opaque one: written as the number.
    Number,
    /// A `Decimal`: written as its float value.
    Decimal,
}

/// One field's column: which field, and how its value is converted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnField {
    pub field: String,
    pub kind: ColumnKind,
}

/// The mapping a record type `T` gets: the `@index` field, then the blob and
/// double fields, each in declaration order (column `n` is entry `n - 1`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AnalyticsMapping {
    pub index: Option<String>,
    pub blobs: Vec<ColumnField>,
    pub doubles: Vec<ColumnField>,
}

/// Whether the derived instance is a `bynk.cloudflare.Analytics` one and `method` its typed write.
pub(crate) fn is_write_value(derived_from: Option<&(String, String)>, method: &str) -> bool {
    method == "writeValue"
        && matches!(derived_from, Some((unit, base)) if unit == crate::firstparty::CLOUDFLARE_UNIT && base == "Analytics")
}

/// The column a field of this type is written to, `None` for a type with no column.
fn column_kind(r: &TypeRef, types: &HashMap<String, Arc<TypeDecl>>) -> Option<ColumnKind> {
    let of_base = |b: BaseType| match b {
        BaseType::String => Some(ColumnKind::Text),
        BaseType::Bool => Some(ColumnKind::Flag),
        BaseType::Int | BaseType::Float | BaseType::Duration | BaseType::Instant => {
            Some(ColumnKind::Number)
        }
        BaseType::Decimal => Some(ColumnKind::Decimal),
        _ => None,
    };
    match r {
        TypeRef::Base(b, _) => of_base(*b),
        TypeRef::Named(id) => match &types.get(&id.name)?.body {
            TypeBody::Refined { base, .. } | TypeBody::Opaque { base, .. } => of_base(*base),
            TypeBody::Sum(s) if s.variants.iter().all(|v| v.payload.is_empty()) => {
                Some(ColumnKind::Variant)
            }
            _ => None,
        },
        _ => None,
    }
}

/// Whether a field is an `@index` one, and whether its annotation is the bare `@index`.
fn index_annotation(f: &RecordField) -> Option<&bynk_syntax::ast::Annotation> {
    f.annotations.iter().find(|a| a.name.name == "index")
}

/// The mapping of the record `name`, or the compile errors that refuse it (each names the field).
pub fn mapping_of(
    name: &str,
    types: &HashMap<String, Arc<TypeDecl>>,
) -> Result<AnalyticsMapping, Vec<(Span, String)>> {
    let Some(TypeBody::Record(record)) = types.get(name).map(|d| &d.body) else {
        return Err(vec![(
            types.get(name).map_or(Span::default(), |d| d.span),
            format!("`{name}` is not a record, and a data point's columns are its fields"),
        )]);
    };
    let mut mapping = AnalyticsMapping::default();
    let mut errors = Vec::new();
    for f in &record.fields {
        let field = f.name.name.clone();
        if let Some(ann) = index_annotation(f) {
            let is_text = column_kind(&f.type_ref, types) == Some(ColumnKind::Text);
            if !is_text {
                errors.push((
                    ann.span,
                    format!("`@index` field `{field}` of `{name}` is not a `String`: the sampling key is text"),
                ));
            } else if let Some(first) = &mapping.index {
                errors.push((
                    ann.span,
                    format!("`{name}` has two `@index` fields, `{first}` and `{field}`: a data point has one sampling key"),
                ));
            } else {
                mapping.index = Some(field);
            }
            continue;
        }
        match column_kind(&f.type_ref, types) {
            Some(kind @ (ColumnKind::Text | ColumnKind::Variant | ColumnKind::Flag)) => {
                mapping.blobs.push(ColumnField { field, kind });
            }
            Some(kind @ (ColumnKind::Number | ColumnKind::Decimal)) => {
                mapping.doubles.push(ColumnField { field, kind });
            }
            None => errors.push((
                f.type_ref.span(),
                format!("field `{field}` of `{name}` has no Analytics column: a data point holds `String`, payload-less enum, `Bool`, `Int`, `Float`, `Decimal`, `Duration` and `Instant` fields (or a refined or opaque type over one)"),
            )),
        }
    }
    for (columns, family, over) in [
        (&mapping.blobs, "blob", mapping.blobs.get(MAX_COLUMNS)),
        (&mapping.doubles, "double", mapping.doubles.get(MAX_COLUMNS)),
    ] {
        if let Some(extra) = over
            && let Some(f) = record.fields.iter().find(|f| f.name.name == extra.field)
        {
            errors.push((
                f.name.span,
                format!(
                    "field `{}` of `{name}` would be `{family}{}`, past the {MAX_COLUMNS} {family} columns a data point has (`{name}` has {} {family} fields)",
                    extra.field,
                    MAX_COLUMNS + 1,
                    columns.len()
                ),
            ));
        }
    }
    if errors.is_empty() {
        Ok(mapping)
    } else {
        Err(errors)
    }
}

/// `Cap.writeValue[T](value)` on a derived `Analytics` instance: `T` is a record, the argument a
/// `T`. Records the lowering (`Callee::AnalyticsWrite`, the mapping computed from `T`) and types the
/// call `Effect[()]`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn check_write_value(
    cap: &str,
    method: &Ident,
    type_args: &[TypeRef],
    args: &[Expr],
    span: Span,
    expr_id: ExprId,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    super::calls::refuse_experimental("bynk.cloudflare.Analytics.writeValue", method.span, ctx);
    let name = format!("{cap}.{}", method.name);
    let bail = |ctx: &mut Ctx| {
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        None
    };
    if type_args.len() > 1 {
        ctx.errors.push(CompileError::new(
            "bynk.generics.type_arg_mismatch",
            span,
            format!(
                "`{name}` takes at most one type argument (the data point's record type), got {}",
                type_args.len()
            ),
        ));
        return bail(ctx);
    }
    if args.len() != 1 {
        ctx.errors.push(CompileError::new(
            "bynk.capability.op_arity",
            span,
            format!(
                "capability operation `{name}` expects 1 argument(s), but {} were given",
                args.len()
            ),
        ));
        return bail(ctx);
    }
    // The record is written, or inferred from the argument (typed once either way).
    let (t, t_span, actual) = match type_args.first() {
        Some(written) => {
            let Some(t) = resolve_expr_type_ref(written, ctx) else {
                return bail(ctx);
            };
            (t, written.span(), type_of(&args[0], Some(t), ctx))
        }
        None => {
            let actual = type_of(&args[0], None, ctx)?;
            (actual, args[0].span, Some(actual))
        }
    };
    let record = match &*tys.get(t) {
        Ty::Named {
            name: record,
            kind: NamedKind::Record,
            args,
        } if args.is_empty() => Some(record.clone()),
        _ => None,
    };
    let mapping = match &record {
        Some(record) => mapping_of(record, &ctx.input.types),
        None => Err(vec![(
            t_span,
            format!(
                "`{}` is not a record type, and a data point's columns are a record's fields",
                t.display(tys)
            ),
        )]),
    };
    let mapping = match mapping {
        Ok(m) => Some(m),
        Err(errors) => {
            for (at, why) in errors {
                ctx.errors.push(
                    CompileError::new(
                        "bynk.types.analytics_point_shape",
                        at,
                        format!("`{name}` writes a record as a data point, but {why}"),
                    )
                    .with_note(
                        "text-like fields (`String`, a payload-less enum, `Bool`) are blob columns, numeric ones (`Int`, `Float`, `Decimal`, `Duration`, `Instant`) double columns, at most twenty of each; mark one `String` field `@index` for the sampling key; use `write` for a point that is not a record",
                    ),
                );
            }
            None
        }
    };
    let actual = actual?;
    if !compatible(actual, t, tys) {
        ctx.errors.push(CompileError::new(
            "bynk.types.argument_mismatch",
            args[0].span,
            format!(
                "argument 1 to capability `{name}` has type `{}`, but parameter expects `{}`",
                actual.display(tys),
                t.display(tys)
            ),
        ));
        return None;
    }
    let mapping = mapping?;
    let record_name = record?;
    ctx.callees.insert(
        expr_id,
        Callee::AnalyticsWrite {
            cap: cap.to_string(),
            record: record_name,
            mapping,
        },
    );
    let unit = tys.intern(Ty::Unit);
    Some(tys.intern(Ty::Effect(unit)))
}

/// Whether this call is `AnalyticsSql.select` of `bynk.cloudflare`, found by the unit the name
/// was consumed from, so a user capability of the same name is untouched.
pub(crate) fn is_select(cap: &str, method: &str, ctx: &Ctx) -> bool {
    cap == "AnalyticsSql"
        && method == "select"
        && ctx
            .input
            .cross_context
            .flattened_caps
            .get(cap)
            .is_some_and(|unit| unit == crate::firstparty::CLOUDFLARE_UNIT)
}

/// The names a select tail defines: each `AS <name>` or `AS "<name>"` before its first `FROM`
/// word that follows a comma-continued list (a tail that starts at `FROM` defines none).
fn tail_aliases(tail: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut words = tail
        .split(|c: char| c.is_whitespace() || c == ',' || c == '(' || c == ')')
        .filter(|w| !w.is_empty());
    while let Some(word) = words.next() {
        if word.eq_ignore_ascii_case("from") {
            break;
        }
        if word.eq_ignore_ascii_case("as")
            && let Some(name) = words.next()
        {
            found.push(name.trim_matches('"').to_string());
        }
    }
    found
}

/// Whether the tail has a `FROM` word at all (outside quotes is not decided here; the platform
/// refuses SQL that does not parse).
fn has_from(tail: &str) -> bool {
    tail.split(|c: char| c.is_whitespace() || c == ',' || c == '(' || c == ')')
        .any(|w| w.eq_ignore_ascii_case("from"))
}

/// `AnalyticsSql.select[T](Instance, tail, params)`: the instance is named first (as for
/// `query`), `T` is a record, the tail a literal that starts at `FROM` or at a comma continuing
/// the select list. Records the lowering ([`Callee::AnalyticsSelect`], its SQL filled by
/// [`resolve_selects`] once the instance's written record is known) and types the call
/// `Effect[Result[List[T], AnalyticsReadError]]`.
pub(crate) fn check_select(
    cap: &str,
    method: &Ident,
    type_args: &[TypeRef],
    args: &[Expr],
    span: Span,
    expr_id: ExprId,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    super::calls::refuse_experimental("bynk.cloudflare.AnalyticsSql.select", method.span, ctx);
    let name = format!("{cap}.{}", method.name);
    // What the parameters are typed against: the first-party `SqlValue` list, as `D1`'s are.
    let params_ty = ctx.input.types.contains_key("SqlValue").then(|| {
        let value_ty = tys.intern(Ty::Named {
            name: "SqlValue".to_string(),
            kind: NamedKind::Sum,
            args: Vec::new(),
        });
        tys.intern(Ty::List(value_ty))
    });
    let bail = |ctx: &mut Ctx| {
        for a in args.iter().skip(2) {
            let _ = type_of(a, params_ty, ctx);
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
    if args.len() != 3 {
        ctx.errors.push(
            CompileError::new(
                "bynk.capability.op_arity",
                span,
                format!(
                    "capability operation `{name}` expects 3 argument(s), but {} were given",
                    args.len()
                ),
            )
            .with_note(
                "pass the `Analytics` instance, the SQL tail and the parameters: `AnalyticsSql.select[Row](Signups, \"FROM {dataset} …\", params)`",
            ),
        );
        return bail(ctx);
    }
    // The instance and the literal tail, checked as for `query`.
    super::d1::check_analytics_sql_query("select", args, ctx);
    let instance = match &args[0].kind {
        ExprKind::Ident(id) => Some(id.name.clone()),
        _ => None,
    };
    let t = resolve_expr_type_ref(&type_args[0], ctx);
    let value_ty = ctx.input.types.contains_key("SqlValue").then(|| {
        tys.intern(Ty::Named {
            name: "SqlValue".to_string(),
            kind: NamedKind::Sum,
            args: Vec::new(),
        })
    });
    let read_err = ctx.input.types.contains_key("AnalyticsReadError").then(|| {
        tys.intern(Ty::Named {
            name: "AnalyticsReadError".to_string(),
            kind: NamedKind::Sum,
            args: Vec::new(),
        })
    });
    let (Some(t), Some(value_ty), Some(read_err)) = (t, value_ty, read_err) else {
        return bail(ctx);
    };
    // The params, typed as `D1`'s are.
    let params_ty = tys.intern(Ty::List(value_ty));
    if let Some(actual) = type_of(&args[2], Some(params_ty), ctx)
        && !compatible(actual, params_ty, tys)
    {
        ctx.errors.push(CompileError::new(
            "bynk.types.argument_mismatch",
            args[2].span,
            format!(
                "argument 3 to capability `{name}` has type `{}`, but parameter expects `{}`",
                actual.display(tys),
                params_ty.display(tys)
            ),
        ));
    }
    let record = match &*tys.get(t) {
        Ty::Named {
            name: record,
            kind: NamedKind::Record,
            args,
        } if args.is_empty() => Some(record.clone()),
        _ => None,
    };
    let Some(record) = record else {
        ctx.errors.push(
            CompileError::new(
                "bynk.types.sql_row_shape",
                type_args[0].span(),
                format!(
                    "`{name}` reads each row into a record, but `{}` is not one",
                    t.display(tys)
                ),
            )
            .with_note("`T` is a record whose fields are text or numbers; `select` already returns a `List` of rows"),
        );
        return bail(ctx);
    };
    let tail = super::d1::sql_literal_text(&args[1], ctx)?;
    if !tail.trim_start().starts_with(',') && super::d1::begins_without_from(&tail) {
        ctx.errors.push(
            CompileError::new(
                "bynk.types.select_tail_has_select",
                args[1].span,
                format!("the SQL of `{name}` must start at `FROM`, or at a comma that continues the select list"),
            )
            .with_note(
                "the compiler writes `SELECT` and the columns from the row type; add what it cannot know after a comma: `\", count() AS n FROM {dataset} …\"`",
            ),
        );
        return bail(ctx);
    }
    if !has_from(&tail) {
        ctx.errors.push(CompileError::new(
            "bynk.types.select_tail_has_select",
            args[1].span,
            format!("the SQL of `{name}` has no `FROM`"),
        ));
        return bail(ctx);
    }
    let _ = record;
    let instance = instance?;
    let rows = tys.intern(Ty::List(t));
    ctx.callees.insert(
        expr_id,
        Callee::AnalyticsSelect {
            cap: instance,
            row: t,
            aliases: tail_aliases(&tail),
            tail,
            sql: None,
            span: args[1].span,
        },
    );
    let result = tys.intern(Ty::Result(rows, read_err));
    Some(tys.intern(Ty::Effect(result)))
}

/// The column name a mapped field reads: `index1`, `blobN` or `doubleN`.
fn column_of(mapping: &AnalyticsMapping, field: &str) -> Option<(String, ColumnKind)> {
    if mapping.index.as_deref() == Some(field) {
        return Some(("index1".to_string(), ColumnKind::Text));
    }
    let find = |columns: &[ColumnField], family: &str| {
        columns
            .iter()
            .position(|c| c.field == field)
            .map(|i| (format!("{family}{}", i + 1), columns[i].kind))
    };
    find(&mapping.blobs, "blob").or_else(|| find(&mapping.doubles, "double"))
}

/// How a read field's type reads a column: text, or a number (an `Option` of either reads the
/// same). `None` for a type no Analytics column reads into.
fn read_kind(r: &TypeRef, types: &HashMap<String, Arc<TypeDecl>>) -> Option<ColumnKind> {
    match r {
        TypeRef::Option(inner, _) => read_kind(inner, types),
        _ => match column_kind(r, types)? {
            ColumnKind::Text => Some(ColumnKind::Text),
            ColumnKind::Number => Some(ColumnKind::Number),
            _ => None,
        },
    }
}

/// After a unit is checked: write each `AnalyticsSql.select[T]`'s `SELECT` list from the record
/// its instance writes (`Callee::AnalyticsWrite`, found anywhere in the unit), or refuse it. Each
/// read field is a column of the written record, or an alias the tail defines; a text field reads
/// a blob (or the index) and a number field a double.
pub(crate) fn resolve_selects(
    callees: &mut HashMap<ExprId, Callee>,
    types: &HashMap<String, Arc<TypeDecl>>,
    tys: &Types,
    errors: &mut Vec<CompileError>,
) {
    let mut written: HashMap<String, std::collections::BTreeSet<String>> = HashMap::new();
    for callee in callees.values() {
        if let Callee::AnalyticsWrite { cap, record, .. } = callee {
            written
                .entry(cap.clone())
                .or_default()
                .insert(record.clone());
        }
    }
    let mut selects: Vec<(ExprId, Span)> = callees
        .iter()
        .filter_map(|(id, c)| match c {
            Callee::AnalyticsSelect { span, .. } => Some((*id, *span)),
            _ => None,
        })
        .collect();
    selects.sort_by_key(|(_, span)| (span.file, span.start));
    for (id, span) in selects {
        let Some(Callee::AnalyticsSelect {
            cap,
            row,
            tail,
            aliases,
            ..
        }) = callees.get(&id).cloned()
        else {
            continue;
        };
        let Ty::Named { name: row_name, .. } = &*tys.get(row) else {
            continue;
        };
        let Some(TypeBody::Record(read)) = types.get(row_name).map(|d| &d.body) else {
            continue;
        };
        let records = written.get(&cap);
        if records.is_some_and(|r| r.len() > 1) {
            let names: Vec<&String> = records.into_iter().flatten().collect();
            errors.push(
                CompileError::new(
                    "bynk.types.argument_mismatch",
                    span,
                    format!(
                        "`{cap}` is written with more than one record type ({}), so a typed read cannot tell which columns it holds",
                        names.iter().map(|n| format!("`{n}`")).collect::<Vec<_>>().join(", ")
                    ),
                )
                .with_note("write one record type to an `Analytics` instance, or read it with `AnalyticsSql.query`"),
            );
            continue;
        }
        let mapping = records
            .and_then(|r| r.iter().next())
            .and_then(|record| mapping_of(record, types).ok());
        let mut columns: Vec<String> = Vec::new();
        let mut ok = true;
        for f in &read.fields {
            let field = &f.name.name;
            let want = read_kind(&f.type_ref, types);
            let Some(want) = want else {
                errors.push(
                    CompileError::new(
                        "bynk.types.sql_row_shape",
                        f.type_ref.span(),
                        format!("field `{field}` of `{row_name}` has a type no Analytics column reads into"),
                    )
                    .with_note("a read field is text (`String`, or a refined or opaque one) or a number (`Int`, `Float`, `Duration`, `Instant`), or an `Option` of either; a `Bool` or an enum is stored as text, so read it as `String`"),
                );
                ok = false;
                continue;
            };
            if let Some((column, have)) = mapping.as_ref().and_then(|m| column_of(m, field)) {
                let numeric = matches!(have, ColumnKind::Number | ColumnKind::Decimal);
                if numeric != (want == ColumnKind::Number) {
                    errors.push(
                        CompileError::new(
                            "bynk.types.sql_row_shape",
                            f.type_ref.span(),
                            format!(
                                "field `{field}` of `{row_name}` is read as {}, but `{cap}` writes it as {} (`{column}`)",
                                if want == ColumnKind::Number { "a number" } else { "text" },
                                if numeric { "a number" } else { "text" },
                            ),
                        )
                        .with_note("read a number column as `Int`, `Float`, `Duration` or `Instant`, and a text column as `String`"),
                    );
                    ok = false;
                    continue;
                }
                columns.push(format!("{column} AS \"{field}\""));
            } else if !aliases.iter().any(|a| a == field) {
                errors.push(
                    CompileError::new(
                        "bynk.types.sql_row_shape",
                        f.name.span,
                        match records {
                            Some(r) => format!(
                                "field `{field}` of `{row_name}` is neither a field of `{}`, the record `{cap}` is written with, nor an alias of the select list",
                                r.iter().next().map(String::as_str).unwrap_or("")
                            ),
                            None => format!(
                                "no written type for `{cap}` in this context: it is never written with `writeValue`, so field `{field}` of `{row_name}` has no column, and it is not an alias of the select list either"
                            ),
                        },
                    )
                    .with_note("name a field of the written record, or define the field in the tail: `\", count() AS n FROM {dataset} …\"`"),
                );
                ok = false;
            }
        }
        if !ok {
            continue;
        }
        let list = columns.join(", ");
        let rest = tail.trim_start();
        let sql = match (list.is_empty(), rest.strip_prefix(',')) {
            (true, Some(extras)) => format!("SELECT {}", extras.trim_start()),
            (true, None) => format!("SELECT * {rest}"),
            (false, Some(_)) => format!("SELECT {list}{rest}"),
            (false, None) => format!("SELECT {list} {rest}"),
        };
        if let Some(Callee::AnalyticsSelect { sql: slot, .. }) = callees.get_mut(&id) {
            *slot = Some(sql);
        }
    }
}

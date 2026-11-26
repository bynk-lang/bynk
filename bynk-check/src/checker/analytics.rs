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
    // The record is written, or inferred from the argument.
    let (t, t_span) = match type_args.first() {
        Some(written) => (resolve_expr_type_ref(written, ctx)?, written.span()),
        None => (type_of(&args[0], None, ctx)?, args[0].span),
    };
    let record = match &*tys.get(t) {
        Ty::Named {
            name: record,
            kind: NamedKind::Record,
            args,
        } if args.is_empty() => Some(record.clone()),
        _ => None,
    };
    let mapping = match record {
        Some(record) => mapping_of(&record, &ctx.input.types),
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
    let actual = type_of(&args[0], Some(t), ctx)?;
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
    ctx.callees.insert(
        expr_id,
        Callee::AnalyticsWrite {
            cap: cap.to_string(),
            mapping,
        },
    );
    let unit = tys.intern(Ty::Unit);
    Some(tys.intern(Ty::Effect(unit)))
}

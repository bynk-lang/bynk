use std::sync::Arc;

/// #2607: why a field default has no wire (or in-memory) form. A default that
/// reaches a name no type declares is kept apart, because in a generic record
/// that name is a type parameter, and the checker says so rather than relaying
/// the message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefaultReason {
    /// The default reaches a type named by nothing in scope (in a generic
    /// record, its type parameter).
    UnresolvedType(String),
    /// Any other reason, as a message.
    Invalid(String),
}

impl std::fmt::Display for DefaultReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DefaultReason::UnresolvedType(name) => write!(f, "cannot resolve type `{name}`"),
            DefaultReason::Invalid(message) => f.write_str(message),
        }
    }
}

impl From<String> for DefaultReason {
    fn from(message: String) -> Self {
        DefaultReason::Invalid(message)
    }
}

impl From<&str> for DefaultReason {
    fn from(message: &str) -> Self {
        DefaultReason::Invalid(message.to_string())
    }
}

use bynk_syntax::ast::{
    BaseType, Expr, ExprKind, RecordBody, SumBody, TypeBody, TypeDecl, TypeRef, UnaryOp,
};

/// Events slice 3a (#972): the **wire-form** JSON literal an event field's
/// default deserialises from when its key is absent from the incoming JSON.
/// Type-directed — given the field's expected `TypeRef` and the visible
/// `types` table, every syntactic ambiguity (a bare `Ident` that is really a
/// sum variant, a `FieldAccess` that is a qualified nullary variant) resolves
/// the same way the checker resolves it, just narrower (a bare name must
/// match a variant of *this* expected sum specifically) — so this needs no
/// per-expression `Ty` map (`expr_types`), which a subscriber regenerating a
/// publisher's own event codec (`emit_consumed_context_helpers`) has no way
/// to obtain anyway (no cross-unit `expr_types` store exists).
///
/// Produces the value in its **wire** shape (`kind` discriminant for a sum,
/// matching `emit_sum_codec`'s generated `Option`/`Result` instantiations
/// exactly — never the in-memory `tag` discriminant, and never a qualified
/// reference like `Region.Domestic` into a value namespace the emitting
/// module may not import), so it can be spliced in as the field's raw JSON
/// access and re-enter `emit_field_deserialise` (`bynk-emit`) completely
/// unchanged.
///
/// `Err(reason)` for anything not closed-form. The caller
/// (`bynk-emit/src/project/validate.rs`'s event-field-default check) turns
/// that into `bynk.types.bad_field_default` at check time, so a value this
/// function cannot build should never reach `emit_record` in practice — the
/// `.ok()` fallback there is a non-panicking safety net, not the intended
/// rejection path.
pub fn lower_field_default_wire(
    init: &Expr,
    expected: &TypeRef,
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
) -> Result<String, DefaultReason> {
    lower_field_default_wire_in(init, expected, types, &[])
}

/// The deepest chain of generic instantiations a default may expand through.
/// A default that names its own type (`type Tree[T] = { kid: Tree[T] = Tree {} }`)
/// would otherwise render forever (#2690).
const MAX_DEFAULT_DEPTH: usize = 16;

/// Where a default is being rendered: the type parameters of the declaration it
/// belongs to (#2690), which stand for no value whatever `types` holds under the
/// same name, and how deep the instantiations nest.
#[derive(Clone, Copy)]
struct Cx<'a> {
    params: &'a [String],
    depth: usize,
    /// Whether a field that applies a user generic may carry a default. A record
    /// field may (#2690); an event field and an old-state field may not.
    generics: bool,
}

impl<'a> Cx<'a> {
    fn deeper(self) -> Result<Self, DefaultReason> {
        if self.depth >= MAX_DEFAULT_DEPTH {
            return Err(DefaultReason::from(
                "the default nests generic instantiations too deeply".to_string(),
            ));
        }
        Ok(Cx {
            depth: self.depth + 1,
            ..self
        })
    }
}

/// [`lower_field_default_wire`] for a field of a declaration whose type
/// parameters are `params` (#2690): a default that needs a value of one of them
/// is [`DefaultReason::UnresolvedType`], even when a type of that name is
/// declared.
pub fn lower_field_default_wire_in(
    init: &Expr,
    expected: &TypeRef,
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
    params: &[String],
) -> Result<String, DefaultReason> {
    wire_in(
        init,
        expected,
        types,
        Cx {
            params,
            depth: 0,
            generics: true,
        },
    )
}

/// [`lower_field_default_wire`] for an event field or an old-state field, whose
/// default may not apply a user generic anywhere in it (#2690 widened record
/// fields only; the event codec and the migration read the default with no
/// diagnostic path of their own).
pub fn lower_event_field_default_wire(
    init: &Expr,
    expected: &TypeRef,
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
) -> Result<String, DefaultReason> {
    wire_in(
        init,
        expected,
        types,
        Cx {
            params: &[],
            depth: 0,
            generics: false,
        },
    )
}

fn wire_in(
    init: &Expr,
    expected: &TypeRef,
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
    cx: Cx<'_>,
) -> Result<String, DefaultReason> {
    if let ExprKind::Paren(inner) = &init.kind {
        return wire_in(inner, expected, types, cx);
    }
    match expected {
        TypeRef::Base(b, _) => lower_base_literal(*b, init),
        TypeRef::Named(id) => {
            if cx.params.contains(&id.name) {
                return Err(DefaultReason::UnresolvedType(id.name.clone()));
            }
            let Some(decl) = types.get(&id.name) else {
                return Err(DefaultReason::UnresolvedType(id.name.clone()));
            };
            match &decl.body {
                TypeBody::Refined { base, .. } => lower_base_literal(*base, init),
                // ADR 0182: `.unsafe`'s value is an identity cast (`return
                // value as T;`, `emit.rs`'s opaque emission) — the literal
                // itself, verbatim, is the wire form. (Its refinement is
                // additionally checked *statically* for an event field
                // default specifically — `check_event_field_default` — since
                // this literal re-enters the same codec a real wire value
                // would, unlike an ordinary `.unsafe` bypass.)
                TypeBody::Opaque { base, .. } => match qualified_call(init) {
                    Some((type_name, "unsafe", [lit])) if type_name == id.name => {
                        lower_base_literal(*base, lit)
                    }
                    Some((_, "unsafe", _)) => Err(DefaultReason::from(
                        "`.unsafe` takes exactly one argument".to_string(),
                    )),
                    _ => Err(DefaultReason::from(format!(
                        "an opaque type's default must be `{}.unsafe(<literal>)`",
                        id.name
                    ))),
                },
                TypeBody::Sum(s) => lower_sum_default(&id.name, &body_variants(s), init, types, cx),
                TypeBody::Record(r) => lower_record_default(&body_fields(r), init, types, cx),
            }
        }
        // #2690: a field typed by a generic record or sum. The declaration's
        // fields are read at the instantiation, so a default is a closed-form
        // value of `Paginated[String]`; an argument that is the enclosing
        // declaration's own parameter stays unresolved, so only a value with no
        // content of that type qualifies.
        TypeRef::App { name, args, .. } => {
            if !cx.generics {
                return Err(DefaultReason::from(
                    "only a record field's default may apply a generic record or sum".to_string(),
                ));
            }
            let cx = cx.deeper()?;
            match types.get(&name.name).map(|d| &d.body) {
                Some(TypeBody::Record(_)) => {
                    let fields = inst_record_fields(&name.name, args, types)?;
                    lower_record_default(&fields, init, types, cx)
                }
                Some(TypeBody::Sum(_)) => {
                    let variants = crate::wire::sum_inst_variants(&name.name, args, types)
                        .ok_or_else(|| {
                            DefaultReason::from("a generic sum's arity does not match".to_string())
                        })?;
                    lower_sum_default(&name.name, &variants, init, types, cx)
                }
                _ => Err(DefaultReason::UnresolvedType(name.name.clone())),
            }
        }
        TypeRef::Option(inner, _) => match &init.kind {
            ExprKind::None => Ok("{ kind: \"None\" }".to_string()),
            ExprKind::Some(e) => {
                let v = wire_in(e, inner, types, cx)?;
                Ok(format!("{{ kind: \"Some\", value: {v} }}"))
            }
            _ => Err(DefaultReason::from(
                "an `Option` field default must be `Some(...)` or `None`".to_string(),
            )),
        },
        TypeRef::Result(ok, err, _) => match &init.kind {
            ExprKind::Ok(e) => {
                let v = wire_in(e, ok, types, cx)?;
                Ok(format!("{{ kind: \"Ok\", value: {v} }}"))
            }
            ExprKind::Err(e) => {
                let v = wire_in(e, err, types, cx)?;
                Ok(format!("{{ kind: \"Err\", error: {v} }}"))
            }
            _ => Err(DefaultReason::from(
                "a `Result` field default must be `Ok(...)` or `Err(...)`".to_string(),
            )),
        },
        TypeRef::List(elem, _) => match &init.kind {
            ExprKind::ListLit(items, ..) => {
                let parts = items
                    .iter()
                    .map(|e| wire_in(e, elem, types, cx))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(format!("[{}]", parts.join(", ")))
            }
            _ => Err(DefaultReason::from(
                "a `List` field default must be a list literal".to_string(),
            )),
        },
        // #2607 (Decision B): `Map.empty()` / `Set.empty()` are the only
        // closed-form map and set values. Both travel as a JSON array (a map
        // as its entries, a set as its elements), so both are `[]`.
        TypeRef::Map(..) => empty_collection(init, "Map").map(|()| "[]".to_string()),
        TypeRef::Set(..) => empty_collection(init, "Set").map(|()| "[]".to_string()),
        TypeRef::Effect(..)
        | TypeRef::HttpResult(..)
        | TypeRef::QueueResult(_)
        | TypeRef::Asset(_)
        | TypeRef::RequestHeaders(_)
        | TypeRef::RequestQuery(_)
        | TypeRef::RequestCookies(_)
        | TypeRef::TokenClaims(_)
        | TypeRef::BoundaryRejection(_)
        | TypeRef::Query(..)
        | TypeRef::Stream(..)
        | TypeRef::Connection(..)
        | TypeRef::History(..)
        | TypeRef::ValidationError(_)
        | TypeRef::DecodeError(_)
        | TypeRef::Unit(_)
        | TypeRef::Fn(..) => Err(DefaultReason::from(
            "this field type is not wire-representable".to_string(),
        )),
    }
}

/// The wire form of a base-type literal — the raw literal a real wire value
/// of this base type would also be, so it re-enters
/// `emit_field_deserialise`'s (`bynk-emit`) ordinary `typeof`/
/// `Number.isInteger` checks unchanged. `Bytes` has no literal syntax, so it
/// is not admitted.
/// A qualified `TypeName.method(args)` call, in whichever `ExprKind` shape it
/// actually parses as. Confirmed empirically (`OrderId.unsafe("x")` parses to
/// `ExprKind::MethodCall { receiver: Ident("OrderId"), method: "unsafe", .. }`
/// — the parser never distinguishes a type-qualified call from an ordinary
/// instance method call; that's a resolver-time decision).
fn qualified_call(e: &Expr) -> Option<(&str, &str, &[Expr])> {
    match &e.kind {
        ExprKind::MethodCall {
            receiver,
            method,
            args,
            ..
        } => {
            let ExprKind::Ident(recv) = &receiver.kind else {
                return None;
            };
            Some((recv.name.as_str(), method.name.as_str(), args.as_slice()))
        }
        _ => None,
    }
}

fn lower_base_literal(base: BaseType, e: &Expr) -> Result<String, DefaultReason> {
    // Strip one level of negation so `-5`/`-5.0` reach the literal arms below,
    // mirroring `const_literal`'s admission — `i64::checked_neg` guards
    // `i64::MIN`, which has no positive counterpart to negate away from.
    let (negate, inner) = match &e.kind {
        ExprKind::UnaryOp(UnaryOp::Neg, inner) => (true, &inner.kind),
        other => (false, other),
    };
    match (base, inner) {
        (BaseType::Int | BaseType::Instant, ExprKind::IntLit { value, .. }) => {
            let v = if negate {
                value
                    .checked_neg()
                    .ok_or_else(|| "integer literal has no negation".to_string())?
            } else {
                *value
            };
            Ok(v.to_string())
        }
        (BaseType::Float, ExprKind::IntLit { value, .. }) => {
            let v = if negate { -*value } else { *value };
            Ok(v.to_string())
        }
        (BaseType::Float, ExprKind::FloatLit { lexeme, .. }) => Ok(if negate {
            format!("-{lexeme}")
        } else {
            lexeme.clone()
        }),
        // #2622: a `Decimal` is a JSON string on the wire, so its default is the
        // canonical text, which the decoder then parses.
        (BaseType::Decimal, ExprKind::DecimalLit { coef, scale, .. }) => {
            let signed = if negate && bynk_syntax::decimal::sign(coef) != 0 {
                format!("-{coef}")
            } else {
                coef.clone()
            };
            Ok(format!(
                "\"{}\"",
                bynk_syntax::decimal::render(&signed, *scale)
            ))
        }
        (BaseType::String, ExprKind::StrLit(s)) if !negate => {
            Ok(format!("\"{}\"", bynk_ts::escape_ts_literal(s)))
        }
        (BaseType::Bool, ExprKind::BoolLit(b)) if !negate => Ok(b.to_string()),
        (BaseType::Duration, ExprKind::DurationLit { millis, .. }) if !negate => {
            Ok(millis.to_string())
        }
        (BaseType::Bytes, _) => Err(DefaultReason::from(
            "a `Bytes` field has no literal default form".to_string(),
        )),
        _ => Err(DefaultReason::from(format!(
            "expected a `{}` literal",
            base.name()
        ))),
    }
}

/// The wire form of a sum-variant default: `{ kind: "Variant" }` (nullary) or
/// `{ kind: "Variant", f1: ..., f2: ... }` (payload, positionally recursed
/// against each field's *declared* type) — never a qualified reference into
/// the sum's generated value namespace (`Sum.Variant`), which is what the
/// ordinary handler-body lowering (`lower_expr_into`) would produce and which
/// a foreign module regenerating this codec cannot import.
fn lower_sum_default(
    sum_name: &str,
    variants: &InstVariants,
    init: &Expr,
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
    cx: Cx<'_>,
) -> Result<String, DefaultReason> {
    let (variant, args) = sum_default_parts(sum_name, variants, init)?;
    let mut parts = vec![format!("kind: \"{}\"", variant.0)];
    for ((field, ty), arg) in variant.1.iter().zip(args.iter()) {
        let v = wire_in(arg, ty, types, cx)?;
        parts.push(format!("{field}: {v}"));
    }
    Ok(format!("{{ {} }}", parts.join(", ")))
}

/// A sum's variants with their payload field types, as written (a non-generic
/// sum) or substituted (an instantiation, `wire::sum_inst_variants`).
type InstVariants = Vec<InstVariant>;

/// One variant: its name and payload `(field, type)` list.
type InstVariant = (String, Vec<(String, TypeRef)>);

fn body_variants(body: &SumBody) -> InstVariants {
    body.variants
        .iter()
        .map(|v| {
            (
                v.name.name.clone(),
                v.payload
                    .iter()
                    .map(|f| (f.name.name.clone(), f.type_ref.clone()))
                    .collect(),
            )
        })
        .collect()
}

/// A record's fields as `(name, type, declared default)`: as written (a
/// non-generic record) or substituted (an instantiation).
type InstFields<'a> = Vec<(String, TypeRef, Option<&'a Expr>)>;

fn body_fields(body: &RecordBody) -> InstFields<'_> {
    body.fields
        .iter()
        .map(|f| (f.name.name.clone(), f.type_ref.clone(), f.init.as_ref()))
        .collect()
}

/// #2690: the fields of the generic record `name[args]`, types substituted, each
/// with the default the declaration gives it.
fn inst_record_fields<'a>(
    name: &str,
    args: &[TypeRef],
    types: &'a std::collections::HashMap<String, Arc<TypeDecl>>,
) -> Result<InstFields<'a>, DefaultReason> {
    let inst = crate::wire::record_inst_fields(name, args, types).ok_or_else(|| {
        DefaultReason::from("a generic record's arity does not match".to_string())
    })?;
    let Some(TypeBody::Record(r)) = types.get(name).map(|d| &d.body) else {
        return Err(DefaultReason::UnresolvedType(name.to_string()));
    };
    Ok(inst
        .into_iter()
        .zip(r.fields.iter())
        .map(|((n, t), f)| (n, t, f.init.as_ref()))
        .collect())
}

/// The variant a sum default names, and its payload arguments — shared by the
/// wire and in-memory renderings so both read a default identically.
fn sum_default_parts<'a>(
    sum_name: &str,
    variants: &'a InstVariants,
    init: &'a Expr,
) -> Result<(&'a InstVariant, &'a [Expr]), DefaultReason> {
    let (variant_name, args): (&str, &[Expr]) = match &init.kind {
        ExprKind::Ident(id) => (id.name.as_str(), &[]),
        ExprKind::FieldAccess { receiver, field } => {
            let ExprKind::Ident(recv) = &receiver.kind else {
                return Err(DefaultReason::from(format!(
                    "expected a variant of `{sum_name}`"
                )));
            };
            if recv.name != sum_name {
                return Err(DefaultReason::from(format!(
                    "expected a variant of `{sum_name}`, not `{}`",
                    recv.name
                )));
            }
            (field.name.as_str(), &[])
        }
        ExprKind::Call { name, args, .. } => (name.name.as_str(), args.as_slice()),
        _ => match qualified_call(init) {
            Some((recv, method, args)) if recv == sum_name => (method, args),
            Some((recv, ..)) => {
                return Err(DefaultReason::from(format!(
                    "expected a variant of `{sum_name}`, not `{recv}`"
                )));
            }
            None => {
                return Err(DefaultReason::from(format!(
                    "expected a variant of `{sum_name}`"
                )));
            }
        },
    };
    let Some(variant) = variants.iter().find(|v| v.0 == variant_name) else {
        return Err(DefaultReason::from(format!(
            "`{variant_name}` is not a variant of `{sum_name}`"
        )));
    };
    if args.len() != variant.1.len() {
        return Err(DefaultReason::from(format!(
            "`{variant_name}` takes {} payload field(s), got {}",
            variant.1.len(),
            args.len()
        )));
    }
    Ok((variant, args))
}

/// The wire form of a record-literal default: a plain object literal, each
/// field recursed against its *declared* type. Records are structurally
/// wire-shaped (never tagged, never a named constructor), so this never
/// needs qualification either.
fn lower_record_default(
    fields_of: &InstFields<'_>,
    init: &Expr,
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
    cx: Cx<'_>,
) -> Result<String, DefaultReason> {
    let ExprKind::RecordConstruction { fields, .. } = &init.kind else {
        return Err(DefaultReason::from("expected a record literal".to_string()));
    };
    let mut parts = Vec::new();
    for (name, ty, declared) in fields_of {
        let value = record_default_field(fields, name, *declared)?;
        let v = wire_in(value, ty, types, cx)?;
        parts.push(format!("{name}: {v}"));
    }
    Ok(format!("{{ {} }}", parts.join(", ")))
}

/// #2607: the value a record literal written as a default gives field `name` —
/// its own written value, or, when it leaves it out, the field's declared default.
fn record_default_field<'a>(
    fields: &'a [bynk_syntax::ast::FieldInit],
    name: &str,
    declared: Option<&'a Expr>,
) -> Result<&'a Expr, DefaultReason> {
    match fields.iter().find(|fi| fi.name.name == name) {
        Some(given) => given.value.as_ref().ok_or_else(|| {
            DefaultReason::from(format!(
                "record default's field `{name}` cannot use shorthand (no bindings are in scope)"
            ))
        }),
        None => declared.ok_or_else(|| {
            DefaultReason::from(format!("record default is missing field `{name}`"))
        }),
    }
}

/// #2607 (Decision B): accepts exactly `<collection>.empty()`.
fn empty_collection(init: &Expr, collection: &str) -> Result<(), DefaultReason> {
    match qualified_call(init) {
        Some((recv, "empty", [])) if recv == collection => Ok(()),
        _ => Err(DefaultReason::from(format!(
            "a `{collection}` field default must be `{collection}.empty()`"
        ))),
    }
}

/// #2607 (Decision C): the **in-memory** TypeScript value a record literal
/// takes for a field it leaves out — the twin of [`lower_field_default_wire`],
/// over the same closed set of static values, but in the shape a Bynk value
/// has at runtime (`{ tag: … }` sums and `Option`/`Result`, a real `Map`/`Set`)
/// rather than its wire JSON.
///
/// It is self-contained on purpose: a literal may build a record declared in
/// another unit (`FetchOptions` from `bynk`), whose default names values
/// (`Redirect.Follow`) the literal's own module never imports. Writing each
/// value structurally needs no import at all, the reason the wire form is
/// structural too.
pub fn lower_field_default_value(
    init: &Expr,
    expected: &TypeRef,
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
) -> Result<String, DefaultReason> {
    value_in(
        init,
        expected,
        types,
        Cx {
            params: &[],
            depth: 0,
            generics: true,
        },
    )
}

fn value_in(
    init: &Expr,
    expected: &TypeRef,
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
    cx: Cx<'_>,
) -> Result<String, DefaultReason> {
    if let ExprKind::Paren(inner) = &init.kind {
        return value_in(inner, expected, types, cx);
    }
    match expected {
        TypeRef::Base(b, _) => lower_base_literal(*b, init),
        TypeRef::Named(id) => {
            if cx.params.contains(&id.name) {
                return Err(DefaultReason::UnresolvedType(id.name.clone()));
            }
            let Some(decl) = types.get(&id.name) else {
                return Err(DefaultReason::UnresolvedType(id.name.clone()));
            };
            match &decl.body {
                TypeBody::Refined { base, .. } => lower_base_literal(*base, init),
                TypeBody::Opaque { base, .. } => match qualified_call(init) {
                    Some((type_name, "unsafe", [lit])) if type_name == id.name => {
                        lower_base_literal(*base, lit)
                    }
                    _ => Err(DefaultReason::from(format!(
                        "an opaque type's default must be `{}.unsafe(<literal>)`",
                        id.name
                    ))),
                },
                TypeBody::Sum(s) => value_sum(&id.name, &body_variants(s), init, types, cx),
                TypeBody::Record(r) => value_record(&body_fields(r), init, types, cx),
            }
        }
        // #2690: see `wire_in`.
        TypeRef::App { name, args, .. } => {
            if !cx.generics {
                return Err(DefaultReason::from(
                    "only a record field's default may apply a generic record or sum".to_string(),
                ));
            }
            let cx = cx.deeper()?;
            match types.get(&name.name).map(|d| &d.body) {
                Some(TypeBody::Record(_)) => {
                    let fields = inst_record_fields(&name.name, args, types)?;
                    value_record(&fields, init, types, cx)
                }
                Some(TypeBody::Sum(_)) => {
                    let variants = crate::wire::sum_inst_variants(&name.name, args, types)
                        .ok_or_else(|| {
                            DefaultReason::from("a generic sum's arity does not match".to_string())
                        })?;
                    value_sum(&name.name, &variants, init, types, cx)
                }
                _ => Err(DefaultReason::UnresolvedType(name.name.clone())),
            }
        }
        TypeRef::Option(inner, _) => match &init.kind {
            ExprKind::None => Ok("{ tag: \"None\" }".to_string()),
            ExprKind::Some(e) => {
                let v = value_in(e, inner, types, cx)?;
                Ok(format!("{{ tag: \"Some\", value: {v} }}"))
            }
            _ => Err(DefaultReason::from(
                "an `Option` field default must be `Some(...)` or `None`".to_string(),
            )),
        },
        TypeRef::Result(ok, err, _) => match &init.kind {
            ExprKind::Ok(e) => {
                let v = value_in(e, ok, types, cx)?;
                Ok(format!("{{ tag: \"Ok\", value: {v} }}"))
            }
            ExprKind::Err(e) => {
                let v = value_in(e, err, types, cx)?;
                Ok(format!("{{ tag: \"Err\", error: {v} }}"))
            }
            _ => Err(DefaultReason::from(
                "a `Result` field default must be `Ok(...)` or `Err(...)`".to_string(),
            )),
        },
        TypeRef::List(elem, _) => match &init.kind {
            ExprKind::ListLit(items, ..) => {
                let parts = items
                    .iter()
                    .map(|e| value_in(e, elem, types, cx))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(format!("[{}]", parts.join(", ")))
            }
            _ => Err(DefaultReason::from(
                "a `List` field default must be a list literal".to_string(),
            )),
        },
        TypeRef::Map(..) => {
            empty_collection(init, "Map").map(|()| "new globalThis.Map()".to_string())
        }
        TypeRef::Set(..) => {
            empty_collection(init, "Set").map(|()| "new globalThis.Set()".to_string())
        }
        // Every other type is rejected by `lower_field_default_wire` at check
        // time, so a program that reaches emission never asks: a field that
        // applies a user generic (`App`), and the types with no wire form
        // (`Effect`, `HttpResult`, `QueueResult`, `Query`, `Stream`,
        // `Connection`, `History`, `Fn`, `Asset`, `RequestHeaders`,
        // `TokenClaims`, `BoundaryRejection`, `ValidationError`, `DecodeError`
        // and `Unit`).
        _ => Err(DefaultReason::from(
            "this field type cannot carry a default".to_string(),
        )),
    }
}

/// The in-memory twin of [`lower_sum_default`]: the wire form is
/// `{ kind: "V", … }`; in memory the discriminant is `tag`, and the payload
/// fields recurse in their own in-memory form.
fn value_sum(
    sum_name: &str,
    variants: &InstVariants,
    init: &Expr,
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
    cx: Cx<'_>,
) -> Result<String, DefaultReason> {
    let (variant, args) = sum_default_parts(sum_name, variants, init)?;
    let mut parts = vec![format!("tag: \"{}\"", variant.0)];
    for ((field, ty), arg) in variant.1.iter().zip(args.iter()) {
        let v = value_in(arg, ty, types, cx)?;
        parts.push(format!("{field}: {v}"));
    }
    Ok(format!("{{ {} }}", parts.join(", ")))
}

/// The in-memory twin of [`lower_record_default`].
fn value_record(
    fields_of: &InstFields<'_>,
    init: &Expr,
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
    cx: Cx<'_>,
) -> Result<String, DefaultReason> {
    let ExprKind::RecordConstruction { fields, .. } = &init.kind else {
        return Err(DefaultReason::from("expected a record literal".to_string()));
    };
    let mut parts = Vec::new();
    for (name, ty, declared) in fields_of {
        let value = record_default_field(fields, name, *declared)?;
        let v = value_in(value, ty, types, cx)?;
        parts.push(format!("{name}: {v}"));
    }
    Ok(format!("{{ {} }}", parts.join(", ")))
}

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

/// Events slice 3a (#972): whether an event field's default is closed-form, so that the JSON
/// it deserialises from when its key is absent from the incoming JSON can be built. Type-directed
/// — given the field's expected `TypeRef` and the visible `types` table, every syntactic
/// ambiguity (a bare `Ident` that is really a sum variant, a `FieldAccess` that is a qualified
/// nullary variant) resolves the same way the checker resolves it, just narrower (a bare name
/// must match a variant of *this* expected sum specifically) — so this needs no per-expression
/// `Ty` map (`expr_types`), which a subscriber regenerating a publisher's own event codec
/// (`emit_consumed_context_helpers`) has no way to obtain anyway.
///
/// It is the checker's validation and nothing else (1A.40b, #3996): `Err(reason)` for anything
/// not closed-form, which `bynk.types.bad_field_default` reports at check time, and `Ok(())`
/// otherwise. The **wire form** of a default the emitter splices into a codec is a pure function
/// of [`field_default_value`]'s tree (`wire_default_node` in `bynk-emit`), so this crate writes
/// no TypeScript text for it. The two walks share `sum_default_parts`, `record_default_field`,
/// `inst_record_fields` and `base_value`; a test pins that they agree on every case.
pub fn lower_field_default_wire(
    init: &Expr,
    expected: &TypeRef,
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
) -> Result<(), DefaultReason> {
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
) -> Result<(), DefaultReason> {
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
) -> Result<(), DefaultReason> {
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
) -> Result<(), DefaultReason> {
    if let ExprKind::Paren(inner) = &init.kind {
        return wire_in(inner, expected, types, cx);
    }
    match expected {
        TypeRef::Base(b, _) => check_base(*b, init),
        TypeRef::Named(id) => {
            if cx.params.contains(&id.name) {
                return Err(DefaultReason::UnresolvedType(id.name.clone()));
            }
            let Some(decl) = types.get(&id.name) else {
                return Err(DefaultReason::UnresolvedType(id.name.clone()));
            };
            match &decl.body {
                TypeBody::Refined { base, .. } => check_base(*base, init),
                // ADR 0182: `.unsafe`'s value is an identity cast (`return
                // value as T;`, `emit.rs`'s opaque emission) — the literal
                // itself, verbatim, is the wire form. (Its refinement is
                // additionally checked *statically* for an event field
                // default specifically — `check_event_field_default` — since
                // this literal re-enters the same codec a real wire value
                // would, unlike an ordinary `.unsafe` bypass.)
                TypeBody::Opaque { base, .. } => match qualified_call(init) {
                    Some((type_name, "unsafe", [lit])) if type_name == id.name => {
                        check_base(*base, lit)
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
            ExprKind::None => Ok(()),
            ExprKind::Some(e) => wire_in(e, inner, types, cx),
            _ => Err(DefaultReason::from(
                "an `Option` field default must be `Some(...)` or `None`".to_string(),
            )),
        },
        TypeRef::Result(ok, err, _) => match &init.kind {
            ExprKind::Ok(e) => wire_in(e, ok, types, cx),
            ExprKind::Err(e) => wire_in(e, err, types, cx),
            _ => Err(DefaultReason::from(
                "a `Result` field default must be `Ok(...)` or `Err(...)`".to_string(),
            )),
        },
        TypeRef::List(elem, _) => match &init.kind {
            ExprKind::ListLit(items, ..) => {
                items.iter().try_for_each(|e| wire_in(e, elem, types, cx))
            }
            _ => Err(DefaultReason::from(
                "a `List` field default must be a list literal".to_string(),
            )),
        },
        // #2607 (Decision B): `Map.empty()` / `Set.empty()` are the only
        // closed-form map and set values. Both travel as a JSON array (a map
        // as its entries, a set as its elements), and both are empty.
        TypeRef::Map(..) => empty_collection(init, "Map"),
        TypeRef::Set(..) => empty_collection(init, "Set"),
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

/// Whether `e` is a closed-form literal of `base`: [`base_value`], read for its verdict.
fn check_base(base: BaseType, e: &Expr) -> Result<(), DefaultReason> {
    base_value(base, e).map(drop)
}

/// A base-type literal as plain data. [`field_default_value`] reads it for the value, and
/// [`check_base`] for the verdict.
fn base_value(base: BaseType, e: &Expr) -> Result<DefaultValue, DefaultReason> {
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
            Ok(DefaultValue::Num(v.to_string()))
        }
        (BaseType::Float, ExprKind::IntLit { value, .. }) => {
            let v = if negate { -*value } else { *value };
            Ok(DefaultValue::Num(v.to_string()))
        }
        (BaseType::Float, ExprKind::FloatLit { lexeme, .. }) => Ok(DefaultValue::Num(if negate {
            format!("-{lexeme}")
        } else {
            lexeme.clone()
        })),
        // #3801: a `Decimal` is its coefficient and scale; the wire text is rendered from
        // them by [`lower_base_literal`] (#2622), the in-memory value by the emitter.
        (BaseType::Decimal, ExprKind::DecimalLit { coef, scale, .. }) => {
            let signed = if negate && bynk_syntax::decimal::sign(coef) != 0 {
                format!("-{coef}")
            } else {
                coef.clone()
            };
            Ok(DefaultValue::Decimal {
                coefficient: signed,
                scale: *scale,
            })
        }
        (BaseType::String, ExprKind::StrLit(s)) if !negate => Ok(DefaultValue::Str(s.clone())),
        (BaseType::Bool, ExprKind::BoolLit(b)) if !negate => Ok(DefaultValue::Bool(*b)),
        (BaseType::Duration, ExprKind::DurationLit { millis, .. }) if !negate => {
            Ok(DefaultValue::Num(millis.to_string()))
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
) -> Result<(), DefaultReason> {
    let (variant, args) = sum_default_parts(sum_name, variants, init)?;
    for ((_, ty), arg) in variant.1.iter().zip(args.iter()) {
        wire_in(arg, ty, types, cx)?;
    }
    Ok(())
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
) -> Result<(), DefaultReason> {
    let ExprKind::RecordConstruction { fields, .. } = &init.kind else {
        return Err(DefaultReason::from("expected a record literal".to_string()));
    };
    for (name, ty, declared) in fields_of {
        let value = record_default_field(fields, name, *declared)?;
        wire_in(value, ty, types, cx)?;
    }
    Ok(())
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

/// #2607 (Decision C): the **in-memory** value a record literal takes for a field it leaves
/// out, as plain data — the twin of [`lower_field_default_wire`], over the same closed set of
/// static values, but in the shape a Bynk value has at runtime (a sum and `Option`/`Result`
/// carry a variant tag, a `Map`/`Set` is a real collection) rather than its wire JSON.
///
/// It is a tree, not text, so the front half knows nothing of TypeScript (1A.31, #3732): the
/// emitter turns it into nodes (`default_value_node` in `bynk-emit`), writing each tag
/// `as const` so that an unannotated `const x = { … }` in a test body does not widen `tag` to
/// `string` (#3404).
///
/// It is self-contained on purpose: a literal may build a record declared in another unit
/// (`FetchOptions` from `bynk`), whose default names values (`Redirect.Follow`) the literal's
/// own module never imports. Each value is structural, so it needs no import at all, the
/// reason the wire form is structural too.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefaultValue {
    /// A number's lexeme, sign included (`5`, `-5`, `1e3`, `3600000`).
    Num(String),
    /// A string's value, unescaped.
    Str(String),
    Bool(bool),
    /// A `Decimal`: its coefficient's digits, sign included, and its scale (#3801). In memory
    /// it is `{ coefficient, scale }`; only the wire form is the canonical text.
    Decimal {
        coefficient: String,
        scale: u32,
    },
    /// A value of the refined or opaque type `name` (#3759, #3842): its base value, which the
    /// checker has already found satisfies the refinement, or is the literal of
    /// `name.unsafe(<literal>)`. The emitter brands the field that holds it. `opaque` says which:
    /// test scaffolding brands an opaque value through `name.unsafe(…)` and a refined one with a
    /// cast, since a refined type has no `unsafe`.
    Branded {
        name: String,
        value: Box<DefaultValue>,
        opaque: bool,
    },
    /// A record: every declared field, in declaration order.
    Record(Vec<(String, DefaultValue)>),
    /// A sum variant and its payload fields.
    Sum {
        variant: String,
        fields: Vec<(String, DefaultValue)>,
    },
    Some(Box<DefaultValue>),
    None,
    Ok(Box<DefaultValue>),
    Err(Box<DefaultValue>),
    List(Vec<DefaultValue>),
    EmptyMap,
    EmptySet,
}

impl DefaultValue {
    /// Whether the value holds a refined or opaque value anywhere (#3759, #3842): its field then
    /// needs a cast to the field's declared type, which brands it.
    pub fn holds_branded(&self) -> bool {
        match self {
            DefaultValue::Branded { .. } => true,
            DefaultValue::Record(fields) | DefaultValue::Sum { fields, .. } => {
                fields.iter().any(|(_, v)| v.holds_branded())
            }
            DefaultValue::Some(v) | DefaultValue::Ok(v) | DefaultValue::Err(v) => v.holds_branded(),
            DefaultValue::List(items) => items.iter().any(DefaultValue::holds_branded),
            DefaultValue::Num(_)
            | DefaultValue::Str(_)
            | DefaultValue::Bool(_)
            | DefaultValue::Decimal { .. }
            | DefaultValue::None
            | DefaultValue::EmptyMap
            | DefaultValue::EmptySet => false,
        }
    }
}

/// The [`DefaultValue`] of a field's default `init`, given the field's `expected` type and the
/// visible `types` table. `Err` for anything not closed-form; the checker has already required
/// every default to have one.
pub fn field_default_value(
    init: &Expr,
    expected: &TypeRef,
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
) -> Result<DefaultValue, DefaultReason> {
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
) -> Result<DefaultValue, DefaultReason> {
    if let ExprKind::Paren(inner) = &init.kind {
        return value_in(inner, expected, types, cx);
    }
    match expected {
        TypeRef::Base(b, _) => base_value(*b, init),
        TypeRef::Named(id) => {
            if cx.params.contains(&id.name) {
                return Err(DefaultReason::UnresolvedType(id.name.clone()));
            }
            let Some(decl) = types.get(&id.name) else {
                return Err(DefaultReason::UnresolvedType(id.name.clone()));
            };
            match &decl.body {
                TypeBody::Refined { base, .. } => Ok(DefaultValue::Branded {
                    name: id.name.clone(),
                    value: Box::new(base_value(*base, init)?),
                    opaque: false,
                }),
                TypeBody::Opaque { base, .. } => match qualified_call(init) {
                    // #3842: a value of the opaque type, which its field must be branded as.
                    Some((type_name, "unsafe", [lit])) if type_name == id.name => {
                        Ok(DefaultValue::Branded {
                            name: id.name.clone(),
                            value: Box::new(base_value(*base, lit)?),
                            opaque: true,
                        })
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
            ExprKind::None => Ok(DefaultValue::None),
            ExprKind::Some(e) => Ok(DefaultValue::Some(Box::new(value_in(e, inner, types, cx)?))),
            _ => Err(DefaultReason::from(
                "an `Option` field default must be `Some(...)` or `None`".to_string(),
            )),
        },
        TypeRef::Result(ok, err, _) => match &init.kind {
            ExprKind::Ok(e) => Ok(DefaultValue::Ok(Box::new(value_in(e, ok, types, cx)?))),
            ExprKind::Err(e) => Ok(DefaultValue::Err(Box::new(value_in(e, err, types, cx)?))),
            _ => Err(DefaultReason::from(
                "a `Result` field default must be `Ok(...)` or `Err(...)`".to_string(),
            )),
        },
        TypeRef::List(elem, _) => match &init.kind {
            ExprKind::ListLit(items, ..) => items
                .iter()
                .map(|e| value_in(e, elem, types, cx))
                .collect::<Result<Vec<_>, _>>()
                .map(DefaultValue::List),
            _ => Err(DefaultReason::from(
                "a `List` field default must be a list literal".to_string(),
            )),
        },
        TypeRef::Map(..) => empty_collection(init, "Map").map(|()| DefaultValue::EmptyMap),
        TypeRef::Set(..) => empty_collection(init, "Set").map(|()| DefaultValue::EmptySet),
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
/// `{ kind: "V", … }`; in memory the discriminant is the variant, and the payload
/// fields recurse in their own in-memory form.
fn value_sum(
    sum_name: &str,
    variants: &InstVariants,
    init: &Expr,
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
    cx: Cx<'_>,
) -> Result<DefaultValue, DefaultReason> {
    let (variant, args) = sum_default_parts(sum_name, variants, init)?;
    let mut fields = Vec::new();
    for ((field, ty), arg) in variant.1.iter().zip(args.iter()) {
        fields.push((field.clone(), value_in(arg, ty, types, cx)?));
    }
    Ok(DefaultValue::Sum {
        variant: variant.0.clone(),
        fields,
    })
}

/// The in-memory twin of [`lower_record_default`].
fn value_record(
    fields_of: &InstFields<'_>,
    init: &Expr,
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
    cx: Cx<'_>,
) -> Result<DefaultValue, DefaultReason> {
    let ExprKind::RecordConstruction { fields, .. } = &init.kind else {
        return Err(DefaultReason::from("expected a record literal".to_string()));
    };
    let mut parts = Vec::new();
    for (name, ty, declared) in fields_of {
        let value = record_default_field(fields, name, *declared)?;
        parts.push((name.to_string(), value_in(value, ty, types, cx)?));
    }
    Ok(DefaultValue::Record(parts))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bynk_syntax::ast::CommonsItem;
    use std::collections::HashMap;

    /// The shared declarations the cases lean on.
    const PRELUDE: &str = "commons x\n\n\
        type Age = Int where InRange(0, 150)\n\
        type Tok = opaque String\n\
        type Plan = enum { Free, Pro }\n\
        type Shape = | Dot | Circle(r: Float) | Rect(w: Int, h: Int)\n\
        type Inner = { n: Int = 7, s: String = \"i\" }\n\
        type Box[A] = { value: A }\n\
        type Opt[T] = | Has(value: T) | Nil\n\
        type Loop[T] = { kid: Loop[T] = Loop { } }\n";

    fn num(s: &str) -> DefaultValue {
        DefaultValue::Num(s.to_string())
    }
    fn text(s: &str) -> DefaultValue {
        DefaultValue::Str(s.to_string())
    }
    fn some(v: DefaultValue) -> DefaultValue {
        DefaultValue::Some(Box::new(v))
    }
    fn dec(coefficient: &str, scale: u32) -> DefaultValue {
        DefaultValue::Decimal {
            coefficient: coefficient.to_string(),
            scale,
        }
    }
    fn refined(name: &str, v: DefaultValue) -> DefaultValue {
        DefaultValue::Branded {
            name: name.to_string(),
            value: Box::new(v),
            opaque: false,
        }
    }
    fn opaque(name: &str, v: DefaultValue) -> DefaultValue {
        DefaultValue::Branded {
            name: name.to_string(),
            value: Box::new(v),
            opaque: true,
        }
    }
    fn sum(variant: &str, fields: Vec<(&str, DefaultValue)>) -> DefaultValue {
        DefaultValue::Sum {
            variant: variant.to_string(),
            fields: fields
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
        }
    }
    fn record(fields: Vec<(&str, DefaultValue)>) -> DefaultValue {
        DefaultValue::Record(
            fields
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
        )
    }

    /// `(field type, default, the tree, the text the deleted `lower_field_default_value` produced)`.
    /// The text column was captured from the `String` renderer before it was deleted
    /// (1A.31, #3732), so the old output and the new tree sit side by side; `bynk-emit` prints
    /// each tree and compares it with the same text. The `Decimal` rows (#3801) are the corrected
    /// text: the old renderer wrote the wire string there.
    pub fn cases() -> Vec<(&'static str, &'static str, DefaultValue, &'static str)> {
        vec![
            ("Int", "5", num("5"), "5"),
            ("Int", "-5", num("-5"), "-5"),
            ("Instant", "0", num("0"), "0"),
            ("Float", "1.5", num("1.5"), "1.5"),
            ("Float", "5", num("5"), "5"),
            ("Float", "1e3", num("1e3"), "1e3"),
            ("Float", "-2.5", num("-2.5"), "-2.5"),
            ("Float", "-5", num("-5"), "-5"),
            ("Duration", "5.minutes", num("300000"), "300000"),
            ("Bool", "true", DefaultValue::Bool(true), "true"),
            ("String", "\"x\"", text("x"), "\"x\""),
            ("String", "\"a\\\"b\\n\"", text("a\"b\n"), "\"a\\\"b\\n\""),
            ("String", "\"\"", text(""), "\"\""),
            // #3801: in memory, a `Decimal` is its coefficient and scale, not its wire text.
            (
                "Decimal",
                "0.00d",
                dec("000", 2),
                "{ coefficient: 0n, scale: 2 }",
            ),
            (
                "Decimal",
                "-1.50d",
                dec("-150", 2),
                "{ coefficient: -150n, scale: 2 }",
            ),
            // #3759: a refined default is marked; the emitter brands the field that holds it.
            ("Age", "3", refined("Age", num("3")), "3"),
            // #3842: an opaque value is branded as its field is, like a refined one.
            (
                "Tok",
                "Tok.unsafe(\"k\")",
                opaque("Tok", text("k")),
                "\"k\"",
            ),
            (
                "Inner",
                "Inner { }",
                record(vec![("n", num("7")), ("s", text("i"))]),
                "{ n: 7, s: \"i\" }",
            ),
            (
                "Inner",
                "Inner { n: 9 }",
                record(vec![("n", num("9")), ("s", text("i"))]),
                "{ n: 9, s: \"i\" }",
            ),
            (
                "Plan",
                "Free",
                sum("Free", vec![]),
                "{ tag: \"Free\" as const }",
            ),
            (
                "Shape",
                "Dot",
                sum("Dot", vec![]),
                "{ tag: \"Dot\" as const }",
            ),
            (
                "Shape",
                "Rect(1, -2)",
                sum("Rect", vec![("w", num("1")), ("h", num("-2"))]),
                "{ tag: \"Rect\" as const, w: 1, h: -2 }",
            ),
            (
                "Option[Int]",
                "Some(1)",
                some(num("1")),
                "{ tag: \"Some\" as const, value: 1 }",
            ),
            (
                "Option[Int]",
                "None",
                DefaultValue::None,
                "{ tag: \"None\" as const }",
            ),
            (
                "Option[Option[Int]]",
                "Some(None)",
                some(DefaultValue::None),
                "{ tag: \"Some\" as const, value: { tag: \"None\" as const } }",
            ),
            (
                "Result[Int, String]",
                "Ok(1)",
                DefaultValue::Ok(Box::new(num("1"))),
                "{ tag: \"Ok\" as const, value: 1 }",
            ),
            (
                "Result[Int, String]",
                "Err(\"e\")",
                DefaultValue::Err(Box::new(text("e"))),
                "{ tag: \"Err\" as const, error: \"e\" }",
            ),
            ("List[Int]", "[]", DefaultValue::List(vec![]), "[]"),
            (
                "List[Int]",
                "[1, 2]",
                DefaultValue::List(vec![num("1"), num("2")]),
                "[1, 2]",
            ),
            (
                "List[Option[Int]]",
                "[None, Some(2)]",
                DefaultValue::List(vec![DefaultValue::None, some(num("2"))]),
                "[{ tag: \"None\" as const }, { tag: \"Some\" as const, value: 2 }]",
            ),
            (
                "Map[String, Int]",
                "Map.empty()",
                DefaultValue::EmptyMap,
                "new globalThis.Map()",
            ),
            (
                "Set[Int]",
                "Set.empty()",
                DefaultValue::EmptySet,
                "new globalThis.Set()",
            ),
            (
                "Box[Int]",
                "Box { value: 3 }",
                record(vec![("value", num("3"))]),
                "{ value: 3 }",
            ),
            (
                "Opt[Int]",
                "Has(4)",
                sum("Has", vec![("value", num("4"))]),
                "{ tag: \"Has\" as const, value: 4 }",
            ),
            (
                "Opt[Int]",
                "Nil",
                sum("Nil", vec![]),
                "{ tag: \"Nil\" as const }",
            ),
            (
                "Box[Opt[String]]",
                "Box { value: Has(\"s\") }",
                record(vec![("value", sum("Has", vec![("value", text("s"))]))]),
                "{ value: { tag: \"Has\" as const, value: \"s\" } }",
            ),
        ]
    }

    /// The `types` table and the `f` field of a record `Holder { f: <ty> = <init> }`.
    pub fn holder(ty: &str, init: &str) -> (HashMap<String, Arc<TypeDecl>>, Expr, TypeRef) {
        let src = format!("{PRELUDE}\ntype Holder = {{ f: {ty} = {init} }}\n");
        let tokens = bynk_syntax::lexer::tokenize(&src).expect("lex");
        let commons = bynk_syntax::parser::parse(&tokens, &src).expect("parse");
        let types: HashMap<String, Arc<TypeDecl>> = commons
            .items
            .iter()
            .filter_map(|i| match i {
                CommonsItem::Type(t) => Some((t.name.name.clone(), Arc::new(t.clone()))),
                _ => None,
            })
            .collect();
        let TypeBody::Record(r) = &types["Holder"].body else {
            panic!("Holder is a record");
        };
        let field = &r.fields[0];
        (
            types.clone(),
            field.init.clone().expect("a default"),
            field.type_ref.clone(),
        )
    }

    /// #3759, #3842: only a value with a refined or opaque part needs its field branded.
    #[test]
    fn holds_branded_finds_a_refined_or_opaque_value_at_any_depth() {
        let r = refined("Age", num("3"));
        for v in [
            opaque("Tok", text("k")),
            some(opaque("Tok", text("k"))),
            r.clone(),
            some(r.clone()),
            DefaultValue::Err(Box::new(r.clone())),
            DefaultValue::List(vec![num("1"), r.clone()]),
            record(vec![("a", num("1")), ("b", r.clone())]),
            sum("V", vec![("x", r)]),
        ] {
            assert!(v.holds_branded(), "{v:?}");
        }
        for v in [
            num("3"),
            dec("150", 2),
            DefaultValue::None,
            DefaultValue::List(vec![num("1")]),
            record(vec![("a", text("x"))]),
            DefaultValue::EmptyMap,
        ] {
            assert!(!v.holds_branded(), "{v:?}");
        }
    }

    #[test]
    fn field_default_value_builds_the_tree_for_every_shape() {
        for (ty, init, tree, _) in cases() {
            let (types, e, t) = holder(ty, init);
            assert_eq!(
                field_default_value(&e, &t, &types),
                Ok(tree),
                "{ty} = {init}"
            );
        }
    }

    /// The validator and the tree the emitter renders agree on every default (1A.40b): the
    /// validator is `Ok` exactly when [`field_default_value`] is, so a default the checker admits
    /// is never silently dropped by the emitter's `.ok()`, and one it rejects never reaches a
    /// codec. The wording of a rejection may differ (`.unsafe`'s arity has its own message in the
    /// validator); the verdict may not.
    #[test]
    fn the_validator_and_the_tree_agree_on_every_default() {
        for (ty, init, _, _) in cases() {
            let (types, e, t) = holder(ty, init);
            assert_eq!(
                lower_field_default_wire(&e, &t, &types),
                Ok(()),
                "{ty} = {init}"
            );
            assert!(field_default_value(&e, &t, &types).is_ok(), "{ty} = {init}");
        }
        // The messages may differ (`.unsafe`'s arity has its own in the validator), and each walk
        // keeps its own wording; what must agree is the verdict.
        for (ty, init, _) in REJECTIONS {
            let (types, e, t) = holder(ty, init);
            assert!(
                field_default_value(&e, &t, &types).is_err(),
                "{ty} = {init}"
            );
            assert!(
                lower_field_default_wire(&e, &t, &types).is_err(),
                "{ty} = {init}"
            );
        }
    }

    /// The two walks agree on the other two entry points too (1A.40c, a #4069 nit): the event and
    /// old-state path, whose defaults may not apply a user generic, and the generic-parameters
    /// path, where a parameter stands for no value. Same verdict on every accepted and rejected
    /// default, and the generic cases are where the three entry points differ.
    #[test]
    fn the_walks_agree_on_the_event_and_generic_params_paths() {
        let params = ["T".to_string()];
        let mut seen_generic_refusal = false;
        let defaults = cases()
            .into_iter()
            .map(|(ty, init, _, _)| (ty, init))
            .chain(REJECTIONS.iter().map(|(ty, init, _)| (*ty, *init)));
        for (ty, init) in defaults {
            let (types, e, t) = holder(ty, init);
            let event_cx = Cx {
                params: &[],
                depth: 0,
                generics: false,
            };
            let event = lower_event_field_default_wire(&e, &t, &types);
            let event_tree = value_in(&e, &t, &types, event_cx);
            assert_eq!(event.is_ok(), event_tree.is_ok(), "event: {ty} = {init}");
            let params_cx = Cx {
                params: &params,
                depth: 0,
                generics: true,
            };
            let generic = lower_field_default_wire_in(&e, &t, &types, &params);
            let generic_tree = value_in(&e, &t, &types, params_cx);
            assert_eq!(
                generic.is_ok(),
                generic_tree.is_ok(),
                "params: {ty} = {init}"
            );
            // The record path accepts what the event path refuses (a default applying a user
            // generic), so the three entry points are not one walk with the flag ignored.
            seen_generic_refusal |=
                lower_field_default_wire(&e, &t, &types).is_ok() && event.is_err();
        }
        assert!(
            seen_generic_refusal,
            "no case tells the event path from the record path"
        );
    }

    /// `(field type, default, the rejection)` the old renderer gave, kept verbatim.
    pub const REJECTIONS: &[(&str, &str, &str)] = &[
        (
            "Loop[Int]",
            "Loop { }",
            "the default nests generic instantiations too deeply",
        ),
        ("Bytes", "x", "a `Bytes` field has no literal default form"),
        ("Int", "\"s\"", "expected a `Int` literal"),
        (
            "Tok",
            "Tok.unsafe(\"k\", 1)",
            "an opaque type's default must be `Tok.unsafe(<literal>)`",
        ),
        (
            "Tok",
            "5",
            "an opaque type's default must be `Tok.unsafe(<literal>)`",
        ),
        (
            "Option[Int]",
            "5",
            "an `Option` field default must be `Some(...)` or `None`",
        ),
        (
            "Result[Int, Int]",
            "5",
            "a `Result` field default must be `Ok(...)` or `Err(...)`",
        ),
        (
            "List[Int]",
            "5",
            "a `List` field default must be a list literal",
        ),
        (
            "Map[String, Int]",
            "5",
            "a `Map` field default must be `Map.empty()`",
        ),
        (
            "Set[Int]",
            "Set.empty(1)",
            "a `Set` field default must be `Set.empty()`",
        ),
        ("Effect[Int]", "5", "this field type cannot carry a default"),
        ("Inner", "5", "expected a record literal"),
        ("Shape", "Nope", "`Nope` is not a variant of `Shape`"),
        ("Box[Int]", "5", "expected a record literal"),
    ];

    #[test]
    fn field_default_value_rejects_with_the_old_messages() {
        for (ty, init, msg) in REJECTIONS {
            let (types, e, t) = holder(ty, init);
            assert_eq!(
                field_default_value(&e, &t, &types),
                Err(DefaultReason::Invalid((*msg).to_string())),
                "{ty} = {init}"
            );
        }
        let (types, e, t) = holder("Missing", "Missing { }");
        assert_eq!(
            field_default_value(&e, &t, &types),
            Err(DefaultReason::UnresolvedType("Missing".to_string()))
        );
    }
}

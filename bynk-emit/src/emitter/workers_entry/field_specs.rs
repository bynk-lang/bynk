//! The request field specs of the Worker entry: the field-spec tables the router reads a query,
//! form, multipart or cookie body by, and the leaf kinds they are built from.

use crate::emitter::serialisation;
use crate::ts_build::{bool_lit, str_lit};
use bynk_check::symbols::UnitTable;
use bynk_syntax::ast::{Handler, LimitsPolicy, TypeRef};
use bynk_ts::TsExpr;

/// One field of a record read off a query string, a form, a `Cookie` header or a
/// multipart body: `(name, leaf kind, list, option, wire name)`. The wire name is
/// the `@name("…")` rename (#3343), `None` when the field reads under its own name.
pub(crate) type FieldSpec = (String, &'static str, bool, bool, Option<String>);

/// The `QueryField` object the runtime's readers take for one field.
fn field_spec_expr((name, kind, list, option, wire): &FieldSpec) -> TsExpr {
    let mut props = vec![("name".to_string(), str_lit(name.clone()))];
    if let Some(wire) = wire {
        props.push(("wire".to_string(), str_lit(wire.clone())));
    }
    props.push(("kind".to_string(), str_lit(*kind)));
    props.push(("list".to_string(), bool_lit(*list)));
    props.push(("option".to_string(), bool_lit(*option)));
    TsExpr::object(props)
}

/// The same object as TypeScript source (`{ name: "sid", wire: "__Host-sid", kind: … }`),
/// for the system-tier drivers and the `Form` lowering, which write text.
pub fn field_spec_source((name, kind, list, option, wire): &FieldSpec) -> String {
    let wire = wire
        .as_ref()
        .map(|w| format!(" wire: {w:?},"))
        .unwrap_or_default();
    format!("{{ name: {name:?},{wire} kind: {kind:?}, list: {list}, option: {option} }}")
}

/// #3343: the `QueryField` array a system-tier driver passes the request writers
/// (`__queryString`, `__cookieString`) for a typed record parameter, when a field of
/// the record is renamed by `@name`; `None` otherwise (the writers need no table).
pub fn renamed_specs_source(
    type_ref: &TypeRef,
    types: &std::collections::HashMap<String, std::sync::Arc<bynk_syntax::ast::TypeDecl>>,
) -> Option<String> {
    use bynk_syntax::ast::TypeBody;
    let TypeRef::Named(id) = type_ref else {
        return None;
    };
    let Some(TypeBody::Record(rec)) = types.get(&id.name).map(|t| &t.body) else {
        return None;
    };
    let specs = record_field_specs(rec, types, false);
    if specs.iter().all(|s| s.4.is_none()) {
        return None;
    }
    let items: Vec<String> = specs.iter().map(field_spec_source).collect();
    Some(format!("[{}] as const", items.join(", ")))
}

pub(super) fn field_specs_exprs(fields: &[FieldSpec]) -> Vec<TsExpr> {
    fields.iter().map(field_spec_expr).collect()
}

/// #3276: what the router needs to read a `@multipart` body.
#[derive(Debug, Clone)]
pub(super) struct MultipartSpec {
    pub(super) fields: Vec<FieldSpec>,
    /// The per-file byte cap (`@limit(maxFile:)`, else `limits { maxFile }`), if any.
    pub(super) max_file: Option<i64>,
    /// The part-count cap (`@limit(maxParts:)`, else `limits { maxParts }`, else 64).
    pub(super) max_parts: i64,
    /// Whether the record has a `File` field, whose real values are laid over
    /// the decoded record.
    pub(super) has_files: bool,
}

/// #2854: the wire kind of one query-string value of type `r`, as the runtime's
/// `QueryField.kind` names it.
pub(crate) fn query_leaf_kind(
    r: &TypeRef,
    types: &std::collections::HashMap<String, std::sync::Arc<bynk_syntax::ast::TypeDecl>>,
    allow_file: bool,
) -> &'static str {
    use bynk_syntax::ast::{BaseType, TypeBody};
    let of_base = |b: &BaseType| match b {
        BaseType::Int => "int",
        BaseType::Bool => "bool",
        // #3276: a `Decimal` is read as text (its own codec parses it), but is not a
        // plain `String`: a form's empty value is absent for it.
        BaseType::Decimal => "decimal",
        BaseType::String
        | BaseType::Float
        | BaseType::Duration
        | BaseType::Instant
        | BaseType::Bytes => "string",
    };
    match r {
        TypeRef::Base(b, _) => of_base(b),
        // #3276: the `File` a `@multipart` body names a part with.
        // The checker admits only the std `File` as a multipart file field
        // (`bynk.http.form_field_not_stringy` refuses a user type of that name), so
        // the name is enough here, on the multipart path alone.
        TypeRef::Named(id) if allow_file && id.name == "File" => "file",
        TypeRef::Named(id) => match types.get(&id.name).map(|t| &t.body) {
            Some(TypeBody::Refined { base, .. }) | Some(TypeBody::Opaque { base, .. }) => {
                of_base(base)
            }
            Some(TypeBody::Sum(_)) => "enum",
            _ => "string",
        },
        TypeRef::Result(..)
        | TypeRef::Option(..)
        | TypeRef::Effect(..)
        | TypeRef::HttpResult(..)
        | TypeRef::QueueResult(_)
        | TypeRef::Asset(_)
        | TypeRef::RequestHeaders(_)
        | TypeRef::RequestQuery(_)
        | TypeRef::RequestCookies(_)
        | TypeRef::TokenClaims(_)
        | TypeRef::BoundaryRejection(_)
        | TypeRef::List(..)
        | TypeRef::Map(..)
        | TypeRef::Set(..)
        | TypeRef::Query(..)
        | TypeRef::Stream(..)
        | TypeRef::Connection(..)
        | TypeRef::History(..)
        | TypeRef::ValidationError(_)
        | TypeRef::DecodeError(_)
        | TypeRef::Unit(_)
        | TypeRef::Fn(..)
        | TypeRef::App { .. } => "string",
    }
}

/// #2854: the fields of a handler's typed `query` record, or `None` when the
/// handler has no `query` parameter, or it is the raw `RequestQuery` view (or a
/// path parameter named `query`).
pub(super) fn query_field_specs(
    h: &Handler,
    path: &str,
    types: &std::collections::HashMap<String, std::sync::Arc<bynk_syntax::ast::TypeDecl>>,
) -> Option<Vec<FieldSpec>> {
    use bynk_syntax::ast::TypeBody;
    if path.split('/').any(|seg| seg == ":query") {
        return None;
    }
    let p = h.params.iter().find(|p| p.name.name == "query")?;
    let TypeRef::Named(id) = &p.type_ref else {
        return None;
    };
    let Some(TypeBody::Record(rec)) = types.get(&id.name).map(|t| &t.body) else {
        return None;
    };
    Some(record_field_specs(rec, types, false))
}

/// #3276: the fields of a `@form` handler's `body` record, as `formObject` reads
/// them. `None` when the handler is not a `@form` route.
pub(super) fn form_field_specs(
    h: &Handler,
    types: &std::collections::HashMap<String, std::sync::Arc<bynk_syntax::ast::TypeDecl>>,
) -> Option<Vec<FieldSpec>> {
    use bynk_syntax::ast::TypeBody;
    if !h.annotations.iter().any(|a| a.name.name == "form") {
        return None;
    }
    let p = h.params.iter().find(|p| p.name.name == "body")?;
    let TypeRef::Named(id) = &p.type_ref else {
        return None;
    };
    let Some(TypeBody::Record(rec)) = types.get(&id.name).map(|t| &t.body) else {
        return None;
    };
    Some(record_field_specs(rec, types, false))
}

/// #3277: the fields of a handler's `cookies` record, or `None` when it has no
/// `cookies` parameter (or a `:cookies` path segment binds the name).
pub(super) fn cookies_field_specs(
    h: &Handler,
    path: &str,
    types: &std::collections::HashMap<String, std::sync::Arc<bynk_syntax::ast::TypeDecl>>,
) -> Option<Vec<FieldSpec>> {
    use bynk_syntax::ast::TypeBody;
    if path.split('/').any(|seg| seg == ":cookies") {
        return None;
    }
    let p = h.params.iter().find(|p| p.name.name == "cookies")?;
    let TypeRef::Named(id) = &p.type_ref else {
        return None;
    };
    let Some(TypeBody::Record(rec)) = types.get(&id.name).map(|t| &t.body) else {
        return None;
    };
    Some(record_field_specs(rec, types, false))
}

/// #3276: the body field spec of a `@form` or `@multipart` handler, as the system
/// test drivers write the body (`(name, wire kind, list, option)`).
pub fn form_body_specs(
    h: &Handler,
    types: &std::collections::HashMap<String, std::sync::Arc<bynk_syntax::ast::TypeDecl>>,
) -> Option<Vec<FieldSpec>> {
    form_field_specs(h, types).or_else(|| multipart_spec(None, h, types).map(|m| m.fields))
}

/// #3276: a `@multipart` handler's body spec, or `None` when it is not one.
pub(super) fn multipart_spec(
    service_limits: Option<&LimitsPolicy>,
    h: &Handler,
    types: &std::collections::HashMap<String, std::sync::Arc<bynk_syntax::ast::TypeDecl>>,
) -> Option<MultipartSpec> {
    use bynk_syntax::ast::TypeBody;
    if !h.annotations.iter().any(|a| a.name.name == "multipart") {
        return None;
    }
    let p = h.params.iter().find(|p| p.name.name == "body")?;
    let TypeRef::Named(id) = &p.type_ref else {
        return None;
    };
    let Some(TypeBody::Record(rec)) = types.get(&id.name).map(|t| &t.body) else {
        return None;
    };
    let fields = record_field_specs(rec, types, true);
    let has_files = fields.iter().any(|(_, kind, _, _, _)| *kind == "file");
    Some(MultipartSpec {
        fields,
        max_file: h
            .limit_max_file()
            .or_else(|| service_limits.and_then(|p| p.max_file())),
        max_parts: h
            .limit_max_parts()
            .or_else(|| service_limits.and_then(|p| p.max_parts()))
            .unwrap_or(bynk_syntax::ast::DEFAULT_MAX_PARTS),
        has_files,
    })
}

/// The wire shape of each field of a record read off a query string or a form:
/// `(name, leaf kind, list, option)`.
pub(crate) fn record_field_specs(
    rec: &bynk_syntax::ast::RecordBody,
    types: &std::collections::HashMap<String, std::sync::Arc<bynk_syntax::ast::TypeDecl>>,
    allow_file: bool,
) -> Vec<FieldSpec> {
    rec.fields
        .iter()
        .map(|f| {
            let (inner, list, option) = serialisation::peel_list_option(&f.type_ref);
            (
                f.name.name.clone(),
                query_leaf_kind(inner, types, allow_file),
                list,
                option,
                f.wire_name(),
            )
        })
        .collect()
}

/// #2858: whether any `from http` route in `table` writes plain JSON, so the
/// generated `Option` encoders carry the plain branch.
pub fn table_has_plain_json(table: &UnitTable) -> bool {
    table.services.values().any(|svc| {
        svc.handlers.iter().any(|h| {
            bynk_syntax::ast::effective_json_options(svc.json.as_ref(), h)
                == bynk_syntax::ast::JsonOptions::Plain
        })
    })
}

/// v0.142 (ADR 0165): resolve a route's effective request-body ceiling in bytes.
/// A route's `@limit(maxBody:)` annotation wins over the service's `limits { }`
/// default (DECISION B); with neither, the route has no cap (`None`). Project
/// validation (`bynk.http.limit_*` / `limits_*`) has already rejected a malformed
/// or misplaced `@limit`/`limits`, so a bad/absent value here simply yields no
/// cap. Only a body-taking method can carry a cap — a `@limit` on a GET/DELETE is
/// a checker error, so it never reaches a live route here. The route-annotation
/// half is `bynk-emit::ir`'s own [`bynk_model::lower_route_limit_ir`]
/// (#1228); only the service-wide fallback composition stays here.
pub(super) fn effective_max_body(
    service_limits: Option<&LimitsPolicy>,
    h: &Handler,
) -> Option<i64> {
    h.limit_max_body()
        .or_else(|| service_limits.and_then(|p| p.max_body()))
}

//! `bynk client` (#3508, BC1): a typed TypeScript client for a project's `from
//! http` services.
//!
//! The routes, their parameters and their response sets are the facts `bynk
//! openapi` reads (the wire-contract peek, `bynk_ide::wire_contract`), so the two
//! cannot disagree on a status or a shape. This module collects them into the
//! generator's input (`bynk_emit::emitter::http_client`) and writes the result.
//!
//! A route the first version cannot represent faithfully is named and left out,
//! never emitted wrong: a form, multipart or raw body, a typed `cookies` record, a
//! raw or streamed response, and a route guarded by a request signature (the
//! client holds no secret). Later slices of #3508 add them.

use std::path::Path;
use std::process::ExitCode;

use bynk_emit::emitter::http_client::{
    ClientError, HttpClientInput, ParamInput, ReplyInput, ReplyPayload, RouteInput, ServiceInput,
    emit_http_client,
};
use bynk_ide::wire_contract::{
    BoundaryKind, IrHttpMethod, ResponseOrigin, WireContractModel, real_context_count,
    wire_contract_for_service,
};
use bynk_ide::{ContextBoundaryInfo, ProjectDiagnostics};
use bynk_model::{IrHandlerKind, lower_handler_kind_ir};
use bynk_syntax::ast::{
    Handler, HttpVariantPayload, JsonOptions, TypeRef, effective_json_options, http_variant,
};

use crate::openapi::{self, Error as ProjectError, Selected};

/// Why a client could not be produced.
#[derive(Debug)]
pub enum Error {
    /// The project does not check, names no such service, or has none.
    Project(ProjectError),
    /// Every route of the selected services is one this version cannot represent.
    NothingToGenerate(Vec<String>),
    /// The generator refused.
    Generate(ClientError),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Project(e) => e.fmt(f),
            Error::Generate(e) => e.fmt(f),
            Error::NothingToGenerate(skipped) => {
                write!(
                    f,
                    "no route of the selected services is one this version of `bynk client` can represent"
                )?;
                for s in skipped {
                    write!(f, "\n  {s}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for Error {}

/// The generated module and the routes it leaves out, with why.
#[derive(Debug)]
pub struct Client {
    pub text: String,
    pub skipped: Vec<String>,
}

/// `bynk client`: analyse the project at `root`, write the module to `out` (or
/// stdout).
pub fn run(root: &Path, services: &[String], out: Option<&Path>) -> ExitCode {
    let analysis = openapi::analyse(root);
    let names: Vec<&str> = services.iter().map(String::as_str).collect();
    let client = match generate(&analysis, &names) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("bynk: {e}");
            return ExitCode::FAILURE;
        }
    };
    for s in &client.skipped {
        eprintln!("bynk: skipped {s}");
    }
    match out {
        None => {
            print!("{}", client.text);
            ExitCode::SUCCESS
        }
        Some(path) => match std::fs::write(path, &client.text) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("bynk: cannot write {}: {e}", path.display());
                ExitCode::FAILURE
            }
        },
    }
}

/// The client for `analysis`' `from http` services named by `want` (every one when
/// empty).
pub fn generate(analysis: &ProjectDiagnostics, want: &[&str]) -> Result<Client, Error> {
    let errors = openapi::errors_of(analysis);
    if !errors.is_empty() {
        return Err(Error::Project(ProjectError::Invalid(errors)));
    }
    let selected = openapi::select_services(analysis, want).map_err(Error::Project)?;
    let context_count = real_context_count(&analysis.boundary_info, &analysis.unit_sources);
    let mut services = Vec::new();
    let mut skipped = Vec::new();
    for sel in &selected {
        let routes = routes_of(analysis, sel, context_count, &mut skipped);
        if routes.is_empty() {
            continue;
        }
        services.push(ServiceInput {
            unit: sel.unit.to_string(),
            name: sel.service.name.name.clone(),
            doc: sel
                .service
                .documentation
                .as_ref()
                .map(|d| d.text.trim().to_string()),
            types: sel.info.types.clone(),
            routes,
        });
    }
    if services.is_empty() {
        return Err(Error::NothingToGenerate(skipped));
    }
    let input = HttpClientInput {
        services,
        compiler_version: env!("CARGO_PKG_VERSION").to_string(),
    };
    let emitted = emit_http_client(&input).map_err(Error::Generate)?;
    skipped.extend(emitted.skipped);
    Ok(Client {
        text: emitted.text,
        skipped,
    })
}

/// A service's routes this version represents; the others go to `skipped`.
fn routes_of(
    analysis: &ProjectDiagnostics,
    sel: &Selected,
    context_count: usize,
    skipped: &mut Vec<String>,
) -> Vec<RouteInput> {
    let expr_types = unit_expr_types(analysis, sel.unit);
    let mut routes = Vec::new();
    for h in &sel.service.handlers {
        if !matches!(lower_handler_kind_ir(&h.kind), IrHandlerKind::Http { .. }) {
            continue;
        }
        let Some(model) = wire_contract_for_service(
            sel.unit,
            "",
            &sel.service.name.name,
            h,
            sel.info,
            &expr_types,
            &analysis.ty_intern,
            context_count,
        ) else {
            continue;
        };
        let BoundaryKind::Http { method, path } = &model.kind else {
            continue;
        };
        match route_of(sel, h, &model, *method, path) {
            Ok(r) => routes.push(r),
            Err(why) => skipped.push(format!(
                "`{} {path}` of `{}`: {why}",
                method.as_str(),
                sel.service.name.name
            )),
        }
    }
    routes
}

/// Every file of `unit`'s checked expression types, as `bynk openapi` reads them.
fn unit_expr_types(
    analysis: &ProjectDiagnostics,
    unit: &str,
) -> Vec<(bynk_syntax::span::Span, bynk_check::checker::TyId)> {
    analysis
        .unit_sources
        .get(unit)
        .into_iter()
        .flatten()
        .filter_map(|p| analysis.expr_types.get(p))
        .flatten()
        .cloned()
        .collect()
}

/// One route as the generator takes it, or why this version leaves it out.
fn route_of(
    sel: &Selected,
    h: &Handler,
    model: &WireContractModel,
    method: IrHttpMethod,
    path: &str,
) -> Result<RouteInput, String> {
    unsupported(sel.info, h)?;
    let in_path: Vec<&str> = path
        .split('/')
        .filter_map(|s| s.strip_prefix(':'))
        .collect();
    let mut params = Vec::new();
    for p in &h.params {
        let name = p.name.name.as_str();
        if name == "body" {
            params.push(ParamInput::Body {
                ty: p.type_ref.clone(),
            });
        } else if in_path.contains(&name) {
            params.push(ParamInput::Path {
                name: name.to_string(),
                ty: p.type_ref.clone(),
            });
        } else if name == "query" && matches!(p.type_ref, TypeRef::Named(_)) {
            params.push(ParamInput::Query {
                ty: p.type_ref.clone(),
            });
        }
        // The raw `query` view and `headers` declare nothing a caller supplies.
    }
    let success = match success_of(h) {
        Some(TypeRef::Unit(_)) | None => None,
        Some(t) => Some(t.clone()),
    };
    Ok(RouteInput {
        method: method.as_str().to_string(),
        path: path.to_string(),
        doc: h.documentation.as_ref().map(|d| d.text.trim().to_string()),
        params,
        success,
        replies: replies_of(model, effective_json_options(sel.service.json.as_ref(), h))?,
    })
}

/// The route's features this version cannot represent, if any.
fn unsupported(info: &ContextBoundaryInfo, h: &Handler) -> Result<(), String> {
    let note = |name: &str| h.annotations.iter().any(|a| a.name.name == name);
    if h.raw_content_type().is_some() {
        return Err("a `@raw` body is not supported yet".to_string());
    }
    if h.params
        .iter()
        .any(|p| p.name.name == "query" && matches!(p.type_ref, TypeRef::RequestQuery(_)))
    {
        return Err("a raw `query: RequestQuery` is not supported yet".to_string());
    }
    if note("form") || note("multipart") {
        return Err("a `@form` or `@multipart` body is not supported yet".to_string());
    }
    if h.params.iter().any(|p| p.name.name == "cookies") {
        return Err("a typed `cookies` record is not supported yet".to_string());
    }
    if bynk_check::actors::signature_seam_for(h, &info.actors).is_some() {
        return Err(
            "the route is guarded by a request signature, which a client without the secret cannot make"
                .to_string(),
        );
    }
    Ok(())
}

/// `Effect[HttpResult[T]]` or `HttpResult[T]` to `T`.
fn success_of(h: &Handler) -> Option<&TypeRef> {
    let t = match &h.return_type {
        TypeRef::Effect(inner, _) => inner.as_ref(),
        other => other,
    };
    match t {
        TypeRef::HttpResult(inner, _) => Some(inner.as_ref()),
        _ => None,
    }
}

/// The answers the handler can make: the set `bynk openapi` documents.
fn replies_of(model: &WireContractModel, _json: JsonOptions) -> Result<Vec<ReplyInput>, String> {
    let mut out = Vec::new();
    for resp in &model.responses {
        if resp.origin == ResponseOrigin::DeclaredSuccess && !model.ok_reachable {
            continue;
        }
        let payload = if let ResponseOrigin::BoundaryImplicit { .. } = &resp.origin {
            match resp.variant.as_str() {
                "StructuralMismatch" => ReplyPayload::BoundaryError,
                "PayloadTooLarge" | "UnsupportedMediaType" => ReplyPayload::BoundaryRefusal,
                _ => continue,
            }
        } else {
            let Some(v) = http_variant(&resp.variant) else {
                continue;
            };
            match v.payload {
                HttpVariantPayload::Value => ReplyPayload::Value,
                HttpVariantPayload::Message => ReplyPayload::Message,
                HttpVariantPayload::Location => ReplyPayload::Location,
                HttpVariantPayload::None => ReplyPayload::None,
                HttpVariantPayload::Streamed
                | HttpVariantPayload::Raw
                | HttpVariantPayload::RawTagged
                | HttpVariantPayload::RawStreamed
                | HttpVariantPayload::RawError => {
                    return Err(format!(
                        "the route can answer `{}`, a raw or streamed body, which is not supported yet",
                        resp.variant
                    ));
                }
            }
        };
        out.push(ReplyInput {
            status: resp.status,
            variant: resp.variant.clone(),
            payload,
        });
    }
    Ok(out)
}

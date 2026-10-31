//! `bynk openapi` (#856) — an OpenAPI 3.1 document for a project's `from http`
//! services, a lossless re-projection of facts the compiler already holds.
//!
//! Nothing here re-derives the boundary. Routes and params come from the
//! retained service declarations; each type's shape comes from the shared
//! wire-contract IR (`bynk_check::wire`, #855 DECISION C) that also drives the
//! emitter's codecs and the editor's wire peek; and each route's response set
//! is the peek's body-walked set (`bynk_ide::wire_contract`, DECISION B), so
//! the spec cannot drift from either.
//!
//! # Decision A: refinements
//!
//! Every predicate a boundary type carries is one the codec re-validates, and
//! every one has a JSON Schema equivalent (`minLength`, `minimum`, `pattern`,
//! …). Cross-field predicates (`implies`) are agent invariants, never wire
//! types, so none reaches this document. Each refined schema still carries the
//! predicates as written in `x-bynk-refinements` and spells them out in its
//! `description`, so a consumer reading only the extension, or only the prose,
//! loses nothing.
//!
//! | Bynk                  | JSON Schema                                        |
//! |-----------------------|----------------------------------------------------|
//! | `NonEmpty`            | `minLength: 1`                                     |
//! | `MinLength(n)`        | `minLength: n`                                     |
//! | `MaxLength(n)`        | `maxLength: n`                                     |
//! | `Length(n)`           | `minLength: n`, `maxLength: n`                     |
//! | `Positive`            | `exclusiveMinimum: 0`                              |
//! | `NonNegative`         | `minimum: 0`                                       |
//! | `InRange(a, b)`       | `minimum: a`, `maximum: b`                         |
//! | `Matches("p")`        | `pattern: "^(?:p)$"` (the codec anchors the match) |

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use bynk_check::wire::{self, WireBody, WireInst, WireModel, WireRef, WireType};
use bynk_ide::wire_contract::{
    BoundaryKind, ResponseOrigin, WireContractModel, real_context_count, wire_contract_for_service,
};
use bynk_ide::{AnalysisRoots, ContextBoundaryInfo, ProjectDiagnostics, SchemaLock};
use bynk_syntax::ast::{
    BaseType, Handler, HttpMethod, HttpVariantPayload, PredKind, ServiceDecl, ServiceProtocol,
    TypeDecl, TypeRef, http_variant,
};
use bynk_syntax::error::Severity;
use serde_json::{Map, Value, json};

use crate::cli::OpenapiFormatArg;

/// The OpenAPI version the document declares.
pub const OPENAPI_VERSION: &str = "3.1.0";

/// The component the `400` boundary rejection is described once as.
const BOUNDARY_ERROR: &str = "BynkBoundaryError";

/// What names the document: the project's manifest identity.
#[derive(Debug, Clone)]
pub struct Info {
    pub title: String,
    pub version: String,
}

/// Why a document could not be produced.
#[derive(Debug)]
pub enum Error {
    /// The project has errors; the document would describe a program that does
    /// not compile.
    Invalid(Vec<String>),
    /// `--service` names no `from http` service.
    UnknownService { name: String, known: Vec<String> },
    /// The project has no `from http` service.
    NoHttpService,
    /// Two selected services answer the same method and path.
    DuplicateRoute { method: &'static str, path: String },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Invalid(lines) => {
                writeln!(f, "the project has errors; fix them first (`bynk check`):")?;
                for l in lines {
                    writeln!(f, "  {l}")?;
                }
                Ok(())
            }
            Error::UnknownService { name, known } if known.is_empty() => {
                write!(f, "no `from http` service `{name}`; the project has none")
            }
            Error::UnknownService { name, known } => write!(
                f,
                "no `from http` service `{name}`; the project's are: {}",
                known.join(", ")
            ),
            Error::NoHttpService => write!(f, "the project has no `from http` service"),
            Error::DuplicateRoute { method, path } => write!(
                f,
                "more than one service answers `{method} {path}`; pass `--service` to pick one"
            ),
        }
    }
}

impl std::error::Error for Error {}

// ---------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------

/// `bynk openapi`: analyse the project at `root`, write the document.
pub fn run(
    root: &Path,
    service: Option<&str>,
    out: Option<&Path>,
    format: Option<OpenapiFormatArg>,
) -> ExitCode {
    let analysis = analyse(root);
    let doc = match document(&manifest_info(root), &analysis, service) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("bynk: {e}");
            return ExitCode::FAILURE;
        }
    };
    let format = format.unwrap_or_else(|| match out.and_then(|p| p.extension()) {
        Some(e) if e == "yaml" || e == "yml" => OpenapiFormatArg::Yaml,
        _ => OpenapiFormatArg::Json,
    });
    let text = match format {
        OpenapiFormatArg::Json => {
            let mut s = serde_json::to_string_pretty(&doc).expect("a Value serialises");
            s.push('\n');
            s
        }
        OpenapiFormatArg::Yaml => to_yaml(&doc),
    };
    match out {
        None => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Some(path) => match std::fs::write(path, text) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("bynk: cannot write {}: {e}", path.display());
                ExitCode::FAILURE
            }
        },
    }
}

/// The project's analysis from the files on disk — the round `bynk check` and
/// the language server run.
fn analyse(root: &Path) -> ProjectDiagnostics {
    let roots = AnalysisRoots::Project(root.to_path_buf());
    let mut content: HashMap<PathBuf, String> = HashMap::new();
    let manifest = root.join("bynk.toml");
    if let Ok(text) = std::fs::read_to_string(&manifest) {
        content.insert(manifest, text);
    }
    for path in bynk_ide::discover_files(&roots, &content) {
        if let Ok(text) = std::fs::read_to_string(&path) {
            content.insert(path, text);
        }
    }
    let lock = match std::fs::read_to_string(root.join("bynk.schema.lock")) {
        Ok(text) => SchemaLock::On {
            existing: Some(text),
        },
        Err(_) => SchemaLock::Off,
    };
    bynk_ide::diagnose_project_with(&roots, &content, &lock)
}

/// The document's `info` from `bynk.toml`'s `[project]` / `[package]`, falling
/// back to the directory's name.
fn manifest_info(root: &Path) -> Info {
    let table: Option<toml::Table> = std::fs::read_to_string(root.join("bynk.toml"))
        .ok()
        .and_then(|t| t.parse().ok());
    let field = |key: &str| {
        let t = table.as_ref()?;
        ["project", "package"]
            .iter()
            .find_map(|s| t.get(*s)?.get(key)?.as_str().map(str::to_string))
    };
    Info {
        title: field("name").unwrap_or_else(|| {
            root.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "bynk".to_string())
        }),
        version: field("version").unwrap_or_else(|| "0.0.0".to_string()),
    }
}

// ---------------------------------------------------------------------
// The document
// ---------------------------------------------------------------------

/// One `from http` service selected for the document.
struct Selected<'a> {
    unit: &'a str,
    service: &'a ServiceDecl,
    info: &'a ContextBoundaryInfo,
}

/// Build the OpenAPI document for `analysis`' `from http` services — every
/// one, or only `service` (matched as `Name` or `unit.Name`).
pub fn document(
    info: &Info,
    analysis: &ProjectDiagnostics,
    service: Option<&str>,
) -> Result<Value, Error> {
    let errors = errors_of(analysis);
    if !errors.is_empty() {
        return Err(Error::Invalid(errors));
    }

    let mut all: Vec<Selected> = Vec::new();
    for (unit, ctx) in &analysis.boundary_info {
        // A synthetic unit (the `bynk` capability surface) has no source.
        if !analysis.unit_sources.contains_key(unit) {
            continue;
        }
        for svc in ctx.services.values() {
            if matches!(svc.protocol, ServiceProtocol::Http) {
                all.push(Selected {
                    unit,
                    service: svc,
                    info: ctx,
                });
            }
        }
    }
    all.sort_by(|a, b| (a.unit, &a.service.name.name).cmp(&(b.unit, &b.service.name.name)));

    let selected: Vec<&Selected> = match service {
        None => all.iter().collect(),
        Some(want) => all
            .iter()
            .filter(|s| {
                s.service.name.name == want || format!("{}.{}", s.unit, s.service.name.name) == want
            })
            .collect(),
    };
    if selected.is_empty() {
        return Err(match service {
            Some(name) => Error::UnknownService {
                name: name.to_string(),
                known: all.iter().map(|s| s.service.name.name.clone()).collect(),
            },
            None => Error::NoHttpService,
        });
    }

    let context_count = real_context_count(&analysis.boundary_info, &analysis.unit_sources);
    let mut models: Vec<(&Selected, &Handler, WireContractModel)> = Vec::new();
    for sel in &selected {
        let expr_types = unit_expr_types(analysis, sel.unit);
        for h in &sel.service.handlers {
            if !matches!(h.kind, bynk_syntax::ast::HandlerKind::Http { .. }) {
                continue;
            }
            if let Some(m) = wire_contract_for_service(
                sel.unit,
                "",
                &sel.service.name.name,
                h,
                sel.info,
                &expr_types,
                &analysis.ty_intern,
                context_count,
            ) {
                models.push((sel, h, m));
            }
        }
    }

    let names = SchemaNames::new(&models);
    let mut components = Components::default();
    let mut paths: BTreeMap<String, Map<String, Value>> = BTreeMap::new();
    let mut routes: BTreeSet<(String, String)> = BTreeSet::new();

    for (sel, handler, model) in &models {
        let BoundaryKind::Http { method, path } = &model.kind else {
            continue;
        };
        let r = Renderer {
            unit: sel.unit,
            types: &sel.info.types,
            model: &model.boundary,
            names: &names,
        };
        let op = operation(&r, &mut components, sel, handler, model, *method, path);
        // OpenAPI treats templated paths that differ only in parameter names
        // (`/a/{id}`, `/a/{key}`) as the same path.
        let shape = openapi_path(path)
            .split('/')
            .map(|s| if s.starts_with('{') { "{}" } else { s })
            .collect::<Vec<_>>()
            .join("/");
        let item = paths.entry(openapi_path(path)).or_default();
        let key = method.as_str().to_lowercase();
        if !routes.insert((key.clone(), shape)) || item.insert(key, op).is_some() {
            return Err(Error::DuplicateRoute {
                method: method.as_str(),
                path: path.clone(),
            });
        }
    }

    let tags: Vec<Value> = selected
        .iter()
        .map(|s| {
            let mut t = Map::new();
            t.insert("name".into(), json!(s.service.name.name));
            if let Some(d) = &s.service.documentation {
                t.insert("description".into(), json!(d.text.trim()));
            }
            Value::Object(t)
        })
        .collect();

    let mut doc = Map::new();
    doc.insert("openapi".into(), json!(OPENAPI_VERSION));
    doc.insert(
        "info".into(),
        json!({ "title": info.title, "version": info.version }),
    );
    doc.insert("tags".into(), Value::Array(tags));
    doc.insert(
        "paths".into(),
        Value::Object(
            paths
                .into_iter()
                .map(|(p, item)| (p, Value::Object(item)))
                .collect(),
        ),
    );
    if !components.schemas.is_empty() {
        doc.insert(
            "components".into(),
            json!({ "schemas": Value::Object(components.schemas.into_iter().collect()) }),
        );
    }
    Ok(sorted(Value::Object(doc)))
}

/// Rebuild `v` with every object's keys in sorted order. `serde_json`'s map is
/// sorted unless some crate in the build enables `preserve_order` (the grammar
/// tool does), and the document must not depend on which crates were built
/// beside this one.
fn sorted(v: Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut entries: Vec<(String, Value)> = m.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(entries.into_iter().map(|(k, v)| (k, sorted(v))).collect())
        }
        Value::Array(a) => Value::Array(a.into_iter().map(sorted).collect()),
        other => other,
    }
}

/// `path:line: message` for each error the analysis holds, cheapest first.
fn errors_of(analysis: &ProjectDiagnostics) -> Vec<String> {
    let mut out = Vec::new();
    for f in &analysis.files {
        for d in &f.diagnostics {
            if d.severity == Severity::Error {
                let line = f
                    .text
                    .get(..d.error.span.start)
                    .unwrap_or("")
                    .matches('\n')
                    .count()
                    + 1;
                out.push(format!(
                    "{}:{line}: {}",
                    f.source_path.to_string_lossy().replace('\\', "/"),
                    d.error.message
                ));
            }
        }
    }
    for d in &analysis.unattributed {
        if d.severity == Severity::Error {
            out.push(d.error.message.clone());
        }
    }
    out
}

/// Every file of `unit`'s checked expression types — the response walk
/// filters by the handler's own file id, so the union is safe.
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

/// `/check/:client` → `/check/{client}`.
fn openapi_path(path: &str) -> String {
    path.split('/')
        .map(|seg| match seg.strip_prefix(':') {
            Some(name) => format!("{{{name}}}"),
            None => seg.to_string(),
        })
        .collect::<Vec<_>>()
        .join("/")
}

// ---------------------------------------------------------------------
// Operations
// ---------------------------------------------------------------------

fn operation(
    r: &Renderer,
    components: &mut Components,
    sel: &Selected,
    handler: &Handler,
    model: &WireContractModel,
    method: HttpMethod,
    path: &str,
) -> Value {
    let mut op = Map::new();
    op.insert("tags".into(), json!([sel.service.name.name]));
    op.insert(
        "operationId".into(),
        json!(operation_id(&sel.service.name.name, method, path)),
    );
    if let Some(d) = &handler.documentation {
        let text = d.text.trim();
        if let Some(first) = text.lines().next() {
            op.insert("summary".into(), json!(first.trim()));
        }
        op.insert("description".into(), json!(text));
    }

    let in_path: BTreeSet<&str> = path
        .split('/')
        .filter_map(|s| s.strip_prefix(':'))
        .collect();
    let mut parameters = Vec::new();
    let mut request_body = None;
    for p in &handler.params {
        let name = p.name.name.as_str();
        if name == "body" {
            request_body = Some(request_body_of(r, components, handler, &p.type_ref));
        } else if in_path.contains(name) {
            parameters.push(json!({
                "name": name,
                "in": "path",
                "required": true,
                "schema": r.parameter_schema(components, &p.type_ref),
            }));
        }
        // Any other param is a `headers` (the request's headers, read-only and
        // untyped here); the checker admits no other.
    }
    if !parameters.is_empty() {
        op.insert("parameters".into(), Value::Array(parameters));
    }
    if let Some(b) = request_body {
        op.insert("requestBody".into(), b);
    }

    op.insert(
        "responses".into(),
        responses_of(r, components, handler, model),
    );
    Value::Object(op)
}

/// `api` + `GET` + `/check/:client` → `api_get_check_client`.
fn operation_id(service: &str, method: HttpMethod, path: &str) -> String {
    let mut id = format!("{service}_{}", method.as_str().to_lowercase());
    for seg in path.split('/').filter(|s| !s.is_empty()) {
        id.push('_');
        id.extend(
            seg.trim_start_matches(':')
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }),
        );
    }
    id
}

fn request_body_of(
    r: &Renderer,
    components: &mut Components,
    handler: &Handler,
    ty: &TypeRef,
) -> Value {
    if let Some(ct) = handler.raw_content_type() {
        return json!({
            "required": true,
            "content": { ct: { "schema": { "type": "string", "format": "binary" } } },
        });
    }
    let shape = wire::wire_ref(ty, r.types);
    json!({
        "required": true,
        "content": { "application/json": { "schema": r.ref_schema(components, &shape) } },
    })
}

fn responses_of(
    r: &Renderer,
    components: &mut Components,
    handler: &Handler,
    model: &WireContractModel,
) -> Value {
    let success = http_result_inner(&handler.return_type);
    /// One status's accumulated descriptions, bodies and headers.
    #[derive(Default)]
    struct Entry {
        descriptions: Vec<String>,
        /// `(media type, schema)`, deduplicated.
        bodies: Vec<(String, Value)>,
        headers: Map<String, Value>,
    }
    let mut by_status: BTreeMap<u16, Entry> = BTreeMap::new();
    for resp in &model.responses {
        // The declared success is listed whether or not the body returns it; the
        // document lists only what the handler can answer (Decision B).
        if resp.origin == ResponseOrigin::DeclaredSuccess && !model.ok_reachable {
            continue;
        }
        let e = by_status.entry(resp.status).or_default();
        let mut describe = |d: String| {
            if !e.descriptions.contains(&d) {
                e.descriptions.push(d);
            }
        };
        let body = |media: &str, schema: Value, e: &mut Entry| {
            if !e.bodies.iter().any(|(m, s)| m == media && *s == schema) {
                e.bodies.push((media.to_string(), schema));
            }
        };
        if let ResponseOrigin::BoundaryImplicit { why } = &resp.origin {
            describe(format!("{} — {why}", resp.variant));
            match resp.variant.as_str() {
                "StructuralMismatch" => {
                    components.boundary_error();
                    body(
                        "application/json",
                        json!({ "$ref": format!("#/components/schemas/{BOUNDARY_ERROR}") }),
                        e,
                    );
                }
                "PayloadTooLarge" | "UnsupportedMediaType" => body(
                    "application/json",
                    json!({
                        "type": "object",
                        "required": ["kind", "details"],
                        "properties": {
                            "kind": { "const": resp.variant },
                            "details": { "type": "string" },
                        },
                    }),
                    e,
                ),
                _ => {}
            }
            continue;
        }
        describe(resp.variant.clone());
        let Some(v) = http_variant(&resp.variant) else {
            continue;
        };
        match v.payload {
            HttpVariantPayload::Value => {
                if let Some(t) = success
                    && !matches!(t, TypeRef::Unit(_))
                {
                    let shape = wire::wire_ref(t, r.types);
                    body("application/json", r.ref_schema(components, &shape), e);
                }
            }
            HttpVariantPayload::Message => body(
                "application/json",
                json!({
                    "type": "object",
                    "required": ["error"],
                    "properties": { "error": { "type": "string" } },
                }),
                e,
            ),
            HttpVariantPayload::Location => {
                e.headers.insert(
                    "Location".into(),
                    json!({ "required": true, "schema": { "type": "string" } }),
                );
            }
            HttpVariantPayload::Streamed => {
                body("text/event-stream", json!({ "type": "string" }), e);
            }
            HttpVariantPayload::Raw
            | HttpVariantPayload::RawTagged
            | HttpVariantPayload::RawStreamed
            | HttpVariantPayload::RawError => {
                // Author-owned bytes under an author-chosen media type; no codec.
                body("*/*", json!({ "type": "string", "format": "binary" }), e);
                if v.payload == HttpVariantPayload::RawTagged {
                    e.headers.insert(
                        "ETag".into(),
                        json!({ "required": true, "schema": { "type": "string" } }),
                    );
                }
            }
            HttpVariantPayload::None => {}
        }
    }

    let mut out = Map::new();
    for (status, e) in by_status {
        let mut resp = Map::new();
        resp.insert("description".into(), json!(e.descriptions.join("; ")));
        if !e.headers.is_empty() {
            resp.insert("headers".into(), Value::Object(e.headers));
        }
        if !e.bodies.is_empty() {
            let mut by_media: BTreeMap<String, Vec<Value>> = BTreeMap::new();
            for (m, s) in e.bodies {
                by_media.entry(m).or_default().push(s);
            }
            let content: Map<String, Value> = by_media
                .into_iter()
                .map(|(m, mut schemas)| {
                    let schema = if schemas.len() == 1 {
                        schemas.remove(0)
                    } else {
                        json!({ "oneOf": schemas })
                    };
                    (m, json!({ "schema": schema }))
                })
                .collect();
            resp.insert("content".into(), Value::Object(content));
        }
        out.insert(status.to_string(), Value::Object(resp));
    }
    Value::Object(out)
}

/// `Effect[HttpResult[T]]` / `HttpResult[T]` → `T`.
fn http_result_inner(t: &TypeRef) -> Option<&TypeRef> {
    let t = match t {
        TypeRef::Effect(inner, _) => inner.as_ref(),
        other => other,
    };
    match t {
        TypeRef::HttpResult(inner, _) => Some(inner.as_ref()),
        _ => None,
    }
}

// ---------------------------------------------------------------------
// Schemas
// ---------------------------------------------------------------------

/// The `components/schemas` collected while rendering.
#[derive(Default)]
struct Components {
    schemas: BTreeMap<String, Value>,
}

impl Components {
    /// Insert once: a second request for the same name is the same type.
    fn put(&mut self, name: String, make: impl FnOnce() -> Value) {
        self.schemas.entry(name).or_insert_with(make);
    }

    /// The `400` body the boundary answers a malformed or invalid request with
    /// (`runtime/boundary.ts`'s `BoundaryError`, minus the caller-side
    /// `Transport`).
    fn boundary_error(&mut self) {
        self.put(BOUNDARY_ERROR.to_string(), || {
            json!({
                "description": "A request the boundary rejected: malformed JSON, a value of the \
                                wrong shape, or one that violates a refinement.",
                "oneOf": [
                    {
                        "type": "object",
                        "required": ["kind", "details"],
                        "properties": {
                            "kind": { "const": "MalformedJson" },
                            "details": { "type": "string" },
                        },
                    },
                    {
                        "type": "object",
                        "required": ["kind", "path", "expected", "actual"],
                        "properties": {
                            "kind": { "const": "StructuralMismatch" },
                            "path": { "type": "string" },
                            "expected": { "type": "string" },
                            "actual": { "type": "string" },
                        },
                    },
                    {
                        "type": "object",
                        "required": ["kind", "path", "violation"],
                        "properties": {
                            "kind": { "const": "RefinementViolation" },
                            "path": { "type": "string" },
                            "violation": { "type": "object" },
                        },
                    },
                ],
            })
        });
    }
}

/// The component name each `(unit, type)` renders under: the bare name, unless
/// two units declare *different* types of that name, in which case both are
/// qualified by their unit (`orders.Money`, `billing.Money`).
struct SchemaNames {
    colliding: BTreeSet<String>,
}

impl SchemaNames {
    fn new(models: &[(&Selected, &Handler, WireContractModel)]) -> Self {
        // component name → the distinct declarations carrying it. A generic
        // instantiation's name is its codec key (`Page_Item`), and its identity
        // is the generic's declaration plus each argument's.
        let mut seen: HashMap<String, BTreeSet<String>> = HashMap::new();
        for (sel, _, model) in models {
            let decl_id = |name: &str| {
                sel.info
                    .types
                    .get(name)
                    .map(|d| format!("{}:{}", d.span.file.0, d.span.start))
                    .unwrap_or_default()
            };
            for t in &model.boundary.types {
                seen.entry(t.name.clone())
                    .or_default()
                    .insert(decl_id(&t.name));
            }
            for i in &model.boundary.instantiations {
                if let WireInst::RecordInst { name, args } | WireInst::SumInst { name, args } = i {
                    let mut id = decl_id(name);
                    for a in args {
                        id.push('|');
                        id.push_str(&wire::codec_suffix(a));
                        if let TypeRef::Named(n) = a {
                            id.push('@');
                            id.push_str(&decl_id(&n.name));
                        }
                    }
                    seen.entry(i.ts_name()).or_default().insert(id);
                }
            }
        }
        SchemaNames {
            colliding: seen
                .into_iter()
                .filter(|(_, decls)| decls.len() > 1)
                .map(|(n, _)| n)
                .collect(),
        }
    }

    fn of(&self, unit: &str, name: &str) -> String {
        if self.colliding.contains(name) {
            format!("{unit}.{name}")
        } else {
            name.to_string()
        }
    }
}

/// Renders one handler's boundary types as JSON Schema.
struct Renderer<'a> {
    unit: &'a str,
    types: &'a HashMap<String, std::sync::Arc<TypeDecl>>,
    model: &'a WireModel,
    names: &'a SchemaNames,
}

impl Renderer<'_> {
    fn component_ref(&self, name: &str) -> Value {
        json!({ "$ref": format!("#/components/schemas/{}", self.names.of(self.unit, name)) })
    }

    /// A path parameter's schema. A refined scalar is inlined so the
    /// parameter itself carries its constraints (`minLength: 1`), not only a
    /// reference to a component that does.
    fn parameter_schema(&self, components: &mut Components, ty: &TypeRef) -> Value {
        let shape = wire::wire_ref(ty, self.types);
        if let WireRef::Named { name } = &shape
            && let Some(t) = self.model.types.iter().find(|t| &t.name == name)
            && matches!(t.body, WireBody::Scalar(_))
        {
            // Still register the component, so the document names the type.
            self.ref_schema(components, &shape);
            return self.type_schema(components, t);
        }
        self.ref_schema(components, &shape)
    }

    /// The schema for one occurrence of a type.
    fn ref_schema(&self, components: &mut Components, r: &WireRef) -> Value {
        match r {
            WireRef::Base { base, .. } => Value::Object(base_schema(*base)),
            WireRef::Bytes => Value::Object(base_schema(BaseType::Bytes)),
            WireRef::Named { name } => {
                if let Some(t) = self.model.types.iter().find(|t| &t.name == name) {
                    let n = self.names.of(self.unit, name);
                    // Reserve before rendering so a self-referential type ends.
                    if !components.schemas.contains_key(&n) {
                        components.schemas.insert(n.clone(), Value::Null);
                        let v = self.type_schema(components, t);
                        components.schemas.insert(n, v);
                    }
                }
                self.component_ref(name)
            }
            WireRef::Inst { key } => self.inst_schema(components, key),
            WireRef::Unit => json!({ "type": "null" }),
            WireRef::Unchecked { .. } => json!({}),
        }
    }

    fn type_schema(&self, components: &mut Components, t: &WireType) -> Value {
        let doc = self
            .types
            .get(&t.name)
            .and_then(|d| d.documentation.as_ref())
            .map(|d| d.text.trim().to_string());
        let mut s = match &t.body {
            WireBody::Scalar(sc) => {
                let mut s = base_schema(sc.base);
                if sc.opaque {
                    s.insert("x-bynk-opaque".into(), json!(true));
                }
                refine(&mut s, &sc.predicates);
                s
            }
            WireBody::Record { fields } => {
                let mut props = Map::new();
                let mut required = Vec::new();
                for f in fields {
                    let mut fs = match self.ref_schema(components, &f.shape) {
                        Value::Object(m) => m,
                        other => Map::from_iter([("allOf".to_string(), json!([other]))]),
                    };
                    refine(&mut fs, &f.predicates);
                    props.insert(f.name.clone(), Value::Object(fs));
                    if f.default.is_none() {
                        required.push(json!(f.name));
                    }
                }
                let mut s = Map::new();
                s.insert("type".into(), json!("object"));
                s.insert("properties".into(), Value::Object(props));
                s.insert("required".into(), Value::Array(required));
                s
            }
            WireBody::Sum(sum) => {
                let variants: Vec<Value> = sum
                    .variants
                    .iter()
                    .map(|v| {
                        let fields: Vec<(String, Value, bool)> = v
                            .payload
                            .iter()
                            .map(|f| {
                                let mut fs = match self.ref_schema(components, &f.shape) {
                                    Value::Object(m) => m,
                                    other => {
                                        Map::from_iter([("allOf".to_string(), json!([other]))])
                                    }
                                };
                                refine(&mut fs, &f.predicates);
                                (f.name.clone(), Value::Object(fs), f.default.is_none())
                            })
                            .collect();
                        variant_schema(sum.wire_discriminant, &v.name, fields)
                    })
                    .collect();
                Map::from_iter([("oneOf".to_string(), Value::Array(variants))])
            }
        };
        if let Some(doc) = doc {
            let d = match s.remove("description") {
                Some(Value::String(r)) => format!("{doc}\n\n{r}"),
                _ => doc,
            };
            s.insert("description".into(), json!(d));
        }
        Value::Object(s)
    }

    fn inst_schema(&self, components: &mut Components, key: &str) -> Value {
        let Some(inst) = self
            .model
            .instantiations
            .iter()
            .find(|i| i.ts_name() == key)
        else {
            return json!({});
        };
        let shape = |t: &TypeRef| wire::wire_ref(t, self.types);
        match inst {
            WireInst::ListInst { elem } => {
                json!({ "type": "array", "items": self.ref_schema(components, &shape(elem)) })
            }
            // K6a (#2596): a JSON array, insertion-ordered, without duplicates.
            WireInst::SetInst { elem } => json!({
                "type": "array",
                "items": self.ref_schema(components, &shape(elem)),
                "uniqueItems": true,
            }),
            // Entries array, insertion-ordered: `[[k, v], …]`.
            WireInst::MapInst { key, val } => json!({
                "type": "array",
                "items": {
                    "type": "array",
                    "prefixItems": [
                        self.ref_schema(components, &shape(key)),
                        self.ref_schema(components, &shape(val)),
                    ],
                    "minItems": 2,
                    "maxItems": 2,
                },
            }),
            WireInst::OptionInst { inner } => {
                let v = self.ref_schema(components, &shape(inner));
                json!({ "oneOf": [
                    variant_schema(wire::WIRE_DISCRIMINANT, "Some", vec![("value".into(), v, true)]),
                    variant_schema(wire::WIRE_DISCRIMINANT, "None", vec![]),
                ]})
            }
            WireInst::ResultInst { ok, err } => {
                let o = self.ref_schema(components, &shape(ok));
                let e = self.ref_schema(components, &shape(err));
                json!({ "oneOf": [
                    variant_schema(wire::WIRE_DISCRIMINANT, "Ok", vec![("value".into(), o, true)]),
                    variant_schema(wire::WIRE_DISCRIMINANT, "Err", vec![("error".into(), e, true)]),
                ]})
            }
            WireInst::RecordInst { name, args } => {
                let n = self.names.of(self.unit, key);
                if let Some(fields) = wire::record_inst_fields(name, args, self.types) {
                    let mut props = Map::new();
                    let mut required = Vec::new();
                    for (fname, ty) in &fields {
                        props.insert(fname.clone(), self.ref_schema(components, &shape(ty)));
                        required.push(json!(fname));
                    }
                    components.put(
                        n.clone(),
                        || json!({ "type": "object", "properties": props, "required": required }),
                    );
                }
                json!({ "$ref": format!("#/components/schemas/{n}") })
            }
            WireInst::SumInst { name, args } => {
                let n = self.names.of(self.unit, key);
                if let Some(variants) = wire::sum_inst_variants(name, args, self.types) {
                    let vs: Vec<Value> = variants
                        .iter()
                        .map(|(vname, fields)| {
                            let fields = fields
                                .iter()
                                .map(|(f, t)| {
                                    (f.clone(), self.ref_schema(components, &shape(t)), true)
                                })
                                .collect();
                            variant_schema(wire::WIRE_DISCRIMINANT, vname, fields)
                        })
                        .collect();
                    components.put(n.clone(), || json!({ "oneOf": vs }));
                }
                json!({ "$ref": format!("#/components/schemas/{n}") })
            }
        }
    }
}

/// One sum variant: an object whose `kind` is the variant's name and whose
/// payload fields sit beside it.
fn variant_schema(discriminant: &str, name: &str, fields: Vec<(String, Value, bool)>) -> Value {
    let mut props = Map::new();
    props.insert(discriminant.to_string(), json!({ "const": name }));
    let mut required = vec![json!(discriminant)];
    for (f, schema, req) in fields {
        props.insert(f.clone(), schema);
        if req {
            required.push(json!(f));
        }
    }
    json!({ "type": "object", "properties": props, "required": required })
}

fn base_schema(b: BaseType) -> Map<String, Value> {
    let v = match b {
        BaseType::Int => json!({ "type": "integer" }),
        BaseType::Float => json!({ "type": "number" }),
        BaseType::Bool => json!({ "type": "boolean" }),
        BaseType::String => json!({ "type": "string" }),
        BaseType::Bytes => json!({ "type": "string", "contentEncoding": "base64" }),
        BaseType::Instant => json!({ "type": "integer", "x-bynk-base": "Instant" }),
        BaseType::Duration => json!({ "type": "number", "x-bynk-base": "Duration" }),
    };
    match v {
        Value::Object(m) => m,
        _ => unreachable!(),
    }
}

/// Apply `preds` to a schema (Decision A): the JSON Schema equivalent of each,
/// the predicates as written in `x-bynk-refinements`, and a description.
fn refine(s: &mut Map<String, Value>, preds: &[PredKind]) {
    if preds.is_empty() {
        return;
    }
    let mut patterns: Vec<String> = Vec::new();
    for p in preds {
        match p {
            PredKind::NonEmpty => tighten_min(s, "minLength", 1.0),
            PredKind::MinLength(n) => tighten_min(s, "minLength", *n as f64),
            PredKind::MaxLength(n) => tighten_max(s, "maxLength", *n as f64),
            PredKind::Length(n) => {
                tighten_min(s, "minLength", *n as f64);
                tighten_max(s, "maxLength", *n as f64);
            }
            PredKind::Positive => tighten_min(s, "exclusiveMinimum", 0.0),
            PredKind::NonNegative => tighten_min(s, "minimum", 0.0),
            PredKind::InRange(a, b) => {
                tighten_min(s, "minimum", a.value as f64);
                tighten_max(s, "maximum", b.value as f64);
            }
            PredKind::InRangeF(a, b) => {
                tighten_min(s, "minimum", a.value);
                tighten_max(s, "maximum", b.value);
            }
            // The codec anchors the whole value; JSON Schema's `pattern` searches.
            PredKind::Matches(p) => patterns.push(format!("^(?:{p})$")),
        }
    }
    let mut it = patterns.into_iter();
    if let Some(first) = it.next() {
        s.insert("pattern".into(), json!(first));
    }
    let more: Vec<Value> = it.map(|p| json!({ "pattern": p })).collect();
    if !more.is_empty() {
        s.insert("allOf".into(), Value::Array(more));
    }

    let written: Vec<String> = preds.iter().map(render_pred).collect();
    let prose: Vec<String> = preds.iter().map(pred_prose).collect();
    s.insert("x-bynk-refinements".into(), json!(written));
    let note = format!("Re-validated at the boundary: {}.", prose.join("; "));
    let d = match s.remove("description") {
        Some(Value::String(d)) => format!("{d}\n\n{note}"),
        _ => note,
    };
    s.insert("description".into(), json!(d));
}

fn num(n: f64) -> Value {
    if n.fract() == 0.0 && n.abs() < 9.0e15 {
        json!(n as i64)
    } else {
        json!(n)
    }
}

fn tighten_min(s: &mut Map<String, Value>, key: &str, n: f64) {
    let cur = s.get(key).and_then(Value::as_f64);
    if cur.is_none_or(|c| n > c) {
        s.insert(key.into(), num(n));
    }
}

fn tighten_max(s: &mut Map<String, Value>, key: &str, n: f64) {
    let cur = s.get(key).and_then(Value::as_f64);
    if cur.is_none_or(|c| n < c) {
        s.insert(key.into(), num(n));
    }
}

/// The predicate as written: `InRange(1, 100)`.
fn render_pred(p: &PredKind) -> String {
    match p {
        PredKind::Matches(re) => format!("Matches({re:?})"),
        PredKind::InRange(a, b) => format!("InRange({}, {})", a.value, b.value),
        PredKind::InRangeF(a, b) => format!("InRange({}, {})", a.lexeme, b.lexeme),
        PredKind::MinLength(n) => format!("MinLength({n})"),
        PredKind::MaxLength(n) => format!("MaxLength({n})"),
        PredKind::Length(n) => format!("Length({n})"),
        PredKind::NonNegative => "NonNegative".into(),
        PredKind::Positive => "Positive".into(),
        PredKind::NonEmpty => "NonEmpty".into(),
    }
}

/// The codec's own wording for the predicate's failure
/// (`pred_condition_and_message`), so the prose matches what a rejected
/// request says.
fn pred_prose(p: &PredKind) -> String {
    match p {
        PredKind::Matches(re) => format!("must match `{re}`"),
        PredKind::InRange(a, b) => format!("must be in range [{}, {}]", a.value, b.value),
        PredKind::InRangeF(a, b) => format!("must be in range [{}, {}]", a.lexeme, b.lexeme),
        PredKind::MinLength(n) => format!("length must be at least {n}"),
        PredKind::MaxLength(n) => format!("length must be at most {n}"),
        PredKind::Length(n) => format!("length must be exactly {n}"),
        PredKind::NonNegative => "must be non-negative".into(),
        PredKind::Positive => "must be positive".into(),
        PredKind::NonEmpty => "must be non-empty".into(),
    }
}

// ---------------------------------------------------------------------
// YAML
// ---------------------------------------------------------------------

/// Render `v` as block-style YAML. Strings are always double-quoted with
/// JSON's escapes (a YAML double-quoted scalar), so nothing a schema holds —
/// `"true"`, `"1.0"`, a regex — is read back as another type.
pub fn to_yaml(v: &Value) -> String {
    let mut out = String::new();
    match v {
        Value::Object(m) if !m.is_empty() => yaml_map(m, 0, &mut out),
        Value::Array(a) if !a.is_empty() => yaml_seq(a, 0, &mut out),
        other => {
            out.push_str(&yaml_scalar(other));
            out.push('\n');
        }
    }
    out
}

fn yaml_scalar(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => serde_json::to_string(s).expect("a string serialises"),
        Value::Array(_) => "[]".into(),
        Value::Object(_) => "{}".into(),
    }
}

fn yaml_key(k: &str) -> String {
    let plain = !k.is_empty()
        && k.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | '{' | '}'))
        && !k.starts_with(['-', '{', '.'])
        // YAML 1.1 reads these (any case) as booleans or null.
        && !matches!(
            k.to_ascii_lowercase().as_str(),
            "true" | "false" | "null" | "yes" | "no" | "on" | "off" | "y" | "n"
        )
        && k.parse::<f64>().is_err();
    if plain {
        k.to_string()
    } else {
        serde_json::to_string(k).expect("a string serialises")
    }
}

fn is_block(v: &Value) -> bool {
    matches!(v, Value::Object(m) if !m.is_empty()) || matches!(v, Value::Array(a) if !a.is_empty())
}

fn yaml_map(m: &Map<String, Value>, indent: usize, out: &mut String) {
    let pad = " ".repeat(indent);
    for (k, v) in m {
        out.push_str(&pad);
        out.push_str(&yaml_key(k));
        out.push(':');
        yaml_value(v, indent, out);
    }
}

fn yaml_seq(a: &[Value], indent: usize, out: &mut String) {
    let pad = " ".repeat(indent);
    for v in a {
        out.push_str(&pad);
        out.push('-');
        match v {
            Value::Object(m) if !m.is_empty() => {
                // First key on the dash's line, the rest aligned under it.
                let mut inner = String::new();
                yaml_map(m, indent + 2, &mut inner);
                out.push(' ');
                out.push_str(inner.trim_start());
            }
            Value::Array(inner) if !inner.is_empty() => {
                out.push('\n');
                yaml_seq(inner, indent + 2, out);
            }
            other => {
                out.push(' ');
                out.push_str(&yaml_scalar(other));
                out.push('\n');
            }
        }
    }
}

fn yaml_value(v: &Value, indent: usize, out: &mut String) {
    if is_block(v) {
        out.push('\n');
        match v {
            Value::Object(m) => yaml_map(m, indent + 2, out),
            Value::Array(a) => yaml_seq(a, indent + 2, out),
            _ => unreachable!(),
        }
    } else {
        out.push(' ');
        out.push_str(&yaml_scalar(v));
        out.push('\n');
    }
}

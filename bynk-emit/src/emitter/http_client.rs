//! `bynk client` (#3508, slice BC1): the typed TypeScript client for a project's
//! `from http` services, one module a front end imports.
//!
//! The command in `bynk/src/http_client.rs` reads each route's facts from the
//! checked project (the wire-contract peek `bynk openapi` reads) and hands them
//! over as an [`HttpClientInput`]; this module turns them into the module's text.
//!
//! # One codec, one source of truth
//!
//! Nothing here is a second implementation of the boundary. The codecs are the
//! ones a Bynk *consumer* generates for a callee's types
//! (`serialisation::emit_helpers_for_owner_qualified` under a qualifier, which
//! inlines a transparent refinement's check and casts an opaque one), and each
//! call site names them through `serialise_expr_via` / `deserialise_ref_via`,
//! exactly as the foreign-caller client (`client.rs`, #1970) does. What differs is
//! the half the codec does not own:
//!
//! - **Optional values are `T | null`** in the generated types (the maintainer's
//!   decision on #3508), not Bynk's tagged `Option`. The codecs still speak
//!   `Option`, so each boundary type is declared twice: the *wire* type
//!   (`__Wire_Order`, what a codec produces and consumes) and the *public* type
//!   (`Order`, what a caller sees), with a generated function per type between
//!   them. A type with no `Option` reachable has no conversion, and the two are
//!   the same shape.
//! - **Refinements are checked on the way out** by running the decoder over the
//!   value just encoded, so the predicates are the server's own and a violation is
//!   returned as `CallFailure` before anything is sent. On the way in the
//!   response is decoded by the same codecs.
//! - **A route's answer is a union keyed by status**, built from the response set
//!   `bynk openapi` documents, and never thrown.
//!
//! The runtime half is `http_client_runtime.ts` (generated from `runtime/src/client/http_client.ts`
//! by `bundle.mjs`), inlined: a browser-safe subset
//! with no signing and no Workers types, plus the runtime's own `result`, `bytes`,
//! `decimal` and `keys` modules for just the names the codecs use.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::sync::Arc;

use bynk_check::contract;
use bynk_check::project_model::TypeDecl;
use bynk_check::wire::TypeBody;
use bynk_check::wire::TypeRef;
use bynk_ts::{
    TsArrowBody, TsBinaryOp, TsDecl, TsExpr, TsLit, TsObjectEntry, TsParam, TsProgram, TsStmt,
    TsType, TsTypeMember,
};

use super::client::{SurfaceClosure, surface_closure};
use super::runtime_use::RuntimeUse;
use super::serialisation;

mod route;

/// The generated module's own type names. A boundary type of the same name is
/// renamed (`Client` to `Client_`), never the surface.
const SURFACE_TYPES: &[&str] = &[
    "BoundaryError",
    "CallFailure",
    "Client",
    "ClientOptions",
    "JsonValue",
    "Option",
    "Result",
    "ValidationError",
];

/// The wire shape of the standard `File`, a `@multipart` file part.
const FILE_TYPE: &str = "export interface __Wire_File {\n  readonly filename: Option<string>;\n  readonly contentType: string;\n  readonly bytes: globalThis.Uint8Array;\n}\n";

/// A file part's codec, which only the stand-in a multipart check passes reaches.
const FILE_CODEC: &str = "export function __serialise_File(value: __Wire_File): __JsonValue {\n  return {\n    filename: value.filename.tag === \"Some\" ? { kind: \"Some\", value: value.filename.value } : { kind: \"None\" },\n    contentType: value.contentType,\n    bytes: __bynkBytesToBase64(value.bytes),\n  };\n}\n\nexport function __deserialise_File(json: __JsonValue, path: string = \"$\"): Result<__Wire_File, __BoundaryError> {\n  if (typeof json !== \"object\" || json === null || globalThis.Array.isArray(json)) {\n    return Err({ kind: \"StructuralMismatch\", path, expected: \"object\", actual: typeof json });\n  }\n  return Ok(json as unknown as __Wire_File);\n}\n";

/// The names [`FILE_TYPE`] uses without declaring (1A.36a, Decision D).
const FILE_TYPE_USES: &[&str] = &["Option"];

/// The names [`FILE_CODEC`] uses without declaring: the wire type [`FILE_TYPE`]
/// declares, the codec runtime and the result vocabulary.
const FILE_CODEC_USES: &[&str] = &[
    "__Wire_File",
    "__JsonValue",
    "__bynkBytesToBase64",
    "__BoundaryError",
    "Result",
    "Ok",
    "Err",
];

/// The prefix of a boundary type's wire declaration, the shape its codec makes.
const WIRE_PREFIX: &str = "__Wire_";

/// The runtime modules the codecs may reach, in dependency order: `bytes`, `decimal`,
/// `time` (`IsoDate`), `keys` (`Map` and `Set`, which reads `decimal`), `text`
/// (the code-point length of a `String` predicate).
const OPTIONAL_MODULES: [(&str, &str); 5] = [
    ("bytes", BYTES_MODULE),
    ("decimal", DECIMAL_MODULE),
    ("time", TIME_MODULE),
    ("keys", KEYS_MODULE),
    ("text", TEXT_MODULE),
];

const RUNTIME: &str = include_str!("http_client_runtime.ts");
const RESULT_MODULE: &str = include_str!("../../runtime/src/result.ts");
const BYTES_MODULE: &str = include_str!("../../runtime/src/bytes.ts");
const DECIMAL_MODULE: &str = include_str!("../../runtime/src/decimal.ts");
const KEYS_MODULE: &str = include_str!("../../runtime/src/keys.ts");
const TIME_MODULE: &str = include_str!("../../runtime/src/time.ts");
const TEXT_MODULE: &str = include_str!("../../runtime/src/text.ts");

/// The `bynk client` experimental notice, in the generated header.
const EXPERIMENTAL: &str = "Experimental: member names and the shape of this module may change in a minor version until it is stabilised.";

/// What the command hands the generator.
#[derive(Debug, Clone)]
pub struct HttpClientInput {
    pub services: Vec<ServiceInput>,
    /// The compiler's version, for the header.
    pub compiler_version: String,
}

/// One `from http` service.
#[derive(Debug, Clone)]
pub struct ServiceInput {
    pub unit: String,
    pub name: String,
    pub doc: Option<String>,
    /// The type namespace the service's signatures are written in.
    pub types: HashMap<String, Arc<TypeDecl>>,
    pub routes: Vec<RouteInput>,
}

/// One route of a service.
#[derive(Debug, Clone)]
pub struct RouteInput {
    /// `GET`, `POST`, ...
    pub method: String,
    /// `/orders/:id`.
    pub path: String,
    pub doc: Option<String>,
    /// In handler order.
    pub params: Vec<ParamInput>,
    /// The declared success type; `None` when it carries no value.
    pub success: Option<TypeRef>,
    pub replies: Vec<ReplyInput>,
}

/// What a route takes.
#[derive(Debug, Clone)]
pub enum ParamInput {
    /// A path segment, typed.
    Path { name: String, ty: TypeRef },
    /// The body, in the form its route reads it.
    Body { ty: TypeRef, kind: BodyKind },
    /// The typed `query` record, whose type is a named record.
    Query { ty: TypeRef },
    /// The typed `cookies` record, whose type is a named record.
    Cookies { ty: TypeRef },
    /// The typed `headers` record, whose type is a named record.
    Headers { ty: TypeRef },
}

/// How a route reads its `body`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BodyKind {
    /// JSON, the default.
    Json,
    /// `@form`: `application/x-www-form-urlencoded`.
    Form,
    /// `@multipart`: `multipart/form-data`.
    Multipart,
    /// `@raw`: the caller's bytes, sent as written under the author's content type
    /// (`None` for the wildcard `*/*`, which names none).
    Raw { content_type: Option<String> },
}

/// One answer a route can make, in `bynk openapi`'s terms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplyInput {
    pub status: u16,
    /// The `HttpResult` variant name, or the boundary's own (`StructuralMismatch`).
    pub variant: String,
    pub payload: ReplyPayload,
}

/// What an answer's body carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReplyPayload {
    /// The route's success value.
    Value,
    /// `{ "error": string }`.
    Message,
    /// A `Location` header.
    Location,
    /// Nothing.
    None,
    /// The boundary's own `400` body: a `BoundaryError`.
    BoundaryError,
    /// The boundary's own `413`/`415` body: `{ kind, details }`.
    BoundaryRefusal,
    /// An author-owned body, untouched (`RawCreated`, `RawStreamed`, `RawError`).
    Raw,
    /// A `Raw`: untouched, and on a small `GET` carrying a weak `ETag` the server made.
    RawEtag,
    /// A `RawTagged`: untouched, with the `ETag` its handler chose.
    RawTagged,
    /// A `Streaming` answer: server-sent events, each a `string`.
    Events,
}

/// What a raw member says of the `ETag`.
#[derive(Clone, Copy)]
enum RawEtag {
    None,
    Maybe,
    Required,
}

impl ReplyPayload {
    /// The payload's name in a contract hash: spelt out, so renaming a variant of this
    /// enum does not move every route's hash.
    fn name(self) -> &'static str {
        match self {
            Self::Value => "Value",
            Self::Message => "Message",
            Self::Location => "Location",
            Self::None => "None",
            Self::BoundaryError => "BoundaryError",
            Self::BoundaryRefusal => "BoundaryRefusal",
            Self::Raw => "Raw",
            Self::RawEtag => "RawEtag",
            Self::RawTagged => "RawTagged",
            Self::Events => "Events",
        }
    }

    /// Whether the body is the caller's to read, not the client's to decode.
    fn is_raw(self) -> bool {
        matches!(self, Self::Raw | Self::RawEtag | Self::RawTagged)
    }
}

/// Why a client cannot be generated.
#[derive(Debug, Clone)]
pub struct ClientError(pub String);

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ClientError {}

fn fail<T>(message: impl Into<String>) -> Result<T, ClientError> {
    Err(ClientError(message.into()))
}

/// How the client is laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Layout {
    /// One module.
    #[default]
    File,
    /// `index.ts`, `types.ts`, `codecs.ts` and `runtime.ts`.
    Dir,
}

/// The files of a directory layout, in the order they are written.
pub const DIR_FILES: [&str; 4] = ["index.ts", "types.ts", "codecs.ts", "runtime.ts"];

/// The file name a single-file client has in [`Emitted::files`]: the caller
/// writes it wherever `--out` says.
pub const SINGLE_FILE: &str = "client.ts";

/// The client, and the routes it leaves out with why.
#[derive(Debug, Clone)]
pub struct Emitted {
    /// `(file name, text)`: one file, or the four of [`DIR_FILES`].
    pub files: Vec<(String, String)>,
    /// `` `GET /x` of `service`: why ``, one per route left out.
    pub skipped: Vec<String>,
}

/// The module for `input`. A route whose types have no `T | null` form (an `Option`
/// inside a `Map`, `Set` or `Result`, or an `Option[Option[T]]`) is left out and
/// named, never emitted wrong.
pub fn emit_http_client(input: &HttpClientInput, layout: Layout) -> Result<Emitted, ClientError> {
    let (kept, skipped) = viable_routes(input)?;
    if kept.services.is_empty() {
        return fail(format!(
            "no route of the selected services is one this version of `bynk client` can represent{}",
            skipped
                .iter()
                .map(|s| format!("\n  {s}"))
                .collect::<String>()
        ));
    }
    let mut gen_ = Gen::new(closure_of(&kept)?);
    let plan = gen_.plan(&kept)?;
    let parts = gen_.emit(&kept, &plan)?;
    Ok(Emitted {
        files: assemble(&kept, parts, layout),
        skipped,
    })
}

/// The codec closure of every route of `input`.
fn closure_of(input: &HttpClientInput) -> Result<SurfaceClosure, ClientError> {
    let own_types = merge_types(&input.services)?;
    let mut roots: Vec<TypeRef> = Vec::new();
    for s in &input.services {
        for r in &s.routes {
            for p in &r.params {
                match p {
                    ParamInput::Path { ty, .. }
                    | ParamInput::Body { ty, .. }
                    | ParamInput::Query { ty }
                    | ParamInput::Cookies { ty }
                    | ParamInput::Headers { ty } => roots.push(ty.clone()),
                }
            }
            roots.extend(r.success.iter().cloned());
        }
    }
    Ok(surface_closure(&roots, &own_types, SURFACE_TYPES))
}

/// `input` without the routes that cannot be generated, and a note for each of those.
fn viable_routes(input: &HttpClientInput) -> Result<(HttpClientInput, Vec<String>), ClientError> {
    let mut trial = Gen::new(closure_of(input)?);
    let plan = trial.plan(input)?;
    let mut kept = input.clone();
    let mut skipped = Vec::new();
    for ((s, sp), out) in input
        .services
        .iter()
        .zip(&plan.services)
        .zip(&mut kept.services)
    {
        out.routes.clear();
        for (r, rp) in s.routes.iter().zip(&sp.routes) {
            let (convs, holds) = (trial.convs.clone(), trial.holds.clone());
            match trial.route(&sp.key, r, rp, &s.types) {
                Ok(_) => out.routes.push(r.clone()),
                Err(e) => {
                    (trial.convs, trial.holds) = (convs, holds);
                    skipped.push(format!("`{} {}` of `{}`: {e}", r.method, r.path, s.name));
                }
            }
        }
    }
    kept.services.retain(|s| !s.routes.is_empty());
    Ok((kept, skipped))
}

/// Every service's types in one table. Two services of different units may name
/// a type alike; one file cannot hold both.
fn merge_types(services: &[ServiceInput]) -> Result<HashMap<String, Arc<TypeDecl>>, ClientError> {
    let mut out: HashMap<String, Arc<TypeDecl>> = HashMap::new();
    let mut owner: HashMap<String, &str> = HashMap::new();
    for s in services {
        for (name, decl) in &s.types {
            match out.get(name) {
                None => {
                    out.insert(name.clone(), decl.clone());
                    owner.insert(name.clone(), &s.unit);
                }
                Some(prev) => {
                    if !Arc::ptr_eq(prev, decl) && format!("{prev:?}") != format!("{decl:?}") {
                        return fail(format!(
                            "`{}` and `{}` declare different types named `{}`; one client module cannot hold both. Generate them one at a time with `--service`",
                            owner[name],
                            s.unit,
                            bynk_check::symbols::display_type_name(name)
                        ));
                    }
                }
            }
        }
    }
    Ok(out)
}

/// Everything decided about names before any text is written.
struct Plan {
    services: Vec<ServicePlan>,
}

struct ServicePlan {
    /// The property of the client this service's functions hang from.
    key: String,
    routes: Vec<RoutePlan>,
}

struct RoutePlan {
    /// The function name, `getOrderById`.
    function: String,
    /// The response union's type name.
    response: String,
    /// The query argument's type name, when the route takes one.
    query: Option<String>,
    /// The cookies argument's type name, when the route takes one.
    cookies: Option<String>,
    /// The headers argument's type name, when the route takes one.
    headers: Option<String>,
}

/// One declared boundary type, as the two declarations need it.
struct Gen {
    sc: SurfaceClosure,
    /// The names a codec names a type by in its own signatures: the wire types.
    qual: HashMap<String, String>,
    ru: RuntimeUse,
    /// Generated conversion functions, by name.
    convs: BTreeMap<String, Option<TsDecl>>,
    /// Types already found to hold an `Option`, or not.
    holds: HashMap<String, bool>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Dir {
    /// A decoded wire value to the public shape.
    ToPublic,
    /// A public value to the shape the encoder takes.
    ToWire,
}

impl Dir {
    fn prefix(self) -> &'static str {
        match self {
            Dir::ToPublic => "__toPublic_",
            Dir::ToWire => "__toWire_",
        }
    }
}

impl Gen {
    fn new(sc: SurfaceClosure) -> Self {
        let mut qual: HashMap<String, String> = HashMap::new();
        for n in sc.names.iter().chain(sc.generic_bases.iter()) {
            qual.insert(n.clone(), format!("{WIRE_PREFIX}{n}"));
        }
        Gen {
            sc,
            qual,
            ru: RuntimeUse::default(),
            convs: BTreeMap::new(),
            holds: HashMap::new(),
        }
    }

    fn renamed(&self, t: &TypeRef) -> TypeRef {
        serialisation::rename_type_ref(t, &self.sc.subst)
    }

    // -----------------------------------------------------------------
    // Names
    // -----------------------------------------------------------------

    fn plan(&self, input: &HttpClientInput) -> Result<Plan, ClientError> {
        let mut taken: BTreeSet<String> = self.sc.names.iter().cloned().collect();
        taken.extend(self.sc.generic_bases.iter().cloned());
        taken.extend(SURFACE_TYPES.iter().map(|s| s.to_string()));
        let mut keys: BTreeSet<String> = BTreeSet::new();
        let mut services = Vec::new();
        // A response type is named for the function; two services' functions of
        // one name take their service's name too.
        let mut seen: BTreeMap<String, usize> = BTreeMap::new();
        for s in &input.services {
            for r in &s.routes {
                *seen.entry(function_name(&r.method, &r.path)).or_default() += 1;
            }
        }
        for s in &input.services {
            let mut key = s.name.clone();
            if keys.contains(&key) {
                key = format!("{}_{}", s.unit.replace('.', "_"), s.name);
            }
            keys.insert(key.clone());
            let mut fns: BTreeMap<String, &RouteInput> = BTreeMap::new();
            let mut routes = Vec::new();
            for r in &s.routes {
                let function = function_name(&r.method, &r.path);
                if let Some(other) = fns.insert(function.clone(), r) {
                    return fail(format!(
                        "`{} {}` and `{} {}` of service `{}` both make the function `{function}`; the client cannot name them apart",
                        other.method, other.path, r.method, r.path, s.name
                    ));
                }
                let stem = if seen[&function] > 1 {
                    format!("{}{}", pascal(&s.name), pascal(&function))
                } else {
                    pascal(&function)
                };
                let response = unique(format!("{stem}Response"), &mut taken);
                let has_query = r
                    .params
                    .iter()
                    .any(|p| matches!(p, ParamInput::Query { .. }));
                let query = has_query.then(|| unique(format!("{stem}Query"), &mut taken));
                let has_cookies = r
                    .params
                    .iter()
                    .any(|p| matches!(p, ParamInput::Cookies { .. }));
                let cookies = has_cookies.then(|| unique(format!("{stem}Cookies"), &mut taken));
                let has_headers = r
                    .params
                    .iter()
                    .any(|p| matches!(p, ParamInput::Headers { .. }));
                let headers = has_headers.then(|| unique(format!("{stem}Headers"), &mut taken));
                routes.push(RoutePlan {
                    function,
                    response,
                    query,
                    cookies,
                    headers,
                });
            }
            services.push(ServicePlan { key, routes });
        }
        Ok(Plan { services })
    }

    // -----------------------------------------------------------------
    // Types
    // -----------------------------------------------------------------

    /// The type a caller sees: `Option[T]` is `T | null`.
    fn public_type(&self, t: &TypeRef) -> Result<TsType, ClientError> {
        Ok(match t {
            TypeRef::Option(inner, _) => {
                if matches!(inner.as_ref(), TypeRef::Option(..)) {
                    return fail("an `Option[Option[T]]` has no `T | null` form");
                }
                TsType::union(vec![self.public_type(inner)?, TsType::named("null")])
            }
            TypeRef::List(inner, _) => TsType::readonly_array(self.public_type(inner)?),
            TypeRef::Set(inner, _) => {
                TsType::named_with_args("ReadonlySet", vec![self.public_type(inner)?])
            }
            TypeRef::Map(k, v, _) => TsType::named_with_args(
                "ReadonlyMap",
                vec![self.public_type(k)?, self.public_type(v)?],
            ),
            TypeRef::Result(a, b, _) => {
                TsType::named_with_args("Result", vec![self.public_type(a)?, self.public_type(b)?])
            }
            TypeRef::App { name, args, .. } => {
                let args: Result<Vec<TsType>, ClientError> =
                    args.iter().map(|a| self.public_type(a)).collect();
                TsType::named_with_args(name.name.clone(), args?)
            }
            // A `@multipart` file part: the standard `File` is a `Blob` here, which
            // is what a `File` input yields, and what `fetch` appends.
            TypeRef::Named(id) if self.is_file(&id.name) => TsType::named("globalThis.Blob"),
            other @ TypeRef::Base(..)
            | other @ TypeRef::Named(_)
            | other @ TypeRef::Effect(..)
            | other @ TypeRef::HttpResult(..)
            | other @ TypeRef::QueueResult(_)
            | other @ TypeRef::Asset(_)
            | other @ TypeRef::RequestHeaders(_)
            | other @ TypeRef::RequestQuery(_)
            | other @ TypeRef::RequestCookies(_)
            | other @ TypeRef::TokenClaims(_)
            | other @ TypeRef::BoundaryRejection(_)
            | other @ TypeRef::Query(..)
            | other @ TypeRef::Stream(..)
            | other @ TypeRef::Connection(..)
            | other @ TypeRef::History(..)
            | other @ TypeRef::ValidationError(_)
            | other @ TypeRef::DecodeError(_)
            | other @ TypeRef::Unit(_)
            | other @ TypeRef::Fn(..) => {
                super::ts_types::ts_type_ref_qualified_multi_ts_type(other, &HashMap::new())
            }
        })
    }

    /// Whether `name` is the standard library's `File`, the part of a `@multipart`
    /// body that carries a file (`bynk.File`: `filename`, `contentType`, `bytes`).
    fn is_file(&self, name: &str) -> bool {
        if bynk_check::symbols::display_type_name(name) != "File" {
            return false;
        }
        match self.sc.types.get(name).map(|d| &d.body) {
            // The standard `File` is declared by the `bynk` unit, which a service's
            // own type table does not hold.
            None => name == "File" || name == "bynk__File",
            Some(TypeBody::Record(r)) => {
                let fields: Vec<&str> = r.fields.iter().map(|f| f.name.name.as_str()).collect();
                fields == ["filename", "contentType", "bytes"]
            }
            Some(_) => false,
        }
    }

    /// The type a codec makes and takes.
    fn wire_type(&self, t: &TypeRef) -> TsType {
        serialisation::qualified_ts_type(t, &self.qual)
    }

    /// One boundary type's declaration, in whichever of its two shapes `ty` renders.
    fn declare(
        &self,
        name: &str,
        decl: &TypeDecl,
        public: bool,
    ) -> Result<Vec<TsStmt>, ClientError> {
        let ty = |t: &TypeRef| -> Result<TsType, ClientError> {
            if public {
                self.public_type(t)
            } else {
                Ok(self.wire_type(t))
            }
        };
        let params: Vec<String> = decl
            .type_params
            .iter()
            .map(|p| p.name.name.clone())
            .collect();
        let shown = if public {
            name.to_string()
        } else {
            format!("{WIRE_PREFIX}{name}")
        };
        let d = match &decl.body {
            TypeBody::Record(r) => {
                let mut members = Vec::new();
                for f in &r.fields {
                    members.push(TsTypeMember::readonly_prop(
                        f.name.name.clone(),
                        ty(&f.type_ref)?,
                    ));
                }
                TsDecl::Interface {
                    name: shown,
                    type_params: params,
                    members,
                }
            }
            TypeBody::Sum(sum) => {
                let mut variants = Vec::new();
                for v in &sum.variants {
                    let mut members = vec![TsTypeMember::readonly_prop(
                        "tag",
                        TsType::named(format!("\"{}\"", v.name.name)),
                    )];
                    for f in &v.payload {
                        members.push(TsTypeMember::readonly_prop(
                            super::payload_prop(&f.name.name),
                            ty(&f.type_ref)?,
                        ));
                    }
                    variants.push(TsType::Object(members));
                }
                TsDecl::TypeAlias {
                    name: shown,
                    type_params: params,
                    ty: TsType::multiline_union(variants),
                }
            }
            TypeBody::Refined {
                base, base_span, ..
            } => TsDecl::TypeAlias {
                name: shown,
                type_params: params,
                ty: ty(&TypeRef::Base(*base, *base_span))?,
            },
            TypeBody::Opaque {
                base, base_span, ..
            } => TsDecl::TypeAlias {
                name: shown,
                type_params: params,
                ty: TsType::intersection(vec![
                    ty(&TypeRef::Base(*base, *base_span))?,
                    TsType::Object(vec![TsTypeMember::readonly_prop(
                        "__opaque",
                        TsType::named(format!("\"{name}\"")),
                    )]),
                ]),
            },
        };
        let mut out = Vec::new();
        if public && let Some(text) = decl.documentation.as_deref() {
            out.push(TsStmt::doc_comment(text, None));
        }
        let mut stmt = TsStmt::decl(TsDecl::Export(Box::new(d)), None);
        stmt.no_blank_before = !out.is_empty();
        out.push(stmt);
        Ok(out)
    }

    // -----------------------------------------------------------------
    // Conversions between the wire shape and the public one
    // -----------------------------------------------------------------

    /// Whether a value of `t` holds an `Option` anywhere, so converts.
    fn holds_option(&mut self, t: &TypeRef) -> bool {
        match t {
            TypeRef::Option(..) => true,
            TypeRef::List(i, _) | TypeRef::Set(i, _) => self.holds_option(i),
            TypeRef::Map(k, v, _) | TypeRef::Result(k, v, _) => {
                self.holds_option(k) || self.holds_option(v)
            }
            TypeRef::Named(id) => {
                let key = id.name.clone();
                self.named_holds(&key, &key, &[])
            }
            TypeRef::App { name, args, .. } => {
                let key = serialisation::app_ts_name(&name.name, args);
                self.named_holds(&key, &name.name, args)
            }
            TypeRef::Base(..)
            | TypeRef::Effect(..)
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
            | TypeRef::Fn(..) => false,
        }
    }

    fn named_holds(&mut self, key: &str, decl: &str, args: &[TypeRef]) -> bool {
        if let Some(h) = self.holds.get(key) {
            return *h;
        }
        // A type met again while being decided is no new reason to convert.
        self.holds.insert(key.to_string(), false);
        let fields = self.fields_of(decl, args);
        let found = fields.iter().any(|f| self.holds_option(&f.1));
        self.holds.insert(key.to_string(), found);
        found
    }

    /// The `(variant, [(field, type)])` list of a record or sum, with a generic
    /// declaration's parameters replaced by `args`. A record is one unnamed variant.
    fn variants_of(&self, decl: &str, args: &[TypeRef]) -> Option<(bool, Vec<Variant>)> {
        let types = &self.sc.types;
        let d = types.get(decl)?;
        match &d.body {
            TypeBody::Record(_) => {
                let fields = if args.is_empty() {
                    let TypeBody::Record(r) = &d.body else {
                        return None;
                    };
                    r.fields
                        .iter()
                        .map(|f| (f.name.name.clone(), f.type_ref.clone()))
                        .collect()
                } else {
                    bynk_check::wire::record_inst_fields(decl, args, types)?
                };
                Some((true, vec![(String::new(), fields)]))
            }
            TypeBody::Sum(_) => {
                if args.is_empty() {
                    let TypeBody::Sum(s) = &d.body else {
                        return None;
                    };
                    Some((
                        false,
                        s.variants
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
                            .collect(),
                    ))
                } else {
                    Some((
                        false,
                        bynk_check::wire::sum_inst_variants(decl, args, types)?,
                    ))
                }
            }
            TypeBody::Refined { .. } | TypeBody::Opaque { .. } => None,
        }
    }

    fn fields_of(&self, decl: &str, args: &[TypeRef]) -> Vec<(String, TypeRef)> {
        self.variants_of(decl, args)
            .map(|(_, vs)| vs.into_iter().flat_map(|v| v.1).collect())
            .unwrap_or_default()
    }

    /// `expr`, a value of `t`, converted in direction `dir`. `depth` numbers the
    /// lambda parameters.
    fn convert(
        &mut self,
        t: &TypeRef,
        dir: Dir,
        expr: TsExpr,
        depth: usize,
    ) -> Result<TsExpr, ClientError> {
        if !self.holds_option(t) {
            return Ok(expr);
        }
        let x = format!("x{depth}");
        let lambda = |body: TsExpr| TsExpr::Arrow {
            params: vec![TsParam::new(x.clone(), None)],
            is_async: false,
            generics: Vec::new(),
            return_type: None,
            body: Box::new(TsArrowBody::Expr(Box::new(body))),
        };
        match t {
            TypeRef::Option(inner, _) => {
                if matches!(inner.as_ref(), TypeRef::Option(..)) {
                    return fail("an `Option[Option[T]]` has no `T | null` form");
                }
                let body = self.convert(inner, dir, TsExpr::Ident(x.clone()), depth + 1)?;
                let f = match dir {
                    Dir::ToPublic => "__optToNull",
                    Dir::ToWire => "__nullToOpt",
                };
                Ok(TsExpr::Call {
                    callee: Box::new(TsExpr::Ident(f.to_string())),
                    args: vec![expr, lambda(body)],
                })
            }
            TypeRef::List(inner, _) => {
                let body = self.convert(inner, dir, TsExpr::Ident(x.clone()), depth + 1)?;
                Ok(TsExpr::Call {
                    callee: Box::new(TsExpr::Member {
                        object: Box::new(expr),
                        property: "map".to_string(),
                    }),
                    args: vec![lambda(body)],
                })
            }
            TypeRef::Set(..) | TypeRef::Map(..) | TypeRef::Result(..) => fail(
                "an `Option` inside a `Map`, `Set` or `Result` has no `T | null` form in this version of `bynk client`",
            ),
            TypeRef::Named(id) => {
                let key = id.name.clone();
                self.named_conversion(&key, &key, &[], t, dir)?;
                Ok(conversion_call(dir, &key, expr))
            }
            TypeRef::App { name, args, .. } => {
                let key = serialisation::app_ts_name(&name.name, args);
                self.named_conversion(&key, &name.name, args, t, dir)?;
                Ok(conversion_call(dir, &key, expr))
            }
            TypeRef::Base(..)
            | TypeRef::Effect(..)
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
            | TypeRef::Fn(..) => Ok(expr),
        }
    }

    /// Build (once) the function converting values of the record or sum `t`.
    fn named_conversion(
        &mut self,
        key: &str,
        decl: &str,
        args: &[TypeRef],
        t: &TypeRef,
        dir: Dir,
    ) -> Result<(), ClientError> {
        let name = format!("{}{key}", dir.prefix());
        if self.convs.contains_key(&name) {
            return Ok(());
        }
        // Reserve the name first, so a type that holds itself terminates.
        self.convs.insert(name.clone(), None);
        let Some((is_record, variants)) = self.variants_of(decl, args) else {
            return Ok(());
        };
        let (wire, public) = (self.wire_type(t), self.public_type(t)?);
        let (from, to) = match dir {
            Dir::ToPublic => (wire, public),
            Dir::ToWire => (public, wire),
        };
        let v = || TsExpr::Ident("v".to_string());
        let mut body = Vec::new();
        for (variant, fields) in &variants {
            let mut entries = vec![TsObjectEntry::Spread(v())];
            let mut converted = false;
            for (field, ft) in fields {
                if self.holds_option(ft) {
                    let prop = if is_record {
                        field.clone()
                    } else {
                        super::payload_prop(field)
                    };
                    let from_prop = TsExpr::Member {
                        object: Box::new(v()),
                        property: prop.clone(),
                    };
                    let conv = self.convert(ft, dir, from_prop, 0)?;
                    entries.push(TsObjectEntry::prop(prop, conv));
                    converted = true;
                }
            }
            if !converted {
                continue;
            }
            let object = cast_via_unknown(TsExpr::object_entries(entries), &to);
            if is_record {
                body.push(TsStmt::return_stmt(Some(object), None));
            } else {
                let is_variant = TsExpr::Binary {
                    op: TsBinaryOp::StrictEq,
                    left: Box::new(TsExpr::Member {
                        object: Box::new(v()),
                        property: "tag".to_string(),
                    }),
                    right: Box::new(TsExpr::Lit(TsLit::Str(variant.clone()))),
                };
                body.push(TsStmt::if_stmt(
                    is_variant,
                    TsStmt::return_stmt(Some(object), None),
                    None,
                ));
            }
        }
        if !is_record {
            body.push(TsStmt::return_stmt(Some(cast_via_unknown(v(), &to)), None));
        }
        self.convs.insert(
            name.clone(),
            Some(TsDecl::Function {
                name,
                generics: Vec::new(),
                params: vec![TsParam::new("v", Some(from))],
                return_type: Some(to),
                body,
                is_async: false,
                inline: false,
            }),
        );
        Ok(())
    }

    // -----------------------------------------------------------------
    // The module
    // -----------------------------------------------------------------

    fn emit(&mut self, input: &HttpClientInput, plan: &Plan) -> Result<Parts, ClientError> {
        let mut stmts: Vec<TsStmt> = Vec::new();

        // The route glue is built first: it decides which conversions exist.
        let mut glue: Vec<TsStmt> = Vec::new();
        let mut unions: Vec<TsStmt> = Vec::new();
        let mut services: Vec<TsObjectEntry> = Vec::new();
        let mut contracts: Vec<TsObjectEntry> = Vec::new();
        for (s, sp) in input.services.iter().zip(&plan.services) {
            let mut methods: Vec<TsObjectEntry> = Vec::new();
            let mut hashes: Vec<(String, String)> = Vec::new();
            for (r, rp) in s.routes.iter().zip(&sp.routes) {
                let route = self.route(&sp.key, r, rp, &s.types)?;
                hashes.push((rp.function.clone(), route.hash.clone()));
                glue.push(route.constant);
                unions.extend(route.response_type.into_iter().flatten());
                unions.extend(route.arg_types);
                methods.push(route.method);
            }
            services.push(TsObjectEntry::prop(
                property(&sp.key),
                TsExpr::multiline_object_entries(methods),
            ));
            let all: Vec<String> = hashes.iter().map(|(_, h)| h.clone()).collect();
            let routes = hashes
                .iter()
                .map(|(function, hash)| {
                    TsObjectEntry::prop(property(function), TsExpr::Lit(TsLit::Str(hash.clone())))
                })
                .collect();
            contracts.push(TsObjectEntry::prop(
                property(&sp.key),
                TsExpr::multiline_object_entries(vec![
                    TsObjectEntry::prop(
                        "service",
                        TsExpr::Lit(TsLit::Str(contract::http_service_contract_hash(&all))),
                    ),
                    TsObjectEntry::prop("routes", TsExpr::multiline_object_entries(routes)),
                ]),
            ));
        }

        // Boundary types, public then wire, then the codecs over the wire ones.
        let mut type_names: Vec<String> = self
            .sc
            .names
            .iter()
            .filter(|n| {
                self.sc
                    .types
                    .get(*n)
                    .is_some_and(|d| d.type_params.is_empty())
            })
            .cloned()
            .collect();
        type_names.extend(self.sc.generic_bases.iter().cloned());
        type_names.sort();
        type_names.dedup();
        for public in [true, false] {
            for name in &type_names {
                // A file part is a `Blob` to the caller: it has no public declaration.
                if public && self.is_file(name) {
                    continue;
                }
                if let Some(decl) = self.sc.types.get(name) {
                    stmts.extend(self.declare(name, decl, public)?);
                }
            }
        }
        stmts.extend(unions);
        let mut codecs: Vec<TsStmt> = Vec::new();
        // A file part has no codec in the project's own table: the caller's bytes
        // are not the client's to encode, so its wire form is a stand-in the decoder
        // accepts, and the part itself goes into the `FormData`.
        if self.sc.names.iter().any(|n| self.is_file(n)) {
            // 1A.38: fixed authored snippet, no interpolation
            stmts.push(TsStmt::raw_using(FILE_TYPE, FILE_TYPE_USES));
            // 1A.38: fixed authored snippet, no interpolation
            codecs.push(TsStmt::raw_using(FILE_CODEC, FILE_CODEC_USES));
        }
        let types = self.sc.types.clone();
        codecs.extend(serialisation::decls_as_stmts(
            serialisation::emit_helpers_for_owner_qualified(
                &self.sc.names,
                &types,
                "",
                &self.qual,
                &self.ru,
            ),
        ));
        codecs.extend(serialisation::decls_as_stmts(
            serialisation::emit_generic_helpers_qualified(
                &self.sc.insts,
                &types,
                &self.qual,
                &self.ru,
            ),
        ));
        for decl in self.convs.values().flatten() {
            codecs.push(TsStmt::decl(TsDecl::Export(Box::new(decl.clone())), None));
        }
        let mut index: Vec<TsStmt> = glue;
        index.extend(contracts_stmts(contracts));
        index.extend(client_stmts(services));

        let print = |stmts: Vec<TsStmt>| {
            // A refinement's `field` names the type the codec was given: the
            // declared one, not its wire alias.
            bynk_ts::print(&TsProgram { stmts }, "", "", "client.ts")
                .text
                .replace(&format!("field: \"{WIRE_PREFIX}"), "field: \"")
        };
        Ok(Parts {
            types: print(stmts),
            codecs: print(codecs),
            index: print(index),
        })
    }
}

type Variant = (String, Vec<(String, TypeRef)>);

/// `__toPublic_Key(expr)`, the call of a generated conversion.
fn conversion_call(dir: Dir, key: &str, expr: TsExpr) -> TsExpr {
    let function = format!("{}{key}", dir.prefix());
    TsExpr::Call {
        callee: Box::new(TsExpr::Ident(function)),
        args: vec![expr],
    }
}

/// A documented export that follows its own doc comment with no blank line between.
fn documented(doc: &str, decl: TsDecl) -> Vec<TsStmt> {
    let mut stmt = TsStmt::decl(TsDecl::Export(Box::new(decl)), None);
    stmt.no_blank_before = true;
    vec![TsStmt::doc_comment(doc, None), stmt]
}

/// `export const API_CONTRACTS = { .. } as const;` under its doc block: the hash of
/// each service and route.
fn contracts_stmts(services: Vec<TsObjectEntry>) -> Vec<TsStmt> {
    documented(
        "The contract of each service and route this client was generated for: a hash of\nwhat a client acts on (the route's method and path, what it takes, what it can\nanswer). It moves when that does and not for a handler's body. Informational: log\nit, compare it with the server's, or send it with `sendContract`.",
        TsDecl::ConstDecl {
            name: "API_CONTRACTS".to_string(),
            ty: None,
            init: TsExpr::AsConst(Box::new(TsExpr::multiline_object_entries(services))),
        },
    )
}

/// `createClient` and the `Client` type, each under its doc comment.
fn client_stmts(services: Vec<TsObjectEntry>) -> Vec<TsStmt> {
    let mut out = documented(
        "The client: one property per service, one function per route. Each returns\nthe route's own response, a union keyed by `status`, or a `CallFailure`.",
        TsDecl::Function {
            name: "createClient".to_string(),
            generics: Vec::new(),
            params: vec![
                TsParam::new("options", Some(TsType::named("ClientOptions")))
                    .with_default(TsExpr::object(Vec::new())),
            ],
            return_type: None,
            body: vec![TsStmt::return_stmt(
                Some(TsExpr::AsConst(Box::new(TsExpr::multiline_object_entries(
                    services,
                )))),
                None,
            )],
            is_async: false,
            inline: false,
        },
    );
    out.extend(documented(
        "The client `createClient` makes.",
        TsDecl::TypeAlias {
            name: "Client".to_string(),
            type_params: Vec::new(),
            ty: TsType::named_with_args(
                "ReturnType",
                vec![TsType::type_of(TsExpr::Ident("createClient".to_string()))],
            ),
        },
    ));
    out
}

/// `expr as unknown as ty`.
fn cast_via_unknown(expr: TsExpr, ty: &TsType) -> TsExpr {
    TsExpr::As {
        expr: Box::new(TsExpr::As {
            expr: Box::new(expr),
            ty: TsType::named("unknown"),
        }),
        ty: ty.clone(),
    }
}

// ---------------------------------------------------------------------
// Names
// ---------------------------------------------------------------------

/// `GET /orders/:id` is `getOrderById`; `POST /orders` is `postOrders`. A
/// static segment before a parameter is singular (`orders/:id` reads "order by
/// id"). A route has no name of its own to use (#3508 open question 4).
pub fn function_name(method: &str, path: &str) -> String {
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let mut out = method.to_lowercase();
    if segments.is_empty() {
        out.push_str("Root");
    }
    for (i, seg) in segments.iter().enumerate() {
        if let Some(param) = seg.strip_prefix(':') {
            out.push_str("By");
            out.push_str(&pascal(param));
        } else {
            let before_param = segments.get(i + 1).is_some_and(|n| n.starts_with(':'));
            let words = pascal(seg);
            out.push_str(&if before_param {
                singular(&words)
            } else {
                words
            });
        }
    }
    out
}

fn singular(word: &str) -> String {
    if let Some(stem) = word.strip_suffix("ies") {
        format!("{stem}y")
    } else if word.ends_with("ss") || word.len() < 4 {
        word.to_string()
    } else if let Some(stem) = word.strip_suffix('s') {
        stem.to_string()
    } else {
        word.to_string()
    }
}

/// `get-order_by.id` as `GetOrderById`.
fn pascal(s: &str) -> String {
    let mut out = String::new();
    let mut upper = true;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            if upper {
                out.extend(c.to_uppercase());
            } else {
                out.push(c);
            }
            upper = false;
        } else {
            upper = true;
        }
    }
    out
}

fn sanitise(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// `name`, then `name_`, `name__`, ... until it is not in `taken`; taken.
fn unique(mut name: String, taken: &mut BTreeSet<String>) -> String {
    while taken.contains(&name) {
        name.push('_');
    }
    taken.insert(name.clone());
    name
}

/// A property name: bare when it is an identifier, quoted otherwise.
fn property(name: &str) -> String {
    let ident = name.chars().enumerate().all(|(i, c)| {
        c == '_' || c == '$' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit())
    });
    if ident {
        name.to_string()
    } else {
        format!("{name:?}")
    }
}

/// The `__`-prefixed names a runtime module exports: what generated code calls.
fn exported_names(source: &str) -> Vec<String> {
    source
        .lines()
        .filter_map(|l| {
            let rest = l.strip_prefix("export ")?;
            let rest = rest
                .strip_prefix("function ")
                .or_else(|| rest.strip_prefix("const "))
                .or_else(|| rest.strip_prefix("class "))?;
            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '$')
                .collect();
            name.starts_with("__").then_some(name)
        })
        .collect()
}

/// A runtime module's text with its imports of siblings dropped: after
/// concatenation every name lives in one scope.
fn strip_imports(source: &str) -> String {
    let mut out = String::new();
    for line in source.lines() {
        let t = line.trim_start();
        if t.starts_with("import ") && t.contains("from \"./") {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// The three printed parts of a client, before they are given files.
struct Parts {
    /// The boundary types (public and wire), and each route's response and query types.
    types: String,
    /// The codecs and the conversions between the wire and public shapes.
    codecs: String,
    /// Each route and `createClient`.
    index: String,
}

/// The `// @generated` header every file of a client carries.
fn header(input: &HttpClientInput) -> String {
    format!(
        "// @generated by `bynk client` — do not edit by hand.\n// bynk {}. Regenerate with `bynk client`; see https://bynk-lang.org/reference/http-client/\n// {EXPERIMENTAL}\n\n",
        input.compiler_version
    )
}

/// The inlined runtime for text that uses the names of `scan`: the runtime's own
/// `result` module, then the `bytes`, `decimal`, `time` and `keys` modules the
/// codecs reach, then this client's own, and the wire version.
fn runtime_text(scan: &str) -> String {
    // `keys` reads `decimal`.
    let mut want = vec![false; OPTIONAL_MODULES.len()];
    for (i, (_, source)) in OPTIONAL_MODULES.iter().enumerate() {
        want[i] = exported_names(source)
            .iter()
            .any(|n| scan.contains(n.as_str()));
    }
    if want[3] {
        want[1] = true;
    }
    let mut chunks = vec![RESULT_MODULE];
    for (i, (_, source)) in OPTIONAL_MODULES.iter().enumerate() {
        if want[i] {
            chunks.push(source);
        }
    }
    chunks.push(RUNTIME);
    let mut out = String::new();
    for chunk in chunks {
        out.push_str(&strip_imports(chunk));
        out.push('\n');
    }
    out.push_str(&format!(
        "/** The wire format version this client was generated for. */\nexport const WIRE_VERSION = \"{}\";\n",
        bynk_check::wire_format::WIRE_VERSION
    ));
    out
}

/// The client's files: one module, or the four of a directory.
fn assemble(input: &HttpClientInput, parts: Parts, layout: Layout) -> Vec<(String, String)> {
    let head = header(input);
    let scan = format!("{}{}{}", parts.types, parts.codecs, parts.index);
    let runtime = runtime_text(&scan);
    match layout {
        Layout::File => vec![(
            SINGLE_FILE.to_string(),
            format!(
                "{head}{runtime}\n{}\n{}\n{}",
                parts.types, parts.codecs, parts.index
            ),
        )],
        Layout::Dir => {
            let files: Vec<(&str, String)> = vec![
                ("runtime.ts", runtime),
                ("types.ts", parts.types),
                ("codecs.ts", parts.codecs),
                (
                    "index.ts",
                    format!(
                        "export * from \"./types.js\";\nexport * from \"./runtime.js\";\n\n{}",
                        parts.index
                    ),
                ),
            ];
            // Which file declares each name, so each file imports exactly what it
            // uses and nothing else (a build with `noUnusedLocals` stays quiet).
            let mut declared: BTreeMap<String, (&str, bool)> = BTreeMap::new();
            for (name, text) in &files {
                for (n, is_type) in declared_exports(text) {
                    declared.insert(n, (name, is_type));
                }
            }
            let mut out = Vec::new();
            for (name, text) in &files {
                let imports = imports_for(name, text, &declared);
                out.push((name.to_string(), format!("{head}{imports}{text}")));
            }
            // Written in the order of [`DIR_FILES`].
            out.sort_by_key(|(n, _)| DIR_FILES.iter().position(|f| f == n));
            out
        }
    }
}

/// The names a file's text declares with `export`, each with whether it is a type.
fn declared_exports(text: &str) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("export ") else {
            continue;
        };
        let (rest, is_type) = if let Some(r) = rest.strip_prefix("type ") {
            (r, true)
        } else if let Some(r) = rest.strip_prefix("interface ") {
            (r, true)
        } else if let Some(r) = rest
            .strip_prefix("function ")
            .or_else(|| rest.strip_prefix("async function "))
            .or_else(|| rest.strip_prefix("const "))
            .or_else(|| rest.strip_prefix("class "))
        {
            (r, false)
        } else {
            continue;
        };
        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '$')
            .collect();
        if !name.is_empty() {
            out.push((name, is_type));
        }
    }
    out
}

/// The `import` lines `file` needs for the names of other files that its code uses.
fn imports_for(file: &str, text: &str, declared: &BTreeMap<String, (&str, bool)>) -> String {
    let used = code_identifiers(text);
    let mut by_file: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for (name, (from, is_type)) in declared {
        if *from != file && used.contains(name.as_str()) {
            let spec = if *is_type {
                format!("type {name}")
            } else {
                name.clone()
            };
            by_file.entry(from).or_default().push(spec);
        }
    }
    let mut out = String::new();
    for from in ["runtime.ts", "types.ts", "codecs.ts"] {
        if let Some(names) = by_file.get(from) {
            let module = from.trim_end_matches(".ts");
            out.push_str(&format!(
                "import {{ {} }} from \"./{module}.js\";\n",
                names.join(", ")
            ));
        }
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

/// The identifiers of the code in `text`, leaving out comments and the text of
/// string literals (a type named in a doc comment is not a use of it).
fn code_identifiers(text: &str) -> BTreeSet<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = BTreeSet::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '/' if chars.get(i + 1) == Some(&'/') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                i += 2;
                while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                    i += 1;
                }
                i += 2;
            }
            '"' | '\'' => {
                i += 1;
                while i < chars.len() && chars[i] != c {
                    i += if chars[i] == '\\' { 2 } else { 1 };
                }
                i += 1;
            }
            '`' => {
                i += 1;
                i = skip_template(&chars, i, &mut out);
            }
            _ if c.is_ascii_alphabetic() || c == '_' || c == '$' => {
                let start = i;
                while i < chars.len()
                    && (chars[i].is_ascii_alphanumeric() || chars[i] == '_' || chars[i] == '$')
                {
                    i += 1;
                }
                out.insert(chars[start..i].iter().collect());
            }
            _ => i += 1,
        }
    }
    out
}

/// Past the end of a template literal that starts at `i`, collecting the
/// identifiers of its `${ }` expressions; the literal's own text is skipped.
fn skip_template(chars: &[char], mut i: usize, out: &mut BTreeSet<String>) -> usize {
    while i < chars.len() {
        match chars[i] {
            '\\' => i += 2,
            '`' => return i + 1,
            '$' if chars.get(i + 1) == Some(&'{') => {
                let start = i + 2;
                let mut depth = 1;
                i = start;
                while i < chars.len() && depth > 0 {
                    match chars[i] {
                        '{' => depth += 1,
                        '}' => depth -= 1,
                        _ => {}
                    }
                    i += 1;
                }
                let inner: String = chars[start..i.saturating_sub(1)].iter().collect();
                out.extend(code_identifiers(&inner));
            }
            _ => i += 1,
        }
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_route_is_named_from_its_method_and_path() {
        for (method, path, name) in [
            ("GET", "/orders/:id", "getOrderById"),
            ("POST", "/orders", "postOrders"),
            ("DELETE", "/orders/:sku", "deleteOrderBySku"),
            ("GET", "/orders/:sku/lines/:line", "getOrderBySkuLineByLine"),
            ("GET", "/", "getRoot"),
            ("GET", "/health-check", "getHealthCheck"),
            ("GET", "/categories/:id", "getCategoryById"),
            ("GET", "/address/:id", "getAddressById"),
            (
                "PUT",
                "/v1/user_profiles/:user.id",
                "putV1UserProfileByUserId",
            ),
        ] {
            assert_eq!(function_name(method, path), name, "{method} {path}");
        }
    }

    #[test]
    fn a_name_not_taken_gains_underscores_until_it_is() {
        let mut taken: BTreeSet<String> = ["Order".to_string(), "Order_".to_string()].into();
        assert_eq!(unique("Order".to_string(), &mut taken), "Order__");
        assert_eq!(unique("Order".to_string(), &mut taken), "Order___");
        assert_eq!(unique("Fresh".to_string(), &mut taken), "Fresh");
    }

    #[test]
    fn a_property_is_quoted_only_when_it_must_be() {
        assert_eq!(property("orders"), "orders");
        assert_eq!(property("unit_Name"), "unit_Name");
        assert_eq!(property("a.b"), "\"a.b\"");
    }

    /// The runtime's own `BoundaryError`, `JsonValue` and `ValidationError`
    /// are the ones the server's boundary speaks: this module restates them, so
    /// they must not drift.
    #[test]
    fn the_inlined_boundary_types_are_the_runtimes() {
        let boundary = include_str!("../../runtime/src/boundary.ts");
        let errors = include_str!("../../runtime/src/errors.ts");
        let block = |source: &str, start: &str, end: &str| -> String {
            let from = source
                .find(start)
                .unwrap_or_else(|| panic!("`{start}` in source"));
            let to = source[from..]
                .find(end)
                .unwrap_or_else(|| panic!("`{end}`"))
                + from;
            source[from..to + end.len()].to_string()
        };
        for (source, start, end) in [
            (
                boundary,
                "export type JsonValue =",
                "{ [k: string]: JsonValue };",
            ),
            (
                boundary,
                "export type BoundaryError =",
                "status: number; readonly details: string };",
            ),
            (
                errors,
                "export interface ValidationError {",
                "readonly value: unknown;\n}",
            ),
        ] {
            let want = block(source, start, end);
            assert!(
                RUNTIME.contains(&want),
                "http_client_runtime.ts restates a runtime type that has changed:\n{want}"
            );
        }
    }
}

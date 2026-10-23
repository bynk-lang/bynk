//! #855: the wire-contract peek — the model behind "hover a service handler"
//! and "Show Wire Contract". Renders `bynk_check::wire::WireModel` (the
//! same IR `bynk-emit`'s codec generation consumes) narrowed to *one*
//! handler, plus the facts the IR does not carry: the request envelope
//! shape, the cross-context contract form + hash (`on call` only), and the
//! HTTP response set a route can actually answer with.
//!
//! **Handler resolution** mirrors `bynk-lsp/src/sequence_request.rs`'s
//! `sequence_model_at`: re-parse the committed snapshot with
//! `parse_unit_with_recovery`, find the `Handler` whose span contains the
//! cursor. The re-parsed `Handler` supplies spans/body; the **retained**
//! [`ContextBoundaryInfo`] supplies the type table — a single-file re-parse
//! cannot see a `uses` target's types, which is the whole reason that table
//! is retained project-wide (Phase 3).
//!
//! **Provenance.** [`bynk_check::wire::Provenance`] distinguishes a type this
//! module owns from one reached through another unit. `ContextBoundaryInfo`'s
//! `types` table is `contract_types_for(unit, …)` — *this* unit's own type
//! namespace: own declarations, its `uses` targets', and (#2073) the types of
//! the contexts it consumes, which its own signatures may name — the exact
//! table `own_contract_hashes` (`bynk-emit/src/project.rs`) hashes a
//! service's contract through. Every name resolvable through it is this
//! unit's *own* view of its boundary, so the peek reports
//! `Provenance::Owned` throughout, a borrowed consumed type included. A
//! caller's view of another unit's boundary is out of scope for a peek that
//! answers "what does this handler send and receive".
//!
//! **Scope.** Service handlers only. An agent's `on call` handler crosses a
//! Durable-Object RPC boundary too, but the issue's worked examples (an HTTP
//! route, a cross-context `on call`) are both service handlers, and
//! `ContextBoundaryInfo` does not retain agents' own handler bodies in a form
//! this module needs. Left for a later slice if an agent peek is wanted.

use std::collections::HashMap;
use std::path::PathBuf;

use bynk_check::analysis::ContextBoundaryInfo;
use bynk_check::checker::{Ty, TyId, Types};
use bynk_check::contract;
use bynk_check::resolver::CrossContextService;
use bynk_check::wire::{self, WireModel, WireRef};
use bynk_syntax::ast::*;
use bynk_syntax::span::{FileId, Span};

const HTTP_RESULT: &str = bynk_check::builtin_names::types::HTTP_RESULT;

/// Which protocol hosts the hovered handler — the header facts
/// `discriminator` (`bynk-ide/src/sequence.rs`) also renders, kept here as
/// their own IDE-shaped type rather than re-exporting [`HandlerKind`]
/// directly, so a future field this peek needs (that `HandlerKind` has no
/// room for) is a local addition, not an upstream one.
#[derive(Debug, Clone, PartialEq)]
pub enum BoundaryKind {
    Http { method: HttpMethod, path: String },
    Call,
    Cron { expr: String },
    Message,
    Open,
    Close,
    Event,
}

impl BoundaryKind {
    fn from_handler(h: &Handler) -> Self {
        match &h.kind {
            HandlerKind::Http { method, path } => BoundaryKind::Http {
                method: *method,
                path: path.clone(),
            },
            HandlerKind::Call => BoundaryKind::Call,
            HandlerKind::Cron { expr } => BoundaryKind::Cron { expr: expr.clone() },
            HandlerKind::Message => BoundaryKind::Message,
            HandlerKind::Open => BoundaryKind::Open,
            HandlerKind::Close => BoundaryKind::Close,
            HandlerKind::Event => BoundaryKind::Event,
        }
    }
}

/// The request shape a handler's caller sends. **Three cases, not two** —
/// `bynk-emit/src/emitter/workers_entry.rs`'s `h.params.len() == 1` branch is
/// the *only* bare-value case; everything else, including **zero** params,
/// takes the object branch. Rendering only `Bare`/`Keyed` would misstate a
/// zero-arg service as accepting an (empty) object body it never inspects,
/// when what actually happens is the body is not read at all.
#[derive(Debug, Clone)]
pub enum Envelope {
    /// No params: the request body is not read.
    Empty,
    /// Exactly one param: the request body **is** the value of `param`,
    /// with no wrapping object/key.
    Bare { param: String, shape: WireRef },
    /// Two or more params: an object keyed by parameter name, in
    /// declaration order.
    Keyed { params: Vec<(String, WireRef)> },
    /// A `@raw(contentType:)` route: the request body is the octets of `param`
    /// (a `Bytes`), sent under `content_type` (`"*/*"` for any) — not JSON, not
    /// base64, and not wrapped in an object whatever the other params.
    Raw { param: String, content_type: String },
}

/// The canonical form + hash of an `on call` handler's contract — the same
/// projection `bynk-emit/src/project.rs`'s `own_contract_hashes` builds, so
/// the hash this peek shows is provably the hash the emitted
/// `X-Bynk-Contract` constant stamps.
#[derive(Debug, Clone)]
pub struct ContractForm {
    pub normal_form: String,
    pub hash: String,
}

/// Why a response was reachable — so a renderer can tell an author-declared
/// outcome from one the boundary injects on their behalf without their
/// having written a line for it.
#[derive(Debug, Clone, PartialEq)]
pub enum ResponseOrigin {
    /// The handler's declared return type, `Effect[HttpResult[T]]` stripped
    /// to its `Ok`/200 case.
    DeclaredSuccess,
    /// A variant literally constructed somewhere in the handler body, at
    /// this span.
    Constructed { span: Span },
    /// A response the body never names — injected by the boundary itself.
    BoundaryImplicit { why: &'static str },
}

/// One reachable HTTP outcome.
#[derive(Debug, Clone, PartialEq)]
pub struct HttpResponse {
    pub status: u16,
    /// The `HttpResult` variant name (`"Ok"`, `"TooManyRequests"`), or a
    /// representative `BoundaryError` `kind` for a boundary-implicit
    /// response with no variant of its own (`"StructuralMismatch"`).
    pub variant: String,
    pub origin: ResponseOrigin,
}

/// Why there is no cross-context contract to show for this handler.
/// Rendered on the panel only — hover stays quiet about an absence (Part 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoCrossContextReason {
    /// Not an `on call` handler — HTTP/cron/message/websocket/event
    /// handlers are never reached by another context's call.
    NotACallHandler,
    /// The project has only this one context/adapter — there is no *other*
    /// context that could call in, so the question does not arise.
    SingleContext,
}

/// The wire contract of one handler: its request envelope, its reachable
/// response set (HTTP handlers only), its cross-context contract form + hash
/// (`on call` only), and the boundary type shapes both reference.
#[derive(Debug, Clone)]
pub struct WireContractModel {
    pub unit: String,
    pub service: String,
    pub kind: BoundaryKind,
    pub handler_span: Span,
    /// 1-indexed line of `handler_span`'s start, against the text this
    /// model was built from.
    pub handler_line: usize,
    pub envelope: Envelope,
    /// `on call` only; `None` whenever [`Self::no_cross_context`] is `Some`.
    pub contract: Option<ContractForm>,
    /// The boundary types this handler's envelope + responses reference,
    /// resolved through the retained [`ContextBoundaryInfo`] — the same IR
    /// `bynk-emit`'s codec generation renders.
    pub boundary: WireModel,
    /// Declaration span for every named type in `boundary.types`, keyed by
    /// name — click-to-code for the panel's per-type blocks.
    pub type_sites: HashMap<String, Span>,
    /// Empty for a non-HTTP handler.
    pub responses: Vec<HttpResponse>,
    pub no_cross_context: Option<NoCrossContextReason>,
}

/// How many **real** contexts/adapters a project has — the input
/// [`wire_contract_at`]/[`wire_contract_for_service`]'s `context_count`
/// wants, deciding [`NoCrossContextReason::SingleContext`].
///
/// **Must** be computed this way, not as a bare `boundary_info.len()`:
/// `boundary_info` (mirroring `sequence_info`, #846) retains an entry for
/// every `UnitKind::Context | UnitKind::Adapter`, which includes the
/// synthetic toolchain-injected `bynk` capability-surface unit — so a
/// project with nothing but `consumes bynk { Clock }` would count *two*
/// "contexts" and `SingleContext` would almost never fire. `unit_sources`
/// (ADR 0095) already excludes synthetic units for exactly this reason
/// (document links, consumed-context navigation); intersecting against it
/// reuses that existing filter rather than a bespoke `name == "bynk"` check.
///
/// A free function (not buried in a caller's own helper) precisely so
/// `bynk-lsp`'s `bynk/wireContract` handler calls this — the one place the
/// filter is encoded — instead of re-deriving `boundary_info.len()` from
/// the plan's literal parameter name and reintroducing the bug.
pub fn real_context_count(
    boundary_info: &HashMap<String, ContextBoundaryInfo>,
    unit_sources: &HashMap<String, Vec<PathBuf>>,
) -> usize {
    boundary_info
        .keys()
        .filter(|k| unit_sources.contains_key(k.as_str()))
        .count()
}

/// Locate the `Handler` enclosing `offset` in `text` and build its wire
/// contract. Mirrors `sequence_request::sequence_model_at`'s re-parse
/// convention. `info` is the owning unit's retained boundary table (Phase
/// 3); `expr_types` is that same file's checked expression types (empty for
/// a file with errors — the `Ok`-overload disambiguation then falls back to
/// the declared return type, see `ResponseWalk::is_http_result_expr` below).
///
/// `context_count` is how many **real** contexts/adapters the project has —
/// build it with [`real_context_count`], not a bare `boundary_info.len()`
/// (see that function's doc for why the difference matters).
///
/// Scoped to service handlers (see the module doc); an agent handler at
/// `offset` answers `None`.
pub fn wire_contract_at(
    unit: &str,
    text: &str,
    offset: usize,
    info: &ContextBoundaryInfo,
    expr_types: &[(Span, TyId)],
    tys: &Types,
    context_count: usize,
) -> Option<WireContractModel> {
    let tokens = bynk_syntax::lexer::tokenize(text).ok()?;
    let (parsed, _errs) = bynk_syntax::parser::parse_unit_with_recovery(&tokens, text);
    let items: &[CommonsItem] = match parsed.as_ref()? {
        SourceUnit::Context(c) => &c.items,
        SourceUnit::Adapter(a) => &a.items,
        SourceUnit::Commons(_) | SourceUnit::Suite(_) => return None,
    };
    for item in items {
        if let CommonsItem::Service(s) = item
            && let Some(h) = handler_at(&s.handlers, offset)
        {
            return wire_contract_for_service(
                unit,
                text,
                &s.name.name,
                h,
                info,
                expr_types,
                tys,
                context_count,
            );
        }
    }
    None
}

fn handler_at(handlers: &[Handler], offset: usize) -> Option<&Handler> {
    handlers
        .iter()
        .find(|h| h.span.start <= offset && offset < h.span.end)
}

/// Build the wire contract for one already-located handler. The pure half of
/// [`wire_contract_at`] — the part that never touches the offset/re-parse
/// machinery — split out the same way `sequence_model_at` delegates to
/// `sequence::sequence_model`.
///
/// `service_name` must name an entry in `info.services` (the owning unit's
/// retained service table) — the handler is spliced into a clone of that
/// declaration (a synthetic single-handler service) so
/// `bynk_check::wire::collect_boundary_types` narrows its walk to exactly
/// this handler's params/return, not the whole service's. `None` if the
/// service is not (yet) in the retained table — a live-buffer/committed-round
/// mismatch (the same class of staleness `sequence_model_at` accepts for
/// `sequence_info`).
#[allow(clippy::too_many_arguments)]
pub fn wire_contract_for_service(
    unit: &str,
    text: &str,
    service_name: &str,
    handler: &Handler,
    info: &ContextBoundaryInfo,
    expr_types: &[(Span, TyId)],
    tys: &Types,
    context_count: usize,
) -> Option<WireContractModel> {
    let real = info.services.get(service_name)?;
    let mut synthetic = real.clone();
    synthetic.handlers = vec![handler.clone()];
    let services: HashMap<String, ServiceDecl> =
        HashMap::from([(service_name.to_string(), synthetic)]);
    let agents: HashMap<String, AgentDecl> = HashMap::new();

    let boundary_names = wire::collect_boundary_types(&info.types, &services, &agents, &[]);
    let insts =
        wire::collect_generic_instantiations(&services, &agents, &boundary_names, &info.types, &[]);
    // Every name reachable through `info.types` is this unit's own view of
    // its boundary — see the module doc's Provenance note.
    let boundary = wire::boundary_model(&boundary_names, &info.types, insts, |_| {
        wire::Provenance::Owned
    });

    let type_sites: HashMap<String, Span> = boundary_names
        .iter()
        .filter_map(|n| info.types.get(n).map(|d| (n.clone(), d.span)))
        .collect();

    let envelope = envelope_for(handler, &info.types);
    let kind = BoundaryKind::from_handler(handler);

    let no_cross_context = if context_count <= 1 {
        Some(NoCrossContextReason::SingleContext)
    } else if handler.kind != HandlerKind::Call {
        Some(NoCrossContextReason::NotACallHandler)
    } else {
        None
    };
    let contract = if no_cross_context.is_none() {
        Some(contract_form(service_name, handler, &info.types))
    } else {
        None
    };

    let responses = if matches!(kind, BoundaryKind::Http { .. }) {
        http_responses(handler, real.span.file, expr_types, tys)
    } else {
        Vec::new()
    };

    Some(WireContractModel {
        unit: unit.to_string(),
        service: service_name.to_string(),
        kind,
        handler_span: handler.span,
        handler_line: line_of(text, handler.span.start),
        envelope,
        contract,
        boundary,
        type_sites,
        responses,
        no_cross_context,
    })
}

/// 1-indexed line number of `offset` in `text`.
fn line_of(text: &str, offset: usize) -> usize {
    text.get(..offset).unwrap_or(text).matches('\n').count() + 1
}

fn envelope_for(handler: &Handler, types: &HashMap<String, std::sync::Arc<TypeDecl>>) -> Envelope {
    if let Some(content_type) = handler.raw_content_type() {
        return Envelope::Raw {
            param: "body".to_string(),
            content_type: content_type.to_string(),
        };
    }
    match handler.params.as_slice() {
        [] => Envelope::Empty,
        [p] => Envelope::Bare {
            param: p.name.name.clone(),
            shape: wire::wire_ref(&p.type_ref, types),
        },
        params => Envelope::Keyed {
            params: params
                .iter()
                .map(|p| (p.name.name.clone(), wire::wire_ref(&p.type_ref, types)))
                .collect(),
        },
    }
}

/// The same `CrossContextService` projection `own_contract_hashes`
/// (`bynk-emit/src/project.rs`) builds, canonicalised through the same
/// `info.types` table — so the hash this peek shows is provably the hash
/// the emitted `X-Bynk-Contract` constant stamps.
fn contract_form(
    service_name: &str,
    handler: &Handler,
    types: &HashMap<String, std::sync::Arc<TypeDecl>>,
) -> ContractForm {
    let svc = CrossContextService {
        name: service_name.to_string(),
        params: handler
            .params
            .iter()
            .map(|p| (p.name.name.clone(), p.type_ref.clone()))
            .collect(),
        return_type: handler.return_type.clone(),
        span: handler.span,
        foreign_callers: Vec::new(),
    };
    let normal_form = contract::service_normal_form(&svc, types);
    let hash = contract::contract_hash(&normal_form);
    ContractForm { normal_form, hash }
}

/// The reachable HTTP response set: the declared success case, every
/// variant literally constructed in the body, and the boundary-implicit
/// cases the body never names (400 on any param, 404 on an `Option?`
/// short-circuit — ADR 0177). 401 on a caller binding is deferred (plan
/// risk 8; needs actor/`by` resolution this phase does not do).
///
/// `file` is the handler's own file — the retained service declaration's
/// `FileId`, since a service lives in one file — which scopes the
/// `expr_types` lookup (see `ResponseWalk::file`).
fn http_responses(
    handler: &Handler,
    file: FileId,
    expr_types: &[(Span, TyId)],
    tys: &Types,
) -> Vec<HttpResponse> {
    let declared_is_http_result =
        matches!(strip_effect(&handler.return_type), TypeRef::HttpResult(..));

    let mut out = Vec::new();
    if declared_is_http_result {
        out.push(HttpResponse {
            status: 200,
            variant: "Ok".to_string(),
            origin: ResponseOrigin::DeclaredSuccess,
        });
    }

    let mut walk = ResponseWalk {
        expr_types,
        file,
        tys,
        declared_is_http_result,
        seen: out.iter().map(|r| r.variant.clone()).collect(),
        saw_option_question: false,
        out: Vec::new(),
    };
    walk.walk_block(&handler.body);
    out.extend(walk.out);

    if !handler.params.is_empty() {
        out.push(HttpResponse {
            status: 400,
            variant: "StructuralMismatch".to_string(),
            origin: ResponseOrigin::BoundaryImplicit {
                why: "every param is structurally re-validated on the way in; a malformed or \
                      refinement-violating value fails closed with a 400 the handler body \
                      never names",
            },
        });
    }
    if handler.raw_content_type().is_some() {
        out.push(HttpResponse {
            status: 413,
            variant: "PayloadTooLarge".to_string(),
            origin: ResponseOrigin::BoundaryImplicit {
                why: "a raw body is counted as it is read and cut off at the route's cap",
            },
        });
        out.push(HttpResponse {
            status: 415,
            variant: "UnsupportedMediaType".to_string(),
            origin: ResponseOrigin::BoundaryImplicit {
                why: "the request's Content-Type is not the media type `@raw` declares",
            },
        });
    }
    if walk.saw_option_question {
        out.push(HttpResponse {
            status: 404,
            variant: "NotFound".to_string(),
            origin: ResponseOrigin::BoundaryImplicit {
                why: "an `Option?` short-circuits to 404 on `None` (ADR 0177)",
            },
        });
    }
    out
}

/// Strip `Effect[_]` to expose the inner type — mirrors
/// `bynk-emit/src/emitter/workers_entry.rs`'s `http_result_inner`, one level
/// short of unwrapping `HttpResult` itself (the caller checks that).
fn strip_effect(t: &TypeRef) -> &TypeRef {
    match t {
        TypeRef::Effect(inner, _) => inner.as_ref(),
        other => other,
    }
}

/// The handler-body expression walk finding every literally-constructed
/// `HttpResult` variant, plus whether an `Option?` short-circuit is present
/// (the boundary-implicit 404). Structured after `bynk-ide/src/sequence.rs`'s
/// `Builder` — a small piece of walk state threaded through recursive
/// `walk_*` methods — but this walk is a *full* expression descent (via
/// `bynk_syntax::ast::expr_children`), not `sequence.rs`'s statement-level
/// control-flow-only walk: a response can be constructed anywhere in an
/// expression tree (a call argument, a record field), not only in tail
/// position.
struct ResponseWalk<'a> {
    expr_types: &'a [(Span, TyId)],
    /// The handler's own file. A file's `expr_types` is not only its own
    /// spans: `check_pipeline` merges a sibling file's methods on a type
    /// declared here into this file's checked view, so their bodies' spans
    /// (byte offsets into the *sibling's* text) land in this vector too.
    /// Only entries stamped with this `FileId` are this file's.
    file: FileId,
    /// T3.6b (R4.1): the table `expr_types`' ids resolve against.
    tys: &'a Types,
    /// #855 risk 7 ("the `Ok` overload"): the declared-return-type fallback
    /// used whenever `expr_types` has no entry for a span (an expression the
    /// checker never typed — ADR 0094's partial types still miss those) —
    /// mirrors `bynk-emit/src/emitter/lower.rs`'s own `Ok`/`HttpResult` overload
    /// disambiguation, which has the identical ambiguity and the identical
    /// (checker-backed, so never actually degraded there) resolution.
    /// Applied to `Ok` and bare `Call` (see `is_http_result_expr` below) but
    /// **not** a bare `Ident` (see `is_http_result_ident` below), which has
    /// no concrete-shaped signal to fall back onto and would misreport an
    /// ordinary local whose name collides with a variant name.
    declared_is_http_result: bool,
    seen: std::collections::HashSet<String>,
    saw_option_question: bool,
    out: Vec<HttpResponse>,
}

impl<'a> ResponseWalk<'a> {
    /// The checker-recorded type at `span`: an entry in [`Self::file`] over
    /// the same byte range. `span`'s own `FileId` is ignored — the handler
    /// may come from [`wire_contract_at`]'s reparse, which stamps
    /// `FileId::UNKNOWN`, while `expr_types` carries the analysis round's
    /// real ids (T3.5, #1062), so `Span`'s derived `PartialEq` would never
    /// match. A recorded `Ty::Error` is "no type", so it degrades to the
    /// same fallback as a missing entry.
    fn expr_ty(&self, span: Span) -> Option<std::sync::Arc<Ty>> {
        self.expr_types
            .iter()
            .find(|(s, _)| s.file == self.file && s.start == span.start && s.end == span.end)
            .map(|(_, t)| self.tys.get(*t))
            .filter(|t| !matches!(**t, Ty::Error))
    }

    /// Whether `span`'s expression checked as `HttpResult[_]` — the same
    /// disambiguation `lower.rs:720-731` performs for the `Ok` overload,
    /// degrading to the declared-return heuristic when the checker recorded
    /// nothing (see `declared_is_http_result`'s doc). Used for `Ok` and
    /// `Call`: a bare `Call { name, .. }` already carries a concrete
    /// variant-shaped name (`TooManyRequests(...)`), so the same fallback
    /// is low-risk there. **Not** used for a bare `Ident` — see
    /// `is_http_result_ident` below.
    fn is_http_result_expr(&self, span: Span) -> bool {
        match self.expr_ty(span).as_deref() {
            Some(Ty::HttpResult(_)) => true,
            Some(_) => false,
            None => self.declared_is_http_result,
        }
    }

    /// The stricter check for a bare `Ident` — never falls back to the
    /// declared-return heuristic. `HTTP_VARIANTS` includes names an author
    /// can plausibly bind as an ordinary local (`Found`, `Gone`, `Conflict`,
    /// `Accepted`, `Created`, `Raw`, `Streaming`): with the same fallback as
    /// `is_http_result_expr`, a file with errors and a `let found = …;
    /// found` tail on an `HttpResult`-returning handler would misreport a
    /// 302 that was never constructed. `Ok`/`Call` need the fallback because
    /// degrading them means answering nothing at all for the *one*
    /// expression the plan's risk 7 is about; a bare identifier read has no
    /// such asymmetry — unknown stays unknown.
    fn is_http_result_ident(&self, span: Span) -> bool {
        matches!(self.expr_ty(span).as_deref(), Some(Ty::HttpResult(_)))
    }

    fn push(&mut self, variant: HttpVariant, span: Span) {
        self.push_status(variant, variant.status, span);
    }

    /// #2341 (L4): a `RawError` answers the status its first argument names,
    /// so each distinct status is its own response.
    fn push_raw_error(&mut self, variant: HttpVariant, args: &[Expr], span: Span) {
        if let Some(status) = args.first().and_then(raw_error_status) {
            self.push_status(variant, status, span);
        }
    }

    fn push_status(&mut self, variant: HttpVariant, status: u16, span: Span) {
        let key = if variant.status == AUTHOR_CHOSEN_STATUS {
            format!("{} {status}", variant.name)
        } else {
            variant.name.to_string()
        };
        if self.seen.insert(key) {
            self.out.push(HttpResponse {
                status,
                variant: variant.name.to_string(),
                origin: ResponseOrigin::Constructed { span },
            });
        }
    }

    fn walk_block(&mut self, b: &Block) {
        for s in &b.statements {
            let mut exprs = Vec::new();
            statement_exprs(s, &mut exprs);
            for e in exprs {
                self.walk_expr(e);
            }
        }
        self.walk_expr(&b.tail);
    }

    fn walk_expr(&mut self, e: &Expr) {
        self.classify(e);
        for child in expr_children(e) {
            self.walk_expr(child);
        }
    }

    /// The four recognition shapes `lower.rs` renders through
    /// (`HttpResult.Variant(args)`/`HttpResult.Variant` qualified forms at
    /// `:1417`/`:4149`, the bare `Ident`/`Call` forms disambiguated by
    /// checker type at `:3781`/`:3834`), plus the `Ok` overload
    /// (`:720-731`) and the `?`-on-`Option` boundary-implicit 404
    /// (`:764-768`, ADR 0177).
    fn classify(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::MethodCall {
                receiver,
                method,
                args,
                ..
            } => {
                if let ExprKind::Ident(id) = &receiver.kind
                    && id.name == HTTP_RESULT
                    && let Some(v) = http_variant(&method.name)
                {
                    if v.status == AUTHOR_CHOSEN_STATUS {
                        self.push_raw_error(v, args, e.span);
                    } else {
                        self.push(v, e.span);
                    }
                }
            }
            ExprKind::FieldAccess { receiver, field } => {
                if let ExprKind::Ident(id) = &receiver.kind
                    && id.name == HTTP_RESULT
                    && let Some(v) = http_variant(&field.name)
                {
                    self.push(v, e.span);
                }
            }
            ExprKind::Ident(id) => {
                if self.is_http_result_ident(e.span)
                    && let Some(v) = http_variant(&id.name)
                {
                    self.push(v, e.span);
                }
            }
            ExprKind::Call { name, args, .. } => {
                if self.is_http_result_expr(e.span)
                    && let Some(v) = http_variant(&name.name)
                {
                    if v.status == AUTHOR_CHOSEN_STATUS {
                        self.push_raw_error(v, args, e.span);
                    } else {
                        self.push(v, e.span);
                    }
                }
            }
            ExprKind::Ok(_) => {
                if self.is_http_result_expr(e.span)
                    && let Some(v) = http_variant("Ok")
                {
                    self.push(v, e.span);
                }
            }
            ExprKind::Question(inner) => {
                if matches!(self.expr_ty(inner.span).as_deref(), Some(Ty::Option(_))) {
                    self.saw_option_question = true;
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Same convention as `sequence.rs`/`architecture.rs`'s `setup_project`:
    /// a temp dir unique to the test name, self-contained fixtures only
    /// (never `examples/` — `bynk-ide` is published standalone).
    fn setup_project(test_name: &str, files: &[(&str, &str)]) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "bynk-ide-wire-contract-test-{test_name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create test root");
        for (rel, contents) in files {
            let p = root.join(rel);
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent).expect("create parent");
            }
            std::fs::write(&p, contents).expect("write file");
        }
        root
    }

    // -- Fixture: examples/rate-limiter's `GET /check/:client`, reproduced
    // -- self-contained (not read from `examples/` — see `setup_project`)
    // -- and simplified to a single file (no `uses window` — irrelevant to
    // -- what this module resolves).
    const RATELIMIT_SRC: &str = r#"context ratelimit

consumes bynk { Clock }

type ClientId = String where NonEmpty

type RateView = {
  allowed:   Bool,
  remaining: Int,
  resetAt:   Int,
}

agent Limiter {
  key client: ClientId

  store count: Cell[Int]

  on call hit(now: Int) -> Effect[RateView] {
    let _ <- count.update((c) => c + 1)
    RateView { allowed: count < 10, remaining: 10 - count, resetAt: now }
  }
}

service api from http {
  on GET("/check/:client") (client: ClientId) -> Effect[HttpResult[RateView]] by Visitor given Clock {
    let now  <- Clock.now()
    let view <- Limiter(client).hit(now.toEpochMillis())
    if view.allowed {
      Ok(view)
    } else {
      TooManyRequests("rate limit exceeded")
    }
  }
}
"#;

    fn find_offset(text: &str, needle: &str) -> usize {
        text.find(needle)
            .unwrap_or_else(|| panic!("`{needle}` not found in fixture"))
    }

    /// Thin wrapper over the module's own [`real_context_count`] — exercises
    /// the exact function `bynk-lsp`'s `bynk/wireContract` handler must call
    /// (Phase 5), rather than a test-local re-derivation that could drift
    /// from it.
    fn real_context_count(diag: &crate::ProjectDiagnostics) -> usize {
        super::real_context_count(&diag.boundary_info, &diag.unit_sources)
    }

    const RAW_SRC: &str = r#"context uploads

service api from http {
  limits { maxBody: 1000 }

  @raw(contentType: "image/png")
  on PUT("/avatars/:user") (user: String, body: Bytes) -> Effect[HttpResult[String]] by Visitor {
    Ok(user)
  }
}
"#;

    #[test]
    fn a_raw_route_is_a_raw_envelope_whatever_its_other_params_with_413_and_415() {
        let root = setup_project("uploads", &[("uploads.bynk", RAW_SRC)]);
        let diag = crate::testkit::diagnose_project(&root);
        let info = diag
            .boundary_info
            .get("uploads")
            .expect("boundary_info entry for uploads");
        let offset = find_offset(RAW_SRC, "PUT(\"/avatars/:user\")");
        let model = wire_contract_at(
            "uploads",
            RAW_SRC,
            offset,
            info,
            &[],
            &diag.ty_intern,
            real_context_count(&diag),
        )
        .expect("a wire contract at the PUT handler's header");

        match &model.envelope {
            Envelope::Raw {
                param,
                content_type,
            } => {
                assert_eq!(param, "body");
                assert_eq!(content_type, "image/png");
            }
            other => panic!("expected Envelope::Raw, got {other:?}"),
        }
        let statuses: Vec<u16> = model.responses.iter().map(|r| r.status).collect();
        assert!(statuses.contains(&413), "{statuses:?}");
        assert!(statuses.contains(&415), "{statuses:?}");
    }

    #[test]
    fn rate_limiter_get_check_client_is_a_bare_envelope_with_a_revalidated_client_id() {
        let root = setup_project("ratelimit", &[("ratelimit.bynk", RATELIMIT_SRC)]);
        let diag = crate::testkit::diagnose_project(&root);
        let info = diag
            .boundary_info
            .get("ratelimit")
            .expect("boundary_info entry for ratelimit");

        let offset = find_offset(RATELIMIT_SRC, "GET(\"/check/:client\")");
        let model = wire_contract_at(
            "ratelimit",
            RATELIMIT_SRC,
            offset,
            info,
            &[],
            &diag.ty_intern,
            real_context_count(&diag),
        )
        .expect("a wire contract at the GET handler's header");

        assert_eq!(model.unit, "ratelimit");
        assert_eq!(model.service, "api");
        assert_eq!(
            model.kind,
            BoundaryKind::Http {
                method: HttpMethod::Get,
                path: "/check/:client".to_string(),
            }
        );

        // One param (`client`) sends the bare value — not wrapped in an
        // object, and not the two-case envelope the issue text describes.
        let (param, shape) = match &model.envelope {
            Envelope::Bare { param, shape } => (param, shape),
            other => panic!("expected Envelope::Bare, got {other:?}"),
        };
        assert_eq!(param, "client");
        assert!(
            matches!(shape, WireRef::Named { name } if name == "ClientId"),
            "the bare param's shape should resolve to the named ClientId type: {shape:?}"
        );

        // `ClientId` is in the boundary type set, owned (declared in this
        // same context), refined `NonEmpty`, revalidated via its own
        // constructor.
        let client_id = model
            .boundary
            .types
            .iter()
            .find(|t| t.name == "ClientId")
            .expect("ClientId is a boundary type");
        assert_eq!(client_id.provenance, bynk_check::wire::Provenance::Owned);
        let bynk_check::wire::WireBody::Scalar(scalar) = &client_id.body else {
            panic!("ClientId should be a scalar, got {:?}", client_id.body);
        };
        assert!(
            scalar
                .predicates
                .iter()
                .any(|p| matches!(p, PredKind::NonEmpty)),
            "ClientId's predicates should include NonEmpty: {:?}",
            scalar.predicates
        );
        assert_eq!(
            scalar.revalidation,
            bynk_check::wire::Revalidation::ViaConstructor
        );
        assert!(model.type_sites.contains_key("ClientId"));

        // A single-context project: even though `api`'s handler is `Http`
        // (never `NotACallHandler` would even get a chance to fire), the
        // more fundamental "there is no other context" reason wins.
        assert_eq!(
            model.no_cross_context,
            Some(NoCrossContextReason::SingleContext)
        );
        assert!(model.contract.is_none());
    }

    /// [`ResponseWalk::expr_ty`]'s answer for every expression in the
    /// handler at `offset`, keyed by its source text — reparsed the same way
    /// [`wire_contract_at`] reparses, so the probe of whether the walk's
    /// primary, checker-typed path is actually taken rather than the
    /// declared-return fallback.
    fn walk_expr_tys(
        text: &str,
        offset: usize,
        file: FileId,
        expr_types: &[(Span, TyId)],
        tys: &Types,
    ) -> Vec<(String, Option<std::sync::Arc<Ty>>)> {
        fn collect<'e>(e: &'e Expr, out: &mut Vec<&'e Expr>) {
            out.push(e);
            for c in expr_children(e) {
                collect(c, out);
            }
        }
        let tokens = bynk_syntax::lexer::tokenize(text).expect("lexes");
        let (parsed, _) = bynk_syntax::parser::parse_unit_with_recovery(&tokens, text);
        let Some(SourceUnit::Context(c)) = parsed else {
            panic!("fixture is a context");
        };
        let handler = c
            .items
            .iter()
            .find_map(|i| match i {
                CommonsItem::Service(s) => handler_at(&s.handlers, offset),
                _ => None,
            })
            .expect("a handler at offset");
        let mut exprs = Vec::new();
        for s in &handler.body.statements {
            statement_exprs(s, &mut exprs);
        }
        exprs.push(&handler.body.tail);
        let mut all = Vec::new();
        for e in exprs {
            collect(e, &mut all);
        }
        let walk = ResponseWalk {
            expr_types,
            file,
            tys,
            declared_is_http_result: true,
            seen: Default::default(),
            saw_option_question: false,
            out: Vec::new(),
        };
        all.iter()
            .map(|e| {
                (
                    text[e.span.start..e.span.end].to_string(),
                    walk.expr_ty(e.span),
                )
            })
            .collect()
    }

    #[test]
    fn rate_limiter_response_set_has_declared_constructed_and_boundary_implicit() {
        let root = setup_project("ratelimit-responses", &[("ratelimit.bynk", RATELIMIT_SRC)]);
        let diag = crate::testkit::diagnose_project(&root);
        let info = diag.boundary_info.get("ratelimit").expect("entry");

        // Drive with the round's own retained `expr_types` for this file —
        // the primary path (not the degraded fallback).
        let rel = diag
            .files
            .iter()
            .find(|f| {
                f.source_path
                    .file_name()
                    .is_some_and(|n| n == "ratelimit.bynk")
            })
            .map(|f| f.source_path.clone())
            .expect("ratelimit.bynk in the round's files");
        let expr_types: &[(Span, TyId)] = diag
            .expr_types
            .get(&rel)
            .map(|v| v.as_slice())
            .unwrap_or(&[]);

        assert!(
            !expr_types.is_empty(),
            "the clean fixture recorded no expr types"
        );
        let offset = find_offset(RATELIMIT_SRC, "GET(\"/check/:client\")");
        // The constructed responses themselves resolve to `HttpResult`, not
        // just some incidental sub-expression — otherwise the walk took the
        // degraded fallback for exactly the expressions it classifies.
        let walked = walk_expr_tys(
            RATELIMIT_SRC,
            offset,
            info.services["api"].span.file,
            expr_types,
            &diag.ty_intern,
        );
        for src in ["Ok(view)", "TooManyRequests(\"rate limit exceeded\")"] {
            let ty = walked
                .iter()
                .find(|(t, _)| t == src)
                .unwrap_or_else(|| panic!("`{src}` not walked: {walked:?}"))
                .1
                .as_deref();
            assert!(
                matches!(ty, Some(Ty::HttpResult(_))),
                "`{src}` should resolve to a recorded HttpResult, got {ty:?}"
            );
        }
        let model = wire_contract_at(
            "ratelimit",
            RATELIMIT_SRC,
            offset,
            info,
            expr_types,
            &diag.ty_intern,
            real_context_count(&diag),
        )
        .expect("a wire contract at the GET handler's header");

        let statuses: Vec<(u16, &str)> = model
            .responses
            .iter()
            .map(|r| (r.status, r.variant.as_str()))
            .collect();
        assert!(
            statuses.contains(&(200, "Ok")),
            "declared success missing: {statuses:?}"
        );
        assert!(
            statuses.contains(&(429, "TooManyRequests")),
            "constructed TooManyRequests missing: {statuses:?}"
        );
        assert!(
            statuses.iter().any(|&(s, _)| s == 400),
            "boundary-implicit 400 (the handler has a param) missing: {statuses:?}"
        );
        assert!(
            model
                .responses
                .iter()
                .find(|r| r.status == 200)
                .is_some_and(|r| r.origin == ResponseOrigin::DeclaredSuccess)
        );
        assert!(
            model.responses.iter().any(|r| matches!(
                r.origin,
                ResponseOrigin::BoundaryImplicit { .. }
            ) && r.status == 400)
        );
    }

    // -- Fixture 2: a two-context project with a real `on call` boundary —
    // -- the contract form + hash + Envelope::Keyed path (multi-param).
    const PROVIDER_SRC: &str = r#"context billing

type Quote = { amount: Int, currency: String }

service Pricing {
  on call(sku: String, qty: Int) -> Effect[Quote] {
    Quote { amount: qty * 100, currency: "USD" }
  }
}
"#;
    const CONSUMER_SRC: &str = r#"context storefront

consumes billing

service checkout {
  on call(sku: String, qty: Int) -> Effect[Int] {
    let q <- billing.Pricing(sku, qty)
    q.amount
  }
}
"#;

    #[test]
    fn two_context_call_handler_has_a_keyed_envelope_and_a_contract_hash() {
        let root = setup_project(
            "two-context",
            &[
                ("billing.bynk", PROVIDER_SRC),
                ("storefront.bynk", CONSUMER_SRC),
            ],
        );
        let diag = crate::testkit::diagnose_project(&root);
        let info = diag
            .boundary_info
            .get("billing")
            .expect("boundary_info entry for billing");

        let offset = find_offset(PROVIDER_SRC, "on call(");
        let model = wire_contract_at(
            "billing",
            PROVIDER_SRC,
            offset,
            info,
            &[],
            &diag.ty_intern,
            real_context_count(&diag),
        )
        .expect("a wire contract at the `price` handler");

        assert_eq!(model.kind, BoundaryKind::Call);
        let params = match &model.envelope {
            Envelope::Keyed { params } => params,
            other => panic!("expected Envelope::Keyed for a two-param call, got {other:?}"),
        };
        assert_eq!(
            params.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
            vec!["sku", "qty"],
            "keyed params stay in declaration order"
        );

        assert!(model.no_cross_context.is_none(), "two contexts + on call");
        let contract = model.contract.expect("on call gets a contract form");
        assert_eq!(
            contract.hash,
            bynk_check::contract::contract_hash(&contract.normal_form)
        );

        // Independent re-derivation via the exact projection
        // `own_contract_hashes` uses, against the same retained table — the
        // hovered hash must equal what the emitter would stamp.
        let svc = CrossContextService {
            name: "Pricing".to_string(),
            params: vec![
                (
                    "sku".to_string(),
                    TypeRef::Base(BaseType::String, Span::new(0, 0)),
                ),
                (
                    "qty".to_string(),
                    TypeRef::Base(BaseType::Int, Span::new(0, 0)),
                ),
            ],
            return_type: TypeRef::Named(Ident {
                name: "Quote".to_string(),
                span: Span::new(0, 0),
            }),
            span: Span::new(0, 0),
            foreign_callers: Vec::new(),
        };
        let independent_form = contract::service_normal_form(&svc, &info.types);
        assert_eq!(contract.normal_form, independent_form);
        assert_eq!(contract.hash, contract::contract_hash(&independent_form));
    }

    #[test]
    fn zero_param_call_handler_is_the_empty_envelope() {
        let src = r#"context solo

service Ping {
  on call() -> Effect[Int] {
    1
  }
}
"#;
        let root = setup_project("zero-param", &[("solo.bynk", src)]);
        let diag = crate::testkit::diagnose_project(&root);
        let info = diag.boundary_info.get("solo").expect("entry");

        let offset = find_offset(src, "on call()");
        let model = wire_contract_at(
            "solo",
            src,
            offset,
            info,
            &[],
            &diag.ty_intern,
            real_context_count(&diag),
        )
        .expect("a wire contract at the `Ping` handler");

        assert!(
            matches!(model.envelope, Envelope::Empty),
            "zero params: the request body is not read, not an empty keyed object"
        );
        // Single-context project: `on call` still answers SingleContext, not
        // a real contract.
        assert_eq!(
            model.no_cross_context,
            Some(NoCrossContextReason::SingleContext)
        );
    }

    // -- Regression: the `Ok`-overload `expr_types` fallback (plan risk 7)
    // -- must not extend to a bare `Ident`. `HTTP_VARIANTS` includes names
    // -- an author can plausibly bind as an ordinary local (`Found`, `Gone`,
    // -- `Conflict`, …) — without a recorded expr type, a bare `Found` read
    // -- must stay unknown, not get misreported as a constructed 302.
    #[test]
    fn bare_ident_collision_with_a_variant_name_is_not_misreported_without_expr_types() {
        const SRC: &str = r#"context oddnames

service api from http {
  on GET("/x") () -> Effect[HttpResult[Int]] by Visitor {
    let Found = 1
    Found
  }
}
"#;
        let root = setup_project("ident-collision", &[("oddnames.bynk", SRC)]);
        let diag = crate::testkit::diagnose_project(&root);
        let info = diag
            .boundary_info
            .get("oddnames")
            .expect("boundary_info entry for oddnames");

        let offset = find_offset(SRC, "GET(\"/x\")");
        // Deliberately pass an empty `expr_types` — the degraded path (an
        // expression the checker never typed, see `expr_types` above) this
        // fixture is standing in for, even though it type-checks cleanly on
        // its own.
        let model = wire_contract_at(
            "oddnames",
            SRC,
            offset,
            info,
            &[],
            &diag.ty_intern,
            real_context_count(&diag),
        )
        .expect("a wire contract at the GET handler");

        assert!(
            model.responses.iter().all(|r| r.variant != "Found"),
            "a bare `Found` local must not be misreported as HttpResult.Found \
             without a recorded expr type: {:?}",
            model.responses
        );
    }

    // -- A bare-`Ident` variant never falls back to the declared-return
    // -- heuristic, so it is reported only when `expr_ty` finds the
    // -- checker's recorded type. That lookup used to compare whole `Span`s,
    // -- `FileId` included: the round's spans carry a real `FileId` (T3.5,
    // -- #1062), the reparse's carry `FileId::UNKNOWN`, so it never matched
    // -- and a tail `NoContent` silently dropped out of the response set.
    #[test]
    fn bare_ident_variant_is_reported_from_real_expr_types() {
        const SRC: &str = r#"context flags

service api from http {
  on DELETE("/flags/:name") (name: String) -> Effect[HttpResult[String]] by Visitor {
    NoContent
  }
}
"#;
        let root = setup_project("bare-ident-variant", &[("flags.bynk", SRC)]);
        let diag = crate::testkit::diagnose_project(&root);
        let info = diag.boundary_info.get("flags").expect("entry");
        let expr_types: &[(Span, TyId)] = diag
            .files
            .iter()
            .find(|f| f.source_path.file_name().is_some_and(|n| n == "flags.bynk"))
            .and_then(|f| diag.expr_types.get(&f.source_path))
            .map(|v| v.as_slice())
            .expect("flags.bynk's recorded expr types");
        assert!(
            expr_types.iter().all(|(s, _)| s.file != FileId::UNKNOWN),
            "the premise: analysis spans carry a real FileId"
        );

        let offset = find_offset(SRC, "DELETE(");
        let model = wire_contract_at(
            "flags",
            SRC,
            offset,
            info,
            expr_types,
            &diag.ty_intern,
            real_context_count(&diag),
        )
        .expect("a wire contract at the DELETE handler");

        assert!(
            model
                .responses
                .iter()
                .any(|r| r.status == 204 && r.variant == "NoContent"),
            "a tail `NoContent` must be reported as constructed: {:?}",
            model.responses
        );
    }

    /// A recorded `Ty::Error` is "no type": it must degrade to the
    /// declared-return fallback exactly like a missing entry, not count as a
    /// definite non-`HttpResult` answer.
    #[test]
    fn expr_ty_matches_by_range_and_treats_error_as_no_type() {
        let tys = Types::new();
        let file = FileId(0);
        let error = tys.intern(Ty::Error);
        let http = tys.intern(Ty::HttpResult(error));
        let expr_types = [
            (Span::new_in(file, 10, 20), http),
            (Span::new_in(file, 30, 40), error),
            (Span::new_in(FileId(1), 50, 60), http),
        ];
        let walk = ResponseWalk {
            expr_types: &expr_types,
            file,
            tys: &tys,
            declared_is_http_result: true,
            seen: Default::default(),
            saw_option_question: false,
            out: Vec::new(),
        };

        // A reparsed span (`FileId::UNKNOWN`) over the same range matches.
        assert!(matches!(
            walk.expr_ty(Span::new(10, 20)).as_deref(),
            Some(Ty::HttpResult(_))
        ));
        assert!(walk.is_http_result_ident(Span::new(10, 20)));

        // `Ty::Error` is no type: the ident check stays unknown, the expr
        // check takes the declared-return fallback.
        assert!(walk.expr_ty(Span::new(30, 40)).is_none());
        assert!(!walk.is_http_result_ident(Span::new(30, 40)));
        assert!(walk.is_http_result_expr(Span::new(30, 40)));

        // Another file's entry over the same range is not this file's type.
        assert!(walk.expr_ty(Span::new(50, 60)).is_none());
    }

    /// `name`'s recorded expr types from a round that must be error-free —
    /// so a fixture that stops type-checking fails loudly here rather than
    /// silently degrading the walk to its fallback.
    fn clean_expr_types<'d>(diag: &'d crate::ProjectDiagnostics, name: &str) -> &'d [(Span, TyId)] {
        for f in &diag.files {
            assert!(
                f.diagnostics.is_empty(),
                "{:?} has diagnostics: {:?}",
                f.source_path,
                f.diagnostics
            );
        }
        diag.files
            .iter()
            .find(|f| f.source_path.file_name().is_some_and(|n| n == name))
            .and_then(|f| diag.expr_types.get(&f.source_path))
            .map(|v| v.as_slice())
            .unwrap_or_else(|| panic!("{name}'s recorded expr types"))
    }

    // -- The primary-path twin of the degraded-path collision test above:
    // -- with the real `expr_types` (so the fixture must check cleanly,
    // -- hence `Ok(Found)`), `Found` is recorded as `Int`, and that recorded
    // -- type is what keeps it out of the response set.
    #[test]
    fn bare_ident_collision_with_a_variant_name_is_rejected_by_its_recorded_type() {
        const SRC: &str = r#"context oddnames

service api from http {
  on GET("/x") () -> Effect[HttpResult[Int]] by Visitor {
    let Found = 1
    Ok(Found)
  }
}
"#;
        let root = setup_project("ident-collision-typed", &[("oddnames.bynk", SRC)]);
        let diag = crate::testkit::diagnose_project(&root);
        let info = diag.boundary_info.get("oddnames").expect("entry");
        let expr_types = clean_expr_types(&diag, "oddnames.bynk");

        let offset = find_offset(SRC, "GET(\"/x\")");
        let walked = walk_expr_tys(
            SRC,
            offset,
            info.services["api"].span.file,
            expr_types,
            &diag.ty_intern,
        );
        assert!(
            walked.iter().any(|(t, ty)| t == "Found"
                && ty
                    .as_deref()
                    .is_some_and(|ty| !matches!(ty, Ty::HttpResult(_)))),
            "the tail `Found` should resolve to its recorded non-HttpResult type: {walked:?}"
        );

        let model = wire_contract_at(
            "oddnames",
            SRC,
            offset,
            info,
            expr_types,
            &diag.ty_intern,
            real_context_count(&diag),
        )
        .expect("a wire contract at the GET handler");
        assert!(
            model.responses.iter().all(|r| r.variant != "Found"),
            "a bare `Found` local must not be reported as HttpResult.Found: {:?}",
            model.responses
        );
    }

    // -- `classify`'s `Option?` arm has no fallback, so — like the bare
    // -- `Ident` arm — the boundary-implicit 404 (ADR 0177) was unreachable
    // -- while `expr_ty` never matched.
    #[test]
    fn option_question_in_a_handler_adds_the_boundary_implicit_404() {
        const SRC: &str = r#"context lookup

fn find(n: String) -> Option[Int] {
  Some(1)
}

service api from http {
  on GET("/x/:n") (n: String) -> Effect[HttpResult[Int]] by Visitor {
    let v = find(n)?
    Ok(v)
  }
}
"#;
        let root = setup_project("option-question", &[("lookup.bynk", SRC)]);
        let diag = crate::testkit::diagnose_project(&root);
        let info = diag.boundary_info.get("lookup").expect("entry");
        let expr_types = clean_expr_types(&diag, "lookup.bynk");

        let model = wire_contract_at(
            "lookup",
            SRC,
            find_offset(SRC, "GET("),
            info,
            expr_types,
            &diag.ty_intern,
            real_context_count(&diag),
        )
        .expect("a wire contract at the GET handler");
        assert!(
            model.responses.iter().any(|r| r.status == 404
                && r.variant == "NotFound"
                && matches!(r.origin, ResponseOrigin::BoundaryImplicit { .. })),
            "an `Option?` short-circuit must add the boundary-implicit 404: {:?}",
            model.responses
        );
    }

    // -- A file's `expr_types` also carries a sibling file's spans: the
    // -- checker merges `b.bynk`'s methods on a type declared in `a.bynk`
    // -- into `a.bynk`'s checked view. `b.bynk` is padded so its `Ok(1)`
    // -- (an `HttpResult`) sits at exactly the byte range of `a.bynk`'s
    // -- `Found` local; a range-only match would mint a 302 the handler never
    // -- constructs.
    #[test]
    fn a_sibling_files_span_at_the_same_range_is_not_this_files_type() {
        const A: &str = r#"context shop

type Q = { n: Int }

service api from http {
  on GET("/x") () -> Effect[HttpResult[Int]] by Visitor {
    let Found = 1
    Ok(Found)
  }
}
"#;
        let target = find_offset(A, "Ok(Found)") + "Ok(".len();
        let head = "context shop\n\nfn Q.r(self) -> HttpResult[Int] {\n";
        let pad = target
            .checked_sub(head.len())
            .expect("a.bynk's `Found` sits past b.bynk's header");
        let b = format!("{head}{}Ok(1)\n}}\n", " ".repeat(pad));
        assert_eq!(
            b.find("Ok(1)"),
            Some(target),
            "the padding lines the spans up"
        );

        let root = setup_project("sibling-span", &[("shop/a.bynk", A), ("shop/b.bynk", &b)]);
        let diag = crate::testkit::diagnose_project(&root);
        let info = diag.boundary_info.get("shop").expect("entry");
        let expr_types = clean_expr_types(&diag, "a.bynk");
        let file = info.services["api"].span.file;
        assert!(
            expr_types.iter().any(|(s, t)| s.file != file
                && (s.start, s.end) == (target, target + 5)
                && matches!(*diag.ty_intern.get(*t), Ty::HttpResult(_))),
            "the premise: a.bynk's expr types hold b.bynk's `Ok(1)` at the same range"
        );

        let model = wire_contract_at(
            "shop",
            A,
            find_offset(A, "GET("),
            info,
            expr_types,
            &diag.ty_intern,
            real_context_count(&diag),
        )
        .expect("a wire contract at the GET handler");
        assert!(
            model.responses.iter().all(|r| r.variant != "Found"),
            "b.bynk's `Ok(1)` must not type a.bynk's `Found`: {:?}",
            model.responses
        );
    }
}

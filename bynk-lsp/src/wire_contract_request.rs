//! #855: `bynk/wireContract` — the wire-contract-peek custom LSP request.
//!
//! File-scoped + position, like #846's `bynk/sequenceModel` (the panel is
//! per-handler, not project-wide like #851's `bynk/architectureModel`): the
//! params carry a `textDocument` + `position` and the result is one
//! handler's wire contract, or `null` when nothing resolves. Same on-demand
//! posture as every custom request in this server — no `workspace/*/refresh`
//! nudge exists for one and none is needed (see `sequence_request`'s module
//! doc for why).
//!
//! Two responsibilities live here: [`WcModel`] and its hand-mirrored `Wc*`
//! siblings — a plain serde shape for [`bynk_ide::wire_contract::WireContractModel`]
//! and everything it contains ([`bynk_check::wire::WireModel`] included) —
//! and [`to_wire`], which lowers one to the other, `Span` → `{uri, range}`
//! the same way `architecture_request.rs` does (this module reuses its
//! `WireLoc` under the `WcLoc` alias rather than redeclaring an identical
//! struct).
//!
//! **Cross-file type locations.** A boundary type may be declared in a
//! *different* file than the handler being hovered — the whole reason
//! `ContextBoundaryInfo::types` (Phase 3) is a *combined* table (own
//! declarations plus every `uses` target's). `WireContractModel::type_sites`
//! (`bynk-ide/src/wire_contract.rs`) carries only a bare `Span` per type
//! name, with no file — a single project-relative path cannot be assumed
//! (that path is the *handler's* file, not necessarily the type's). This
//! module resolves each boundary type's own file the same way every other
//! cross-file lookup in this server does: through the already-assembled
//! [`bynk_check::index::ProjectIndex`], searching the candidate units in
//! `doc_scope`'s order (itself first, then `uses` targets, then `consumes`
//! targets — #848's existing search order, reused rather than re-derived).
//! A type whose def cannot be resolved this way renders with `loc: null`
//! rather than a guessed location — the same "never send a bogus location"
//! rule `architecture_request.rs`'s `wire_loc` follows.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bynk_check::index::{ProjectIndex, SymbolKey, SymbolKind};
use bynk_check::wire::{
    self, BaseGuard, Expected, JsonKind, Provenance, Revalidation, UncheckedReason, WireBody,
    WireField, WireInst, WireModel, WireRef, WireScalar, WireSum, WireType,
};
use bynk_ide::wire_contract::{
    BoundaryKind, Envelope, HttpResponse, NoCrossContextReason, ResponseOrigin, WireContractModel,
};
use bynk_syntax::ast::{PredKind, TypeDecl};
use tower_lsp::lsp_types::Range;

pub use crate::architecture_request::WireLoc as WcLoc;

/// The `bynk/wireContract` request payload — a text-document position, the
/// same two-field shape `SequenceModelParams`/every cursor-anchored request
/// in this server uses.
///
/// `rename_all = "camelCase"` is load-bearing: the client sends the LSP wire
/// names `textDocument`/`position`, and #846 already shipped once as a
/// missing-field bug from omitting this (see `sequence_request`'s and
/// `architecture_request`'s own params docs).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireContractParams {
    pub text_document: tower_lsp::lsp_types::TextDocumentIdentifier,
    pub position: tower_lsp::lsp_types::Position,
}

// ---------------------------------------------------------------------
// Wire shape: a plain serde mirror of `bynk_ide::wire_contract::WireContractModel`
// (and, transitively, `bynk_check::wire::WireModel`) — `Wc`-prefixed to
// avoid the `WireWireContractModel`-style stutter a bare `Wire*` prefix
// would produce given this module already imports the real `Wire*` types.
// ---------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize)]
pub struct WcModel {
    pub unit: String,
    pub service: String,
    pub kind: WcBoundaryKind,
    /// The handler's own header/signature span, against the request
    /// document (the handler's own file — never cross-file, unlike a
    /// boundary type's `loc`).
    pub range: Range,
    #[serde(rename = "handlerLine")]
    pub handler_line: usize,
    pub envelope: WcEnvelope,
    /// `on call` only; `null` whenever `noCrossContext` is set.
    pub contract: Option<WcContractForm>,
    pub types: Vec<WcType>,
    pub instantiations: Vec<WcInst>,
    pub recursive: Vec<String>,
    /// Empty for a non-HTTP handler.
    pub responses: Vec<WcHttpResponse>,
    #[serde(rename = "noCrossContext")]
    pub no_cross_context: Option<&'static str>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind")]
pub enum WcBoundaryKind {
    Http { method: &'static str, path: String },
    Call,
    Cron { expr: String },
    Message,
    Open,
    Close,
    Event,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind")]
pub enum WcEnvelope {
    Empty,
    Bare {
        param: String,
        shape: WcRef,
    },
    Keyed {
        params: Vec<WcParam>,
    },
    Raw {
        param: String,
        #[serde(rename = "contentType")]
        content_type: String,
    },
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WcParam {
    pub name: String,
    pub shape: WcRef,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WcContractForm {
    #[serde(rename = "normalForm")]
    pub normal_form: String,
    pub hash: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WcHttpResponse {
    pub status: u16,
    pub variant: String,
    pub origin: WcResponseOrigin,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind")]
pub enum WcResponseOrigin {
    DeclaredSuccess,
    Constructed { range: Range },
    BoundaryImplicit { why: &'static str },
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WcType {
    pub name: String,
    #[serde(rename = "codecSuffix")]
    pub codec_suffix: String,
    pub provenance: WcProvenance,
    pub body: WcBody,
    /// `null` when the declaring file could not be resolved through the
    /// index (defensive only — see the module doc).
    pub loc: Option<WcLoc>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind")]
pub enum WcProvenance {
    Owned,
    Consumed {
        #[serde(rename = "ownerUnit")]
        owner_unit: String,
    },
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind")]
pub enum WcBody {
    Scalar {
        base: &'static str,
        json: &'static str,
        opaque: bool,
        /// Declaration order — see `bynk_check::wire`'s module doc on why
        /// this must never be sorted.
        predicates: Vec<String>,
        #[serde(rename = "baseGuards")]
        base_guards: Vec<&'static str>,
        revalidation: &'static str,
    },
    Record {
        fields: Vec<WcField>,
    },
    Sum {
        #[serde(rename = "wireDiscriminant")]
        wire_discriminant: &'static str,
        #[serde(rename = "memoryDiscriminant")]
        memory_discriminant: &'static str,
        variants: Vec<WcVariant>,
    },
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WcField {
    pub name: String,
    pub shape: WcRef,
    #[serde(rename = "pathSegment")]
    pub path_segment: String,
    /// The default initialiser's source rendering (`bynk_fmt::expr_to_string`),
    /// `null` when the field has none.
    pub default: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WcVariant {
    pub name: String,
    pub payload: Vec<WcField>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind")]
pub enum WcRef {
    Base {
        base: &'static str,
        json: &'static str,
        guards: Vec<&'static str>,
        expected: WcExpected,
    },
    Bytes,
    Named {
        name: String,
    },
    Inst {
        key: String,
    },
    Unit,
    Unchecked {
        reason: &'static str,
    },
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind")]
pub enum WcExpected {
    Json { json: &'static str },
    Integer,
    FiniteNumber,
    Base64String,
    SumVariantKind,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind")]
pub enum WcInst {
    ResultInst {
        key: String,
        ok: WcRef,
        err: WcRef,
    },
    OptionInst {
        key: String,
        inner: WcRef,
    },
    ListInst {
        key: String,
        elem: WcRef,
    },
    MapInst {
        key: String,
        #[serde(rename = "mapKey")]
        map_key: WcRef,
        val: WcRef,
    },
    RecordInst {
        key: String,
        name: String,
        args: Vec<WcRef>,
    },
    SumInst {
        key: String,
        name: String,
        args: Vec<WcRef>,
    },
}

// -- scalar enum -> wire-string helpers (matches this codebase's convention
// -- of `&'static str` fields over deriving `Serialize` on the upstream
// -- enums directly — see `sequence_request.rs`'s `participant_kind_str` etc).

fn json_kind_str(j: JsonKind) -> &'static str {
    match j {
        JsonKind::Number => "Number",
        JsonKind::String => "String",
        JsonKind::Boolean => "Boolean",
        JsonKind::Object => "Object",
        JsonKind::Array => "Array",
        JsonKind::Null => "Null",
    }
}

fn base_guard_str(g: BaseGuard) -> &'static str {
    match g {
        BaseGuard::Integral => "Integral",
        BaseGuard::Finite => "Finite",
    }
}

fn unchecked_reason_str(r: UncheckedReason) -> &'static str {
    match r {
        UncheckedReason::Effect => "Effect",
        UncheckedReason::ValidationError => "ValidationError",
        UncheckedReason::JsonError => "JsonError",
        UncheckedReason::HttpResult => "HttpResult",
        UncheckedReason::QueueResult => "QueueResult",
        UncheckedReason::Asset => "Asset",
    }
}

fn revalidation_str(r: Revalidation) -> &'static str {
    match r {
        Revalidation::ViaConstructor => "ViaConstructor",
        Revalidation::Inline => "Inline",
        Revalidation::StructuralOnly => "StructuralOnly",
        Revalidation::Base64Decode => "Base64Decode",
    }
}

fn no_cross_context_str(r: NoCrossContextReason) -> &'static str {
    match r {
        NoCrossContextReason::NotACallHandler => "NotACallHandler",
        NoCrossContextReason::SingleContext => "SingleContext",
    }
}

/// A `PredKind`'s source-like rendering, predicate args included
/// (`MaxLength(20)`, not just `"MaxLength"`) — `PredKind::name()`
/// (`bynk-syntax/src/ast.rs`) drops the payload, which the peek's whole
/// point is to show. Mirrors `bynk_fmt::fmt`'s private `pred_to_string`
/// (that one renders a `RefinementPred`, the AST wrapper; `WireScalar::
/// predicates` is bare `Vec<PredKind>`, `bynk-check/src/wire.rs`'s own
/// extraction), reusing its public `escape_string` rather than
/// re-implementing string escaping.
fn pred_kind_str(p: &PredKind) -> String {
    match p {
        PredKind::Matches(re) => format!("Matches(\"{}\")", bynk_fmt::escape_string(re)),
        PredKind::InRange(a, b) => format!("InRange({}, {})", a.value, b.value),
        PredKind::InRangeF(a, b) => format!("InRange({}, {})", a.lexeme, b.lexeme),
        PredKind::MinLength(n) => format!("MinLength({n})"),
        PredKind::MaxLength(n) => format!("MaxLength({n})"),
        PredKind::Length(n) => format!("Length({n})"),
        PredKind::NonNegative => "NonNegative".to_string(),
        PredKind::Positive => "Positive".to_string(),
        PredKind::NonEmpty => "NonEmpty".to_string(),
    }
}

fn provenance_wire(p: &Provenance) -> WcProvenance {
    match p {
        Provenance::Owned => WcProvenance::Owned,
        Provenance::Consumed { owner_unit } => WcProvenance::Consumed {
            owner_unit: owner_unit.clone(),
        },
    }
}

fn expected_wire(e: &Expected) -> WcExpected {
    match e {
        Expected::Json(j) => WcExpected::Json {
            json: json_kind_str(*j),
        },
        Expected::Integer => WcExpected::Integer,
        Expected::FiniteNumber => WcExpected::FiniteNumber,
        Expected::Base64String => WcExpected::Base64String,
        Expected::SumVariantKind => WcExpected::SumVariantKind,
    }
}

fn wc_ref(r: &WireRef) -> WcRef {
    match r {
        WireRef::Base {
            base,
            json,
            guards,
            expected,
        } => WcRef::Base {
            base: base.name(),
            json: json_kind_str(*json),
            guards: guards.iter().map(|g| base_guard_str(*g)).collect(),
            expected: expected_wire(expected),
        },
        WireRef::Bytes => WcRef::Bytes,
        WireRef::Named { name } => WcRef::Named {
            name: bynk_check::symbols::display_type_name(name).to_string(),
        },
        WireRef::Inst { key } => WcRef::Inst { key: key.clone() },
        WireRef::Unit => WcRef::Unit,
        WireRef::Unchecked { reason } => WcRef::Unchecked {
            reason: unchecked_reason_str(*reason),
        },
    }
}

fn wc_field(f: &WireField) -> WcField {
    WcField {
        name: f.name.clone(),
        shape: wc_ref(&f.shape),
        path_segment: f.path_segment.clone(),
        default: f
            .default
            .as_ref()
            .map(|(e, _ty)| bynk_fmt::expr_to_string(e)),
    }
}

fn wc_body(b: &WireBody) -> WcBody {
    match b {
        WireBody::Scalar(WireScalar {
            base,
            json,
            opaque,
            predicates,
            base_guards,
            revalidation,
        }) => WcBody::Scalar {
            base: base.name(),
            json: json_kind_str(*json),
            opaque: *opaque,
            predicates: predicates.iter().map(pred_kind_str).collect(),
            base_guards: base_guards.iter().map(|g| base_guard_str(*g)).collect(),
            revalidation: revalidation_str(*revalidation),
        },
        WireBody::Record { fields } => WcBody::Record {
            fields: fields.iter().map(wc_field).collect(),
        },
        WireBody::Sum(WireSum {
            wire_discriminant,
            memory_discriminant,
            variants,
        }) => WcBody::Sum {
            wire_discriminant,
            memory_discriminant,
            variants: variants
                .iter()
                .map(|v| WcVariant {
                    name: v.name.clone(),
                    payload: v.payload.iter().map(wc_field).collect(),
                })
                .collect(),
        },
    }
}

/// Resolve `name`'s own declaration site through the project index,
/// searching `search_order` (self-first — #848's `doc_scope` order) so a
/// type declared in a different file of the same unit, or in a `uses`
/// target, resolves to *its* file rather than the handler's. `None` when no
/// candidate unit's index entry resolves — the type renders with `loc:
/// null` rather than a guessed location.
fn resolve_type_loc(
    project_root: &Path,
    index: &ProjectIndex,
    snapshots: &HashMap<PathBuf, String>,
    search_order: &[String],
    name: &str,
) -> Option<WcLoc> {
    for unit in search_order {
        let key = SymbolKey {
            unit: unit.clone(),
            kind: SymbolKind::Type,
            name: name.to_string(),
        };
        let Some(entry) = index.symbols.get(&key) else {
            continue;
        };
        let Some(def) = &entry.def else { continue };
        let Some(text) = snapshots.get(&def.path) else {
            continue;
        };
        let Ok(uri) = tower_lsp::lsp_types::Url::from_file_path(project_root.join(&def.path))
        else {
            continue;
        };
        return Some(WcLoc {
            uri,
            range: crate::position::span_to_range(text, def.span),
        });
    }
    None
}

fn wc_type(
    t: &WireType,
    project_root: &Path,
    index: &ProjectIndex,
    snapshots: &HashMap<PathBuf, String>,
    search_order: &[String],
) -> WcType {
    // #2087: a qualified key reads, and resolves, as its declaration's name.
    let name = bynk_check::symbols::display_type_name(&t.name);
    WcType {
        name: name.to_string(),
        codec_suffix: t.codec_suffix.clone(),
        provenance: provenance_wire(&t.provenance),
        body: wc_body(&t.body),
        loc: resolve_type_loc(project_root, index, snapshots, search_order, name),
    }
}

/// Lower one boundary occurrence of a generic instantiation. `WireInst`
/// (`bynk-check/src/wire.rs`) carries raw `TypeRef`s for its type
/// arguments — the same "shape, not TS" IR the emitter's codec generation
/// renders — so each argument is resolved through
/// [`bynk_check::wire::wire_ref`] (the same single-level resolver
/// `wire_contract_for_service` uses for envelope params) before lowering,
/// which is why `types` (the owning unit's combined type table) threads
/// through here.
fn wc_inst(inst: &WireInst, types: &HashMap<String, Arc<TypeDecl>>) -> WcInst {
    let key = inst.ts_name();
    match inst {
        WireInst::ResultInst { ok, err } => WcInst::ResultInst {
            key,
            ok: wc_ref(&wire::wire_ref(ok, types)),
            err: wc_ref(&wire::wire_ref(err, types)),
        },
        WireInst::OptionInst { inner } => WcInst::OptionInst {
            key,
            inner: wc_ref(&wire::wire_ref(inner, types)),
        },
        WireInst::ListInst { elem } => WcInst::ListInst {
            key,
            elem: wc_ref(&wire::wire_ref(elem, types)),
        },
        WireInst::MapInst { key: k, val } => WcInst::MapInst {
            key,
            map_key: wc_ref(&wire::wire_ref(k, types)),
            val: wc_ref(&wire::wire_ref(val, types)),
        },
        WireInst::RecordInst { name, args } => WcInst::RecordInst {
            key,
            name: name.clone(),
            args: args
                .iter()
                .map(|a| wc_ref(&wire::wire_ref(a, types)))
                .collect(),
        },
        WireInst::SumInst { name, args } => WcInst::SumInst {
            key,
            name: name.clone(),
            args: args
                .iter()
                .map(|a| wc_ref(&wire::wire_ref(a, types)))
                .collect(),
        },
    }
}

fn boundary_kind_wire(k: &BoundaryKind) -> WcBoundaryKind {
    match k {
        BoundaryKind::Http { method, path } => WcBoundaryKind::Http {
            method: method.as_str(),
            path: path.clone(),
        },
        BoundaryKind::Call => WcBoundaryKind::Call,
        BoundaryKind::Cron { expr } => WcBoundaryKind::Cron { expr: expr.clone() },
        BoundaryKind::Message => WcBoundaryKind::Message,
        BoundaryKind::Open => WcBoundaryKind::Open,
        BoundaryKind::Close => WcBoundaryKind::Close,
        BoundaryKind::Event => WcBoundaryKind::Event,
    }
}

fn envelope_wire(e: &Envelope) -> WcEnvelope {
    match e {
        Envelope::Empty => WcEnvelope::Empty,
        Envelope::Bare { param, shape } => WcEnvelope::Bare {
            param: param.clone(),
            shape: wc_ref(shape),
        },
        Envelope::Raw {
            param,
            content_type,
        } => WcEnvelope::Raw {
            param: param.clone(),
            content_type: content_type.clone(),
        },
        Envelope::Keyed { params } => WcEnvelope::Keyed {
            params: params
                .iter()
                .map(|(n, s)| WcParam {
                    name: n.clone(),
                    shape: wc_ref(s),
                })
                .collect(),
        },
    }
}

fn wc_response(r: &HttpResponse, text: &str) -> WcHttpResponse {
    WcHttpResponse {
        status: r.status,
        variant: r.variant.clone(),
        origin: match &r.origin {
            ResponseOrigin::DeclaredSuccess => WcResponseOrigin::DeclaredSuccess,
            ResponseOrigin::Constructed { span } => WcResponseOrigin::Constructed {
                range: crate::position::span_to_range(text, *span),
            },
            ResponseOrigin::BoundaryImplicit { why } => WcResponseOrigin::BoundaryImplicit { why },
        },
    }
}

/// Lower a [`WireContractModel`] to its wire shape.
///
/// `text` is the handler's **own** file's committed snapshot (the request
/// document) — `model.handler_span` and each HTTP response's `Constructed`
/// span both index into it. `types` is the owning unit's combined type
/// table (`ContextBoundaryInfo::types`), needed to resolve each generic
/// instantiation's type-argument shapes (see `wc_inst`). `index` +
/// `snapshots` + `search_order` resolve each boundary type's own declaring
/// file (see the module doc's "Cross-file type locations" section) — a
/// deliberate widening of the plan's sketched `to_wire(model, project_root,
/// snapshots)` signature, which had no way to know *which* file a `uses`-
/// imported boundary type's span belongs to.
pub fn to_wire(
    model: &WireContractModel,
    project_root: &Path,
    text: &str,
    types: &HashMap<String, Arc<TypeDecl>>,
    index: &ProjectIndex,
    snapshots: &HashMap<PathBuf, String>,
    search_order: &[String],
) -> WcModel {
    let WireModel {
        types: wtypes,
        instantiations,
        recursive,
    } = &model.boundary;
    WcModel {
        unit: model.unit.clone(),
        service: model.service.clone(),
        kind: boundary_kind_wire(&model.kind),
        range: crate::position::span_to_range(text, model.handler_span),
        handler_line: model.handler_line,
        envelope: envelope_wire(&model.envelope),
        contract: model.contract.as_ref().map(|c| WcContractForm {
            // #2087: shown with each qualified key as its declaration's name;
            // `hash` is still over the canonical form.
            normal_form: bynk_check::symbols::display_type_names_in(&c.normal_form),
            hash: c.hash.clone(),
        }),
        types: wtypes
            .iter()
            .map(|t| wc_type(t, project_root, index, snapshots, search_order))
            .collect(),
        instantiations: instantiations.iter().map(|i| wc_inst(i, types)).collect(),
        recursive: recursive.iter().cloned().collect(),
        responses: model
            .responses
            .iter()
            .map(|r| wc_response(r, text))
            .collect(),
        no_cross_context: model.no_cross_context.map(no_cross_context_str),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_deserialize_from_camel_case_wire_json() {
        let json = serde_json::json!({
            "textDocument": { "uri": "file:///a/b.bynk" },
            "position": { "line": 3, "character": 7 },
        });
        let params: WireContractParams =
            serde_json::from_value(json).expect("camelCase textDocument/position must deserialize");
        assert_eq!(params.text_document.uri.as_str(), "file:///a/b.bynk");
        assert_eq!(params.position.line, 3);
        assert_eq!(params.position.character, 7);
    }

    #[test]
    fn pred_kind_str_renders_predicate_arguments_not_just_the_bare_name() {
        assert_eq!(pred_kind_str(&PredKind::NonEmpty), "NonEmpty");
        assert_eq!(pred_kind_str(&PredKind::MaxLength(20)), "MaxLength(20)");
    }
}

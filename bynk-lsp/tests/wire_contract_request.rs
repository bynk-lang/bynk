//! #855 (Phase 5, plan Part 7 items 1–2): fixtures over the pure
//! `wire_contract_at`/`to_wire` path, driven against real `diagnose_project`
//! output — `examples/rate-limiter` (HTTP, single-context) and a synthetic
//! two-context project (`on call`, a real cross-context contract hash). Item
//! 3 (the anti-drift e2e fixture + `bynkc/tests/wire_shape_drift.rs`) is a
//! later phase, per the plan.
//!
//! `bynk-lsp` is a binary crate: this file includes the pure modules
//! directly, the `architecture_request.rs`/`hover_references.rs` convention
//! established for exactly this reason. Excluded from the published crate
//! (`Cargo.toml`) — fixture 1 reads `../examples/rate-limiter`, outside this
//! crate's package.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bynk_check::wire::{Provenance, Revalidation, WireBody};
use bynk_ide::wire_contract::{
    BoundaryKind, Envelope, IrHttpMethod, NoCrossContextReason, ResponseOrigin, real_context_count,
    wire_contract_at,
};
use bynk_lsp::wire_contract_request::{
    WcBody, WcBoundaryKind, WcEnvelope, WcProvenance, WcResponseOrigin, to_wire,
};
use bynk_syntax::ast::PredKind;

fn find_offset(text: &str, needle: &str) -> usize {
    text.find(needle)
        .unwrap_or_else(|| panic!("`{needle}` not found in fixture"))
}

fn setup_project(test_name: &str, files: &[(&str, &str)]) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "bynk-lsp-wire-contract-request-test-{test_name}-{}",
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

/// Fixture 1 — `examples/rate-limiter`'s `GET /check/:client`: the bare
/// envelope, `ClientId`'s `NonEmpty`/`ViaConstructor` revalidation, the
/// declared/constructed/boundary-implicit response set, `RateView`'s fields
/// in declaration order, and `SingleContext` (the project has one context).
#[test]
fn rate_limiter_http_boundary() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/rate-limiter/src");
    // Content-ownership track (#1086) slice 4: bynk-testkit's complete
    // sources map instead of relying on bynk-emit's disk fallback.
    let sources =
        bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
    let diag = bynk_ide::diagnose_project(&root, &sources);
    let file = diag
        .files
        .iter()
        .find(|f| f.source_path.to_string_lossy().ends_with("ratelimit.bynk"))
        .expect("ratelimit.bynk analysed");
    let (rel, text) = (file.source_path.clone(), file.text.clone());
    let info = diag
        .boundary_info
        .get("ratelimit")
        .expect("boundary_info entry for ratelimit");
    let expr_types = diag
        .expr_types
        .get(&rel)
        .map(|v| v.as_slice())
        .unwrap_or(&[]);
    let context_count = real_context_count(&diag.boundary_info, &diag.unit_sources);

    let offset = find_offset(&text, "GET(\"/check/:client\")");
    let model = wire_contract_at(
        "ratelimit",
        &text,
        offset,
        info,
        expr_types,
        &diag.ty_intern,
        context_count,
    )
    .expect("a wire contract at the GET handler");

    assert_eq!(
        model.kind,
        BoundaryKind::Http {
            method: IrHttpMethod::Get,
            path: "/check/:client".to_string(),
        }
    );
    let bare_param = match &model.envelope {
        Envelope::Bare { param, .. } => param.as_str(),
        other => panic!("expected Envelope::Bare (one param), got {other:?}"),
    };
    assert_eq!(bare_param, "client");

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
        "constructed response missing: {statuses:?}"
    );
    assert!(
        model
            .responses
            .iter()
            .any(|r| r.status == 400 && matches!(r.origin, ResponseOrigin::BoundaryImplicit { .. })),
        "boundary-implicit 400 missing: {statuses:?}"
    );

    let client_id = model
        .boundary
        .types
        .iter()
        .find(|t| t.name == "ClientId")
        .expect("ClientId is a boundary type");
    assert_eq!(client_id.provenance, Provenance::Owned);
    let WireBody::Scalar(scalar) = &client_id.body else {
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
    assert_eq!(scalar.revalidation, Revalidation::ViaConstructor);

    let rate_view = model
        .boundary
        .types
        .iter()
        .find(|t| t.name == "RateView")
        .expect("RateView is a boundary type");
    let WireBody::Record { fields } = &rate_view.body else {
        panic!("RateView should be a record, got {:?}", rate_view.body);
    };
    assert_eq!(
        fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
        vec!["allowed", "remaining", "resetAt"],
        "RateView's fields must stay in declaration order"
    );

    assert_eq!(
        model.no_cross_context,
        Some(NoCrossContextReason::SingleContext)
    );

    // `to_wire`: the JSON shape a real `bynk/wireContract` response carries.
    let snapshots: HashMap<PathBuf, String> = diag
        .files
        .iter()
        .map(|f| (f.source_path.clone(), f.text.clone()))
        .collect();
    let search_order = diag.doc_scope.get("ratelimit").cloned().unwrap_or_default();
    let wire = to_wire(
        &model,
        &root,
        &text,
        &info.types,
        &diag.index,
        &snapshots,
        &search_order,
    );

    match &wire.kind {
        WcBoundaryKind::Http { method, path } => {
            assert_eq!(method, &"GET");
            assert_eq!(path, "/check/:client");
        }
        other => panic!("expected WcBoundaryKind::Http, got {other:?}"),
    }
    match &wire.envelope {
        WcEnvelope::Bare { param, .. } => assert_eq!(param, "client"),
        other => panic!("expected WcEnvelope::Bare, got {other:?}"),
    }
    assert!(
        wire.responses
            .iter()
            .any(|r| r.status == 200 && matches!(r.origin, WcResponseOrigin::DeclaredSuccess)),
        "{:?}",
        wire.responses
    );

    let client_id_wire = wire
        .types
        .iter()
        .find(|t| t.name == "ClientId")
        .expect("ClientId in the wire model");
    assert!(
        client_id_wire.loc.is_some(),
        "ClientId's own declaration site should resolve through the index"
    );
    assert!(matches!(client_id_wire.provenance, WcProvenance::Owned));
    let WcBody::Scalar { predicates, .. } = &client_id_wire.body else {
        panic!("expected a scalar body, got {:?}", client_id_wire.body);
    };
    assert!(predicates.iter().any(|p| p == "NonEmpty"), "{predicates:?}");

    assert_eq!(wire.no_cross_context, Some("SingleContext"));
    assert!(wire.contract.is_none());
}

// -- Fixture 2: a two-context project with a real `on call` cross-context
// -- boundary — the contract form + hash + `Envelope::Keyed` (multi-param)
// -- path, none of which the single-context rate-limiter fixture exercises.
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
fn two_context_call_contract_form_and_hash() {
    let root = setup_project(
        "two-context",
        &[
            ("billing.bynk", PROVIDER_SRC),
            ("storefront.bynk", CONSUMER_SRC),
        ],
    );
    // Content-ownership track (#1086) slice 4: bynk-testkit's complete
    // sources map instead of relying on bynk-emit's disk fallback.
    let sources =
        bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
    let diag = bynk_ide::diagnose_project(&root, &sources);
    let file = diag
        .files
        .iter()
        .find(|f| f.source_path.to_string_lossy().ends_with("billing.bynk"))
        .expect("billing.bynk analysed");
    let text = file.text.clone();
    let info = diag
        .boundary_info
        .get("billing")
        .expect("boundary_info entry for billing");
    let context_count = real_context_count(&diag.boundary_info, &diag.unit_sources);

    let offset = find_offset(&text, "on call(");
    let model = wire_contract_at(
        "billing",
        &text,
        offset,
        info,
        &[],
        &diag.ty_intern,
        context_count,
    )
    .expect("a wire contract at the `Pricing` handler");

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
    let contract = model
        .contract
        .clone()
        .expect("on call gets a contract form");
    // The form *shown* is the form *hashed* — no separate computation path.
    assert_eq!(
        contract.hash,
        bynk_check::contract::contract_hash(&contract.normal_form)
    );

    // Independent re-derivation via the exact projection
    // `own_contract_hashes` (`bynk-emit/src/project.rs`) uses, against the
    // same retained table — the hovered/panelled hash must equal what the
    // emitter would stamp as `X-Bynk-Contract`.
    let svc = bynk_check::resolver::CrossContextService {
        name: "Pricing".to_string(),
        params: vec![
            (
                "sku".to_string(),
                bynk_syntax::ast::TypeRef::Base(
                    bynk_syntax::ast::BaseType::String,
                    bynk_syntax::span::Span::new(0, 0),
                ),
            ),
            (
                "qty".to_string(),
                bynk_syntax::ast::TypeRef::Base(
                    bynk_syntax::ast::BaseType::Int,
                    bynk_syntax::span::Span::new(0, 0),
                ),
            ),
        ],
        return_type: bynk_syntax::ast::TypeRef::Named(bynk_syntax::ast::Ident {
            name: "Quote".to_string(),
            span: bynk_syntax::span::Span::new(0, 0),
        }),
        span: bynk_syntax::span::Span::new(0, 0),
        foreign_callers: Vec::new(),
    };
    let independent_form = bynk_check::contract::service_normal_form(&svc, &info.types);
    assert_eq!(contract.normal_form, independent_form);
    assert_eq!(
        contract.hash,
        bynk_check::contract::contract_hash(&independent_form)
    );

    // `to_wire`: the hash and the keyed-param order both survive the wire
    // conversion untouched.
    let snapshots: HashMap<PathBuf, String> = diag
        .files
        .iter()
        .map(|f| (f.source_path.clone(), f.text.clone()))
        .collect();
    let search_order = diag.doc_scope.get("billing").cloned().unwrap_or_default();
    let wire = to_wire(
        &model,
        &root,
        &text,
        &info.types,
        &diag.index,
        &snapshots,
        &search_order,
    );
    assert!(wire.no_cross_context.is_none());
    let wire_contract = wire.contract.expect("wire model carries the contract form");
    assert_eq!(wire_contract.hash, contract.hash);
    match &wire.envelope {
        WcEnvelope::Keyed { params } => {
            assert_eq!(
                params.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
                vec!["sku", "qty"]
            );
        }
        other => panic!("expected WcEnvelope::Keyed, got {other:?}"),
    }
}

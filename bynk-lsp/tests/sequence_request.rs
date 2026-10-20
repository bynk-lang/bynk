//! #846: `bynk/sequenceModel` — the handler-location and wire-conversion
//! logic in `bynk_lsp::sequence_request`, driven directly (not through a
//! JSON-RPC round trip — `Backend`'s own committed-round gating is exercised
//! by the existing `committed_analysis_serves_the_stale_round_without_refreshing`
//! test in-crate; this file covers the two things that live in
//! `sequence_request` itself: locating the enclosing handler at an arbitrary
//! cursor offset, and the CodeLens site list). Goes through `bynk_ide`, not
//! `Backend`, for the same reason `project_model.rs` does — a hermetic,
//! scratch-project fixture rather than a live server session.

use std::path::PathBuf;

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch(tag: &str, files: &[(&str, &str)]) -> Scratch {
    let dir = std::env::temp_dir().join(format!(
        "bynk_lsp_sequence_request_{tag}_{}_{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    for (rel, body) in files {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, body).unwrap();
    }
    Scratch(dir)
}

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

/// The enclosing-handler lookup must resolve for a cursor anywhere in the
/// body — not just at the handler's opening keyword — since that is where a
/// real "Show Sequence" invocation lands (the CodeLens command carries the
/// range's *start*, but a future cursor-driven trigger would not).
#[test]
fn sequence_model_at_resolves_for_a_cursor_anywhere_in_the_handler_body() {
    let root = scratch("cursor_anywhere", &[("ratelimit.bynk", RATELIMIT_SRC)]);
    // Content-ownership track (#1086) slice 4: bynk-testkit's complete
    // sources map instead of relying on bynk-emit's disk fallback.
    let roots = bynk_ide::AnalysisRoots::SingleTree(root.0.clone());
    let sources = bynk_testkit::read_project_sources(&roots);
    let diag = bynk_ide::diagnose_project_with(&roots, &sources, &bynk_ide::SchemaLock::Off);
    let info = diag
        .sequence_info
        .get("ratelimit")
        .expect("sequence_info entry for ratelimit");

    // A cursor on `Limiter` — deep inside the handler body, nowhere near the
    // `on GET(...)` keyword the handler's span starts at.
    let offset = RATELIMIT_SRC
        .find("Limiter(client)")
        .expect("fixture shape");

    let model = bynk_lsp::sequence_request::sequence_model_at(RATELIMIT_SRC, offset, Some(info))
        .expect("a handler encloses this offset");
    // The `by Visitor` principal is the leftmost participant, ahead of the
    // entry, so: Visitor + Entry + Clock + Limiter.
    assert_eq!(
        model.participants.len(),
        4,
        "Actor + Entry + Clock + Limiter"
    );

    // Confirm the LSP-side wire conversion round-trips the span data too — and
    // that the new `Actor` kind and per-branch `reply` cross the wire.
    let wire = bynk_lsp::sequence_request::to_wire(&model, RATELIMIT_SRC);
    assert_eq!(wire.participants.len(), 4);
    assert_eq!(wire.participants[0].kind, "Actor");
    assert!(
        wire.participants[0].range.is_some(),
        "the actor participant carries the `by` clause span"
    );
    // request + Clock Call/Return + Limiter Call/Return + a reply-to-actor per branch.
    assert_eq!(wire.messages.len(), 7);
    assert_eq!(wire.blocks.len(), 1);
}

/// #2194: an agent's own handler carries its state writes across the wire as
/// `StateWrite` messages from the entry to itself — `Limiter.hit`'s
/// `count.update(…)`, with the write's own range for click-to-code.
#[test]
fn an_agent_handlers_state_write_crosses_the_wire() {
    let root = scratch("state_write", &[("ratelimit.bynk", RATELIMIT_SRC)]);
    let roots = bynk_ide::AnalysisRoots::SingleTree(root.0.clone());
    let sources = bynk_testkit::read_project_sources(&roots);
    let diag = bynk_ide::diagnose_project_with(&roots, &sources, &bynk_ide::SchemaLock::Off);
    let info = diag.sequence_info.get("ratelimit").expect("sequence_info");
    let offset = RATELIMIT_SRC.find("count.update").expect("fixture shape");

    let model = bynk_lsp::sequence_request::sequence_model_at(RATELIMIT_SRC, offset, Some(info))
        .expect("the agent handler encloses this offset");
    let wire = bynk_lsp::sequence_request::to_wire(&model, RATELIMIT_SRC);
    assert_eq!(wire.messages.len(), 1);
    let w = &wire.messages[0];
    assert_eq!(
        (w.kind, w.label.as_str()),
        ("StateWrite", "count.update(…)")
    );
    assert_eq!((w.from, w.to), (0, 0));
    // The range is the write's own line, so a click lands on `count.update`.
    let line = RATELIMIT_SRC[..offset].matches('\n').count() as u32;
    assert_eq!(w.range.start.line, line);
}

/// Outside any handler body (in the leading `consumes`/`type` declarations)
/// there is nothing to diagram.
#[test]
fn sequence_model_at_is_none_outside_any_handler() {
    let offset = RATELIMIT_SRC.find("consumes bynk").unwrap();
    assert!(bynk_lsp::sequence_request::sequence_model_at(RATELIMIT_SRC, offset, None).is_none());
}

/// The CodeLens site list covers both handler kinds this fixture has — an
/// agent's `on call` and a service's `on GET` — which is the regression this
/// function exists for (`index_queries::code_lenses` only sees the former).
#[test]
fn handler_lens_sites_covers_both_agent_and_service_handlers() {
    let sites = bynk_lsp::sequence_request::handler_lens_sites(RATELIMIT_SRC);
    assert_eq!(sites.len(), 2, "one agent handler + one service handler");
}

/// #847: the params must deserialize from the **camelCase** wire shape the
/// client sends (`textDocument`/`position`), not the Rust field names. The
/// in-crate tests above drive `sequence_model_at` directly and never round-trip
/// these params, so this mismatch shipped latent in #846 until the doc-view
/// work surfaced the same class of bug — pinned here so it cannot regress.
#[test]
fn params_deserialize_from_the_camelcase_wire_shape() {
    let wire = r#"{"textDocument":{"uri":"file:///x.bynk"},"position":{"line":3,"character":5}}"#;
    let params: bynk_lsp::sequence_request::SequenceModelParams =
        serde_json::from_str(wire).expect("camelCase textDocument/position must deserialize");
    assert_eq!(params.text_document.uri.as_str(), "file:///x.bynk");
    assert_eq!(params.position.line, 3);
    assert_eq!(params.position.character, 5);
}

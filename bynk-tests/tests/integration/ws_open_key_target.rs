//! #2959: on the Workers target the key of a WebSocket `on open`'s connection
//! transfer must be a route parameter, a context `const` or a literal
//! (`bynk.ws.open_transfer_key`), since the edge routes the upgrade before the
//! body runs. On the bundle target the open is an ordinary surface method that
//! evaluates any key, so the same source compiles. Pins the "Workers-only" claim
//! in the diagnostic's note; the Workers rejection alone is covered by the
//! negative fixture `2959_ws_open_transfer_key`, and the accepted keys by the
//! positive fixture `2959_ws_open_const_literal_key`.

use std::sync::atomic::{AtomicU32, Ordering};

/// A `let` in the body rebinds the key's name.
const LIVE: &str = "context live

type Frame = { text: String }

service feed from websocket(in: Frame, out: Frame) {
  on open () -> Effect[()] by v: Visitor {
    let room = \"lobby\"
    let _ <- Room(room).join(connection)
    ()
  }
}

agent Room {
  key id: String
  store conns: Map[String, Connection[Frame]]

  on call join(conn: Connection[Frame]) -> Effect[()] {
    let _ <- conns.put(\"one\", conn)
    ()
  }
}
";

/// A `par` binding shadows the route parameter keying the transfer: the
/// bindings of a `par` block land in the body's scope, so the key is the local,
/// not the parameter the edge would route by (review of #2960).
const LIVE_PAR: &str = "context live

type Frame = { text: String }

fn pick(s: String) -> Effect[String] { s }

service feed from websocket(in: Frame, out: Frame) {
  on open (room: String) -> Effect[()] by v: Visitor {
    par {
      let room <- pick(\"lobby\")
      let other <- pick(\"other\")
    }
    let _ <- Room(room).join(connection)
    ()
  }
}

agent Room {
  key id: String
  store conns: Map[String, Connection[Frame]]

  on call join(conn: Connection[Frame]) -> Effect[()] {
    let _ <- conns.put(\"one\", conn)
    ()
  }
}
";

fn compile_categories(target: bynk_driver::project::BuildTarget) -> Result<(), Vec<String>> {
    compile_source_categories(LIVE, target)
}

fn compile_source_categories(
    source: &str,
    target: bynk_driver::project::BuildTarget,
) -> Result<(), Vec<String>> {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir =
        std::env::temp_dir().join(format!("bynk_ws_open_key_{}_{unique}", std::process::id()));
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("live.bynk"), source).unwrap();
    let result = bynk_driver::project::compile_project(
        &bynk_testkit::compile_options_single(src.clone()).target(target),
    )
    .map(|_| ())
    .map_err(|failure| {
        bynk_driver::project::ProjectFailure::flatten(failure)
            .into_iter()
            .map(|e| e.category.to_string())
            .collect()
    });
    let _ = std::fs::remove_dir_all(&dir);
    result
}

#[test]
fn a_body_bound_open_key_compiles_on_bundle_and_is_rejected_on_workers() {
    compile_categories(bynk_driver::project::BuildTarget::Bundle)
        .unwrap_or_else(|errs| panic!("the bundle target should accept the key: {errs:?}"));

    let errs = compile_categories(bynk_driver::project::BuildTarget::Workers)
        .expect_err("the Workers target should reject the key");
    assert_eq!(
        errs,
        vec!["bynk.ws.open_transfer_key".to_string()],
        "exactly the key diagnostic"
    );
}

#[test]
fn a_par_bound_key_shadowing_a_route_param_is_rejected_on_workers() {
    let errs = compile_source_categories(LIVE_PAR, bynk_driver::project::BuildTarget::Workers)
        .expect_err("the Workers target should reject the par-bound key");
    assert_eq!(
        errs,
        vec!["bynk.ws.open_transfer_key".to_string()],
        "exactly the key diagnostic"
    );
}

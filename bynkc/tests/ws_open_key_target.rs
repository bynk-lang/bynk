//! #2959: on the Workers target the key of a WebSocket `on open`'s connection
//! transfer must be a route parameter, a context `const` or a literal
//! (`bynk.ws.open_transfer_key`), since the edge routes the upgrade before the
//! body runs. On the bundle target the open is an ordinary surface method that
//! evaluates any key, so the same source compiles. Pins the "Workers-only" claim
//! in the diagnostic's note; the Workers rejection alone is covered by the
//! negative fixture `2959_ws_open_transfer_key`, and the accepted keys by the
//! positive fixture `2959_ws_open_const_literal_key`.

use std::sync::atomic::{AtomicU32, Ordering};

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

fn compile_categories(target: bynkc::BuildTarget) -> Result<(), Vec<String>> {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "bynk_ws_open_key_{}_{unique}",
        std::process::id()
    ));
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("live.bynk"), LIVE).unwrap();
    let result =
        bynkc::compile_project(&bynk_testkit::compile_options_single(src.clone()).target(target))
            .map(|_| ())
            .map_err(|failure| {
                bynkc::ProjectFailure::flatten(failure)
                    .into_iter()
                    .map(|e| e.category.to_string())
                    .collect()
            });
    let _ = std::fs::remove_dir_all(&dir);
    result
}

#[test]
fn a_body_bound_open_key_compiles_on_bundle_and_is_rejected_on_workers() {
    compile_categories(bynkc::BuildTarget::Bundle)
        .unwrap_or_else(|errs| panic!("the bundle target should accept the key: {errs:?}"));

    let errs = compile_categories(bynkc::BuildTarget::Workers)
        .expect_err("the Workers target should reject the key");
    assert_eq!(
        errs,
        vec!["bynk.ws.open_transfer_key".to_string()],
        "exactly the key diagnostic"
    );
}

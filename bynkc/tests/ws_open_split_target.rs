//! #2306: a WebSocket `on open` that transfers its connection to an agent in a
//! different file of a split context is rejected on the Workers target
//! (`bynk.ws.open_transfer_cross_file`) but still compiles on the bundle target,
//! where the open is an ordinary surface method and no Durable Object has to
//! host it. Pins the "Workers-only" claim in the diagnostic's note and ADR; the
//! Workers rejection alone is covered by the negative fixture
//! `2306_workers_ws_open_split_agent`.

use std::sync::atomic::{AtomicU32, Ordering};

const GATEWAY: &str = "context chat

type RoomId = opaque String
type UserId = opaque String
type ServerFrame = { text: String }
type ClientFrame = { text: String }

actor Participant { auth = Bearer(secret = \"AUTH_SECRET\"), identity = UserId }

service ChatGateway from websocket(in: ClientFrame, out: ServerFrame) {
  on open (roomId: RoomId) -> Effect[()] by user: Participant {
    let _ <- Room(roomId).join(user.identity, connection)
    ()
  }
}
";

const ROOM: &str = "context chat

agent Room {
  key id: RoomId
  store conns: Map[UserId, Connection[ServerFrame]]

  on call join(u: UserId, conn: Connection[ServerFrame]) -> Effect[()] {
    let _ <- conns.put(u, conn)
    ()
  }
}
";

/// Compile the two-file `chat` context (service in `gateway.bynk`, the agent it
/// transfers to in `room.bynk`) for `target`, returning the error categories.
fn compile_categories(target: bynkc::BuildTarget) -> Result<(), Vec<String>> {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "bynk_ws_open_split_{}_{unique}",
        std::process::id()
    ));
    let src = dir.join("src");
    std::fs::create_dir_all(src.join("chat")).unwrap();
    std::fs::write(src.join("chat").join("gateway.bynk"), GATEWAY).unwrap();
    std::fs::write(src.join("chat").join("room.bynk"), ROOM).unwrap();
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
fn a_cross_file_open_transfer_compiles_on_bundle_and_is_rejected_on_workers() {
    compile_categories(bynkc::BuildTarget::Bundle)
        .unwrap_or_else(|errs| panic!("the bundle target should accept the layout: {errs:?}"));

    let errs = compile_categories(bynkc::BuildTarget::Workers)
        .expect_err("the Workers target should reject the layout");
    assert_eq!(
        errs,
        vec!["bynk.ws.open_transfer_cross_file".to_string()],
        "exactly the cross-file diagnostic"
    );
}

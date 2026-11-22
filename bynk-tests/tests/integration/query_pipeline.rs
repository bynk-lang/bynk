//! 1B.2b (#4028): the query-helper import tests that drove the driver's in-memory pipeline,
//! moved from `bynk-emit` (`emitter/lower_call/query.rs`).

#[path = "../support/emit_pipeline.rs"]
mod emit_pipeline;
use emit_pipeline::emit_project;

const BAG: &str = "context demo.bag

type Blob = opaque Bytes

agent Bag {
  key id: String
  store nums: Map[String, Int]
  store blobs: Map[String, Bytes]

  on call plain() -> Effect[List[Int]] {
    nums.distinct().collect()
  }
  on call keyed() -> Effect[List[Bytes]] {
    blobs.distinct().collect()
  }
}
";

const ROOM: &str = "context demo.room

type Frame = { text: String }

agent Room {
  key id: String
  store conns: Map[String, Connection[Frame]]

  on call announce(text: String) -> Effect[()] {
    conns.parTraverse((c: Connection[Frame]) => c.send(Frame { text: text }))
  }
}
";

fn emitted(
    file: &str,
    source: &str,
    target: bynk_check::project_model::BuildTarget,
    suffix: &str,
) -> String {
    let out = emit_project(&[(file, source)], target);
    let (_, ts) = out
        .docs
        .iter()
        .find(|(path, _)| path.to_string_lossy().ends_with(suffix))
        .expect("the module is emitted");
    ts.clone()
}

/// A non-primitive key goes through the keyed helper and imports it, only where it is
/// used (`note_key` fires on the keyed branch alone); a primitive key stays a `Set`.
#[test]
fn a_keyed_helper_is_imported_only_where_it_is_chosen() {
    let ts = emitted(
        "demo/bag.bynk",
        BAG,
        bynk_check::project_model::BuildTarget::Bundle,
        "demo/bag.ts",
    );
    assert!(ts.contains("(__q) => [...new globalThis.Set(__q)]"), "{ts}");
    assert!(ts.contains("(__q) => __bynkDistinct(__q)"), "{ts}");
    let imports = ts
        .lines()
        .filter(|l| l.starts_with("import "))
        .collect::<String>();
    assert!(imports.contains("__bynkDistinct"), "{imports}");
    assert!(!imports.contains("__bynkDistinctBy"), "{imports}");
    let plain = BAG
        .replace("blobs.distinct()", "nums.distinct()")
        .replace("Effect[List[Bytes]]", "Effect[List[Int]]");
    let ts = emitted(
        "demo/bag.bynk",
        &plain,
        bynk_check::project_model::BuildTarget::Bundle,
        "demo/bag.ts",
    );
    assert!(!ts.contains("__bynkDistinct"), "{ts}");
}

/// A held `Map`'s resolved connections are the source, and `parTraverse` joins through
/// `__bynkSettleAll`, which the module then imports.
#[test]
fn a_held_map_broadcast_resolves_its_connections_and_imports_the_settle_helper() {
    let ts = emitted(
        "demo/room.bynk",
        ROOM,
        bynk_check::project_model::BuildTarget::Workers,
        "demo-room/handlers.ts",
    );
    assert!(
        ts.contains("__resolveConnection<Frame>(this.state, __cid)"),
        "{ts}"
    );
    assert!(
        ts.contains("await __bynkSettleAll(__q.map((__x) => () =>"),
        "{ts}"
    );
    assert!(ts.contains("__bynkSettleAll"), "{ts}");
}

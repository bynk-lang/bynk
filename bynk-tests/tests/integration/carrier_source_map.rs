//! Whole-map snapshot for the declaration carriers (back-half paydown 1A.32a,
//! Decision C of #3733).
//!
//! The provider, service and agent classes (and the history driver) are each
//! built as one carrier whose members the emitter anchors to their own
//! declaration lines (#2280). 1A.32 moves those anchors from hand-written
//! `record` calls onto member spans the printer records, so this pins the
//! *whole* decoded map, not a few named checkpoints: every non-blank generated
//! line paired with the source line it maps to. Blank lines are skipped on
//! purpose. The printer's class policy puts one blank line between members, so a
//! raw generated-line index would shift for layout alone; pairing each mapped
//! line's text with its source line moves only when a mapping really moves.
//!
//! The carrier expectations were written against the output before the
//! carriers became nodes and must not change with them. The type-method case
//! pins the one ruled change: method bodies in a top-level `const` now map to
//! their own lines.

use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};

const WALLET: &str = "context demo.wallet

type Amount = Int where Positive

---
A calculator capability.
---
capability Calc {
  fn double(n: Int) -> Int
  fn triple(n: Int) -> Effect[Int]
}

---
The simple calculator.
---
provides Calc = SimpleCalc {
  fn double(n: Int) -> Int {
    let doubled = n * 2
    doubled
  }

  fn triple(n: Int) -> Effect[Int] {
    let tripled = n * 3
    tripled
  }
}

---
A sweeper service.
---
service sweeper from cron {
  ---
  Every five minutes.
  ---
  on schedule(\"*/5 * * * *\") (at: Int) -> Effect[Result[(), String]] {
    let _ <- Wallet(\"a\").topUp(1)
    Ok(())
  }

  on schedule(\"0 * * * *\") (at: Int) -> Effect[Result[(), String]] {
    let _ <- Wallet(\"b\").spend(1)
    Ok(())
  }
}

---
A wallet.
---
agent Wallet {
  key id: String

  store balance: Cell[Int] = 0

  invariant nonneg: balance >= 0

  ---
  Tops up.
  ---
  on call topUp(amount: Amount) -> Effect[()] {
    balance.update((b) => b + amount)
  }

  on call spend(amount: Amount) -> Effect[()] {
    balance.update((b) => b - amount)
  }
}
";

const WALLET_TEST: &str = "suite demo.wallet {
  property \"no accepted spend without a prior accepted top-up\" {
    for all run: History[Wallet] {
      expect run.all((s) =>
        (s.call is Spend && s.accepted)
          implies run.upTo(s).any((p) => p.call is TopUp && p.accepted))
    }
  }
}
";

const CHAT: &str = "context chat

type RoomId = opaque String
type UserId = opaque String
type ServerFrame = { text: String }
type ClientFrame = { text: String }

actor Participant { auth = Bearer(secret = \"AUTH_SECRET\"), identity = UserId }

---
The gateway.
---
service ChatGateway from websocket(in: ClientFrame, out: ServerFrame) {
  on open (roomId: RoomId) -> Effect[()] by user: Participant {
    let _ <- connection.send(ServerFrame { text: \"welcome\" })
    let _ <- Room(roomId).join(user.identity, connection)
    ()
  }

  on message (roomId: RoomId, frame: ClientFrame) -> Effect[()] by user: Participant {
    let _ <- Room(roomId).post(user.identity, frame.text)
    ()
  }

  on close (roomId: RoomId) -> Effect[()] by user: Participant {
    let _ <- Room(roomId).leave(user.identity)
    ()
  }
}

---
A room.
---
agent Room {
  key id: RoomId
  store members: Set[UserId]
  store conns: Map[UserId, Connection[ServerFrame]]

  on call join(u: UserId, conn: Connection[ServerFrame]) -> Effect[()] {
    let _ <- members.add(u)
    let _ <- conns.put(u, conn)
    ()
  }

  on call leave(u: UserId) -> Effect[()] {
    let _ <- members.remove(u)
    let _ <- conns.remove(u)
    ()
  }

  on call post(sender: UserId, text: String) -> Effect[()] {
    let _ <- conns.parTraverse((c: Connection[ServerFrame]) => c.send(ServerFrame { text: text }))
    ()
  }
}
";

/// Decode the `mappings` string into `gen_line0 -> Some(src_line0)`, for the
/// one-segment-per-line maps the builder emits.
fn decode(mappings: &str) -> Vec<Option<i64>> {
    const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let dec = |s: &str| -> Vec<i64> {
        let mut out = Vec::new();
        let (mut shift, mut acc) = (0i64, 0i64);
        for &c in s.as_bytes() {
            let d = B64.iter().position(|&b| b == c).unwrap() as i64;
            acc += (d & 0b11111) << shift;
            if d & 0b100000 != 0 {
                shift += 5;
            } else {
                out.push(if acc & 1 == 1 { -(acc >> 1) } else { acc >> 1 });
                shift = 0;
                acc = 0;
            }
        }
        out
    };
    let mut src_line = 0i64;
    let mut lines = Vec::new();
    for seg in mappings.split(';') {
        if seg.is_empty() {
            lines.push(None);
            continue;
        }
        src_line += dec(seg)[2];
        lines.push(Some(src_line));
    }
    lines
}

fn extract_field<'a>(json: &'a str, key: &str) -> &'a str {
    let k = format!("\"{key}\":\"");
    let start = json.find(&k).expect("key present") + k.len();
    let rest = &json[start..];
    &rest[..rest.find('"').unwrap()]
}

/// Compile `files` (relative path, text) as a single-tree project and return the
/// generated file ending in `suffix` with its decoded map.
fn compile(
    files: &[(&str, &str)],
    target: Option<bynk_driver::project::BuildTarget>,
    suffix: &str,
) -> (String, String) {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir =
        std::env::temp_dir().join(format!("bynk_carrier_map_{}_{unique}", std::process::id()));
    let src = dir.join("src");
    for (name, text) in files {
        let path = src.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let mut options = bynk_testkit::compile_options_single(src.clone());
    if let Some(target) = target {
        options = options.target(target);
    }
    let out = bynk_driver::project::compile_project(&options)
        .map_err(bynk_driver::project::ProjectFailure::flatten)
        .unwrap_or_else(|e| panic!("compile failed: {e:?}"));
    let path = out
        .artefacts
        .docs
        .keys()
        .find(|p| p.to_string_lossy().replace('\\', "/").ends_with(suffix))
        .unwrap_or_else(|| panic!("no {suffix} in output"))
        .clone();
    let ts = out.artefacts.docs[&path].text().to_string();
    let map = out
        .artefacts
        .docs
        .get(&bynk_driver::project::sibling_path(Path::new(&path), "map"))
        .unwrap_or_else(|| panic!("{suffix} carries a source map"))
        .text()
        .to_string();
    let _ = std::fs::remove_dir_all(&dir);
    (ts, map)
}

/// The decoded map, one entry per non-blank generated line, in order: the
/// 0-based source line it maps to, or `-` when it is unmapped. Sixteen entries
/// to a row, so a moved checkpoint reads as one changed number in a row.
fn snapshot(ts: &str, map: &str) -> String {
    let lines = decode(extract_field(map, "mappings"));
    let entries: Vec<String> = ts
        .lines()
        .enumerate()
        .filter(|(_, text)| !text.trim().is_empty())
        .map(|(i, _)| match lines.get(i).copied().flatten() {
            Some(src) => src.to_string(),
            None => "-".to_string(),
        })
        .collect();
    entries
        .chunks(16)
        .map(|row| row.join(" "))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn check(actual: String, expected: &str, what: &str) {
    assert!(
        actual == expected,
        "{what}: the decoded source map moved. Actual snapshot:\n{actual}"
    );
}

/// A documented provider, service, agent and history driver on the bundle target.
#[test]
fn provider_service_agent_and_history_driver_map_is_unchanged() {
    let (ts, map) = compile(
        &[
            ("demo/wallet.bynk", WALLET),
            ("tests/wallet.test.bynk", WALLET_TEST),
        ],
        None,
        "demo/wallet.ts",
    );
    check(snapshot(&ts, &map), WALLET_MAP, "wallet.ts");
}

/// A documented agent with a `from websocket` host on the Workers target.
#[test]
fn websocket_agent_map_is_unchanged() {
    let (ts, map) = compile(
        &[("chat.bynk", CHAT)],
        Some(bynk_driver::project::BuildTarget::Workers),
        "handlers.ts",
    );
    check(snapshot(&ts, &map), CHAT_MAP, "handlers.ts");
}

// The carrier part of this snapshot (provider, service, agent) is unchanged. The history
// driver, the trailing run of `48`s (the agent's span, which every driver line inherits), grew
// by 18 lines (the final rows, 39 entries to 57) when it became nodes: its `switch` cases,
// `try`/`catch`, `if`/`else` and `for` body now print one statement per line. That is line growth from the ruled re-layout (1A.32b), not a
// moved mapping.
const WALLET_MAP: &str = "\
- - - - - - - - 2 2 2 2 2 2 2 2
2 2 2 2 7 7 7 7 7 7 7 7 15 15 15 15
16 17 18 18 21 22 23 23 15 15 30 30 30 30 34 34
34 34 35 36 36 39 40 41 41 30 48 48 48 48 48 48
48 48 48 48 48 48 48 48 48 48 48 48 48 48 48 48
48 48 48 48 48 48 48 48 48 48 48 48 48 48 48 48
58 58 58 58 58 58 59 59 59 59 59 59 59 59 62 62
62 63 63 63 63 63 63 63 63 48 48 48 48 48 48 48
48 48 48 48 48 48 48 48 48 48 48 48 48 48 48 48
48 48 48 48 48 48 48 48 48 48 48 48 48 48 48 48
48 48 48 48 48 48 48 48 48 48 48 48 48 48 48 48
48 48 48 48 48 48 48 48 48
";
const CHAT_MAP: &str = "\
- - - - - - - - - - - 2 2 2 2 2
2 2 2 2 3 3 3 3 3 3 3 3 3 4 4 4
4 4 5 5 5 5 5 12 12 12 12 12 33 33 33 33
33 33 33 33 33 33 33 33 33 33 33 33 33 33 33 33
33 33 33 33 33 33 33 33 33 33 33 33 33 33 33 33
33 33 33 33 33 33 33 33 33 33 38 38 38 39 39 39
39 40 40 40 40 40 40 40 40 40 40 40 40 40 41 41
41 41 41 44 44 44 45 45 45 45 46 46 46 46 46 46
46 46 46 46 46 46 47 47 47 47 47 50 50 51 51 51
52 52 13 14 15 16 16 19 20 21 21 24 25 26 26 33
33 33 33 33 33 33 33 33 33 33 33 33 33 33 33 33
33 33 33 33 33 33 33 33 33 33 33 33 33 33 33 33
33 33 33 33 33 33 33 33 33 33 33 33 33 33 33 33
33 33 33 33 33 33 33 33 33 33 33 33 33 33 33 33
33 33 33 33 33 33 33 33 33 33 33 33 33 33 33 33
33 33 33 33 33 33 33 33 33 33 33 33 33 33 33 33
";

const MONEY: &str = "commons commerce.money {
  type Money = Int where NonNegative

  fn Money.fromMinorUnits(n: Int) -> Result[Money, ValidationError] {
    Money.of(n)
  }
}
";

/// A type's methods print as a top-level `const` object. Since 1A.32a a
/// top-level `const` passes its method entries' spans to the module map
/// (before, they were dropped and the whole object mapped to the commons
/// header), so each statement in a method body maps to its own source line, as
/// a free function's does. Ruled on in #3808: the more precise mapping is kept
/// and pinned here.
#[test]
fn a_type_method_body_maps_to_its_own_source_line() {
    let (ts, map) = compile(&[("commerce/money.bynk", MONEY)], None, "commerce/money.ts");
    let lines = decode(extract_field(&map, "mappings"));
    let body = ts
        .lines()
        .position(|l| l.trim() == "return Money.of(n);")
        .expect("the method body line");
    assert_eq!(
        lines.get(body).copied().flatten(),
        Some(4),
        "the method body maps to its own `Money.of(n)` line (0-based source line 4), not the commons header"
    );
    check(snapshot(&ts, &map), MONEY_MAP, "money.ts");
}

const MONEY_MAP: &str = "\
- - - - 1 1 1 1 1 1 1 1 1 1 1 1
4 4 4
";

//! v0.103 (real-time track slice 3a): behavioural proof that a `from websocket`
//! service *runs* on the bundle target against `TestConnection`. Compiles the
//! §20 chat-room fixture in-process, then a Node driver drives the `on open`
//! handler with a `TestConnection` and asserts the held connection flowed
//! through — the welcome frame the handler sent is captured on the connection
//! before it is transferred into the Room agent.
//!
//! Like the tsc-verification stage, this skips loudly when no TypeScript
//! toolchain is available; `BYNK_REQUIRE_TSC=1` turns the skip into a failure.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const REQUIRE_ENV: &str = "BYNK_REQUIRE_TSC";

fn base_command(program: &str) -> Command {
    if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(program);
        c
    } else {
        Command::new(program)
    }
}

fn tool_exists(name: &str) -> bool {
    let finder = if cfg!(windows) { "where" } else { "which" };
    base_command(finder)
        .arg(name)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn discover_tsc() -> Option<(String, Vec<String>)> {
    if tool_exists("tsc") {
        return Some(("tsc".to_string(), vec![]));
    }
    if tool_exists("npx") {
        return Some((
            "npx".to_string(),
            vec![
                "--yes".to_string(),
                "-p".to_string(),
                format!("typescript@{}", bynk_emit::TYPESCRIPT_MAJOR_TESTED),
                "tsc".to_string(),
            ],
        ));
    }
    None
}

fn run(program: &str, prefix: &[String], args: &[&str], cwd: &Path) -> (bool, String) {
    let mut cmd = base_command(program);
    for p in prefix {
        cmd.arg(p);
    }
    for a in args {
        cmd.arg(a);
    }
    cmd.current_dir(cwd);
    let output = match cmd.output() {
        Ok(o) => o,
        Err(e) => return (false, format!("could not launch {program}: {e}")),
    };
    let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(&output.stderr));
    (output.status.success(), combined)
}

const DRIVER_TS: &str = r#"
import { ChatGateway, RoomId, UserId } from "./chat.js";
import { TestConnection } from "./__bynk_runtime/connection.js";

function assert(cond: boolean, msg: string): void {
  if (!cond) {
    throw new Error(`assertion failed: ${msg}`);
  }
}

async function main(): Promise<void> {
  // The §20 chat-room, end-to-end on the bundle target (the track's completion
  // proof). Two participants join one room; a message broadcasts to both.
  const roomId = RoomId.unsafe("room-1");
  const alice = UserId.unsafe("alice");
  const bob = UserId.unsafe("bob");
  const tcA = new TestConnection<{ text: string }>();
  const tcB = new TestConnection<{ text: string }>();

  // `on open` (edge upgrade): each connection sends a welcome frame, then is
  // transferred into the same Room agent (keyed by roomId).
  await ChatGateway.open(tcA, roomId, { identity: alice });
  await ChatGateway.open(tcB, roomId, { identity: bob });
  assert(tcA.sent.length === 1 && tcA.sent[0].text === "welcome", "alice got the welcome frame");
  assert(tcB.sent.length === 1 && tcB.sent[0].text === "welcome", "bob got the welcome frame");
  assert(!tcA.closed && !tcB.closed, "both connections are transferred (open), not closed");

  // Slice 4 — broadcast: alice sends a message; `on message` posts it to the room,
  // which `parTraverse`s every held connection and sends — so BOTH alice and bob
  // receive it (the fan-out). The held-aware iteration borrow at work.
  await ChatGateway.message(tcA, roomId, { text: "hello room" }, { identity: alice });
  assert(tcA.sent.length === 2 && tcA.sent[1].text === "hello room", "alice received the broadcast");
  assert(tcB.sent.length === 2 && tcB.sent[1].text === "hello room", "bob received the broadcast");
  assert(!tcA.closed && !tcB.closed, "the borrowed connections stay open after the broadcast");

  // Slice 3b-iii — close: bob leaves; a subsequent broadcast reaches only alice.
  await ChatGateway.close(tcB, roomId, { identity: bob });
  await ChatGateway.message(tcA, roomId, { text: "after leave" }, { identity: alice });
  assert(tcA.sent.length === 3 && tcA.sent[2].text === "after leave", "alice received the second broadcast");
  assert(tcB.sent.length === 2, "bob (left) received no further broadcast");

  console.log("ALL OK");
}

main().catch((e: unknown) => {
  console.error(e);
  // Re-throw so an assertion failure surfaces as a non-zero exit (and no
  // "ALL OK" is printed).
  throw e;
});
"#;

const TSCONFIG_JSON: &str = r#"{
  "compilerOptions": {
    "module": "Node16",
    "moduleResolution": "node16",
    "target": "ES2022",
    "strict": true,
    "skipLibCheck": true,
    "outDir": "js",
    "rootDir": ".",
    "lib": ["ES2022", "DOM"]
  },
  "include": ["**/*.ts"],
  "exclude": ["js"]
}
"#;

#[test]
fn websocket_chatroom_runs_on_bundle() {
    let runner = match discover_tsc() {
        Some(r) => r,
        None => {
            eprintln!(
                "\n!!! WEBSOCKET BEHAVIOUR VERIFICATION SKIPPED !!!\nno tsc runner on PATH.\n"
            );
            if std::env::var(REQUIRE_ENV).is_ok() {
                panic!("{REQUIRE_ENV} is set but no tsc runner was found");
            }
            return;
        }
    };
    if !tool_exists("node") {
        eprintln!("\n!!! WEBSOCKET BEHAVIOUR VERIFICATION SKIPPED !!!\n`node` is not on PATH.\n");
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("{REQUIRE_ENV} is set but `node` was not found");
        }
        return;
    }

    // Compile the chat-room fixture (bundle) in-process.
    let fixture: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/positive/236_websocket_chatroom/src");
    let out = bynkc::compile_project(
        &bynk_testkit::compile_options_single(fixture).target(bynkc::BuildTarget::Bundle),
    )
    .map_err(bynkc::ProjectFailure::flatten)
    .expect("the chat-room bundle fixture must compile");

    let tmp = std::env::temp_dir().join(format!("bynk-ws-behaviour-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();
    for (path, doc) in &out.artefacts.docs {
        if path.to_string_lossy() == "tsconfig.json" {
            continue;
        }
        let target_path = tmp.join(path);
        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&target_path, doc.text()).unwrap();
    }
    fs::write(tmp.join("driver.ts"), DRIVER_TS).unwrap();
    fs::write(tmp.join("tsconfig.json"), TSCONFIG_JSON).unwrap();

    // Type-check and compile to JS, then run the driver under Node.
    let (ok, log) = run(&runner.0, &runner.1, &["--project", "tsconfig.json"], &tmp);
    assert!(ok, "the chat-room driver must type-check + compile:\n{log}");
    let (ran, log) = run("node", &[], &["js/driver.js"], &tmp);
    assert!(
        ran && log.contains("ALL OK"),
        "the chat-room driver must run green:\n{log}"
    );

    let _ = fs::remove_dir_all(&tmp);
}

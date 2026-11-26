//! #3900: behavioural proof of the Bearer WebSocket handshake. A browser cannot set a header on the
//! upgrade, so it offers `new WebSocket(url, ["bynk.bearer", token])`: the marker, then the token.
//! This compiles the chat-room Workers fixture and drives the emitted Worker's `fetch` with raw
//! upgrade requests under Node, asserting the edge's verdict for each shape of offer:
//!
//! * the pair with a valid token is forwarded to the hosting Durable Object, with its verified
//!   identity;
//! * the old single-element `[token]` form (which only a client that does not enforce the standard
//!   could use) is a `400` with a fixed message naming the pair, and is never forwarded;
//! * no offer, the marker with no token, and a bad or foreign-secret token are `401`, never forwarded.
//!
//! The Durable Object's `101` selecting `bynk.bearer` is the runtime helper's
//! (`bynk-emit/runtime/test/connection.test.ts`) and the golden's.
//!
//! Like the other tsc-driven tests it skips loudly without a toolchain; `BYNK_REQUIRE_TSC=1` turns the
//! skip into a failure.

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
import worker from "./workers/chat/index.js";

function assert(cond: boolean, msg: string): void {
  if (!cond) {
    throw new Error(`assertion failed: ${msg}`);
  }
}

const SECRET = "handshake-test-secret";

function b64url(s: string): string {
  return btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

async function sign(payload: Record<string, unknown>, secret: string): Promise<string> {
  const h = b64url(JSON.stringify({ alg: "HS256", typ: "JWT" }));
  const p = b64url(JSON.stringify(payload));
  const enc = new TextEncoder();
  const key = await crypto.subtle.importKey(
    "raw",
    enc.encode(secret) as BufferSource,
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"],
  );
  const sig = new Uint8Array(await crypto.subtle.sign("HMAC", key, enc.encode(`${h}.${p}`) as BufferSource));
  let bin = "";
  for (const b of sig) bin += String.fromCharCode(b);
  return `${h}.${p}.${btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "")}`;
}

// The hosting Durable Object, as a recording stub: what reaches it is what the edge forwarded.
const forwarded: { protocol: string | null; open: string | null }[] = [];
const ROOM = {
  idFromName: (name: string) => ({ toString: () => name }),
  idFromString: (id: string) => ({ toString: () => id }),
  get: () => ({
    fetch: async (req: Request) => {
      forwarded.push({
        protocol: req.headers.get("Sec-WebSocket-Protocol"),
        open: req.headers.get("X-Bynk-Ws-Open"),
      });
      return new Response("hosted", { status: 200 });
    },
  }),
};
const env = { AUTH_SECRET: SECRET, ROOM };

async function upgrade(protocol: string | null): Promise<Response> {
  const headers: Record<string, string> = { Upgrade: "websocket", Connection: "Upgrade" };
  if (protocol !== null) {
    headers["Sec-WebSocket-Protocol"] = protocol;
  }
  return worker.fetch(new Request("https://chat.example/?roomId=r1", { headers }), env);
}

const now = Math.floor(Date.now() / 1000);
const good = await sign({ sub: "ada", exp: now + 3600 }, SECRET);

// The pair with a valid token reaches the Durable Object, with the verified identity.
let res = await upgrade(`bynk.bearer, ${good}`);
assert(res.status === 200 && (await res.text()) === "hosted", "the pair with a valid token is forwarded");
assert(forwarded.length === 1, "exactly one request reached the Durable Object");
assert(forwarded[0].open !== null && JSON.parse(forwarded[0].open).identity === "ada", "the verified identity rides along");

// The old single-element form is a 400 naming the pair, and is never forwarded.
res = await upgrade(good);
assert(res.status === 400, `the old [token] form is a 400, got ${res.status}`);
const message = await res.text();
assert(message === 'Offer the subprotocols ["bynk.bearer", <token>]', `the refusal names the pair: ${message}`);
assert(!message.includes(good), "the refusal never echoes the token");
assert(forwarded.length === 1, "the old form is not forwarded");

// No offer, the marker with no token, a bad token and a foreign-secret token are 401, never forwarded.
for (const [offer, why] of [
  [null, "no subprotocol"],
  ["bynk.bearer", "the marker with no token"],
  ["bynk.bearer, not.a.jwt", "a malformed token"],
  [`bynk.bearer, ${await sign({ sub: "ada", exp: now + 3600 }, "another-secret")}`, "a token signed with another secret"],
  [`bynk.bearer, ${await sign({ sub: "ada", exp: now - 10 }, SECRET)}`, "an expired token"],
] as [string | null, string][]) {
  res = await upgrade(offer);
  assert(res.status === 401, `${why} is a 401, got ${res.status}`);
}
assert(forwarded.length === 1, "no refused upgrade reached the Durable Object");

// A request that is not an upgrade is still a 404 here (no route), unchanged.
res = await worker.fetch(new Request("https://chat.example/?roomId=r1"), env);
assert(res.status === 404, "a non-upgrade request is unchanged");

console.log("ALL OK");
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
fn the_bearer_handshake_offers_the_marker_then_the_token() {
    let runner = match discover_tsc() {
        Some(r) => r,
        None => {
            eprintln!(
                "\n!!! WEBSOCKET HANDSHAKE VERIFICATION SKIPPED !!!\nno tsc runner on PATH.\n"
            );
            if std::env::var(REQUIRE_ENV).is_ok() {
                panic!("{REQUIRE_ENV} is set but no tsc runner was found");
            }
            return;
        }
    };
    if !tool_exists("node") {
        eprintln!("\n!!! WEBSOCKET HANDSHAKE VERIFICATION SKIPPED !!!\n`node` is not on PATH.\n");
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("{REQUIRE_ENV} is set but `node` was not found");
        }
        return;
    }

    // Compile the chat-room Workers fixture in-process.
    let fixture: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/positive/237_websocket_chatroom_workers/src");
    let out = bynk_driver::project::compile_project(
        &bynk_testkit::compile_options_single(fixture)
            .target(bynk_check::project_model::BuildTarget::Workers),
    )
    .map_err(bynk_driver::project::ProjectFailure::flatten)
    .expect("the chat-room Workers fixture must compile");

    let tmp = std::env::temp_dir().join(format!("bynk-ws-handshake-{}", std::process::id()));
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
    fs::write(tmp.join("package.json"), "{ \"type\": \"module\" }").unwrap();

    // Type-check and compile to JS, then run the driver under Node.
    let (ok, log) = run(&runner.0, &runner.1, &["--project", "tsconfig.json"], &tmp);
    assert!(ok, "the handshake driver must type-check + compile:\n{log}");
    let (ran, log) = run("node", &[], &["js/driver.js"], &tmp);
    assert!(
        ran && log.contains("ALL OK"),
        "the handshake driver must run green:\n{log}"
    );

    let _ = fs::remove_dir_all(&tmp);
}

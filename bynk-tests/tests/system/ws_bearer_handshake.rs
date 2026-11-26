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
//! #3904 adds the cookie and `Oidc` forms (below). The Durable Object's `101` selecting `bynk.bearer` is the runtime helper's
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
assert(forwarded[0].protocol !== null && forwarded[0].protocol.split(",").map((p) => p.trim()).includes("bynk.bearer"), "the offered marker reaches the Durable Object, which selects it");

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

/// #3900: the actor-sum path (`by who: User | Visitor`). The marker with a valid token forwards the
/// `User`; the marker with a bad or no token falls through to the `None` member `Visitor`; the old
/// single-element `[token]` form is still a `400`, even though a catch-all member exists.
const SUM_DRIVER_TS: &str = r#"
import worker from "./workers/room/index.js";

function assert(cond: boolean, msg: string): void {
  if (!cond) {
    throw new Error(`assertion failed: ${msg}`);
  }
}

const SECRET = "sum-handshake-secret";

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

const forwarded: { protocol: string | null; who: { tag: string; identity?: string } | null }[] = [];
const ROOM = {
  idFromName: (name: string) => ({ toString: () => name }),
  idFromString: (id: string) => ({ toString: () => id }),
  get: () => ({
    fetch: async (req: Request) => {
      const open = req.headers.get("X-Bynk-Ws-Open");
      forwarded.push({
        protocol: req.headers.get("Sec-WebSocket-Protocol"),
        who: open === null ? null : JSON.parse(open).who ?? null,
      });
      return new Response("hosted", { status: 200 });
    },
  }),
};
const env = { USER_SECRET: SECRET, ROOM };

async function upgrade(protocol: string | null): Promise<Response> {
  const headers: Record<string, string> = { Upgrade: "websocket", Connection: "Upgrade" };
  if (protocol !== null) {
    headers["Sec-WebSocket-Protocol"] = protocol;
  }
  return worker.fetch(new Request("https://room.example/?room=lobby", { headers }), env);
}

const now = Math.floor(Date.now() / 1000);
const good = await sign({ sub: "ada", exp: now + 3600 }, SECRET);

// A valid pair is forwarded as the User member, with the marker still on the request.
let res = await upgrade(`bynk.bearer, ${good}`);
assert(res.status === 200, `a valid pair is forwarded, got ${res.status}`);
assert(forwarded.length === 1 && forwarded[0].who?.tag === "User", "the verified member is User");
assert(forwarded[0].who?.identity === "ada", "the User carries the verified identity");
assert(forwarded[0].protocol !== null && forwarded[0].protocol.split(",").map((p) => p.trim()).includes("bynk.bearer"), "the marker reaches the Durable Object");

// A bad token, a foreign-secret token and no token after the marker fall through to the None member.
for (const offer of [
  "bynk.bearer, not.a.jwt",
  `bynk.bearer, ${await sign({ sub: "ada", exp: now + 3600 }, "another-secret")}`,
  "bynk.bearer",
  null,
] as (string | null)[]) {
  const before = forwarded.length;
  res = await upgrade(offer);
  assert(res.status === 200, `${offer} falls through to Visitor, got ${res.status}`);
  assert(forwarded.length === before + 1 && forwarded[before].who?.tag === "Visitor", `${offer} resolves to Visitor`);
  assert(forwarded[before].who?.identity === undefined, "Visitor carries no identity");
}

// The old single-element form is a 400 naming the pair, never forwarded, never echoing the token.
const count = forwarded.length;
res = await upgrade(good);
assert(res.status === 400, `the old [token] form is a 400, got ${res.status}`);
const message = await res.text();
assert(message === 'Offer the subprotocols ["bynk.bearer", <token>]', `the refusal names the pair: ${message}`);
assert(!message.includes(good), "the refusal never echoes the token");
assert(forwarded.length === count, "the old form is not forwarded");

console.log("ALL OK");
"#;

/// #3904: a `SignedCookie` actor on `on open`. The upgrade must carry an `Origin` that is exactly one
/// of the service's `cors` origins, checked before the cookie is read, and then a valid session
/// cookie. Anything else is refused at the edge and never forwarded.
const COOKIE_DRIVER_TS: &str = r#"
import worker from "./workers/chat/index.js";
import { __newSessionKey, __signSessionCookie } from "./__bynk_runtime/session.js";

function assert(cond: boolean, msg: string): void {
  if (!cond) {
    throw new Error(`assertion failed: ${msg}`);
  }
}

const KEY = __newSessionKey();
const OTHER_KEY = __newSessionKey();
const LISTED = "https://app.example.com";

const forwarded: { open: string | null }[] = [];
const ROOM = {
  idFromName: (name: string) => ({ toString: () => name }),
  idFromString: (id: string) => ({ toString: () => id }),
  get: () => ({
    fetch: async (req: Request) => {
      forwarded.push({ open: req.headers.get("X-Bynk-Ws-Open") });
      return new Response("hosted", { status: 200 });
    },
  }),
};
const env = { SESSION_KEY: KEY, ROOM };

async function upgrade(origin: string | null, cookie: string | null, extra: Record<string, string> = {}): Promise<Response> {
  const headers: Record<string, string> = { Upgrade: "websocket", Connection: "Upgrade", ...extra };
  if (origin !== null) headers["Origin"] = origin;
  if (cookie !== null) headers["Cookie"] = `__Host-session=${cookie}`;
  return worker.fetch(new Request("https://chat.example/?roomId=r1", { headers }), env);
}

const good = await __signSessionCookie({ secret: KEY }, "Member", "ada", 3600);

// A listed Origin with a valid cookie is forwarded, with the verified identity and the cookie's expiry.
let res = await upgrade(LISTED, good);
assert(res.status === 200 && (await res.text()) === "hosted", "a listed Origin with a valid cookie is forwarded");
assert(forwarded.length === 1, "exactly one request reached the Durable Object");
const open = JSON.parse(forwarded[0].open ?? "{}");
assert(open.identity === "ada", "the verified identity rides along");
assert(typeof open.exp === "number" && open.exp > Math.floor(Date.now() / 1000), "the cookie's expiry rides along");

// A missing, unlisted or near-miss Origin is a 403 before the cookie is read, so even a request with
// no cookie at all is a 403 and not a 401, and a valid cookie does not help.
for (const [origin, why] of [
  [null, "no Origin"],
  ["https://evil.example", "an unlisted Origin"],
  ["https://app.example.com.evil.example", "a Origin that only begins with a listed one"],
  ["http://app.example.com", "a listed host over http"],
  ["https://app.example.com:8443", "a listed host on another port"],
  ["https://APP.example.com", "a listed host in another case"],
  ["https://app.example.com/", "a listed Origin with a trailing slash"],
  ["null", "the opaque Origin `null`"],
  ["*", "a wildcard Origin"],
] as [string | null, string][]) {
  res = await upgrade(origin, good);
  assert(res.status === 403, `${why} is a 403, got ${res.status}`);
  res = await upgrade(origin, null);
  assert(res.status === 403, `${why} with no cookie is a 403 (the Origin is checked first), got ${res.status}`);
}

// `Sec-Fetch-Site` is not an origin check: `same-origin` with no `Origin` is still refused.
res = await upgrade(null, good, { "Sec-Fetch-Site": "same-origin" });
assert(res.status === 403, `Sec-Fetch-Site alone is not enough, got ${res.status}`);
res = await upgrade("https://evil.example", good, { "Sec-Fetch-Site": "same-origin" });
assert(res.status === 403, `Sec-Fetch-Site does not rescue an unlisted Origin, got ${res.status}`);
assert(forwarded.length === 1, "no refused Origin reached the Durable Object");

// A listed Origin with no cookie, a forged, foreign-key, wrong-audience or expired cookie is a 401.
const expired = await __signSessionCookie({ secret: KEY }, "Member", "ada", 60, Date.now() - 3_600_000);
for (const [cookie, why] of [
  [null, "no cookie"],
  ["not.a.cookie", "a malformed cookie"],
  [await __signSessionCookie({ secret: OTHER_KEY }, "Member", "ada", 3600), "a cookie signed with another key"],
  [await __signSessionCookie({ secret: KEY }, "Other", "ada", 3600), "a cookie issued for another actor"],
  [expired, "an expired cookie"],
] as [string | null, string][]) {
  res = await upgrade(LISTED, cookie);
  assert(res.status === 401, `${why} is a 401, got ${res.status}`);
}
assert(forwarded.length === 1, "no refused credential reached the Durable Object");

console.log("ALL OK");
"#;

/// #3904: an `Oidc` actor on `on open`. The token is the subprotocol after `bynk.oidc`, verified
/// against the provider's JWKS, so a driver serves a JWKS from a stubbed `fetch`.
const OIDC_DRIVER_TS: &str = r#"
import worker from "./workers/chat/index.js";

function assert(cond: boolean, msg: string): void {
  if (!cond) {
    throw new Error(`assertion failed: ${msg}`);
  }
}

function b64urlBytes(bytes: Uint8Array): string {
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}
function b64url(s: string): string {
  return b64urlBytes(new TextEncoder().encode(s));
}

const pair = await crypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, true, ["sign", "verify"]);
const other = await crypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, true, ["sign", "verify"]);
const jwk = { ...(await crypto.subtle.exportKey("jwk", pair.publicKey)), kid: "k1", alg: "ES256", use: "sig" };
const realFetch = globalThis.fetch;
globalThis.fetch = (async (input: unknown, init?: unknown) => {
  if (String(input) === "https://id.example/jwks") {
    return new Response(JSON.stringify({ keys: [jwk] }), { status: 200, headers: { "content-type": "application/json" } });
  }
  return realFetch(input as string, init as RequestInit);
}) as typeof fetch;

async function mint(claims: Record<string, unknown>, key: CryptoKey): Promise<string> {
  const h = b64url(JSON.stringify({ alg: "ES256", typ: "JWT", kid: "k1" }));
  const p = b64url(JSON.stringify(claims));
  const sig = new Uint8Array(await crypto.subtle.sign({ name: "ECDSA", hash: "SHA-256" }, key, new TextEncoder().encode(`${h}.${p}`) as BufferSource));
  return `${h}.${p}.${b64urlBytes(sig)}`;
}

const forwarded: { protocol: string | null; open: string | null }[] = [];
const ROOM = {
  idFromName: (name: string) => ({ toString: () => name }),
  idFromString: (id: string) => ({ toString: () => id }),
  get: () => ({
    fetch: async (req: Request) => {
      forwarded.push({ protocol: req.headers.get("Sec-WebSocket-Protocol"), open: req.headers.get("X-Bynk-Ws-Open") });
      return new Response("hosted", { status: 200 });
    },
  }),
};
const env = { ROOM };

async function upgrade(protocol: string | null): Promise<Response> {
  const headers: Record<string, string> = { Upgrade: "websocket", Connection: "Upgrade" };
  if (protocol !== null) headers["Sec-WebSocket-Protocol"] = protocol;
  return worker.fetch(new Request("https://chat.example/?roomId=r1", { headers }), env);
}

const now = Math.floor(Date.now() / 1000);
const claims = { sub: "ada", iss: "https://id.example", aud: "chat", exp: now + 3600 };
const good = await mint(claims, pair.privateKey);

// The pair `["bynk.oidc", token]` with a valid token is forwarded, with the identity and the expiry.
let res = await upgrade(`bynk.oidc, ${good}`);
assert(res.status === 200, `a valid token is forwarded, got ${res.status}`);
const open = JSON.parse(forwarded[0].open ?? "{}");
assert(open.identity === "ada", "the verified identity rides along");
assert(open.exp === now + 3600, "the token's expiry rides along");
assert(forwarded[0].protocol !== null && forwarded[0].protocol.split(",").map((p) => p.trim()).includes("bynk.oidc"), "the marker reaches the Durable Object, which selects it");

// The `bynk.bearer` marker is not this actor's, and the old single-element form is a 400 naming the pair.
res = await upgrade(`bynk.bearer, ${good}`);
assert(res.status === 400, `the Bearer marker is not an Oidc offer, got ${res.status}`);
res = await upgrade(good);
assert(res.status === 400, `the old [token] form is a 400, got ${res.status}`);
const message = await res.text();
assert(message === 'Offer the subprotocols ["bynk.oidc", <token>]', `the refusal names the pair: ${message}`);
assert(!message.includes(good), "the refusal never echoes the token");

// No offer, the marker alone, and a bad, foreign-key, wrong-issuer, wrong-audience or expired token are 401.
for (const [offer, why] of [
  [null, "no subprotocol"],
  ["bynk.oidc", "the marker with no token"],
  ["bynk.oidc, not.a.jwt", "a malformed token"],
  [`bynk.oidc, ${await mint(claims, other.privateKey)}`, "a token signed with another key"],
  [`bynk.oidc, ${await mint({ ...claims, iss: "https://evil.example" }, pair.privateKey)}`, "a token from another issuer"],
  [`bynk.oidc, ${await mint({ ...claims, aud: "other" }, pair.privateKey)}`, "a token for another audience"],
  [`bynk.oidc, ${await mint({ ...claims, exp: now - 3600 }, pair.privateKey)}`, "an expired token"],
] as [string | null, string][]) {
  res = await upgrade(offer);
  assert(res.status === 401, `${why} is a 401, got ${res.status}`);
}
assert(forwarded.length === 1, "no refused upgrade reached the Durable Object");

console.log("ALL OK");
"#;

/// Compile `fixture` (a Workers fixture) and run `driver` against its emitted Worker under Node.
fn drive(fixture_name: &str, driver: &str) {
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
        .join("tests/fixtures/positive")
        .join(fixture_name)
        .join("src");
    let out = bynk_driver::project::compile_project(
        &bynk_testkit::compile_options_single(fixture)
            .target(bynk_check::project_model::BuildTarget::Workers),
    )
    .map_err(bynk_driver::project::ProjectFailure::flatten)
    .expect("the chat-room Workers fixture must compile");

    let tmp = std::env::temp_dir().join(format!(
        "bynk-ws-handshake-{fixture_name}-{}",
        std::process::id()
    ));
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
    fs::write(tmp.join("driver.ts"), driver).unwrap();
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

#[test]
fn the_bearer_handshake_offers_the_marker_then_the_token() {
    drive("237_websocket_chatroom_workers", DRIVER_TS);
}

#[test]
fn an_actor_sum_upgrade_resolves_the_pair_first_wins() {
    drive("3041_ws_open_actor_sum", SUM_DRIVER_TS);
}

#[test]
fn a_cookie_upgrade_needs_a_listed_origin_before_the_cookie_is_read() {
    drive("3904_ws_signed_cookie", COOKIE_DRIVER_TS);
}

#[test]
fn an_oidc_upgrade_offers_the_token_after_bynk_oidc() {
    drive("3904_ws_oidc", OIDC_DRIVER_TS);
}

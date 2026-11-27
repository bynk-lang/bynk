//! #3939 (the API keys track, D2 and D3): behavioural proof of the `ApiKey` seam. This compiles the
//! `3939_api_key` Workers fixture and drives the emitted Worker's `fetch` under Node, with the
//! generated key-store agents running in-process, asserting:
//!
//! * `mint` answers the key itself on the wire, once, and that key is admitted as its identity;
//! * every refusal from parsing on (a malformed key, an unknown key number, an unknown handle, a
//!   forged secret, another prefix, an oversized or repeated key) is one byte-identical `401`;
//! * no key, or a key without `Bearer`, is a `401` whose challenge names no error (RFC 6750);
//! * a store fault is a `500` and `AgentBusy` a `503` with `Retry-After: 1`, never a `401` or an
//!   admit, and a missing pepper faults the request;
//! * the store compares once against the dummy digest for an unknown key number and an unknown
//!   handle, once against the record for a forged secret, and is never called for a key refused at
//!   parsing (counters by raw request, not timing);
//! * one store call per request, whose body carries neither the pepper nor the secret, and the
//!   emitted key-store agents name no pepper;
//! * the CORS preflight allows the key's header;
//! * (slice 2) a revoked key is the same `401` after the same one dummy compare as a key never
//!   minted; another identity's `revoke` answers `false` and leaves the key; `list` carries ids,
//!   labels and times, never a secret; a mint after `revokeAll`'s failed second commit takes a
//!   fresh handle; and `revokeAll` refuses every old key and rotates the handle.
//!
//! A `system`-tier suite cannot make the key store fail or queue, or fail one of two commits, so
//! those live here. Like the other
//! tsc-driven tests it skips loudly without a toolchain; `BYNK_REQUIRE_TSC=1` turns the skip into a
//! failure.

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
// #3939 (the API keys track, D3): the `ApiKey` seam, driven through the emitted Worker's `fetch`.
import worker from "./workers/acme-api/index.js";
import * as handlers from "./workers/acme-api/handlers.js";
import { makeTestState } from "./__bynk_runtime/storage.js";
import { serialiseAgentClass } from "./__bynk_runtime/agent.js";

function assert(cond: boolean, msg: string): void {
  if (!cond) throw new Error(`assertion failed: ${msg}`);
}

type Fetcher = { fetch(input: string, init?: unknown): Promise<Response> };
// #3939 (slice 3): every storage write the key store makes, so a refusal can be shown to write none.
let storeWrites = 0;
function spied(state: { storage: Record<string, unknown> }): unknown {
  const storage = state.storage as Record<string, (...a: unknown[]) => unknown>;
  for (const m of ["put", "delete", "deleteAll", "transaction"]) {
    const f = storage[m];
    if (typeof f !== "function") continue;
    storage[m] = (...a: unknown[]) => {
      storeWrites += 1;
      return f.apply(storage, a);
    };
  }
  return state;
}
// An in-process Durable Object namespace over the emitted agent class, as the test tier builds one.
function namespace(cls: unknown, env: unknown, spy = false): { idFromName(n: string): unknown; get(id: unknown): Fetcher } {
  const instances = new Map<string, { fetch(r: Request): Promise<Response> }>();
  return {
    idFromName: (n: string) => n,
    get(id: unknown): Fetcher {
      const k = String(id);
      let inst = instances.get(k);
      if (inst === undefined) {
        const C = serialiseAgentClass(cls as never, {}) as unknown as new (s: unknown, e: unknown) => { fetch(r: Request): Promise<Response> };
        inst = new C(spy ? spied(makeTestState(k) as never) : makeTestState(k), env);
        instances.set(k, inst);
      }
      const target = inst;
      return { fetch: (input: string, init?: unknown) => target.fetch(new Request(input, init as RequestInit)) };
    },
  };
}

const PEPPER = "Ynluay10ZXN0LWFwaS1rZXktcGVwcGVyLTAxMjM0NTY";
const env: Record<string, unknown> = {
  CLIENT_KEY_PEPPER: PEPPER,
  RATELIMIT_PER_IP: { limit: async () => ({ success: true }) },
};
env.CLIENT_KEYS = namespace(handlers.ClientKeys, env, true);
env.CLIENT_KEY_OWNER = namespace(handlers.ClientKeyOwner, env);
env.ACCOUNTS = namespace(handlers.Accounts, env);

async function req(method: string, path: string, headers: Record<string, string>, body?: unknown, e = env): Promise<Response> {
  const init: RequestInit = { method, headers: { "content-type": "application/json", ...headers } };
  if (body !== undefined) init.body = JSON.stringify(body);
  return worker.fetch(new Request(`https://api.example${path}`, init), e as never);
}
const bearer = (k: string) => ({ authorization: `Bearer ${k}` });

// §9 mint: the response carries the key itself, once, never `<secret>`.
const minted = await req("POST", "/accounts/acct-7/keys", {}, { label: "ci" });
assert(minted.status === 201, `mint answers 201, got ${minted.status}`);
const secret = (await minted.json()) as { id: string; key: string; label: string; created: number; expires: unknown };
assert(typeof secret.key === "string" && secret.key.startsWith("acme_") && secret.key.length === 88, `the minted key is on the wire: ${secret.key}`);
assert(secret.key.startsWith(secret.id + "_"), "the id names the key");
assert(secret.label === "ci", "the label comes back");

// §8 steps 1–5: the key is admitted and mints its identity.
const me = await req("GET", "/me", bearer(secret.key));
assert(me.status === 200 && (await me.json()) === "acct-7", "a minted key is admitted as its identity");

// D3 B: every refusal from parsing on is one identical 401.
async function refusal(headers: Record<string, string>): Promise<string> {
  const r = await req("GET", "/me", headers);
  const hs = [...r.headers.entries()].sort().map(([k, v]) => `${k}: ${v}`).join("\n");
  return `${r.status}\n${hs}\n\n${await r.text()}`;
}
const flip = (k: string) => k.slice(0, -1) + (k.at(-1) === "0" ? "1" : "0");
// Recompute the checksum of a hand-built key the same way the runtime does, so it parses.
const { formatApiKey, parseApiKey } = await import("./__bynk_runtime/apikey.js");
const real = parseApiKey(secret.key, "acme")!;
const forgedKey = formatApiKey("acme", real.handle, real.keyNo, new Uint8Array(32).fill(3));
const unknownKey = formatApiKey("acme", real.handle, "00000000000000aa", new Uint8Array(32).fill(4));
const unknownHandle = formatApiKey("acme", "000000000000000000000000000000bb", real.keyNo, new Uint8Array(32).fill(5));
// D3 B / §13 timing, by raw request: the store compares once, against the dummy digest, for an
// unknown key number and for an unknown handle; once, against the record, for a forged secret; and
// a key refused at parsing never reaches the store at all.
const { apiKeyCountCompares } = await import("./__bynk_runtime/apikey.js");
const compares = apiKeyCountCompares();
let storeCalls = 0;
const counted = {
  ...env,
  CLIENT_KEYS: {
    idFromName: (n: string) => n,
    get: (id: unknown) => {
      storeCalls += 1;
      return (env.CLIENT_KEYS as ReturnType<typeof namespace>).get(id);
    },
  },
};
async function work(key: string): Promise<{ status: number; all: number; dummy: number; calls: number }> {
  const before = { ...compares };
  const calls = storeCalls;
  const r = await req("GET", "/me", bearer(key), undefined, counted);
  return { status: r.status, all: compares.all - before.all, dummy: compares.dummy - before.dummy, calls: storeCalls - calls };
}
const same = (w: { status: number; all: number; dummy: number; calls: number }, want: [number, number, number, number], what: string) =>
  assert(w.status === want[0] && w.all === want[1] && w.dummy === want[2] && w.calls === want[3], `${what}: ${JSON.stringify(w)}`);
same(await work(unknownKey), [401, 1, 1, 1], "an unknown key number is one dummy compare in one store call");
same(await work(unknownHandle), [401, 1, 1, 1], "an unknown handle is one dummy compare in one store call");
same(await work(forgedKey), [401, 1, 0, 1], "a forged secret is one record compare");
same(await work(flip(secret.key)), [401, 0, 0, 0], "a malformed key makes no store call");
same(await work(secret.key.replace(/^acme_/, "beta_")), [401, 0, 0, 0], "another prefix makes no store call");
same(await work(secret.key + "x".repeat(60)), [401, 0, 0, 0], "an oversized key makes no store call");
same(await work(secret.key), [200, 1, 0, 1], "the real key is one record compare");

const want = await refusal(bearer(flip(secret.key)));
assert(want.startsWith("401\n"), `a malformed key is a 401:\n${want}`);
assert(want.includes('www-authenticate: Bearer realm="acme.api"'), `the 401 carries the Bearer challenge:\n${want}`);
for (const [what, headers] of [
  ["an unknown key number", bearer(unknownKey)],
  ["an unknown handle", bearer(unknownHandle)],
  ["a forged secret", bearer(forgedKey)],
  ["another prefix", bearer(secret.key.replace(/^acme_/, "beta_"))],
  ["a longer-than-128 key", bearer(secret.key + "x".repeat(60))],
  ["a repeated header", { authorization: `Bearer ${secret.key}, Bearer ${secret.key}` }],
] as [string, Record<string, string>][]) {
  assert((await refusal(headers)) === want, `${what} is the same 401 as a malformed key`);
}

// §8 step 1: no key is a 401 whose challenge names no error (RFC 6750 §3.1).
const none = await req("GET", "/me", {});
assert(none.status === 401 && none.headers.get("www-authenticate") === 'Bearer realm="acme.api"', "no key is a bare challenge");
const unschemed = await req("GET", "/me", { authorization: secret.key });
assert(unschemed.status === 401 && unschemed.headers.get("www-authenticate") === 'Bearer realm="acme.api"', "a key without `Bearer` is no bearer credential");

// D3 C: a store fault never admits and is never a 401.
const quiet = console.error;
console.error = () => {};
const down = { ...env, CLIENT_KEYS: { idFromName: (n: string) => n, get: () => ({ fetch: async () => { throw new Error("unreachable"); } }) } };
const fault = await req("GET", "/me", bearer(secret.key), undefined, down);
assert(fault.status === 500, `an unreachable key store faults the request (500), got ${fault.status}`);
const busy = {
  ...env,
  CLIENT_KEYS: {
    idFromName: (n: string) => n,
    get: () => ({ fetch: async () => new Response("AgentBusy: agent ClientKeys busy: waited 30 s behind 1 queued call", { status: 503, headers: { "x-bynk-agent-busy": "ClientKeys" } }) }),
  },
};
const queued = await req("GET", "/me", bearer(secret.key), undefined, busy);
assert(queued.status === 503, `AgentBusy answers 503, got ${queued.status}`);
assert(queued.headers.get("retry-after") === "1", "AgentBusy answers Retry-After: 1");
console.error = quiet;

// D2 B: the pepper never rides the key store's request body.
const bodies: string[] = [];
const spying = {
  ...env,
  CLIENT_KEYS: {
    idFromName: (n: string) => n,
    get: (id: unknown) => {
      const inner = (env.CLIENT_KEYS as ReturnType<typeof namespace>).get(id);
      return { fetch: async (input: string, init?: { body?: string }) => { bodies.push(init?.body ?? ""); return inner.fetch(input, init); } };
    },
  },
};
assert((await req("GET", "/me", bearer(secret.key), undefined, spying)).status === 200, "the spied store still admits");
assert(bodies.length === 1, "one store call per request (D3 D, nothing cached; a key used within the hour is not touched)");
assert(!bodies[0].includes(PEPPER) && !bodies[0].includes(secret.key.slice(-49)), "neither the pepper nor the secret reaches the store");

// #3939 (slice 2, §9) / §13 revocation: a revoked key is refused on the next request, as the same
// 401 after the same one dummy compare as a key never minted.
const second = (await (await req("POST", "/accounts/acct-7/keys", {}, { label: "second" })).json()) as { id: string; key: string };
assert((await req("GET", "/me", bearer(second.key))).status === 200, "a second key is admitted");
// `revoke` answers `false` the same way whatever the reason, so a caller cannot probe which keys
// exist: an id that is no key id of this actor (another prefix), another identity's key, a key
// number the store never held, and (below) a store that was retired.
async function answer(method: string, path: string): Promise<string> {
  const r = await req(method, path, {});
  const hs = [...r.headers.entries()].sort().map(([k, v]) => `${k}: ${v}`).join("\n");
  return `${r.status}\n${hs}\n\n${await r.text()}`;
}
const { apiKeyIdOf } = await import("./__bynk_runtime/apikey.js");
const notFound = await answer("DELETE", `/accounts/acct-8/keys/${second.id}`);
assert(notFound.startsWith("404\n"), `revoking another identity's key answers false (404):\n${notFound}`);
const otherPrefix = "beta" + second.id.slice("acme".length);
assert((await answer("DELETE", `/accounts/acct-7/keys/${otherPrefix}`)) === notFound, "an id of another prefix is the same false as another identity's key");
const neverHeld = apiKeyIdOf("acme", real.handle, "00000000000000aa");
assert((await answer("DELETE", `/accounts/acct-7/keys/${neverHeld}`)) === notFound, "a key number never held is the same false as another identity's key");
assert((await req("GET", "/me", bearer(second.key))).status === 200, "another identity's revoke leaves the key");
assert((await req("DELETE", `/accounts/acct-7/keys/${second.id}`, {})).status === 204, "the owner's revoke answers true (204)");
same(await work(second.key), [401, 1, 1, 1], "a revoked key is one dummy compare in one store call");
assert((await refusal(bearer(second.key))) === want, "a revoked key is the same 401 as a malformed key");
assert((await req("GET", "/me", bearer(secret.key))).status === 200, "revoking one key leaves the others");

// §9 list: ids, labels, created, expires and last use; never a secret or a digest.
const listed = (await (await req("GET", "/accounts/acct-7/keys", {})).json()) as Record<string, unknown>[];
assert(listed.length === 1 && listed[0].id === secret.id && listed[0].label === "ci", `the one key left is listed: ${JSON.stringify(listed)}`);
assert(JSON.stringify(Object.keys(listed[0]).sort()) === JSON.stringify(["created", "expires", "id", "label", "lastUsed", "pepper"]), `a listed key has no other field: ${JSON.stringify(listed[0])}`);
assert(!JSON.stringify(listed).includes(secret.key.slice(-49)), "the list never carries the secret");

// §6: `revokeAll`'s first commit empties and retires the store; if its second commit (the owner's
// new handle) fails, the owner still names the retired store, and the next mint rotates first.
// The failure is simulated by clearing the store directly, without the owner's commit.
const clear = await (env.CLIENT_KEYS as ReturnType<typeof namespace>).get(real.handle).fetch("https://agent/_bynk/agent/clear", { method: "POST", body: JSON.stringify({ args: [], deps: {}, key: real.handle }) });
assert(clear.status === 200 && (await clear.json()) === 1, "the store's own clear empties its one key");
assert((await req("GET", "/me", bearer(secret.key))).status === 401, "a key of a cleared store is refused");
assert((await answer("DELETE", `/accounts/acct-7/keys/${secret.id}`)) === notFound, "a revoke against a retired store is the same false as another identity's key");
const rotated = (await (await req("POST", "/accounts/acct-7/keys", {}, { label: "after" })).json()) as { key: string };
const after = parseApiKey(rotated.key, "acme")!;
assert(after.handle !== real.handle, "a mint after a failed second commit takes a fresh handle");
assert((await req("GET", "/me", bearer(rotated.key))).status === 200, "the rotated key is admitted");

// §6 revokeAll: one commit empties the keys, then the handle is replaced, so an old id is refused
// and a later key lives under a new handle.
const closed = await req("POST", "/accounts/acct-7/close", {});
assert(closed.status === 200 && (await closed.json()) === 1, "revokeAll answers the number revoked");
assert((await req("GET", "/me", bearer(rotated.key))).status === 401, "an old key is refused after revokeAll");
const fresh = (await (await req("POST", "/accounts/acct-7/keys", {}, { label: "fresh" })).json()) as { key: string };
const freshHandle = parseApiKey(fresh.key, "acme")!.handle;
assert(freshHandle !== after.handle && freshHandle !== real.handle, "revokeAll rotates the handle");
// The new handle is a fresh random one (the emitted call passes `__newApiKeyHandle()`, pinned by the
// Rust side), never derived from the identity: 16 random bytes as hex, unlike every earlier one.
assert(/^[0-9a-f]{32}$/.test(freshHandle), `the new handle is 16 bytes of hex: ${freshHandle}`);
const identityHex = [...new TextEncoder().encode("acct-7")].map((b) => b.toString(16).padStart(2, "0")).join("");
assert(!freshHandle.includes(identityHex), "the new handle does not encode the identity");
assert((await req("GET", "/me", bearer(fresh.key))).status === 200, "a key minted after revokeAll is admitted");

// #3939 (slice 3) / §13: no refusal writes to the key store. `verify` only reads, so a malformed,
// unknown, forged or expired key leaves the store's storage untouched.
const unwritten = async (key: string, what: string, e: Record<string, unknown> = env) => {
  const before = storeWrites;
  const r = await req("GET", "/me", bearer(key), undefined, e);
  assert(r.status === 401, `${what} is refused, got ${r.status}`);
  assert(storeWrites === before, `${what} wrote to the key store (${storeWrites - before} writes)`);
};
const fresh7 = (await (await req("POST", "/accounts/acct-60/keys", {}, { label: "w" })).json()) as { key: string };
const parsed7 = parseApiKey(fresh7.key, "acme")!;
await unwritten(flip(fresh7.key), "a malformed key");
await unwritten(formatApiKey("acme", parsed7.handle, "00000000000000cc", new Uint8Array(32).fill(6)), "an unknown key number");
await unwritten(formatApiKey("acme", "000000000000000000000000000000dd", parsed7.keyNo, new Uint8Array(32).fill(7)), "an unknown handle");
await unwritten(formatApiKey("acme", parsed7.handle, parsed7.keyNo, new Uint8Array(32).fill(8)), "a forged secret");
// A key past its `ttl` (90 days) under a clock 91 days on.
const later = { ...env, __bynkClock: { now: async () => Date.now() + 91 * 86_400_000 } };
await unwritten(fresh7.key, "an expired key", later);
// §16 Q7: an admitted key is touched once (its last use), and not again within the hour.
let before = storeWrites;
assert((await req("GET", "/me", bearer(fresh7.key))).status === 200, "the fresh key is admitted");
assert(storeWrites > before, "the first admission records last use");
before = storeWrites;
assert((await req("GET", "/me", bearer(fresh7.key))).status === 200, "the fresh key is admitted again");
assert(storeWrites === before, "a second admission within the hour writes nothing");

// #3939 (slice 3, §7) and live check K2's steps, in process: rotate the pepper with `previous`.
const { newApiKeyPepper } = await import("./__bynk_runtime/apikey.js");
const P2 = newApiKeyPepper();
const used = (await (await req("POST", "/accounts/acct-61/keys", {}, { label: "used" })).json()) as { id: string; key: string };
const idle = (await (await req("POST", "/accounts/acct-62/keys", {}, { label: "idle" })).json()) as { id: string; key: string };
const ref401 = await refusal(bearer(flip(used.key)));
const rotating = { ...env, CLIENT_KEY_PEPPER: P2, CLIENT_KEY_PEPPER_PREV: PEPPER };
type Info = { id: string; pepper: { kind: string } };
const pepperOf = async (id: string, e: Record<string, unknown>) =>
  ((await (await req("GET", `/accounts/${id}/keys`, {}, undefined, e)).json()) as Info[]).map((k) => k.pepper.kind);
assert(JSON.stringify(await pepperOf("acct-61", rotating)) === '["Previous"]', "before its next use, a key under the old pepper is listed as Previous");
assert((await req("GET", "/me", bearer(used.key), undefined, rotating)).status === 200, "a key minted before the rotation is admitted under previous");
assert(JSON.stringify(await pepperOf("acct-61", rotating)) === '["Current"]', "the admitted key was re-hashed under the new pepper in the same commit");
const swept = await req("POST", "/accounts/acct-62/sweep", {}, undefined, rotating);
assert(swept.status === 200 && (await swept.json()) === 1, "sweep counts the idle key still under previous");
const minted2 = (await (await req("POST", "/accounts/acct-61/keys", {}, { label: "after" }, rotating)).json()) as { key: string };
assert((await req("GET", "/me", bearer(minted2.key), undefined, rotating)).status === 200, "a key minted during the rotation is admitted");
// Drop `previous`.
const dropped = { ...env, CLIENT_KEY_PEPPER: P2 };
const droppedCounted = { ...counted, CLIENT_KEY_PEPPER: P2 };
assert((await req("GET", "/me", bearer(used.key), undefined, dropped)).status === 200, "the re-hashed key outlives previous");
assert((await req("GET", "/me", bearer(minted2.key), undefined, dropped)).status === 200, "a key minted under the new pepper is admitted");
// A key under neither pepper: one dummy compare, no write, and the ordinary 401, byte for byte.
{
  const b = { ...compares };
  const w = storeWrites;
  const r = await req("GET", "/me", bearer(idle.key), undefined, droppedCounted);
  assert(r.status === 401, `a key under neither pepper is refused, got ${r.status}`);
  assert(compares.all - b.all === 1 && compares.dummy - b.dummy === 1, "a key under neither pepper is one dummy compare");
  assert(storeWrites === w, "a key under neither pepper writes nothing");
}
const refusalIn = async (headers: Record<string, string>, e: Record<string, unknown>) => {
  const r = await req("GET", "/me", headers, undefined, e);
  const hs = [...r.headers.entries()].sort().map(([k, v]) => `${k}: ${v}`).join("\n");
  return `${r.status}\n${hs}\n\n${await r.text()}`;
};
const idleRef = await refusalIn(bearer(idle.key), dropped);
const parsedUsed = parseApiKey(used.key, "acme")!;
const forgedRef = await refusalIn(bearer(formatApiKey("acme", parsedUsed.handle, parsedUsed.keyNo, new Uint8Array(32).fill(9))), dropped);
assert(idleRef === forgedRef, `a key under neither pepper is the same 401 as a forged key:\n${idleRef}\n---\n${forgedRef}`);
assert(idleRef === ref401, "a key under neither pepper is the same 401 as a malformed key");
assert(JSON.stringify(await pepperOf("acct-62", dropped)) === '["Retired"]', "a key under neither pepper is listed as Retired");
const pruned = await req("POST", "/accounts/acct-62/sweep", {}, undefined, dropped);
assert(pruned.status === 200 && (await pruned.json()) === 0, "sweep removes the retired key, leaving none under previous");
assert(JSON.stringify(await pepperOf("acct-62", dropped)) === "[]", "the retired key is gone");

// A missing pepper faults the request; it never admits or refuses quietly.
console.error = () => {};
const noPepper = await req("GET", "/me", bearer(fresh.key), undefined, { ...env, CLIENT_KEY_PEPPER: undefined });
console.error = quiet;
assert(noPepper.status === 500, `a missing pepper faults the request, got ${noPepper.status}`);

// CORS: the preflight allows the key's header.
const pre = await worker.fetch(new Request("https://api.example/me", { method: "OPTIONS", headers: { origin: "https://console.example.com", "access-control-request-method": "GET", "access-control-request-headers": "authorization" } }), env as never);
assert((pre.headers.get("access-control-allow-headers") ?? "").toLowerCase().includes("authorization"), "the preflight allows authorization");

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

/// Compile `fixture` (a Workers fixture) and run `driver` against its emitted Worker under Node.
fn drive(fixture_name: &str, driver: &str) {
    let runner = match discover_tsc() {
        Some(r) => r,
        None => {
            eprintln!("\n!!! API KEY SEAM VERIFICATION SKIPPED !!!\nno tsc runner on PATH.\n");
            if std::env::var(REQUIRE_ENV).is_ok() {
                panic!("{REQUIRE_ENV} is set but no tsc runner was found");
            }
            return;
        }
    };
    if !tool_exists("node") {
        eprintln!("\n!!! API KEY SEAM VERIFICATION SKIPPED !!!\n`node` is not on PATH.\n");
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("{REQUIRE_ENV} is set but `node` was not found");
        }
        return;
    }

    // Compile the API key Workers fixture in-process.
    let root: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/positive")
        .join(fixture_name);
    let paths =
        bynk_driver::project::try_read_project_paths(&root).expect("well-formed fixture manifest");
    let out = bynk_driver::project::compile_project(
        &bynk_testkit::compile_options_split(root, paths)
            .target(bynk_check::project_model::BuildTarget::Workers),
    )
    .map_err(bynk_driver::project::ProjectFailure::flatten)
    .expect("the API key Workers fixture must compile");

    let tmp = std::env::temp_dir().join(format!(
        "bynk-api-key-seam-{fixture_name}-{}",
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
    assert!(
        ok,
        "the API key seam driver must type-check + compile:\n{log}"
    );
    // D2 B: the pepper never reaches agent code. The generated key-store agents, as emitted, name no
    // pepper variable, no `deps.__apiKey`, and read no environment secret.
    let handlers = fs::read_to_string(tmp.join("workers/acme-api/handlers.ts")).unwrap();
    for agent in ["export class ClientKeys ", "export class ClientKeyOwner "] {
        let start = handlers
            .find(agent)
            .unwrap_or_else(|| panic!("`{agent}` is emitted"));
        // The class ends at its first closing brace in column 0.
        let end = handlers[start + 1..]
            .find("\n}\n")
            .map_or(handlers.len(), |i| start + 1 + i);
        let class = &handlers[start..end];
        for forbidden in ["CLIENT_KEY_PEPPER", "deps.__apiKey", "Pepper", "pepper"] {
            assert!(
                !class.contains(forbidden),
                "the emitted `{agent}` mentions `{forbidden}`"
            );
        }
    }
    // #3939 (slice 2, §6): `revokeAll`'s new handle comes from the runtime's random handle generator,
    // the one `mint` uses, at every call site; it is never derived from the identity.
    let calls: Vec<&str> = handlers
        .match_indices(".revokeAll(")
        .map(|(i, _)| &handlers[i..])
        .collect();
    assert!(
        !calls.is_empty(),
        "`revokeAll` is called in the emitted handlers"
    );
    for call in calls {
        assert!(
            call.starts_with(".revokeAll(__newApiKeyHandle(), "),
            "every `revokeAll` call takes a fresh random handle: {}",
            &call[..call.len().min(60)]
        );
    }
    let (ran, log) = run("node", &[], &["js/driver.js"], &tmp);
    assert!(
        ran && log.contains("ALL OK"),
        "the API key seam driver must run green:\n{log}"
    );

    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn the_api_key_seam_refuses_alike_and_faults_closed() {
    drive("3939_api_key", DRIVER_TS);
}

/// Copy `from` into `to`, recursively.
fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// #3939 (track §10, Decision C): `bynk test --against` refuses, before any request, every case
/// that needs the in-process key store: acting as the `ApiKey` actor, `by Malformed`, `by Unknown`,
/// and `by Forged` or `by Revoked` at an `ApiKey` route (its key is built from one minted, and
/// for `Revoked` revoked, in process, and its compare count is taken in process). `--skip-unsupported` skips them with that reason.
#[test]
fn a_remote_run_refuses_every_case_that_needs_the_in_process_key_store() {
    let root: PathBuf =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/positive/3939_api_key");
    let dir = std::env::temp_dir().join(format!("bynk-api-key-remote-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let proj = dir.join("proj");
    copy_tree(&root, &proj);
    let _ = fs::remove_dir_all(proj.join("expected"));
    let manifest = fs::read_to_string(proj.join("bynk.toml")).unwrap();
    // A target nothing listens on: the refusal is a compile error, before any request.
    fs::write(
        proj.join("bynk.toml"),
        format!("{manifest}\n[test.targets.local]\nurl.acme-api = \"http://127.0.0.1:9\"\n"),
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
        .arg("test")
        .arg(&proj)
        .arg("--output")
        .arg(dir.join("out"))
        .args(["--format", "json", "--against", "local"])
        .output()
        .expect("run bynk test");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!out.status.success(), "the run is refused:\n{stdout}");
    let doc: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "not JSON ({e}):\n{stdout}\n{}",
            String::from_utf8_lossy(&out.stderr)
        )
    });
    assert_eq!(doc["error"]["kind"], "compile", "{doc:#}");
    let diagnostics = doc["error"]["diagnostics"].to_string();
    for want in [
        "acts as the `ApiKey` actor `Client`",
        "drives `by Malformed`",
        "drives `by Unknown`",
        "drives `by Forged` at a route an `ApiKey` actor guards",
        "drives `by Revoked` at a route an `ApiKey` actor guards",
    ] {
        assert!(
            diagnostics.contains("bynk.test.remote_unsupported_api_key")
                && diagnostics.contains(want),
            "`{want}` is refused: {doc:#}"
        );
    }
    // `by Nobody` sends no key, so the deployed seam can refuse it: it is not refused.
    assert!(!diagnostics.contains("no key is refused"), "{doc:#}");
    let _ = fs::remove_dir_all(&dir);
}

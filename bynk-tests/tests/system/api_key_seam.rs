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
//! * the CORS preflight allows the key's header.
//!
//! A `system`-tier suite cannot make the key store fail or queue, so those live here. Like the other
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
// An in-process Durable Object namespace over the emitted agent class, as the test tier builds one.
function namespace(cls: unknown, env: unknown): { idFromName(n: string): unknown; get(id: unknown): Fetcher } {
  const instances = new Map<string, { fetch(r: Request): Promise<Response> }>();
  return {
    idFromName: (n: string) => n,
    get(id: unknown): Fetcher {
      const k = String(id);
      let inst = instances.get(k);
      if (inst === undefined) {
        const C = serialiseAgentClass(cls as never, {}) as unknown as new (s: unknown, e: unknown) => { fetch(r: Request): Promise<Response> };
        inst = new C(makeTestState(k), env);
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
env.CLIENT_KEYS = namespace(handlers.ClientKeys, env);
env.CLIENT_KEY_OWNER = namespace(handlers.ClientKeyOwner, env);

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
assert(bodies.length === 1, "one store call per request (D3 D, nothing cached)");
assert(!bodies[0].includes(PEPPER) && !bodies[0].includes(secret.key.slice(-49)), "neither the pepper nor the secret reaches the store");

// A missing pepper faults the request; it never admits or refuses quietly.
console.error = () => {};
const noPepper = await req("GET", "/me", bearer(secret.key), undefined, { ...env, CLIENT_KEY_PEPPER: undefined });
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
        let end = handlers[start + 1..]
            .find("\nexport class ")
            .map_or(handlers.len(), |i| start + 1 + i);
        let class = &handlers[start..end];
        for forbidden in ["CLIENT_KEY_PEPPER", "deps.__apiKey", "Pepper", "pepper"] {
            assert!(
                !class.contains(forbidden),
                "the emitted `{agent}` mentions `{forbidden}`"
            );
        }
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

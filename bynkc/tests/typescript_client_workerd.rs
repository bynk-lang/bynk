//! I4 (#1970): a hand-written TypeScript Worker calls a Bynk context through
//! the generated client (`bynk-client.ts`) and `@bynk/wire-client`, under a
//! real `workerd`.
//!
//! `typescript_client.rs` proves the client type-checks in a caller's build and
//! that skew is a `tsc` error there. Only a real call proves the rest: that the
//! client's request, signed as a declared foreign caller under the secret
//! `bynk deploy` hands over, is one the Bynk Worker's dispatch admits, that the
//! arguments encode and the result decodes, and that each refusal reaches the
//! caller as the error the reference describes. Two `wrangler dev`s, one per
//! Worker, joined by a Service Binding through wrangler's dev registry:
//!
//! - `Legacy` calls `place` (a record in, a `Result` with a payload-carrying
//!   error out) and `ping`;
//! - `Ledger` calls `place`, and its handler tells the two callers apart;
//! - `Ledger` calling `ping`, which admits only `Legacy`, is refused (`403`),
//!   as is a call under the wrong secret (`401`) and one stamped with a stale
//!   contract hash (`409`, read as `ContractMismatch`);
//! - a refinement the callee checks (`qty: Int where NonNegative`) comes back
//!   as a `400`.
//!
//! Both Workers' names carry the test's pid, so a stale instance in the
//! machine-wide dev registry (I2 met one) is never the one a binding reaches.
//! The same toolchain skip and `BYNK_REQUIRE_WORKERD` gate as the other
//! workerd smokes.

mod require;
mod tools;
mod wrangler;

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const REQUIRE_ENV: &str = "BYNK_REQUIRE_WORKERD";

fn skip(reason: &str) -> bool {
    if require::is_required(REQUIRE_ENV) {
        panic!("{REQUIRE_ENV} is set but {reason}");
    }
    eprintln!("\n!!! TYPESCRIPT CLIENT (WORKERD) SKIPPED !!!\n{reason}\n");
    true
}

/// The Bynk context, named per run (`interop.ledger<pid>`).
fn bynk_source(context: &str) -> String {
    format!(
        r#"context {context}

actor Legacy {{ auth = Foreign(secret = "LEGACY_CALL_KEY") }}
actor Ledger {{ auth = Foreign(secret = "LEDGER_CALL_KEY") }}

type Order = {{ id: String, qty: Int where NonNegative }}

type OrderError =
  | Unknown
  | TooMany(limit: Int)

service place {{
  on call(order: Order) -> Effect[Result[String, OrderError]] by who: Caller | Legacy | Ledger {{
    if order.qty > 10 {{
      Err(TooMany(10))
    }} else {{
      match who {{
        Caller(name) => Ok("\(order.id) for \(name)")
        Legacy => Ok("\(order.id) for legacy")
        Ledger => Ok("\(order.id) for the ledger")
      }}
    }}
  }}
}}

service ping {{
  on call(n: Int) -> Effect[Int] by Legacy {{
    n + 1
  }}
}}
"#
    )
}

/// The TypeScript Worker. Each route makes one call through the client and
/// answers with what came back, or the call error's kind and status.
const TS_WORKER: &str = r#"import { connect, CONTEXT, SECRET, type Env, type Services } from "./bynk-client.js";
import { __callService, callErrorOf, Ok, type __ServiceBinding } from "@bynk/wire-client";

interface WorkerEnv extends Env {
  readonly LEDGER: __ServiceBinding;
}

function failure(e: unknown): string {
  const err = callErrorOf(e);
  if (err === undefined) return `other:${String(e)}`;
  return err.kind === "Transport" ? `Transport:${err.status}` : err.kind;
}

async function route(path: string, env: WorkerEnv): Promise<string> {
  const legacy = connect({ caller: "Legacy", binding: env.LEDGER, env });
  const ledger = connect({ caller: "Ledger", binding: env.LEDGER, env });
  switch (path) {
    case "/legacy/place":
      return JSON.stringify(await legacy.place({ id: "o-1", qty: 2 }));
    case "/legacy/too-many":
      return JSON.stringify(await legacy.place({ id: "o-2", qty: 11 }));
    case "/legacy/ping":
      return String(await legacy.ping(41));
    case "/ledger/place":
      return JSON.stringify(await ledger.place({ id: "o-3", qty: 1 }));
    case "/ledger/ping":
      // Not on `Ledger`'s client type; reached around it, the callee refuses.
      return String(await (ledger as unknown as Services).ping(1));
    case "/legacy/negative":
      return JSON.stringify(await legacy.place({ id: "o-4", qty: -1 }));
    case "/wrong-secret": {
      const wrong = { [SECRET]: "not-the-secret" } as unknown as Env;
      return String(await connect({ caller: "Legacy", binding: env.LEDGER, env: wrong }).ping(1));
    }
    case "/stale-contract":
      return String(await __callService(env.LEDGER, "ping", 1, (j) => Ok(j), "Legacy", "0000000000000000", env, CONTEXT));
    default:
      return "unknown route";
  }
}

export default {
  async fetch(request: Request, env: WorkerEnv): Promise<Response> {
    const path = new URL(request.url).pathname;
    try {
      return new Response(await route(path, env));
    } catch (e) {
      return new Response(failure(e));
    }
  },
};
"#;

/// `GET path` on `port`: the body.
fn get(port: u16, path: &str) -> Result<String, String> {
    let addr = format!("127.0.0.1:{port}");
    let mut stream = std::net::TcpStream::connect_timeout(
        &addr.parse().map_err(|e| format!("{e}"))?,
        Duration::from_secs(2),
    )
    .map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .map_err(|e| e.to_string())?;
    let request = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .map_err(|e| e.to_string())?;
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|e| e.to_string())?;
    let (head, body) = response
        .split_once("\r\n\r\n")
        .ok_or_else(|| format!("unparseable response: {response:?}"))?;
    // `wrangler dev` answers chunked; the bodies here are one short chunk.
    if head
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        let mut lines = body.split("\r\n");
        let _size = lines.next();
        return Ok(lines.next().unwrap_or_default().to_string());
    }
    Ok(body.to_string())
}

fn read_log(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| format!("<could not read {path:?}: {e}>"))
}

struct Wranglers(Vec<wrangler::Wrangler>);

impl Drop for Wranglers {
    fn drop(&mut self) {
        wrangler::stop_all(&mut self.0);
    }
}

fn compile(proj: &Path, out: &Path, emit: &str) {
    let compiled = Command::new(env!("CARGO_BIN_EXE_bynkc"))
        .args(["compile", "--target", "workers", "--emit", emit, "-o"])
        .arg(out)
        .arg(proj)
        .output()
        .expect("run bynkc");
    assert!(
        compiled.status.success(),
        "the client smoke does not compile (--emit {emit}):\n{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
}

#[test]
fn a_typescript_worker_calls_a_bynk_context_through_the_generated_client() {
    if !tools::tool_exists("npx") && skip("`npx` is not on PATH") {
        return;
    }
    if !tools::tool_exists("node") && skip("`node` is not on PATH") {
        return;
    }

    let pid = std::process::id();
    let context = format!("interop.ledger{pid}");
    let bynk_worker = format!("interop-ledger{pid}");
    let ts_worker = format!("legacy-billing{pid}");
    let secret = bynk_check::wire_format::edge_key_to(&context).unwrap();

    let tmp: PathBuf = std::env::temp_dir().join(format!("bynk-ts-client-workerd-{pid}"));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(tmp.join("proj/src/interop")).unwrap();
    fs::write(
        tmp.join("proj/bynk.toml"),
        "[project]\nname = \"ts-client-smoke\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(
        tmp.join(format!("proj/src/interop/ledger{pid}.bynk")),
        bynk_source(&context),
    )
    .unwrap();
    // The Bynk Worker as `bynk deploy` ships it, and the client as a TypeScript
    // build gives it to the foreign Worker's repository.
    compile(&tmp.join("proj"), &tmp.join("out"), "js");
    compile(&tmp.join("proj"), &tmp.join("out-ts"), "ts");
    let bynk_dir = tmp.join("out/workers").join(&bynk_worker);
    assert!(
        bynk_dir.join("index.js").is_file(),
        "expected out/workers/{bynk_worker} — context dasherisation changed?"
    );
    let client = tmp
        .join("out-ts/workers")
        .join(&bynk_worker)
        .join(bynk_emit::emitter::client::CLIENT_FILE);

    // The foreign Worker's repository: its own code, the generated client, the
    // package under `node_modules`, and a binding to the Bynk Worker.
    let ts_dir = tmp.join("legacy");
    let pkg = ts_dir.join("node_modules/@bynk/wire-client");
    fs::create_dir_all(&pkg).unwrap();
    let package_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../bynk-emit/wire-client");
    for f in ["package.json", "index.ts"] {
        fs::copy(package_src.join(f), pkg.join(f)).unwrap();
    }
    fs::copy(
        &client,
        ts_dir.join(bynk_emit::emitter::client::CLIENT_FILE),
    )
    .unwrap();
    fs::write(ts_dir.join("worker.ts"), TS_WORKER).unwrap();
    fs::write(
        ts_dir.join("wrangler.toml"),
        format!(
            "name = \"{ts_worker}\"\nmain = \"worker.ts\"\ncompatibility_date = \"2026-07-01\"\n\n[[services]]\nbinding = \"LEDGER\"\nservice = \"{bynk_worker}\"\n"
        ),
    )
    .unwrap();

    // Its own band, clear of the other smokes' (20_000 + 2·pid, 30_000 + 4·pid,
    // 40_000 + 2·pid and 55_000 + 2·pid, each pid mod 4_000).
    let base = 45_000 + 4 * (pid % 2_000) as u16;
    let (bynk_port, ts_port) = (base, base + 1);
    let spawn = |dir: &Path,
                 port: u16,
                 inspector: u16,
                 vars: &[String],
                 log: &Path|
     -> std::io::Result<wrangler::Wrangler> {
        let out_log = fs::File::create(log)?;
        let err_log = out_log.try_clone()?;
        wrangler::Wrangler::spawn(
            tools::base_command("npx")
                .args(["-y", wrangler::SPEC, "dev"])
                .args(vars)
                .args([
                    "--port",
                    &port.to_string(),
                    "--inspector-port",
                    &inspector.to_string(),
                ])
                .current_dir(dir)
                .stdout(Stdio::from(out_log))
                .stderr(Stdio::from(err_log)),
            &tmp.join("registry"),
        )
    };
    // Each foreign caller's secret on the Bynk Worker, as `bynk dev` passes
    // it, and the same value on the foreign Worker under the name `bynk
    // deploy`'s hand-off sets.
    let key = bynk_emit::LOCAL_CALL_KEY;
    let bynk_vars: Vec<String> = ["LEGACY_CALL_KEY", "LEDGER_CALL_KEY"]
        .iter()
        .flat_map(|s| ["--var".to_string(), format!("{s}:{key}")])
        .collect();
    let ts_vars = vec!["--var".to_string(), format!("{secret}:{key}")];
    let bynk_log = tmp.join("bynk-wrangler.log");
    let ts_log = tmp.join("ts-wrangler.log");
    let a = spawn(&bynk_dir, bynk_port, base + 2, &bynk_vars, &bynk_log);
    let b = spawn(&ts_dir, ts_port, base + 3, &ts_vars, &ts_log);
    let wranglers = match (a, b) {
        (Ok(a), Ok(b)) => Wranglers(vec![a, b]),
        (a, b) => {
            drop(Wranglers(a.into_iter().chain(b).collect()));
            skip("could not launch npx for one or both workers");
            return;
        }
    };
    let logs = || {
        format!(
            "--- bynk worker ---\n{}\n--- typescript worker ---\n{}",
            read_log(&bynk_log),
            read_log(&ts_log)
        )
    };

    // Both up, and the binding connected: the first real call answers.
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        if let Ok(body) = get(ts_port, "/legacy/ping")
            && body == "42"
        {
            break;
        }
        if Instant::now() > deadline {
            let logs = logs();
            drop(wranglers);
            skip(&format!(
                "the two Workers did not answer within the boot window (likely no network \
                 to provision {})\n{logs}",
                wrangler::SPEC
            ));
            return;
        }
        std::thread::sleep(Duration::from_millis(500));
    }

    let steps: &[(&str, &str)] = &[
        ("/legacy/ping", "42"),
        ("/legacy/place", r#"{"tag":"Ok","value":"o-1 for legacy"}"#),
        (
            "/legacy/too-many",
            r#"{"tag":"Err","error":{"tag":"TooMany","limit":10}}"#,
        ),
        (
            "/ledger/place",
            r#"{"tag":"Ok","value":"o-3 for the ledger"}"#,
        ),
        ("/ledger/ping", "Transport:403"),
        ("/legacy/negative", "Transport:400"),
        ("/wrong-secret", "Transport:401"),
        ("/stale-contract", "ContractMismatch"),
    ];
    let mut failures = Vec::new();
    for (path, expected) in steps {
        match get(ts_port, path) {
            Ok(body) if body.trim() == *expected => {}
            Ok(body) => failures.push(format!("{path}: {body:?}, expected {expected}")),
            Err(e) => failures.push(format!("{path}: {e}")),
        }
    }
    let logs = logs();
    drop(wranglers);
    assert!(
        failures.is_empty(),
        "the generated client's calls under workerd:\n{}\n{logs}",
        failures.join("\n")
    );
    let _ = fs::remove_dir_all(&tmp);
}

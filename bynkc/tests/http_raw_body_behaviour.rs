//! Raw HTTP request bodies (`@raw(contentType:)`): behavioural proof, through the
//! emitted Workers `fetch`, of what the system-tier fixture cannot reach. Compiles
//! `2320_http_raw_body`, then a Node driver sends real `Request`s:
//!   - a missing or different `Content-Type` is a `415` `{ kind:
//!     "UnsupportedMediaType" }`; parameters and case are not part of the match;
//!   - the declared type delivers the octets untouched (not valid UTF-8);
//!   - a body over the cap with no `Content-Length` (a stream) is cut off at the
//!     cap with `413`, on a raw route and on a capped JSON route alike;
//!   - a `*/*` route takes any `Content-Type`, or none.
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
import worker from "./workers/store-uploads/index.js";

function assert(cond: boolean, msg: string): void {
  if (!cond) {
    throw new Error(`assertion failed: ${msg}`);
  }
}

const env = {} as never;
const TREE = "application/vnd.bynk.tree-v1";
// Not valid UTF-8: 0x80 0xFF 0x00 0x01 0xFE.
const BINARY = new Uint8Array([0x80, 0xff, 0x00, 0x01, 0xfe]);
const BINARY_B64 = "gP8AAf4=";

function put(contentType: string | null, body: BodyInit): Request {
  return new Request("https://x.test/api/acme/widgets", {
    method: "PUT",
    body,
    headers: contentType === null ? {} : { "content-type": contentType },
  });
}

// A body with no Content-Length: a stream of `chunks` chunks of `size` bytes.
function streamed(path: string, contentType: string, chunks: number, size: number): Request {
  let sent = 0;
  const body = new ReadableStream<Uint8Array>({
    pull(controller) {
      if (sent++ === chunks) {
        controller.close();
        return;
      }
      controller.enqueue(new Uint8Array(size).fill(0x61));
    },
  });
  return new Request(`https://x.test${path}`, {
    method: "POST",
    body,
    headers: { "content-type": contentType },
    duplex: "half",
  } as RequestInit);
}

async function main(): Promise<void> {
  // --- The declared type delivers the octets untouched. ---
  let r = await worker.fetch(put(TREE, BINARY), env);
  assert(r.status === 200, `a raw PUT under the declared type is 200 (got ${r.status})`);
  assert((await r.json()) === `acme/widgets ${BINARY_B64}`, "the handler got the octets as sent");

  // --- Parameters and case are not part of the match. ---
  r = await worker.fetch(put("Application/VND.bynk.TREE-v1; charset=binary", BINARY), env);
  assert(r.status === 200, `parameters and case are ignored (got ${r.status})`);

  // --- A different or missing Content-Type is a 415. ---
  for (const ct of ["application/json", "text/plain", null]) {
    r = await worker.fetch(put(ct, BINARY), env);
    assert(r.status === 415, `Content-Type ${ct} is 415 (got ${r.status})`);
    const payload = (await r.json()) as { kind?: string };
    assert(payload.kind === "UnsupportedMediaType", "the 415 body is `{ kind: UnsupportedMediaType }`");
  }

  // --- A Content-Length over the cap is a 413. ---
  r = await worker.fetch(put(TREE, new Uint8Array(65)), env);
  assert(r.status === 413, `a 65-byte raw body over the 64-byte cap is 413 (got ${r.status})`);
  r = await worker.fetch(put(TREE, new Uint8Array(64)), env);
  assert(r.status === 200, `a body exactly at the cap is accepted (got ${r.status})`);

  // --- A stream with no Content-Length is cut off at the cap. ---
  const noLength = streamed("/blob", "application/anything", 4, 3);
  assert(noLength.headers.get("content-length") === null, "the stream declares no length");
  r = await worker.fetch(noLength, env);
  assert(r.status === 413, `a 12-byte stream over the 8-byte cap is 413 (got ${r.status})`);
  r = await worker.fetch(streamed("/blob", "application/anything", 2, 4), env);
  assert(r.status === 200, `an 8-byte stream at the cap is accepted (got ${r.status})`);

  // --- A Content-Length that understates the body does not get past the cap. ---
  const understated = (path: string, contentType: string, body: BodyInit, method: string): Request =>
    new Request(`https://x.test${path}`, {
      method,
      body,
      headers: { "content-type": contentType, "content-length": "5" },
    });
  r = await worker.fetch(understated("/api/acme/widgets", TREE, new Uint8Array(100), "PUT"), env);
  assert(r.status === 413, `a raw body understating its length is 413 (got ${r.status})`);
  r = await worker.fetch(understated("/json", "application/json", JSON.stringify("x".repeat(100)), "POST"), env);
  assert(r.status === 413, `a JSON body understating its length is 413 (got ${r.status})`);

  // --- The same counted read caps a JSON route. ---
  r = await worker.fetch(streamed("/json", "application/json", 3, 30), env);
  assert(r.status === 413, `a streamed JSON body over the 64-byte cap is 413 (got ${r.status})`);
  const json = (await r.json()) as { kind?: string };
  assert(json.kind === "PayloadTooLarge", "the 413 body is `{ kind: PayloadTooLarge }`");

  // --- A `*/*` route takes any Content-Type, or none. ---
  r = await worker.fetch(new Request("https://x.test/blob", { method: "POST", body: new Uint8Array(2) }), env);
  assert(r.status === 200, `a wildcard route needs no Content-Type (got ${r.status})`);

  console.log("ALL OK");
}

main().catch((e: unknown) => {
  console.error(e);
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
fn http_raw_body_contract_runs_on_workers_entry() {
    let runner = match discover_tsc() {
        Some(r) => r,
        None => {
            eprintln!(
                "\n!!! HTTP RAW BODY BEHAVIOUR VERIFICATION SKIPPED !!!\nno tsc runner on PATH.\n"
            );
            if std::env::var(REQUIRE_ENV).is_ok() {
                panic!("{REQUIRE_ENV} is set but no tsc runner was found");
            }
            return;
        }
    };
    if !tool_exists("node") {
        eprintln!(
            "\n!!! HTTP RAW BODY BEHAVIOUR VERIFICATION SKIPPED !!!\n`node` is not on PATH.\n"
        );
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("{REQUIRE_ENV} is set but `node` was not found");
        }
        return;
    }

    // Compile the raw body fixture (workers) in-process.
    let fixture: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/positive/2320_http_raw_body/src");
    let out = bynkc::compile_project(
        &bynk_testkit::compile_options_single(fixture).target(bynkc::BuildTarget::Workers),
    )
    .map_err(bynkc::ProjectFailure::flatten)
    .expect("the raw body fixture must compile");

    let tmp = std::env::temp_dir().join(format!(
        "bynk-http-raw-body-behaviour-{}",
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
    fs::write(tmp.join("driver.ts"), DRIVER_TS).unwrap();
    fs::write(tmp.join("tsconfig.json"), TSCONFIG_JSON).unwrap();

    // Type-check + compile the whole tree, then run the driver under Node.
    let (ok, log) = run(&runner.0, &runner.1, &["--project", "tsconfig.json"], &tmp);
    assert!(ok, "the raw body driver must type-check + compile:\n{log}");
    let (ran, log) = run("node", &[], &["js/driver.js"], &tmp);
    assert!(
        ran && log.contains("ALL OK"),
        "the raw body driver must run green:\n{log}"
    );

    let _ = fs::remove_dir_all(&tmp);
}

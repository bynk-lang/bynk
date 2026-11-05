//! v0.23 behavioural test for the extended `bynk.cloudflare` Kv adapter —
//! the first **executed adapter-op** test (ADRs 0050/0051).
//!
//! Snapshots prove the emitted shape; this proves the behaviour: the
//! binding-side `list` drain accumulates across *multiple* cursor pages,
//! `putTtl` passes `expirationTtl` through, and the structured-values
//! composition genuinely round-trips (`Json.encode(entry)` → `put` → `get`
//! → `Json.decode[Entry]`). Compiles the bundle Kv fixture in-process and
//! drives its services against a ~30-line in-memory fake `env.KV`.
//!
//! Like the tsc-verification stage, this skips loudly when no TypeScript
//! toolchain is available; `BYNK_REQUIRE_TSC=1` turns the skip into a
//! failure (CI).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const REQUIRE_ENV: &str = "BYNK_REQUIRE_TSC";

/// Build a `Command` for `program`, routing through `cmd /C` on Windows so
/// npm's `.cmd` shims (`tsc.cmd`, `npx.cmd`) resolve — Rust's CreateProcess
/// deliberately refuses to run batch scripts directly (the BatBadBut
/// hardening), so a bare `Command::new("npx")` fails there.
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
    // `where` is the Windows counterpart of `which`.
    let finder = if cfg!(windows) { "where" } else { "which" };
    Command::new(finder)
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
import { scan, cache, allKeys } from "./kv/index.js";
import { WorkersKv } from "./bynk/cloudflare.binding.js";

function assert(cond: boolean, msg: string): void {
  if (!cond) {
    throw new Error(`assertion failed: ${msg}`);
  }
}

// An in-memory fake KV namespace. `list` pages with size 2 so the drain
// must cross page boundaries; `put` records its options so the TTL
// pass-through is observable.
const store = new Map<string, string>();
const putOptions: Array<{ key: string; options: unknown }> = [];
const fakeKv = {
  async get(key: string): Promise<string | null> {
    return store.has(key) ? (store.get(key) as string) : null;
  },
  async put(key: string, value: string, options?: { expirationTtl?: number }): Promise<void> {
    store.set(key, value);
    putOptions.push({ key, options });
  },
  async delete(key: string): Promise<void> {
    store.delete(key);
  },
  async list(options?: { prefix?: string; cursor?: string }): Promise<{
    keys: { name: string }[];
    list_complete: boolean;
    cursor?: string;
  }> {
    const all = [...store.keys()]
      .filter((k) => (options?.prefix === undefined ? true : k.startsWith(options.prefix)))
      .sort();
    const start = options?.cursor === undefined ? 0 : Number(options.cursor);
    const page = all.slice(start, start + 2);
    const next = start + 2;
    const complete = next >= all.length;
    return {
      keys: page.map((name) => ({ name })),
      list_complete: complete,
      cursor: complete ? undefined : String(next),
    };
  },
};
const deps = { Kv: new WorkersKv({ KV: fakeKv }) };

// 1) The structured round-trip: Json.encode(entry) -> putTtl -> get ->
//    Json.decode[Entry], all inside the emitted service.
const entry = { sku: "widget", price: 19.99, qty: 2 };
const cached = await cache.call("item:widget", entry, deps);
assert(cached.tag === "Some", "structured round-trip returns the entry");
if (cached.tag === "Some") {
  assert(cached.value.price === 19.99, "price survives the round-trip exactly");
  assert(cached.value.sku === "widget", "sku survives");
}

// 2) putTtl passed expirationTtl through to the namespace.
assert(putOptions.length === 1, "one put recorded");
assert(
  JSON.stringify(putOptions[0].options) === JSON.stringify({ expirationTtl: 60 }),
  `expirationTtl passed through, got ${JSON.stringify(putOptions[0].options)}`,
);

// 3) The list drain crosses page boundaries (page size 2, five keys).
store.clear();
for (const k of ["item:a", "item:b", "item:c", "item:d", "other:e"]) {
  store.set(k, "v");
}
const scanned = await scan.call("item:", deps);
assert(scanned.tag === "Ok", "scan succeeds");
if (scanned.tag === "Ok") {
  assert(
    scanned.value.length === 4 && scanned.value[0] === "item:a" && scanned.value[3] === "item:d",
    `prefix drain crossed pages, got ${JSON.stringify(scanned.value)}`,
  );
}

// 4) The no-prefix drain sees everything.
const everything = await allKeys.call(0, deps);
assert(everything.tag === "Ok", "allKeys succeeds");
if (everything.tag === "Ok") {
  assert(everything.value.length === 5, `unprefixed drain got ${everything.value.length} keys`);
}

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
fn kv_adapter_behaviour() {
    let runner = match discover_tsc() {
        Some(r) => r,
        None => {
            eprintln!(
                "\n!!! KV-ADAPTER VERIFICATION SKIPPED !!!\nneither `tsc` nor `npx` is on PATH.\n"
            );
            if std::env::var(REQUIRE_ENV).is_ok() {
                panic!("{REQUIRE_ENV} is set but no tsc runner was found");
            }
            return;
        }
    };
    if !tool_exists("node") {
        eprintln!("\n!!! KV-ADAPTER VERIFICATION SKIPPED !!!\n`node` is not on PATH.\n");
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("{REQUIRE_ENV} is set but `node` was not found");
        }
        return;
    }

    // Compile the bundle Kv fixture in-process.
    let fixture: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/positive/214_kv_list_bundle/src");
    let out = bynkc::compile_project(
        &bynk_testkit::compile_options_single(fixture)
            .target(bynkc::BuildTarget::Bundle)
            .platform(bynkc::Platform::Cloudflare),
    )
    .map_err(bynkc::ProjectFailure::flatten)
    .expect("the Kv bundle fixture must compile");

    let tmp = std::env::temp_dir().join(format!("bynk-kv-behaviour-{}", std::process::id()));
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

    let (program, prefix) = &runner;
    let (ok, out_text) = run(program, prefix, &["-p", "tsconfig.json"], &tmp);
    assert!(ok, "tsc failed on the kv-adapter driver:\n{out_text}");

    let (ok, out_text) = run("node", &[], &["js/driver.js"], &tmp);
    assert!(
        ok && out_text.contains("ALL OK"),
        "kv-adapter driver did not pass:\n{out_text}"
    );
    let _ = fs::remove_dir_all(&tmp);
}

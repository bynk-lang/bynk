//! I4 (#1970): the typed TypeScript client a context's foreign callers import
//! (`workers/<context>/bynk-client.ts`) and its runtime package,
//! `@bynk/wire-client` (`design/archive/retired-tracks.md`, interop track D3).
//!
//! Three kinds of guard:
//!
//! - **Drift** (no toolchain): every blessed client embeds exactly the contract
//!   hashes its callee enforces, the wire version the compiler speaks and the
//!   secret name `bynk deploy` hands over, and the context's
//!   `bynk-contracts.json` records the same hashes for each foreign caller, as
//!   `contract_hash.rs` checks a Bynk caller's stamps. The package's major
//!   version is the wire version.
//! - **The caller's build** (`tsc`): each client, with the package resolved
//!   from `node_modules` the way a Worker's build resolves it, type-checks with
//!   a hand-written caller under a strict configuration, unused-name checks
//!   included.
//! - **Skew as a `tsc` error**: a caller calling a service its actor is not
//!   admitted to, an `env` without the secret, a package for another wire
//!   version, and a client regenerated after a signature change each fail the
//!   caller's build.
//!
//! The end-to-end call under `workerd` is `typescript_client_workerd.rs`.

#[path = "../require/mod.rs"]
mod require;
#[path = "../tools/mod.rs"]
mod tools;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const REQUIRE_ENV: &str = "BYNK_REQUIRE_TSC";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/positive")
}

fn package_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../bynk-emit/wire-client")
}

/// Every `(fixture, worker dir)` whose blessed output has a client.
fn clients() -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    for e in fs::read_dir(fixtures()).unwrap().flatten() {
        let Ok(workers) = fs::read_dir(e.path().join("expected/workers")) else {
            continue;
        };
        for w in workers.flatten() {
            if w.path()
                .join(bynk_emit::emitter::client::CLIENT_FILE)
                .is_file()
            {
                out.push((e.file_name().to_string_lossy().to_string(), w.path()));
            }
        }
    }
    out.sort();
    out
}

/// `service -> hash` from a client's `CONTRACTS` literal.
fn client_contracts(client: &str) -> BTreeMap<String, String> {
    let block = regex::Regex::new(r"export const CONTRACTS = \{([^}]*)\} as const;").unwrap();
    let entry = regex::Regex::new(r#"([A-Za-z_][A-Za-z0-9_]*): "([0-9a-f]{16})""#).unwrap();
    let body = block
        .captures(client)
        .unwrap_or_else(|| panic!("no CONTRACTS literal in:\n{client}"))[1]
        .to_string();
    entry
        .captures_iter(&body)
        .map(|c| (c[1].to_string(), c[2].to_string()))
        .collect()
}

#[test]
fn every_client_embeds_its_callees_contracts() {
    let found = clients();
    assert!(
        found.len() >= 2,
        "expected the client fixtures (1970, 2013) to ship clients, found {found:?}"
    );
    let ctx_line = regex::Regex::new(r#"export const CONTEXT = "([a-z0-9_.]+)";"#).unwrap();
    for (fixture, dir) in &found {
        let client = fs::read_to_string(dir.join(bynk_emit::emitter::client::CLIENT_FILE)).unwrap();
        let manifest: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(dir.join("bynk-contracts.json")).unwrap())
                .unwrap();
        let provides = manifest["provides"].as_object().unwrap();
        let foreign = manifest["foreign"]
            .as_object()
            .unwrap_or_else(|| panic!("{fixture}: a client but no `foreign` record"));

        // Each embedded hash is the callee's own constant.
        let contracts = client_contracts(&client);
        assert!(
            !contracts.is_empty(),
            "{fixture}: a client with no services"
        );
        for (svc, hash) in &contracts {
            assert_eq!(
                provides.get(svc).and_then(|v| v.as_str()),
                Some(hash.as_str()),
                "{fixture}: the client's hash for `{svc}` is not the callee's"
            );
        }
        // The manifest's per-caller record covers exactly the client's
        // services, at the same hashes.
        let mut covered = std::collections::BTreeSet::new();
        for (actor, svcs) in foreign {
            for (svc, hash) in svcs.as_object().unwrap() {
                assert_eq!(
                    contracts.get(svc).map(String::as_str),
                    hash.as_str(),
                    "{fixture}: `{actor}`'s recorded hash for `{svc}` is not the client's"
                );
                covered.insert(svc.clone());
            }
            assert!(
                client.contains(&format!("\"{actor}\"")),
                "{fixture}: `{actor}` is missing from the client's ForeignCaller"
            );
        }
        assert_eq!(
            covered,
            contracts.keys().cloned().collect(),
            "{fixture}: the client and the manifest disagree on the services"
        );

        // The wire version, the secret and the package.
        assert!(
            client.contains(&format!(
                "export const WIRE_VERSION: __WireVersion = \"{}\";",
                bynk_check::wire_format::WIRE_VERSION
            )),
            "{fixture}: the client's wire version is not the compiler's"
        );
        let context = ctx_line.captures(&client).unwrap()[1].to_string();
        // The env binding's *name* (`BYNK_CALL_TO_<CALLEE>`), never a key value.
        let key_binding = bynk_check::wire_format::edge_key_to(&context).unwrap();
        assert!(
            client.contains(&format!("export const SECRET = \"{key_binding}\";")),
            "{fixture}: the client does not name `{key_binding}`"
        );
        assert!(
            client.contains(&format!(
                "from \"{}\";",
                bynk_emit::emitter::client::CLIENT_PACKAGE
            )),
            "{fixture}: the client does not import the package"
        );
    }
}

/// A context with no foreign caller gets no client, and its manifest no
/// `foreign` record: the narrowing that keeps every other golden unchanged.
#[test]
fn only_a_context_with_a_foreign_caller_gets_a_client() {
    for e in fs::read_dir(fixtures()).unwrap().flatten() {
        let Ok(workers) = fs::read_dir(e.path().join("expected/workers")) else {
            continue;
        };
        for w in workers.flatten() {
            let has_client = w
                .path()
                .join(bynk_emit::emitter::client::CLIENT_FILE)
                .is_file();
            let foreign = fs::read_to_string(w.path().join("bynk-contracts.json"))
                .map(|m| m.contains("\"foreign\""))
                .unwrap_or(false);
            assert_eq!(
                has_client,
                foreign,
                "{}: a client and a `foreign` record go together",
                w.path().display()
            );
        }
    }
}

#[test]
fn the_package_follows_the_wire_version() {
    let pkg: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(package_dir().join("package.json")).unwrap())
            .unwrap();
    assert_eq!(
        pkg["name"].as_str(),
        Some(bynk_emit::emitter::client::CLIENT_PACKAGE)
    );
    let version = pkg["version"].as_str().unwrap();
    let major: u32 = version.split('.').next().unwrap().parse().unwrap();
    assert_eq!(
        major,
        bynk_check::wire_format::WIRE_VERSION,
        "@bynk/wire-client {version}'s major version must be the wire version it speaks"
    );
    let index = fs::read_to_string(package_dir().join("index.ts")).unwrap();
    assert!(
        index.contains(&format!(
            "export const WIRE_VERSION = \"{}\";",
            bynk_check::wire_format::WIRE_VERSION
        )),
        "the package's bundled boundary speaks another wire version"
    );
}

// ---- The caller's build -------------------------------------------------------

/// A strict caller configuration, unused-name checks included, so a client
/// that imports a name it never uses fails here as it would in such a build.
const TSCONFIG: &str = r#"{
  "compilerOptions": {
    "target": "ES2022",
    "lib": ["ES2022", "DOM"],
    "module": "NodeNext",
    "moduleResolution": "NodeNext",
    "strict": true,
    "noUnusedLocals": true,
    "noUnusedParameters": true,
    "noImplicitReturns": true,
    "noFallthroughCasesInSwitch": true,
    "skipLibCheck": true,
    "noEmit": true
  },
  "include": ["*.ts"]
}
"#;

/// A hand-written caller of fixture 2013's `shop.orders` client.
const CALLER_2013: &str = r#"import { connect, CONTRACTS, WIRE_VERSION, type Env } from "./client.js";
import { callErrorOf, type Result } from "@bynk/wire-client";

interface WorkerEnv extends Env {
  readonly SHOP_ORDERS: { fetch(request: Request): Promise<Response> };
}

export async function run(env: WorkerEnv): Promise<string> {
  const legacy = connect({ caller: "Legacy", binding: env.SHOP_ORDERS, env });
  const ledger = connect({ caller: "Ledger", binding: env.SHOP_ORDERS, env });
  try {
    const n: number = await legacy.ping(1);
    const s: Result<string, string> = await legacy.status("o-1");
    const t = await ledger.status("o-2");
    const pinned: "226b9939f85e69ea" = CONTRACTS.status;
    const wire: "1" = WIRE_VERSION;
    return [n, s.tag, t.tag === "Ok" ? t.value : t.error, pinned, wire].join(",");
  } catch (e) {
    return callErrorOf(e)?.kind ?? "other";
  }
}
"#;

/// A hand-written caller of fixture 1970's `shop.ledger` client, building and
/// reading every kind of boundary type it exposes.
const CALLER_1970: &str = r#"import { connect, type Client_, type Entry, type LedgerError, type Money, type Page, type Env } from "./client.js";
import { None, Some } from "@bynk/wire-client";

interface WorkerEnv extends Env {
  readonly LEDGER: { fetch(request: Request): Promise<Response> };
}

function describe(e: LedgerError): string {
  switch (e.tag) {
    case "NotFound":
      return "not found";
    case "Rejected":
      return e.reason;
  }
}

export async function run(env: WorkerEnv): Promise<string> {
  const ledger = connect({ caller: "Billing", binding: env.LEDGER, env });
  const who: Client_ = { name: "acme" };
  const amount: Money = { cents: 1200, currency: "GBP" };
  const posted = await ledger.post(who, amount, Some("rent"));
  if (posted.tag === "Err") return describe(posted.error);
  const found = await ledger.lookup(posted.value);
  const entry: Entry | undefined = found.tag === "Ok" ? found.value : undefined;
  const page: Page<Entry> = await ledger.recent();
  const memo = entry?.memo ?? None;
  return [entry?.receipt.byteLength ?? 0, entry?.tags.join("+") ?? "", page.items.length, memo.tag].join(",");
}
"#;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bynk-ts-client-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A directory holding `client.ts`, `caller.ts` and the package under
/// `node_modules`, as a Worker's repository would.
fn stage(name: &str, client: &str, caller: &str, package_index: &str) -> PathBuf {
    let dir = scratch(name);
    let pkg = dir.join("node_modules/@bynk/wire-client");
    fs::create_dir_all(&pkg).unwrap();
    fs::copy(package_dir().join("package.json"), pkg.join("package.json")).unwrap();
    fs::write(pkg.join("index.ts"), package_index).unwrap();
    fs::write(dir.join("client.ts"), client).unwrap();
    fs::write(dir.join("caller.ts"), caller).unwrap();
    fs::write(dir.join("tsconfig.json"), TSCONFIG).unwrap();
    dir
}

/// `tsc -p` over a staged directory: success and the combined output.
fn tsc(dir: &Path) -> (bool, String) {
    let mut cmd = tools::tsc_command().expect("checked by the caller");
    let out = cmd
        .arg("-p")
        .arg(dir.join("tsconfig.json"))
        .output()
        .expect("run tsc");
    (
        out.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}

fn have_tsc(what: &str) -> bool {
    if tools::tsc_command().is_some() {
        return true;
    }
    tools::skip(
        what,
        require::is_required(REQUIRE_ENV),
        REQUIRE_ENV,
        "no TypeScript toolchain (`tsc` or `npx`) is on PATH",
    );
    false
}

fn blessed_client(fixture: &str, worker: &str) -> String {
    fs::read_to_string(
        fixtures()
            .join(fixture)
            .join("expected/workers")
            .join(worker)
            .join(bynk_emit::emitter::client::CLIENT_FILE),
    )
    .unwrap()
}

fn package_index() -> String {
    fs::read_to_string(package_dir().join("index.ts")).unwrap()
}

#[test]
fn generated_clients_type_check_in_a_strict_callers_build() {
    if !have_tsc("TYPESCRIPT CLIENT (tsc)") {
        return;
    }
    let cases = [
        ("2013", "2013_foreign_callers", "shop-orders", CALLER_2013),
        ("1970", "1970_typescript_client", "shop-ledger", CALLER_1970),
    ];
    let mut failures = Vec::new();
    for (name, fixture, worker, caller) in cases {
        let dir = stage(
            name,
            &blessed_client(fixture, worker),
            caller,
            &package_index(),
        );
        let (ok, output) = tsc(&dir);
        if !ok {
            failures.push(format!("--- {fixture} ---\n{output}"));
        } else {
            let _ = fs::remove_dir_all(&dir);
        }
    }
    assert!(
        failures.is_empty(),
        "a generated client does not type-check in its caller's build:\n{}",
        failures.join("\n")
    );
}

/// Each misuse fails the caller's build, with the error naming what is wrong.
#[test]
fn skew_and_misuse_are_tsc_errors_in_the_callers_build() {
    if !have_tsc("TYPESCRIPT CLIENT SKEW (tsc)") {
        return;
    }
    let orders = blessed_client("2013_foreign_callers", "shop-orders");
    let package = package_index();
    let mut failures = Vec::new();
    let mut expect_error = |name: &str, client: &str, caller: &str, package: &str, needle: &str| {
        let dir = stage(name, client, caller, package);
        let (ok, output) = tsc(&dir);
        if ok || !output.contains(needle) {
            failures.push(format!(
                "--- {name}: expected a tsc error containing {needle:?} ---\n{output}"
            ));
        } else {
            let _ = fs::remove_dir_all(&dir);
        }
    };

    // `Ledger` may call `status`, not `ping`: the method is not on its client.
    expect_error(
        "not-admitted",
        &orders,
        r#"import { connect, type Env } from "./client.js";
export async function run(env: Env & { readonly B: { fetch(r: Request): Promise<Response> } }): Promise<number> {
  return connect({ caller: "Ledger", binding: env.B, env }).ping(1);
}
"#,
        &package,
        "Property 'ping' does not exist",
    );

    // An undeclared caller is not a `ForeignCaller`.
    expect_error(
        "undeclared-caller",
        &orders,
        r#"import { connect, type Env } from "./client.js";
export function run(env: Env & { readonly B: { fetch(r: Request): Promise<Response> } }): unknown {
  return connect({ caller: "Intruder", binding: env.B, env });
}
"#,
        &package,
        "\"Intruder\"",
    );

    // An `env` without the secret `bynk deploy` hands over.
    expect_error(
        "no-secret",
        &orders,
        r#"import { connect } from "./client.js";
export function run(env: { readonly B: { fetch(r: Request): Promise<Response> } }): unknown {
  return connect({ caller: "Legacy", binding: env.B, env });
}
"#,
        &package,
        "BYNK_CALL_TO_SHOP__ORDERS",
    );

    // A package that speaks another wire version.
    let v2 = package.replace(
        "export const WIRE_VERSION = \"1\";",
        "export const WIRE_VERSION = \"2\";",
    );
    assert_ne!(v2, package, "the package's wire version line moved");
    expect_error(
        "wire-version",
        &orders,
        CALLER_2013,
        &v2,
        "Type '\"1\"' is not assignable to type '\"2\"'",
    );

    // A signature change on the callee, picked up by regenerating the client:
    // the caller written against the old one no longer builds.
    let changed = regenerate_1970_with(
        "on call(client: Client, amount: Money, memo: Option[String])",
        "on call(client: Client, amount: Money, memo: String)",
    );
    expect_error(
        "signature-change",
        &changed,
        CALLER_1970,
        &package,
        "Option<string>",
    );

    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Fixture 1970's client, compiled by the `bynk` binary after replacing
/// `from` with `to` in its context.
fn regenerate_1970_with(from: &str, to: &str) -> String {
    let src = fixtures().join("1970_typescript_client/src");
    let dir = scratch("regenerate");
    let proj = dir.join("proj");
    fs::create_dir_all(proj.join("src/shop")).unwrap();
    fs::write(
        proj.join("bynk.toml"),
        "[project]\nname = \"ledger\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::copy(
        src.join("bynk.schema.lock"),
        proj.join("src/bynk.schema.lock"),
    )
    .unwrap();
    fs::copy(
        src.join("shop/money.bynk"),
        proj.join("src/shop/money.bynk"),
    )
    .unwrap();
    let ledger = fs::read_to_string(src.join("shop/ledger.bynk")).unwrap();
    assert!(
        ledger.contains(from),
        "fixture 1970 no longer declares {from:?}"
    );
    fs::write(proj.join("src/shop/ledger.bynk"), ledger.replace(from, to)).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
        .args(["compile", "--target", "workers", "-o"])
        .arg(dir.join("out"))
        .arg(&proj)
        .output()
        .expect("run bynk");
    assert!(
        out.status.success(),
        "the changed fixture does not compile:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let client = fs::read_to_string(
        dir.join("out/workers/shop-ledger")
            .join(bynk_emit::emitter::client::CLIENT_FILE),
    )
    .unwrap();
    let _ = fs::remove_dir_all(&dir);
    client
}

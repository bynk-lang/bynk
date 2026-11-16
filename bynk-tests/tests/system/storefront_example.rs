//! `examples/storefront` (#3508, BC7): a front end over a generated `bynk client`.
//!
//! - The committed `web/src/api.ts` is current (`bynk client --check`).
//! - The front end type-checks under `tsc --strict`.
//! - Its logic (`web/src/storefront.ts`) runs against the real Worker in
//!   process, through a `fetch` that hands each request to the Worker, and
//!   handles every outcome: success, a typed `409`, a refinement refused before
//!   anything is sent, and a transport failure.
//!
//! Skips loudly without `node` or `tsc`; `BYNK_REQUIRE_TSC=1` makes it a failure.

#[path = "../require/mod.rs"]
mod require;
#[path = "../tools/mod.rs"]
mod tools;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const REQUIRE_ENV: &str = "BYNK_REQUIRE_TSC";

fn example() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/storefront")
}

fn bynk() -> Command {
    Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
}

fn skip() -> bool {
    let missing = if !tools::tool_exists("node") {
        Some("`node` is not on PATH")
    } else if tools::tsc_command().is_none() {
        Some("neither `tsc` nor `npx` is on PATH")
    } else {
        None
    };
    missing.is_some_and(|reason| {
        tools::skip(
            "STOREFRONT EXAMPLE",
            require::is_required(REQUIRE_ENV),
            REQUIRE_ENV,
            reason,
        )
    })
}

#[test]
fn the_committed_client_is_current() {
    let out = bynk()
        .args(["client", "--check"])
        .arg(example())
        .output()
        .expect("run bynk");
    assert!(
        out.status.success(),
        "examples/storefront/web/src/api.ts is stale; run `bynk client`:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

const DRIVER: &str = r#"
import worker from "./out/workers/shop-api/index.js";
import { createClient } from "./web/dist/api.js";
import { loadCatalogue, placeOrder } from "./web/dist/storefront.js";

const viaWorker = (url, init) => worker.fetch(new Request(url, init), {});
const shop = createClient({ baseUrl: "http://shop.test", fetch: viaWorker }).shop;
const down = createClient({
  baseUrl: "http://shop.test",
  fetch: () => Promise.reject(new Error("offline")),
}).shop;

console.log("catalogue", JSON.stringify(await loadCatalogue(shop)));
console.log("ordered", await placeOrder(shop, "mug", 2));
console.log("sold-out", await placeOrder(shop, "sold-out", 1));
console.log("invalid", await placeOrder(shop, "mug", 100));
console.log("offline", await placeOrder(down, "mug", 1));
"#;

#[test]
fn the_front_end_type_checks_and_runs_against_the_real_worker() {
    if skip() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("bynk-storefront-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("web")).unwrap();
    for rel in ["web/tsconfig.json", "web/src/api.ts", "web/src/storefront.ts"] {
        let to = dir.join(rel);
        fs::create_dir_all(to.parent().unwrap()).unwrap();
        fs::copy(example().join(rel), to).unwrap();
    }

    let tsc = tools::tsc_command()
        .expect("checked")
        .arg("-p")
        .arg(dir.join("web"))
        .output()
        .expect("run tsc");
    assert!(
        tsc.status.success(),
        "the front end does not pass `tsc --strict`:\n{}{}",
        String::from_utf8_lossy(&tsc.stdout),
        String::from_utf8_lossy(&tsc.stderr)
    );

    let compiled = bynk()
        .args(["compile", "--target", "workers", "--emit", "js", "-o"])
        .arg(dir.join("out"))
        .arg(example())
        .output()
        .expect("run bynk");
    assert!(
        compiled.status.success(),
        "bynk compile failed:\n{}",
        String::from_utf8_lossy(&compiled.stderr)
    );

    fs::write(dir.join("web/dist/package.json"), "{\"type\":\"module\"}\n").unwrap();
    fs::write(dir.join("drive.mjs"), DRIVER).unwrap();
    let run = Command::new("node")
        .arg("drive.mjs")
        .current_dir(&dir)
        .output()
        .expect("run node");
    let stdout = String::from_utf8_lossy(&run.stdout).into_owned();
    assert!(
        run.status.success(),
        "the driver failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let said = |label: &str| {
        stdout
            .lines()
            .find_map(|l| l.strip_prefix(label))
            .unwrap_or_else(|| panic!("no `{label}` line in:\n{stdout}"))
            .trim()
            .to_string()
    };
    assert_eq!(
        said("catalogue"),
        r#"[{"sku":"mug","label":"Enamel mug"},{"sku":"tote","label":"Canvas tote (organic cotton)"}]"#
    );
    assert_eq!(said("ordered"), "Ordered 2 × mug (o-1)");
    assert_eq!(said("sold-out"), "Sorry: that item is sold out");
    assert_eq!(said("invalid"), "Check the quantity (1 to 99); nothing was sent.");
    assert_eq!(said("offline"), "Could not reach the shop. Try again.");
    let _ = fs::remove_dir_all(&dir);
}

//! #3793: a `store` field renamed with `replaces` keeps its state across a deploy.
//!
//! Two builds of one scratch project, as a deploy sees them: version 1 declares `store customer`,
//! version 2 declares `store owner: Cell[String] replaces customer`. The second build's emitted
//! JavaScript is run against a record version 1 stored, and must read `customer`'s value as
//! `owner` and write it back under the new name only. The lock's side of the rename is pinned
//! in `field_rename_lock.rs`.
//!
//! Skips loudly without `node`; `BYNK_REQUIRE_TSC=1` turns the skip into a failure, as the other
//! Node-driven suites do.

#[path = "../require/mod.rs"]
mod require;
#[path = "../tools/mod.rs"]
mod tools;

use std::fs;
use std::path::Path;

const REQUIRE_ENV: &str = "BYNK_REQUIRE_TSC";

fn cart(field: &str, read: &str) -> String {
    format!(
        "context shop\n\nagent Cart {{\n  key id: String\n  {field}\n\n  on call owner() -> Effect[String] {{\n    {read}\n  }}\n\n  on call rename(name: String) -> Effect[()] {{\n    {read} := name\n  }}\n}}\n"
    )
}

/// `bynk compile --emit js -o <out> <project>`, which also reads and writes `bynk.schema.lock`.
fn compile(project: &Path, out: &Path) {
    let run = std::process::Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
        .args(["compile", "--emit", "js", "-o"])
        .arg(out)
        .arg(project)
        .output()
        .expect("run bynk");
    assert!(
        run.status.success(),
        "compile failed:\n{}{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
}

/// A script that wakes `Cart` over `stored` as its `"state"` record, calls `owner`, then
/// `rename("grace")`, and prints what it read and what the storage holds after.
const DRIVER: &str = r#"
import { Cart } from "./shop/cart.js";
const stored = JSON.parse(process.argv[2]);
const store = new Map(stored === null ? [] : [["state", stored]]);
const storage = {
  get: async (k) => (Array.isArray(k) ? new Map(k.map((x) => [x, store.get(x)])) : store.get(k)),
  put: async (k, v) => {
    if (typeof k === "object") for (const [a, b] of Object.entries(k)) store.set(a, b);
    else store.set(k, v);
  },
  delete: async (k) => {
    for (const x of Array.isArray(k) ? k : [k]) store.delete(x);
    return true;
  },
  list: async () => new Map(),
  transaction: async (f) => f(storage),
};
const cart = new Cart({ storage });
const read = await cart.owner({});
await cart.rename("grace", {});
console.log(JSON.stringify({ read, after: store.get("state") }));
"#;

fn run(out: &Path, stored: &str) -> serde_json::Value {
    fs::write(out.join("driver.mjs"), DRIVER).unwrap();
    let run = tools::base_command("node")
        .args(["driver.mjs", stored])
        .current_dir(out)
        .output()
        .expect("run node");
    assert!(
        run.status.success(),
        "the driver failed:\n{}{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    serde_json::from_slice(&run.stdout).expect("the driver prints JSON")
}

#[test]
fn a_renamed_store_field_is_read_from_the_record_the_old_build_stored() {
    if !tools::tool_exists("node") {
        tools::skip(
            "FIELD RENAME BEHAVIOUR",
            require::is_required(REQUIRE_ENV),
            REQUIRE_ENV,
            "`node` is not on PATH",
        );
        return;
    }
    let root = std::env::temp_dir().join(format!("bynk-field-rename-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("shop")).unwrap();
    fs::write(
        root.join("bynk.toml"),
        "[project]\nname = \"field-rename\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();

    // Version 1, as deployed: the value lives under `customer`.
    fs::write(
        root.join("shop/cart.bynk"),
        cart("store customer: Cell[String] = \"\"", "customer"),
    )
    .unwrap();
    compile(&root, &root.join("out1"));

    // Version 2: renamed, with the clause.
    fs::write(
        root.join("shop/cart.bynk"),
        cart(
            "store owner: Cell[String] replaces customer = \"\"",
            "owner",
        ),
    )
    .unwrap();
    let out = root.join("out2");
    compile(&root, &out);

    // A record version 1 wrote: read as `owner`, and written back under `owner` alone.
    let r = run(&out, r#"{"customer": "ada", "__v": 1}"#);
    assert_eq!(r["read"], "ada", "{r}");
    assert_eq!(r["after"]["owner"], "grace", "{r}");
    assert!(
        r["after"].get("customer").is_none(),
        "the old key is dropped by the first commit: {r}"
    );
    assert_eq!(r["after"]["__v"], 2, "{r}");

    // A record version 2 wrote: the new key, untouched by the rename.
    let r = run(&out, r#"{"owner": "ada", "__v": 2}"#);
    assert_eq!(r["read"], "ada", "{r}");

    // A record holding both (a stale `customer` beside a fresh `owner`): the new key wins.
    let r = run(&out, r#"{"customer": "stale", "owner": "fresh", "__v": 1}"#);
    assert_eq!(r["read"], "fresh", "{r}");

    // A fresh key takes the default, as before.
    let r = run(&out, "null");
    assert_eq!(r["read"], "", "{r}");

    let _ = fs::remove_dir_all(&root);
}

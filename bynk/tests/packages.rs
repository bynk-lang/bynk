//! Path dependencies (packaging slice 2a, #2223), over the real binary.
//!
//! Pinned here:
//!  - a consumer reaches a dependency's commons as `<alias>.<unit>`, and its
//!    suite runs against the dependency's code (`bynk test`);
//!  - a dependency adapter is consumed, its provider wired and its binding
//!    vendored under `out/deps/<org>/<package>/`;
//!  - the diamond: one package reached through two aliases is one package,
//!    with one type;
//!  - each refusal, with its exact message: a non-exported or unknown unit, an
//!    alias shadowing a unit, a name mismatch, a duplicate source, a cycle, a
//!    dependency's path dependency outside the workspace, git and registry
//!    sources, a dependency adapter's npm `requires` (git dependencies and
//!    `[dev-dependencies]` are `packages_git.rs`'s);
//!  - an unused dependency changes nothing that is emitted.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn bynk() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_bynk"))
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("packages")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// The sibling `bynkc`, present only when the whole workspace was built.
fn bynkc_sibling() -> Option<PathBuf> {
    let p = bynk().parent().unwrap().join("bynkc");
    p.exists().then_some(p)
}

/// Run `bynk <args>` in `cwd`, capturing (code, stdout, stderr).
fn bynk_in(cwd: &Path, args: &[&str]) -> (i32, String, String) {
    let mut cmd = Command::new(bynk());
    cmd.args(args).current_dir(cwd).env_remove("BYNK_BYNKC");
    // `bynk test` runs a `bynkc`: the one built beside this `bynk`.
    if args.first() == Some(&"test")
        && let Some(bynkc) = bynkc_sibling()
    {
        cmd.env("BYNK_BYNKC", bynkc);
    }
    let out = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// `bynk check --format short`'s diagnostic lines, whichever stream they're on.
fn short(cwd: &Path) -> (i32, String) {
    let (code, out, err) = bynk_in(cwd, &["check", "--format", "short"]);
    (code, format!("{out}{err}"))
}

/// A workspace at `root` with `packages/money` (`acme-labs/money`, exporting
/// `currency`) and `apps/shop`, whose `bynk.toml` is `shop_manifest`.
fn money_workspace(root: &Path, shop_manifest: &str) -> PathBuf {
    write(
        &root.join("bynk.toml"),
        "[workspace]\nmembers = [\"packages/*\", \"apps/*\"]\n",
    );
    write(
        &root.join("packages/money/bynk.toml"),
        "[package]\nname = \"acme-labs/money\"\nversion = \"1.0.0\"\nexports = [\"currency\", \"tokens\"]\n",
    );
    write(
        &root.join("packages/money/src/currency.bynk"),
        "commons currency\n\nuses rounding\n\nfn double(c: Int) -> Int {\n  half(c) * 4\n}\n",
    );
    write(
        &root.join("packages/money/src/rounding.bynk"),
        "commons rounding\n\nfn half(n: Int) -> Int {\n  n / 2\n}\n",
    );
    write(
        &root.join("packages/money/src/tokens.bynk"),
        "adapter tokens {\n  binding \"./tokens.binding.ts\"\n\n  exports capability { Jwt }\n\n  capability Jwt {\n    fn sign(sub: String, secret: String) -> Effect[String]\n  }\n\n  provides Jwt = JoseJwt\n}\n",
    );
    write(
        &root.join("packages/money/src/tokens.binding.ts"),
        "import type { Jwt } from \"./tokens.js\";\n\nexport class JoseJwt implements Jwt {\n  async sign(sub: string, secret: string): Promise<string> {\n    return `${sub}.${secret}`;\n  }\n}\n",
    );
    let shop = root.join("apps/shop");
    write(&shop.join("bynk.toml"), shop_manifest);
    shop
}

const SHOP: &str = "[project]\nname = \"shop\"\n\n[dependencies]\nmoney = { package = \"acme-labs/money\", path = \"../../packages/money\" }\n";

#[test]
fn a_dependency_commons_is_reached_by_alias_and_runs() {
    let root = scratch("commons");
    let shop = money_workspace(&root, SHOP);
    write(
        &shop.join("src/pricing.bynk"),
        "commons pricing\n\nuses money.currency\n\nfn total(n: Int) -> Int {\n  double(n)\n}\n",
    );
    write(
        &shop.join("tests/pricing_test.bynk"),
        "suite pricing\n\ncase \"doubles through the dependency\" {\n  expect total(3) == 4\n}\n",
    );
    let (code, out) = short(&shop);
    assert_eq!(code, 0, "{out}");
    if bynkc_sibling().is_none() {
        eprintln!("skipping `bynk test`: sibling bynkc not built");
        return;
    }
    let (code, out, err) = bynk_in(&shop, &["test"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("doubles through the dependency"), "{out}");
}

#[test]
fn a_dependency_adapter_is_consumed_and_its_binding_vendored() {
    let root = scratch("adapter");
    let shop = money_workspace(&root, SHOP);
    write(
        &shop.join("src/auth/sessions.bynk"),
        "context auth.sessions\n\nconsumes money.tokens\n\nservice login {\n  on call(secret: String) -> Effect[String] given money.tokens.Jwt {\n    let token <- money.tokens.Jwt.sign(\"u1\", secret)\n    token\n  }\n}\n",
    );
    let (code, out) = short(&shop);
    assert_eq!(code, 0, "{out}");
    let bynkc = bynk().parent().unwrap().join("bynkc");
    if !bynkc.exists() {
        return;
    }
    let out_dir = shop.join("out");
    let status = Command::new(&bynkc)
        .args(["compile", ".", "-o"])
        .arg(&out_dir)
        .current_dir(&shop)
        .status()
        .expect("spawn bynkc");
    assert!(status.success());
    for f in [
        "deps/acme-labs/money/tokens.ts",
        "deps/acme-labs/money/tokens.binding.ts",
        "auth/sessions.ts",
    ] {
        assert!(out_dir.join(f).exists(), "missing {f}");
    }
    // A dependency's own unit is never emitted when nothing reaches it.
    assert!(!out_dir.join("deps/acme-labs/money/currency.ts").exists());
    let compose = std::fs::read_to_string(out_dir.join("compose.ts")).unwrap();
    assert!(
        compose.contains("from \"./deps/acme-labs/money/tokens.binding.js\""),
        "{compose}"
    );
}

#[test]
fn the_diamond_is_one_package() {
    let root = scratch("diamond");
    let shop = money_workspace(
        &root,
        "[project]\nname = \"shop\"\n\n[dependencies]\nmoney = { package = \"acme-labs/money\", path = \"../../packages/money\" }\nledger = { package = \"acme/ledger\", path = \"../../packages/ledger\" }\n",
    );
    write(
        &root.join("packages/ledger/bynk.toml"),
        "[package]\nname = \"acme/ledger\"\n\n[dependencies]\nm = { package = \"acme-labs/money\", path = \"../money\" }\n",
    );
    write(
        &root.join("packages/ledger/src/entries.bynk"),
        "commons entries\n\nuses m.currency\n\nfn post(n: Int) -> Int {\n  double(n) + 1\n}\n",
    );
    write(
        &shop.join("src/books.bynk"),
        "commons books\n\nuses money.currency\nuses ledger.entries\n\nfn both(n: Int) -> Int {\n  double(post(n))\n}\n",
    );
    let (code, out) = short(&shop);
    assert_eq!(code, 0, "{out}");
}

#[test]
fn reaching_past_exports_is_refused() {
    let root = scratch("exports");
    let shop = money_workspace(&root, SHOP);
    write(
        &shop.join("src/pricing.bynk"),
        "commons pricing\n\nuses money.rounding\nuses money.nope\n\nfn total(n: Int) -> Int {\n  n\n}\n",
    );
    let (code, out) = short(&shop);
    assert_eq!(code, 1);
    assert!(out.contains(
        "src/pricing.bynk:3:6: error[bynk.package.unit_not_exported]: `rounding` isn't exported by dependency `acme-labs/money`"
    ), "{out}");
    assert!(out.contains(
        "src/pricing.bynk:4:6: error[bynk.package.unknown_unit]: `money.nope` names no commons or adapter in dependency `acme-labs/money` (alias `money`)"
    ), "{out}");
    // Each clause is reported once, not again as an unknown commons.
    assert!(!out.contains("bynk.uses.unknown_commons"), "{out}");
}

#[test]
fn graph_problems_are_refused_with_their_slice() {
    let root = scratch("graph");
    let shop = money_workspace(
        &root,
        &format!(
            "{SHOP}pricing = {{ package = \"acme-labs/money\", path = \"../../packages/money\" }}\nregdep = {{ package = \"acme/y\", version = \"^1\" }}\nwrong = {{ package = \"acme/money\", path = \"../../packages/money\" }}\nmissing = {{ package = \"acme/z\", path = \"../nowhere\" }}\n"
        ),
    );
    write(
        &shop.join("src/pricing.bynk"),
        "commons pricing\n\nfn total(n: Int) -> Int {\n  n\n}\n",
    );
    let (code, out) = short(&shop);
    assert_eq!(code, 1);
    for line in [
        "error[bynk.package.unsupported_source]: this project's dependency `regdep` is a registry dependency, which can't be resolved until packaging slice 4 (#843)",
        "error[bynk.package.name_mismatch]: this project's dependency `wrong` declares `package = \"acme/money\"`, but `../../packages/money` is the package `acme-labs/money`",
        "error[bynk.package.dependency_not_found]: this project's dependency `missing` points at `../nowhere`, which has no readable `bynk.toml`",
        "error[bynk.package.alias_shadows_unit]: the dependency alias `pricing` is also the first segment of one of this project's own units, so `pricing.…` would be ambiguous",
    ] {
        assert!(out.contains(line), "missing {line}\n{out}");
    }
}

#[test]
fn duplicate_sources_and_cycles_are_refused() {
    let root = scratch("cycle");
    let shop = money_workspace(
        &root,
        "[project]\nname = \"shop\"\n\n[dependencies]\nmoney = { package = \"acme-labs/money\", path = \"../../packages/money\" }\ncopy = { package = \"acme-labs/money\", path = \"../../packages/money-copy\" }\na = { package = \"acme/a\", path = \"../../packages/a\" }\n",
    );
    write(
        &root.join("packages/money-copy/bynk.toml"),
        "[package]\nname = \"acme-labs/money\"\n",
    );
    write(
        &root.join("packages/money-copy/src/currency.bynk"),
        "commons currency\n",
    );
    write(
        &root.join("packages/a/bynk.toml"),
        "[package]\nname = \"acme/a\"\n\n[dependencies]\nb = { package = \"acme/b\", path = \"../b\" }\n",
    );
    write(&root.join("packages/a/src/x.bynk"), "commons x\n");
    write(
        &root.join("packages/b/bynk.toml"),
        "[package]\nname = \"acme/b\"\n\n[dependencies]\na = { package = \"acme/a\", path = \"../a\" }\n",
    );
    write(&root.join("packages/b/src/y.bynk"), "commons y\n");
    write(&shop.join("src/books.bynk"), "commons books\n");
    let (code, out) = short(&shop);
    assert_eq!(code, 1);
    assert!(out.contains("error[bynk.package.duplicate_source]: the package `acme-labs/money` is reached from two directories"), "{out}");
    assert!(out.contains("error[bynk.package.dependency_cycle]: the package graph has a cycle: `acme/a` → `acme/b` → `acme/a`"), "{out}");
}

#[test]
fn a_dependency_path_outside_the_workspace_is_refused() {
    let root = scratch("confine");
    let shop = money_workspace(
        &root,
        "[project]\nname = \"shop\"\n\n[dependencies]\nleaky = { package = \"acme/leaky\", path = \"../../packages/leaky\" }\n",
    );
    write(
        &root.join("packages/leaky/bynk.toml"),
        "[package]\nname = \"acme/leaky\"\n\n[dependencies]\nout = { package = \"acme/outside\", path = \"../../../outside\" }\n",
    );
    write(&root.join("packages/leaky/src/l.bynk"), "commons l\n");
    write(&shop.join("src/books.bynk"), "commons books\n");
    let (code, out) = short(&shop);
    assert_eq!(code, 1);
    assert!(out.contains(
        "error[bynk.dependency.path_outside_root]: dependency `acme/leaky` declares the path dependency `out` (`../../../outside`), which is outside the workspace"
    ), "{out}");
}

#[test]
fn what_a_later_slice_delivers_is_refused_in_a_dependency() {
    let root = scratch("later");
    let shop = money_workspace(&root, SHOP);
    write(
        &root.join("packages/money/src/tokens.bynk"),
        "adapter tokens {\n  binding \"./tokens.binding.ts\" requires { \"jose\": \"^5\" }\n\n  exports capability { Jwt }\n\n  capability Jwt {\n    fn sign(sub: String, secret: String) -> Effect[String]\n  }\n\n  provides Jwt = JoseJwt\n}\n",
    );
    write(
        &shop.join("src/auth/sessions.bynk"),
        "context auth.sessions\n\nconsumes money.tokens\n\nservice login {\n  on call(secret: String) -> Effect[String] given money.tokens.Jwt {\n    let token <- money.tokens.Jwt.sign(\"u1\", secret)\n    token\n  }\n}\n",
    );
    let (code, out) = short(&shop);
    assert_eq!(code, 1);
    assert!(out.contains(
        "error[bynk.package.unsupported_in_dependency]: dependency `acme-labs/money`'s adapter requires the npm package `jose`, and npm dependencies of a dependency can't be folded until packaging slice 3 (#843)"
    ), "{out}");
}

#[test]
fn an_unused_dependency_changes_nothing_emitted() {
    let bynkc = bynk().parent().unwrap().join("bynkc");
    if !bynkc.exists() {
        return;
    }
    let root = scratch("unused");
    let shop = money_workspace(&root, SHOP);
    write(
        &shop.join("src/books.bynk"),
        "commons books\n\nfn twice(n: Int) -> Int {\n  n + n\n}\n",
    );
    let compile = |dir: &Path, out: &Path| {
        let status = Command::new(&bynkc)
            .args(["compile", ".", "-o"])
            .arg(out)
            .current_dir(dir)
            .stdout(Stdio::null())
            .status()
            .expect("spawn bynkc");
        assert!(status.success());
    };
    let with = root.join("out-with");
    compile(&shop, &with);
    write(&shop.join("bynk.toml"), "[project]\nname = \"shop\"\n");
    let without = root.join("out-without");
    compile(&shop, &without);
    let files = |dir: &Path| {
        let mut v: Vec<(PathBuf, Vec<u8>)> = walk(dir)
            .into_iter()
            .map(|p| {
                (
                    p.strip_prefix(dir).unwrap().to_path_buf(),
                    std::fs::read(&p).unwrap(),
                )
            })
            .collect();
        v.sort();
        v
    };
    assert!(
        files(&with) == files(&without),
        "an unused dependency changed the output"
    );
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let p = entry.unwrap().path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else {
            out.push(p);
        }
    }
    out
}

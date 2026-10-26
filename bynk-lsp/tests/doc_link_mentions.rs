//! #2456 — the reverse doc-link lookup (`index_queries::doc_link_mentions`)
//! and rename's use of it, over real multi-file projects on disk. Like
//! `rename_validation.rs`, no transport: this runs exactly what the `rename`
//! handler composes.

use bynk_check::index::SiteRef;
use bynk_lsp::index_queries::{self, DocLinkMention};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub const UTIL: &str = "\
commons demo.util

---
A price. See [Money], [Money.cents], [the cost][Money] and [`helper`]; also [helper].
Not [Other] and not [Money](https://example.com).
---
type Money = {
  cents: Int,
}

---
Builds a [Money]. See [helper] and [Money.cents].
---
fn helper(x: Int) -> Int {
  x
}
";

pub const APP: &str = "\
context demo.app

uses demo.util

---
Charges a [Money] via [helper], reading [`Money.cents`].
---
fn fee(m: Money) -> Int {
  helper(2)
}
";

/// Declares its own `Money`/`helper` and does not `uses` `demo.util`, so its
/// same-named links resolve to its own definitions, never to `demo.util`'s.
pub const ELSEWHERE: &str = "\
commons demo.elsewhere

---
Unrelated [Money], [Money.cents] and [helper].
---
type Money = {
  cents: Int,
}

---
Local [helper].
---
fn helper(x: Int) -> Int {
  x
}
";

pub fn setup_project(test_name: &str, files: &[(&str, &str)]) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "bynk-lsp-doclink-{}-{}",
        test_name,
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("create test root");
    for (rel, contents) in files {
        let p = root.join(rel);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(&p, contents).expect("write file");
    }
    root.canonicalize().unwrap_or(root)
}

pub struct Analysed {
    pub index: bynk_check::index::ProjectIndex,
    pub doc_scope: HashMap<String, Vec<String>>,
    pub snapshots: HashMap<PathBuf, String>,
}

pub fn analyse(root: &Path) -> Analysed {
    let sources = bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(
        root.to_path_buf(),
    ));
    let result = bynk_ide::diagnose_project(root, &sources);
    Analysed {
        index: result.index,
        doc_scope: result.doc_scope,
        snapshots: result
            .files
            .iter()
            .map(|f| (f.source_path.clone(), f.text.clone()))
            .collect(),
    }
}

fn fixture(name: &str) -> (PathBuf, Analysed) {
    let root = setup_project(
        name,
        &[
            ("demo/util.bynk", UTIL),
            ("demo/app.bynk", APP),
            ("demo/elsewhere.bynk", ELSEWHERE),
        ],
    );
    let a = analyse(&root);
    (root, a)
}

fn def_of<'a>(a: &'a Analysed, file: &str, name: &str, kind_is_fn: bool) -> &'a SiteRef {
    a.index
        .symbols
        .iter()
        .find(|(k, _)| {
            k.name == name
                && k.unit == file
                && matches!(k.kind, bynk_check::index::SymbolKind::Fn) == kind_is_fn
        })
        .and_then(|(_, e)| e.def.as_ref())
        .unwrap_or_else(|| panic!("{name} defined"))
}

fn texts<'a>(a: &'a Analysed, ms: &[DocLinkMention]) -> Vec<(String, &'a str)> {
    ms.iter()
        .map(|m| {
            (
                m.path.to_string_lossy().replace('\\', "/"),
                &a.snapshots[&m.path][m.name_span.start..m.name_span.end],
            )
        })
        .collect()
}

#[test]
fn mentions_of_a_type_cover_every_unit_that_resolves_to_it() {
    let (_root, a) = fixture("type");
    let money = def_of(&a, "demo.util", "Money", false);
    let ms = index_queries::doc_link_mentions(&a.index, &a.doc_scope, &a.snapshots, money);
    let got = texts(&a, &ms);
    // util: [Money], [the cost][Money], [Money] (fn doc); app: [Money].
    // Not the dotted members, `[Other]`, the URL link, or the other unit's own.
    let files: Vec<&str> = got.iter().map(|(f, _)| f.as_str()).collect();
    assert_eq!(
        files,
        [
            "demo/app.bynk",
            "demo/util.bynk",
            "demo/util.bynk",
            "demo/util.bynk"
        ]
    );
    assert!(got.iter().all(|(_, t)| *t == "Money"), "{got:?}");
}

#[test]
fn mentions_of_a_member_carry_the_dotted_name() {
    let (_root, a) = fixture("member");
    let cents = def_of(&a, "demo.util", "Money.cents", false);
    let ms = index_queries::doc_link_mentions(&a.index, &a.doc_scope, &a.snapshots, cents);
    let got = texts(&a, &ms);
    assert_eq!(got.len(), 3, "{got:?}");
    assert!(got.iter().all(|(_, t)| *t == "Money.cents"), "{got:?}");
    // The code-span form: name_span excludes the backticks.
    let app = ms.iter().find(|m| m.path.ends_with("app.bynk")).unwrap();
    let text = &a.snapshots[&app.path];
    assert_eq!(&text[app.span.start..app.span.end], "[`Money.cents`]");
}

#[test]
fn a_same_named_link_that_resolves_elsewhere_is_not_a_mention() {
    let (_root, a) = fixture("elsewhere");
    let helper = def_of(&a, "demo.util", "helper", true);
    let ms = index_queries::doc_link_mentions(&a.index, &a.doc_scope, &a.snapshots, helper);
    assert!(ms.iter().all(|m| !m.path.ends_with("elsewhere.bynk")));
    assert_eq!(ms.len(), 4, "{:?}", texts(&a, &ms));
}

/// The `rename` handler's pipeline, minus transport: plan (with doc-link
/// edits) → apply → re-analyse → the doc-link validator.
fn try_rename(
    root: &Path,
    pre: &Analysed,
    at: (&str, usize),
    new_name: &str,
) -> Result<HashMap<PathBuf, String>, String> {
    let mut plan = index_queries::plan_rename(&pre.index, Path::new(at.0), at.1, new_name)?;
    index_queries::add_doc_link_edits(&mut plan, &pre.index, &pre.doc_scope, &pre.snapshots);
    let edited: HashMap<PathBuf, String> = pre
        .snapshots
        .iter()
        .map(|(p, t)| {
            let t = match plan.edits.get(p) {
                Some(spans) => index_queries::apply_edits(t, spans, new_name),
                None => t.clone(),
            };
            (p.clone(), t)
        })
        .collect();
    let overlay: HashMap<PathBuf, String> = edited
        .iter()
        .map(|(p, t)| (root.join(p), t.clone()))
        .collect();
    let post = bynk_ide::diagnose_project(root, &overlay);
    assert!(
        index_queries::index_unchanged_modulo_rename(&pre.index, &post.index, &plan),
        "index changed beyond the rename"
    );
    index_queries::doc_links_rebound(
        (&pre.index, &pre.doc_scope),
        (&post.index, &post.doc_scope),
        &pre.snapshots,
        &edited,
        &plan,
    )?;
    Ok(edited)
}

fn at(a: &Analysed, file: &str, needle: &str) -> usize {
    a.snapshots[Path::new(file)].find(needle).expect(needle)
}

fn ctx(a: &Analysed, f: &str) -> String {
    a.snapshots[Path::new(f)].clone()
}

#[test]
fn renaming_a_type_rewrites_its_links_and_its_members_owner() {
    let (root, a) = fixture("rn-type");
    let off = at(&a, "demo/util.bynk", "type Money") + 5;
    let out = try_rename(&root, &a, ("demo/util.bynk", off), "Cash").expect("rename ok");
    let util = &out[Path::new("demo/util.bynk")];
    assert!(util.contains("See [Cash], [Cash.cents], [the cost][Cash] and [`helper`]"));
    assert!(util.contains("Builds a [Cash]. See [helper] and [Cash.cents]."));
    assert!(util.contains("Not [Other] and not [Money](https://example.com)."));
    assert!(util.contains("type Cash = {"));
    let app = &out[Path::new("demo/app.bynk")];
    assert!(app.contains("Charges a [Cash] via [helper], reading [`Cash.cents`]."));
    // Same-named links that resolve elsewhere are untouched.
    assert_eq!(
        out[Path::new("demo/elsewhere.bynk")],
        ctx(&a, "demo/elsewhere.bynk")
    );
}

#[test]
fn renaming_a_member_rewrites_only_the_member_segment() {
    let (root, a) = fixture("rn-member");
    let off = at(&a, "demo/util.bynk", "cents: Int");
    let out = try_rename(&root, &a, ("demo/util.bynk", off), "pence").expect("rename ok");
    let util = &out[Path::new("demo/util.bynk")];
    assert!(util.contains("[Money], [Money.pence], [the cost][Money]"));
    assert!(util.contains("See [helper] and [Money.pence]."));
    assert!(out[Path::new("demo/app.bynk")].contains("[`Money.pence`]"));
    assert_eq!(
        out[Path::new("demo/elsewhere.bynk")],
        ctx(&a, "demo/elsewhere.bynk")
    );
}

#[test]
fn renaming_a_fn_rewrites_plain_and_code_span_links() {
    let (root, a) = fixture("rn-fn");
    let off = at(&a, "demo/util.bynk", "fn helper") + 3;
    let out = try_rename(&root, &a, ("demo/util.bynk", off), "assist").expect("rename ok");
    let util = &out[Path::new("demo/util.bynk")];
    assert!(util.contains("and [`assist`]; also [assist]."));
    assert!(util.contains("See [assist] and [Money.cents]."));
    assert!(out[Path::new("demo/app.bynk")].contains("via [assist],"));
    assert_eq!(
        out[Path::new("demo/elsewhere.bynk")],
        ctx(&a, "demo/elsewhere.bynk")
    );
}

#[test]
fn a_rename_that_rebinds_a_doc_link_is_refused() {
    // `demo.b` uses `demo.a` then `demo.c`; its `[Throttle]` resolves to
    // `demo.c`'s type. Renaming `demo.a`'s `Limiter` to `Throttle` would make
    // the first unit in scope order win: the link silently re-binds.
    let a_src = "commons demo.a\n\ntype Limiter = Int\n";
    let c_src = "commons demo.c\n\ntype Throttle = Int\n";
    let b_src = "context demo.b\n\nuses demo.a\nuses demo.c\n\n---\nSee [Throttle].\n---\nfn f(x: Int) -> Int {\n  x\n}\n";
    let root = setup_project(
        "rn-rebind",
        &[
            ("demo/a.bynk", a_src),
            ("demo/b.bynk", b_src),
            ("demo/c.bynk", c_src),
        ],
    );
    let pre = analyse(&root);
    let off = at(&pre, "demo/a.bynk", "Limiter");
    let err = try_rename(&root, &pre, ("demo/a.bynk", off), "Throttle")
        .expect_err("the re-binding must refuse");
    assert!(err.contains("doc link `[Throttle]`"), "{err}");
}

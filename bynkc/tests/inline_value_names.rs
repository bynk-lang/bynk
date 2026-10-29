//! #2476: the names `textDocument/inlineValue` hands the debug adapter are the
//! variables the emitter really declares. The language server maps a Bynk
//! binding to its emitted name through `bynk_check::emitted_names`; this
//! compiles the same source and checks that mapping against the TypeScript,
//! for a plain function and for a `suite` case (whose scaffold binds names of
//! its own that the server cannot see).

use bynk_ide::locals_nav::{inline_value_sites, is_suite_file};
use bynk_syntax::span::Span;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

const FILE: &str = "m.bynk";

/// A path with `/` separators, so the suffix matches below hold on Windows.
fn unix(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// `(text, locals, project symbol names)` of `file` after a real analysis of
/// the project rooted at `root`.
fn analyse(
    root: &Path,
    roots: bynk_ide::AnalysisRoots,
    file: &str,
) -> (
    String,
    Vec<bynk_check::locals::LocalBinding>,
    HashSet<String>,
) {
    let result = bynk_ide::diagnose_project(root, &bynk_testkit::read_project_sources(&roots));
    let text = result
        .files
        .iter()
        .find(|f| unix(&f.source_path).ends_with(file))
        .expect("analysed")
        .text
        .clone();
    let locals = result
        .locals
        .iter()
        .find(|(p, _)| unix(p).ends_with(file))
        .map(|(_, l)| l.clone())
        .unwrap_or_else(|| {
            panic!(
                "locals for {file}; have {:?}",
                result.locals.keys().collect::<Vec<_>>()
            )
        });
    let names = result
        .index
        .symbols
        .keys()
        .map(|k| k.name.clone())
        .collect();
    (text, locals, names)
}

fn emitted_ts(src: &str) -> String {
    let out = bynk_emit::project::compile_in_memory(
        src,
        bynk_emit::project::BuildTarget::Bundle,
        Default::default(),
    )
    .unwrap_or_else(|_| panic!("fixture should compile:\n{src}"));
    out.artefacts
        .docs
        .iter()
        .find(|(path, _)| path.ends_with("inline.ts"))
        .map(|(_, doc)| doc.text())
        .expect("the fixture's own module is emitted")
}

#[test]
fn inline_value_names_are_the_emitted_declarations() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/inline_values/src");
    let (text, locals, names) = analyse(
        &root,
        bynk_ide::AnalysisRoots::SingleTree(root.clone()),
        FILE,
    );
    assert!(!is_suite_file(&text));

    let stop = text.rfind("total").unwrap();
    let sites = inline_value_sites(
        &locals,
        Span::new(0, text.len()),
        Span::new(stop, stop),
        None,
    );
    let seen: Vec<(&str, &str)> = sites
        .iter()
        .map(|s| (&text[s.range.start..s.range.end], s.variable.as_str()))
        .collect();
    assert_eq!(
        seen,
        vec![
            ("class", "__id_class"),
            ("n", "n"), // the param; `let n = n + 1` re-binds it, so has no name
            ("doubled", "doubled"),
            ("total", "total"), // the first; the re-let has no knowable name
        ]
    );
    assert!(
        names.contains("calc"),
        "the index lists the project's items"
    );

    let ts = emitted_ts(&text);
    let params = ts
        .split("function calc(")
        .nth(1)
        .and_then(|rest| rest.split(')').next())
        .unwrap_or_else(|| panic!("`calc` is emitted:\n{ts}"));
    for (bynk, emitted) in &seen {
        let declared = if *bynk == "class" || *bynk == "n" {
            params.contains(&format!("{emitted}:"))
        } else {
            ts.contains(&format!("const {emitted} = "))
        };
        assert!(declared, "`{bynk}` → `{emitted}` is not declared in:\n{ts}");
    }
    // Both re-lets (`n`, `total`) are emitted under throwaway names the source
    // cannot predict.
    assert_eq!(
        ts.matches("const __r").count(),
        2,
        "the two re-lets are renamed:\n{ts}"
    );
}

fn suite_project(tag: &str) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("inline-values-{tag}"));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src/shop")).unwrap();
    std::fs::create_dir_all(root.join("tests")).unwrap();
    std::fs::write(
        root.join("bynk.toml"),
        "[project]\nname = \"m\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/shop/maths.bynk"),
        "commons shop.maths\n\nfn total(n: Int) -> Int { n + n }\n",
    )
    .unwrap();
    std::fs::write(
        root.join("tests/maths.bynk"),
        "suite shop.maths {\n  case \"c\" {\n    let total = total(3)\n    let other = total + 1\n    expect other == 7\n  }\n  property \"p\" {\n    for all n: Int {\n      let twice = n + n\n      expect twice == n * 2\n    }\n  }\n}\n",
    )
    .unwrap();
    root
}

/// The test scaffold destructures the unit's `total` function into the case's
/// scope, so the case's `let total` is emitted as a throwaway and a lookup of
/// `total` would show the function. The server cannot see the scaffold, so it
/// leaves out any suite `let` named like a project item.
#[test]
fn a_suite_let_named_like_a_scaffold_binding_is_left_out() {
    let root = suite_project("scaffold");
    let out = Command::new(env!("CARGO_BIN_EXE_bynkc"))
        .args(["test", "--no-run", "."])
        .current_dir(&root)
        .env("NO_COLOR", "1")
        .output()
        .expect("bynkc runs");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ts = std::fs::read_to_string(root.join("out/tests/shop_maths.test.ts")).unwrap();
    assert!(
        ts.contains("const { total } = "),
        "scaffold binds `total`:\n{ts}"
    );
    assert!(
        ts.contains("const __r0 = total(3);"),
        "the let is renamed:\n{ts}"
    );

    // A case's locals are recorded (#2517), so a test gets inline values; the
    // guard must keep `total` out, since a lookup of it shows the function.
    let result = bynk_ide::diagnose_project(
        &root,
        &bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::Project(root.clone())),
    );
    let suite_locals = result
        .locals
        .iter()
        .find(|(p, _)| unix(p).ends_with("tests/maths.bynk"))
        .map(|(_, l)| l.clone())
        .unwrap_or_default();
    let (text, _, names) = analyse(
        &root,
        bynk_ide::AnalysisRoots::Project(root.clone()),
        "src/shop/maths.bynk",
    );
    assert!(names.contains("total"), "the project declares `total`");
    let suite_text = std::fs::read_to_string(root.join("tests/maths.bynk")).unwrap();
    assert!(is_suite_file(&suite_text));
    assert!(!is_suite_file(&text));
    let stop = suite_text.find("expect").unwrap();
    let sites = inline_value_sites(
        &suite_locals,
        Span::new(0, suite_text.len()),
        Span::new(stop, stop),
        Some(&names),
    );
    let seen: Vec<&str> = sites
        .iter()
        .map(|s| &suite_text[s.range.start..s.range.end])
        .collect();
    assert!(
        !seen.contains(&"total"),
        "`total` would show the destructured function, not the let: {seen:?}"
    );
    assert_eq!(seen, vec!["other"], "the case's other let is offered");

    // A `property` body's `let` is recorded too; its `for all` binder `n`
    // enters through the initial scope, so it gets no inline value.
    let stop = suite_text.rfind("expect").unwrap();
    let sites = inline_value_sites(
        &suite_locals,
        Span::new(0, suite_text.len()),
        Span::new(stop, stop),
        Some(&names),
    );
    let seen: Vec<&str> = sites
        .iter()
        .map(|s| &suite_text[s.range.start..s.range.end])
        .collect();
    assert_eq!(seen, vec!["twice"], "the property's let, not its binder");
    assert!(
        ts.contains("const other = "),
        "`other` is declared under its own name:\n{ts}"
    );
}

//! `bynk new` — the deterministic surface (proposal §5).
//!
//! Three things are pinned here:
//!  1. **The output messages** — the success "next steps" and the failure
//!     messages — as goldens, the way `dev`'s output is goldened.
//!  2. **The emitted scaffold** — the rendered `bynk.toml`, `.gitignore`, and
//!     starter `.bynk` — as a golden tree, so an accidental template edit is
//!     caught.
//!  3. **The starter actually builds** — rendered with a *non-default* name, it
//!     compiles (so the scaffold can't rot) and is `bynk-fmt`-clean (the
//!     ADR 0086 first-party-source guarantee, applied to the template).
//!
//! Goldens are blessed with `BYNK_BLESS=1 cargo test -p bynk`.

use std::fs;
use std::path::{Path, PathBuf};

use bynk::new::{
    self, cannot_derive_message, clobber_message, invalid_name_message, next_steps_message,
};

fn bless_or_assert(name: &str, actual: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    if std::env::var_os("BYNK_BLESS").is_some() {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, actual).unwrap();
        return;
    }
    let expected = fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "missing golden {}; regenerate with BYNK_BLESS=1 cargo test -p bynk",
            path.display()
        )
    });
    assert_eq!(
        actual, expected,
        "golden {name} drifted; re-bless with BYNK_BLESS=1 cargo test -p bynk"
    );
}

// ---------------------------------------------------------------------------
// Output surface
// ---------------------------------------------------------------------------

#[test]
fn golden_next_steps() {
    bless_or_assert("new-next-steps.txt", &next_steps_message("hello"));
}

#[test]
fn golden_errors() {
    // The three ways `new` refuses, each prefixed `bynk:` and touching nothing.
    let mut out = String::new();
    out.push_str(&invalid_name_message("my-app"));
    out.push_str(&clobber_message("hello"));
    out.push_str(&cannot_derive_message("."));
    bless_or_assert("new-errors.txt", &out);
}

#[test]
fn golden_scaffold_tree() {
    // The emitted scaffold, rendered for `hello`, as one pinned tree.
    let mut out = String::new();
    for (label, body) in [
        (
            "bynk.toml",
            new::render(include_str!("../src/templates/bynk.toml"), "hello"),
        ),
        (
            ".gitignore",
            new::render(include_str!("../src/templates/gitignore"), "hello"),
        ),
        (
            ".gitattributes",
            new::render(include_str!("../src/templates/gitattributes"), "hello"),
        ),
        ("src/hello.bynk", new::starter_source("hello")),
    ] {
        out.push_str(&format!("--- {label} ---\n{body}"));
        if !body.ends_with('\n') {
            out.push('\n');
        }
    }
    bless_or_assert("new-scaffold.txt", &out);
}

// ---------------------------------------------------------------------------
// Name validation — reuses the real lexer, so this table is the contract
// ---------------------------------------------------------------------------

#[test]
fn name_validation() {
    let legal = ["hello", "hello_world", "Hello", "h2", "x", "todo"];
    let illegal = [
        "my-app",    // dash
        "2048",      // leading digit
        "hello.web", // dotted — not a single segment
        "",          // empty
        "has space", // whitespace
        "service",   // reserved keyword
        "from",      // reserved keyword
        "café",      // non-ASCII (the lexer's Ident is ASCII-only)
    ];
    for n in legal {
        assert!(new::is_legal_name(n), "`{n}` should be a legal Bynk name");
    }
    for n in illegal {
        assert!(!new::is_legal_name(n), "`{n}` should be rejected");
    }
}

// ---------------------------------------------------------------------------
// The starter builds — rendered with a non-default name (so substitution is
// exercised), it must compile and be fmt-clean.
// ---------------------------------------------------------------------------

#[test]
fn starter_renders_compiles_and_is_fmt_clean() {
    let name = "demo";
    let rendered = new::starter_source(name);

    // Substitution is complete: the placeholder is gone, the name is in.
    assert!(
        !rendered.contains(new::PLACEHOLDER),
        "placeholder `{}` survived rendering:\n{rendered}",
        new::PLACEHOLDER
    );
    assert!(
        rendered.contains(&format!("context {name}")),
        "rendered starter should declare `context {name}`:\n{rendered}"
    );

    // Fmt-clean: formatting the rendered source is a no-op (ADR 0086).
    let formatted = bynk_fmt::format_source(&rendered, &bynk_fmt::FormatOptions::default())
        .expect("rendered starter must format");
    assert_eq!(
        formatted, rendered,
        "rendered starter is not bynk-fmt-clean; re-format bynk/src/templates/starter.bynk"
    );

    // Compiles: written as `<root>/<name>.bynk`, it builds as a one-file project
    // (the same in-process path `tests/doc_examples.rs` uses for `context`
    // blocks). This is the guard against starter rot.
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("new-starter-compile");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join(format!("{name}.bynk")), &rendered).unwrap();

    let result =
        bynk_driver::project::compile_project(&bynk_testkit::compile_options_single(root.clone()))
            .map(|_| ())
            .map_err(|f| {
                bynk_driver::project::ProjectFailure::flatten(f)
                    .iter()
                    .map(|e| format!("{}: {}", e.category, e.message))
                    .collect::<Vec<_>>()
                    .join("; ")
            });
    let _ = fs::remove_dir_all(&root);
    result.expect("the scaffolded starter must compile");
}

/// #521: the scaffolded `bynk.toml`'s `[paths]` keys must be keys the tools
/// actually read. The old template wrote `src = "src"` / `tests = "tests"`,
/// which the reader silently ignored (the project only worked via the
/// conventional fallback). Render the template into a scaffold-shaped tree
/// and assert the reader consumes the declared layout.
#[test]
fn scaffolded_manifest_paths_are_read_by_the_tools() {
    let dir = std::env::temp_dir().join(format!("bynk-new-manifest-{}", std::process::id()));
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::create_dir_all(dir.join("tests")).unwrap();
    let manifest = new::render(include_str!("../src/templates/bynk.toml"), "demo");
    fs::write(dir.join("bynk.toml"), manifest).unwrap();
    let paths = bynk_driver::project::try_read_project_paths(&dir)
        .expect("the scaffolded manifest must be well-formed TOML");
    assert_eq!(
        paths.include,
        vec![PathBuf::from("src"), PathBuf::from("tests")],
        "the template's [paths] include must round-trip through try_read_project_paths"
    );
    assert!(paths.exclude.is_empty());
    fs::remove_dir_all(&dir).ok();
}

/// #737: a hand-written `.gitignore` passes the D5 clobber check (it's in
/// `SCAFFOLD_IGNORES`), so the scaffold must **not** replace it with the
/// template — `write_scaffold`'s own doc promises it never overwrites. Scaffold
/// into a target that holds only `.git/` and a user `.gitignore`, then assert
/// the user's `.gitignore` survives while the rest of the tree is written.
#[test]
fn scaffold_preserves_a_hand_written_gitignore() {
    use bynk::new::{NewOptions, run};

    let dir = std::env::temp_dir().join(format!("bynk-new-gitignore-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join(".git")).unwrap();
    let user_gitignore = "# my rules\n/secrets\n";
    fs::write(dir.join(".gitignore"), user_gitignore).unwrap();

    run(&NewOptions {
        path: dir.clone(),
        name: Some("demo".to_string()),
    });

    // The scaffold ran (didn't trip the clobber check on the tolerated cruft)
    // — proven by the tree it writes...
    assert!(
        dir.join("bynk.toml").exists() && dir.join("src/demo.bynk").exists(),
        "a `.git`/`.gitignore`-only target should scaffold, not trip the clobber check"
    );
    // ...yet the user's hand-written `.gitignore` is preserved, not clobbered.
    assert_eq!(
        fs::read_to_string(dir.join(".gitignore")).unwrap(),
        user_gitignore,
        "the user's hand-written `.gitignore` must be preserved, not clobbered"
    );

    fs::remove_dir_all(&dir).ok();
}

/// #1763: the same two arms for `.gitattributes`: a user's own survives, and a
/// target without one gets the template's `*.bynk text eol=lf`.
#[test]
fn scaffold_writes_gitattributes_but_never_over_the_users() {
    use bynk::new::{NewOptions, run};

    let dir = std::env::temp_dir().join(format!("bynk-new-gitattributes-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join(".git")).unwrap();
    let user = "* text=auto\n";
    fs::write(dir.join(".gitattributes"), user).unwrap();
    run(&NewOptions {
        path: dir.clone(),
        name: Some("demo".to_string()),
    });
    assert!(
        dir.join("bynk.toml").exists(),
        "a `.gitattributes` is tolerated cruft"
    );
    assert_eq!(
        fs::read_to_string(dir.join(".gitattributes")).unwrap(),
        user
    );
    fs::remove_dir_all(&dir).ok();

    fs::create_dir_all(&dir).unwrap();
    run(&NewOptions {
        path: dir.clone(),
        name: Some("demo".to_string()),
    });
    assert!(
        fs::read_to_string(dir.join(".gitattributes"))
            .unwrap()
            .contains("*.bynk text eol=lf")
    );
    fs::remove_dir_all(&dir).ok();
}

/// #737 (the other branch): when the target has **no** `.gitignore`, the
/// scaffold must still write the template one. Paired with
/// `scaffold_preserves_a_hand_written_gitignore`, this pins both arms of the
/// `if !gitignore.exists()` guard end-to-end (through `run`, not just the
/// template-rendering golden) — so an accidental inversion of the condition is
/// caught by one of the two.
#[test]
fn scaffold_writes_the_template_gitignore_when_absent() {
    use bynk::new::{NewOptions, render, run};

    let dir = std::env::temp_dir().join(format!("bynk-new-gitignore-fresh-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join(".git")).unwrap();

    run(&NewOptions {
        path: dir.clone(),
        name: Some("demo".to_string()),
    });

    assert_eq!(
        fs::read_to_string(dir.join(".gitignore")).unwrap(),
        render(include_str!("../src/templates/gitignore"), "demo"),
        "with no `.gitignore` present, the scaffold must write the template one"
    );

    fs::remove_dir_all(&dir).ok();
}

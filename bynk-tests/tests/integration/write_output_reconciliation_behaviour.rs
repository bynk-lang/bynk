//! Regression: `write_output` never enumerated or pruned its destination, so
//! deleting a `.bynk` unit left its previously emitted `.ts` on disk — still
//! covered by the emitted `tsconfig.json`'s `include: **/*.ts`, so a later
//! `tsc` run fails against a module the project no longer has, reported as
//! "tsc reported errors against out/tsconfig.json" with no hint that the real
//! cause is a stale build artefact.
//!
//! Builds a real two-unit project on disk, compiles it, then deletes one unit
//! and recompiles into the *same* output directory — `write_output` must
//! remove the first unit's stale `.ts` (and, separately, must not touch a
//! same-named file the pruning logic is deliberately narrow about).

use std::path::PathBuf;

fn write(path: &std::path::Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

#[test]
fn deleting_a_unit_prunes_its_stale_emitted_output() {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("write-output-reconcile-src");
    let out_dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("write-output-reconcile-out");
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&out_dir);

    write(
        &root.join("src/keep.bynk"),
        "commons keep\n\nfn f() -> Int {\n  1\n}\n",
    );
    write(
        &root.join("src/gone.bynk"),
        "commons gone\n\nfn g() -> Int {\n  2\n}\n",
    );

    let compile = || {
        let paths = bynkc::try_read_project_paths(&root).expect("well-formed fixture manifest");
        bynkc::compile_project(&bynk_testkit::compile_options_split(root.clone(), paths))
            .unwrap_or_else(|f| {
                panic!(
                    "compile failed: {:?}",
                    f.errors
                        .iter()
                        .map(|e| (&e.source_path, e.error.category, &e.error.message))
                        .collect::<Vec<_>>()
                )
            })
    };

    let out = compile();
    bynkc::write_output(&out, &out_dir).expect("write_output (first build)");
    assert!(
        out_dir.join("keep.ts").is_file(),
        "keep.ts should be written"
    );
    assert!(
        out_dir.join("gone.ts").is_file(),
        "gone.ts should be written"
    );

    // A file the compiler did not write, of a kind it's allowed to prune —
    // must survive, because it IS in the expected set (part of every build).
    assert!(
        out_dir.join("tsconfig.json").is_file(),
        "tsconfig.json should be part of every build's output"
    );

    std::fs::remove_file(root.join("src/gone.bynk")).unwrap();
    let out = compile();
    bynkc::write_output(&out, &out_dir).expect("write_output (second build)");

    assert!(
        !out_dir.join("gone.ts").exists(),
        "gone.ts should have been pruned after gone.bynk was deleted"
    );
    assert!(
        out_dir.join("keep.ts").is_file(),
        "keep.ts must still be present — pruning must not be over-broad"
    );
    assert!(
        out_dir.join("tsconfig.json").is_file(),
        "tsconfig.json is still part of the (second) build's output"
    );
}

#[test]
fn a_node_modules_subtree_is_never_descended_into() {
    // A defensive test for the exclusion itself, independent of the fixture
    // above: a `.json` file under `node_modules/` must survive reconciliation
    // even though `.json` is one of the prunable extensions, and even though
    // it is not part of any `ProjectOutput`.
    let out_dir =
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("write-output-reconcile-node-modules");
    let _ = std::fs::remove_dir_all(&out_dir);
    write(
        &out_dir.join("node_modules/some-pkg/package.json"),
        "{\"name\":\"some-pkg\"}",
    );

    let root =
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("write-output-reconcile-node-modules-src");
    let _ = std::fs::remove_dir_all(&root);
    write(
        &root.join("src/only.bynk"),
        "commons only\n\nfn f() -> Int {\n  1\n}\n",
    );
    let paths = bynkc::try_read_project_paths(&root).expect("well-formed fixture manifest");
    let out = bynkc::compile_project(&bynk_testkit::compile_options_split(root, paths))
        .unwrap_or_else(|f| {
            panic!(
                "compile failed: {:?}",
                f.errors
                    .iter()
                    .map(|e| (&e.source_path, e.error.category, &e.error.message))
                    .collect::<Vec<_>>()
            )
        });
    bynkc::write_output(&out, &out_dir).expect("write_output");

    assert!(
        out_dir.join("node_modules/some-pkg/package.json").is_file(),
        "a file under node_modules must never be pruned"
    );
}

#[test]
fn a_workers_assets_dir_is_written_byte_for_byte_and_pruned_regardless_of_extension() {
    // #1883, slice 0: `Artefacts.assets` reaches disk unchanged, and a later
    // build that no longer has an asset removes it — even though `.css`,
    // `.png` and the extensionless `_headers` are not among the extensions
    // reconciliation prunes elsewhere. A same-extension file *outside* the
    // assets directory is not the compiler's, so it must survive. The assets
    // are added to a real build's output by hand, so this tests the writer
    // alone, whatever produced them.
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("write-output-assets-src");
    let out_dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("write-output-assets-out");
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&out_dir);
    write(
        &root.join("src/only.bynk"),
        "commons only\n\nfn f() -> Int {\n  1\n}\n",
    );
    let compile = || {
        let paths = bynkc::try_read_project_paths(&root).expect("well-formed fixture manifest");
        bynkc::compile_project(&bynk_testkit::compile_options_split(root.clone(), paths))
            .unwrap_or_else(|f| {
                panic!(
                    "compile failed: {:?}",
                    f.errors
                        .iter()
                        .map(|e| (&e.source_path, e.error.category, &e.error.message))
                        .collect::<Vec<_>>()
                )
            })
    };

    let assets = bynk_emit::project::worker_assets_dir("only");
    let css = assets.join("_assets/board.3f9c2a.css");
    let png = assets.join("_assets/icon.9b1d4e.png");
    let headers = assets.join("_headers");
    // Review of #1902: a dot-directory inside the assets dir is still the
    // compiler's, so reconciliation's `.git`/`node_modules` skip must not
    // stop it pruning there.
    let well_known = assets.join(".well-known/security.txt");
    // Not valid UTF-8: the path must be binary-safe end to end.
    let png_bytes: Vec<u8> = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0xff, 0x00];

    let mut out = compile();
    out.artefacts.assets.insert(css.clone(), b"body{}".to_vec());
    out.artefacts.assets.insert(png.clone(), png_bytes.clone());
    out.artefacts
        .assets
        .insert(headers.clone(), b"/_assets/*\n".to_vec());
    out.artefacts
        .assets
        .insert(well_known.clone(), b"Contact: x\n".to_vec());
    bynkc::write_output(&out, &out_dir).expect("write_output (with assets)");
    assert_eq!(std::fs::read(out_dir.join(&css)).unwrap(), b"body{}");
    assert_eq!(std::fs::read(out_dir.join(&png)).unwrap(), png_bytes);
    assert!(
        out_dir.join(&headers).is_file(),
        "_headers should be written"
    );
    assert!(
        out_dir.join(&well_known).is_file(),
        "a dot-dir asset is written"
    );
    // Review of #1902: a symlink inside the assets dir is not a regular
    // file, but the compiler still owns it.
    #[cfg(unix)]
    let stale_link = {
        let l = out_dir.join(&assets).join("stale-link.css");
        std::os::unix::fs::symlink(out_dir.join(&css), &l).unwrap();
        l
    };

    // A file the compiler did not write, beside (not inside) the assets dir.
    let outside = out_dir.join("workers/only/notes.css");
    write(&outside, "keep me");
    // A hard link to the still-declared asset, outside the output dir: if
    // reconciliation deleted the asset and the write recreated it, the
    // asset becomes a new file and its link count drops back to 1. The end
    // state alone can't show that churn (the write puts the file back).
    #[cfg(unix)]
    let witness = {
        let w = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("write-output-assets-link");
        let _ = std::fs::remove_file(&w);
        std::fs::hard_link(out_dir.join(&css), &w).unwrap();
        w
    };

    let mut out = compile();
    out.artefacts.assets.insert(css.clone(), b"body{}".to_vec());
    bynkc::write_output(&out, &out_dir).expect("write_output (one asset dropped)");
    assert!(out_dir.join(&css).is_file(), "a still-declared asset stays");
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(
            std::fs::metadata(out_dir.join(&css)).unwrap().nlink(),
            2,
            "a still-declared asset is rewritten in place, never pruned first"
        );
        std::fs::remove_file(&witness).unwrap();
    }
    assert!(!out_dir.join(&png).exists(), "a dropped .png is pruned");
    assert!(
        !out_dir.join(&headers).exists(),
        "a dropped _headers is pruned"
    );
    assert!(
        !out_dir.join(&well_known).exists(),
        "a dropped asset under a dot-directory is pruned"
    );
    #[cfg(unix)]
    assert!(
        std::fs::symlink_metadata(&stale_link).is_err(),
        "a stale symlink in the assets dir is pruned"
    );
    assert!(
        outside.is_file(),
        "a file outside the assets dir is not owned"
    );

    let out = compile();
    bynkc::write_output(&out, &out_dir).expect("write_output (no assets)");
    assert!(
        !out_dir.join(&assets).exists(),
        "an assets directory left empty is removed"
    );
    assert!(
        outside.is_file(),
        "a file outside the assets dir is not owned"
    );
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "is outside every Worker's assets directory")]
fn an_asset_outside_a_workers_assets_dir_is_rejected_in_debug_builds() {
    // #1883, slice 0: reconciliation only owns a Worker's assets directory,
    // so an asset written anywhere else would never be pruned. A producer
    // that bypasses `worker_assets_dir` must fail loudly in tests.
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("write-output-stray-asset-src");
    let out_dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("write-output-stray-asset-out");
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&out_dir);
    write(
        &root.join("src/only.bynk"),
        "commons only\n\nfn f() -> Int {\n  1\n}\n",
    );
    let paths = bynkc::try_read_project_paths(&root).expect("well-formed fixture manifest");
    let mut out = bynkc::compile_project(&bynk_testkit::compile_options_split(root, paths))
        .unwrap_or_else(|f| panic!("compile failed: {} error(s)", f.errors.len()));
    out.artefacts
        .assets
        .insert(PathBuf::from("workers/only/board.css"), b"body{}".to_vec());
    let _ = bynkc::write_output(&out, &out_dir);
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "is both a document and a static asset")]
fn a_path_that_is_both_a_document_and_an_asset_is_rejected_in_debug_builds() {
    // Review of #1902: which one reaches disk would otherwise be decided only
    // by the order `write_output` walks its two maps in.
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("write-output-collision-src");
    let out_dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("write-output-collision-out");
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&out_dir);
    write(
        &root.join("src/only.bynk"),
        "commons only\n\nfn f() -> Int {\n  1\n}\n",
    );
    let paths = bynkc::try_read_project_paths(&root).expect("well-formed fixture manifest");
    let mut out = bynkc::compile_project(&bynk_testkit::compile_options_split(root, paths))
        .unwrap_or_else(|f| panic!("compile failed: {} error(s)", f.errors.len()));
    out.artefacts
        .assets
        .insert(PathBuf::from("tsconfig.json"), b"{}".to_vec());
    let _ = bynkc::write_output(&out, &out_dir);
}

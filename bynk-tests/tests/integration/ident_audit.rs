//! The identifier audit (1A.39b, #3930 Decision D; closed by 1A.40c, #3996).
//!
//! For every module the positive corpus emits, on both targets, every `TsExpr::Ident` is an
//! identifier or a dotted identifier chain (`this.state`, `globalThis.Number`) and every
//! `TsBindingName::Ident` is an identifier ([`bynk_ts::text_idents`]). An `Ident` that holds
//! expression text hides the names it uses from `name_closure` and the structure it spells from
//! the printer, whatever built it; the `ts_text_idents` line probe cannot see an `Ident(` fed from
//! a variable or a helper, and this test does. It changes no output.
//!
//! Every module the emitter or the driver builds is covered, the generated test modules
//! (`tests/*.test.ts`) included since 1B.5e. No text class remains, so a find is a failure:
//! convert it to the node it spells.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use bynk_driver::project::{BuildTarget, Document};

#[path = "../par/mod.rs"]
mod par;

fn target_name(target: BuildTarget) -> &'static str {
    match target {
        BuildTarget::Workers => "workers",
        _ => "bundle",
    }
}

fn record(
    out: &mut Vec<(String, String)>,
    source: &str,
    target: BuildTarget,
    project: &bynk_driver::project::ProjectOutput,
) -> usize {
    let mut modules = 0;
    for (path, doc) in &project.artefacts.docs {
        let Document::Ts(program) = doc else { continue };
        let module = path
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        modules += 1;
        for text in bynk_ts::text_idents(program) {
            out.push((format!("{source} [{}] {module}", target_name(target)), text));
        }
    }
    modules
}

fn compile(dir: &Path, target: BuildTarget) -> Option<bynk_driver::project::ProjectOutput> {
    let options = if dir.join("bynk.toml").exists() {
        let paths = bynk_driver::project::try_read_project_paths(dir)
            .expect("well-formed fixture manifest");
        bynk_testkit::compile_options_split(dir.to_path_buf(), paths)
    } else {
        bynk_testkit::compile_options_single(dir.join("src"))
    };
    bynk_driver::project::compile_project(&options.target(target)).ok()
}

#[test]
fn every_ident_in_the_built_trees_is_an_identifier_path() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/positive");
    let mut dirs: Vec<PathBuf> = fs::read_dir(&root)
        .expect("positive fixtures")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();

    let per_fixture = par::par_map(&dirs, |dir| {
        let name = dir.file_name().unwrap().to_string_lossy().to_string();
        let mut found = Vec::new();
        let mut modules = 0;
        if dir.join("src").is_dir() {
            for target in [BuildTarget::Bundle, BuildTarget::Workers] {
                match compile(dir, target) {
                    Some(out) => modules += record(&mut found, &name, target, &out),
                    // Only the Workers target may refuse a positive fixture (a host-supplied
                    // capability); a Bundle failure would silently shrink the audit.
                    None if target == BuildTarget::Workers => {}
                    None => panic!("{name} must compile for the Bundle target"),
                }
            }
        } else {
            let source = fs::read_to_string(dir.join("input.bynk")).expect("input.bynk");
            let out = bynk_driver::project::compile_in_memory(
                &source,
                BuildTarget::Bundle,
                Default::default(),
            )
            .unwrap_or_else(|_| panic!("{name} must compile"));
            modules += record(&mut found, &name, BuildTarget::Bundle, &out);
        }
        (found, modules)
    });
    let mut found: Vec<(String, String)> = Vec::new();
    let mut modules = 0;
    for (f, m) in per_fixture {
        found.extend(f);
        modules += m;
    }
    assert!(modules > 1000, "only {modules} modules checked");

    // One line per distinct text, with the number of modules it occurs in and the first place.
    let mut distinct: BTreeMap<&str, (usize, &str)> = BTreeMap::new();
    for (location, text) in &found {
        distinct
            .entry(text.as_str())
            .or_insert((0, location.as_str()))
            .0 += 1;
    }
    assert!(
        distinct.is_empty(),
        "{modules} modules checked. An `Ident` holds expression text (convert it to the node it \
         spells):\n{}",
        distinct
            .iter()
            .take(60)
            .map(|(t, (n, loc))| format!("  {n:>5}  {t}   (first: {loc})"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
}

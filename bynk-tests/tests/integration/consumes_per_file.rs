//! #3536 (per-file `consumes`, warning stage): imports are per file. A unit
//! split over several files shares its own declarations, but a name from a unit
//! it consumes is in scope only in a file that itself consumes it. A file that
//! names one without its own `consumes` warns, with a fix that edits **that**
//! file, so `bynk check --fix=bynk.consumes.type_not_selected` migrates every
//! fragment. Modelled on github-hooks: one `consumes` clause, in `context.bynk`,
//! and the names used across the unit's other files.

use std::path::{Path, PathBuf};

use bynk_driver::fix::{Scope, plan_project_in};

const CODE: &str = "bynk.consumes.type_not_selected";

/// A project of `files` (path under `src/` and text), in a scratch directory
/// named for the test so parallel runs never share one.
fn project(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "bynk-per-file-consumes-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("bynk.toml"),
        "[project]\nname = \"hooks\"\nversion = \"0.1.0\"\n\n[paths]\ninclude = [\"src\"]\n",
    )
    .unwrap();
    for (path, text) in files {
        let at = dir.join("src").join(path);
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::fs::write(at, text).unwrap();
    }
    dir
}

const CONTEXT: &str = "context hooks\n\nconsumes bynk { Logger, Request, Response }\n\ntype Delivery = { id: String }\n\nfn status(r: Response) -> Int {\n    r.status\n}\n";
const VERIFY: &str = "context hooks\n\nfn check(r: Request) -> Bool {\n    true\n}\n";
const CLAIMS: &str = "context hooks\n\nconsumes bynk.cloudflare { D1 }\n\nfn won(r: Claim) -> Bool {\n    match r {\n        Claimed => true\n        AlreadyClaimed => false\n    }\n}\n\nfn kind(r: Request) -> Bool {\n    true\n}\n";

fn fix(dir: &Path) -> bynk_driver::fix::FixPlan {
    plan_project_in(dir, &Scope::codes(&[CODE.to_string()]).unwrap())
        .expect("project sets up")
        .expect("fixes settle")
}

#[test]
fn a_sibling_fragment_gets_its_own_consumes() {
    let dir = project(
        "sibling",
        &[
            ("hooks/context.bynk", CONTEXT),
            ("hooks/verify.bynk", VERIFY),
            ("hooks/claims.bynk", CLAIMS),
        ],
    );
    let plan = fix(&dir);
    let fixed = |suffix: &str| {
        plan.files
            .iter()
            .find(|f| f.path.ends_with(suffix))
            .map(|f| f.fixed.clone())
    };
    // The file with no clause gains one, listing every name it needs.
    assert_eq!(
        fixed("verify.bynk").as_deref(),
        Some(
            "context hooks\n\nconsumes bynk { Request }\n\nfn check(r: Request) -> Bool {\n    true\n}\n"
        )
    );
    // The file with a clause for another unit gains a line beside it.
    let claims = fixed("claims.bynk").expect("claims.bynk changes");
    assert!(
        claims.contains("consumes bynk.cloudflare { D1 }\nconsumes bynk { Claim, Request }\n"),
        "{claims}"
    );
    // The file that already consumes what it names is left alone.
    assert!(fixed("context.bynk").is_none());
    // Written back, the project is clean and a second run has nothing to do.
    for f in &plan.files {
        std::fs::write(&f.path, &f.fixed).unwrap();
    }
    assert!(fix(&dir).files.is_empty());
}

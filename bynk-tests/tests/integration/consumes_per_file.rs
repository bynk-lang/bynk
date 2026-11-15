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

/// A project of `files` (path under the project root, and text), in a scratch directory
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
        "[project]\nname = \"hooks\"\nversion = \"0.1.0\"\n\n[paths]\ninclude = [\"src\", \"tests\"]\n",
    )
    .unwrap();
    for (path, text) in files {
        let at = dir.join(path);
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
            ("src/hooks/context.bynk", CONTEXT),
            ("src/hooks/verify.bynk", VERIFY),
            ("src/hooks/claims.bynk", CLAIMS),
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

const SHOP: &str = "context shop\n\nconsumes bynk { Claim }\n\nfn won(r: Claim) -> Bool {\n    match r {\n        Claimed => true\n        AlreadyClaimed => false\n    }\n}\n";
const SUITE_NONE: &str =
    "suite shop as unit\n\ncase \"won\" {\n    expect won(Claimed) == true\n}\n";
const SUITE_SOME: &str = "suite shop as unit\n\nconsumes bynk { Logger }\n\ncase \"won two\" {\n    let c: Claim = AlreadyClaimed\n    expect won(c) == false\n}\n";
const SUITE_BRACED: &str =
    "suite shop as unit {\n  case \"won braced\" {\n    expect won(Claimed) == true\n  }\n}\n";
const SUITE_OK: &str =
    "suite shop\n\nconsumes bynk\n\ncase \"won three\" {\n    expect won(Claimed) == true\n}\n";

/// A suite file consumes what it names: the target's `consumes` is not
/// inherited. The fix edits the suite file, past a `as <tier>` header.
#[test]
fn a_suite_file_gets_its_own_consumes() {
    let dir = project(
        "suite",
        &[
            ("src/shop.bynk", SHOP),
            ("tests/none.bynk", SUITE_NONE),
            ("tests/some.bynk", SUITE_SOME),
            ("tests/ok.bynk", SUITE_OK),
            ("tests/braced.bynk", SUITE_BRACED),
        ],
    );
    let plan = fix(&dir);
    let fixed = |suffix: &str| {
        plan.files
            .iter()
            .find(|f| f.path.ends_with(suffix))
            .map(|f| f.fixed.clone())
    };
    let none = fixed("none.bynk").expect("a suite with no clause changes");
    assert!(
        none.starts_with("suite shop as unit\n\nconsumes bynk { Claim }\n\ncase"),
        "{none}"
    );
    let some = fixed("some.bynk").expect("a suite with another clause changes");
    assert!(some.contains("consumes bynk { Logger, Claim }\n"), "{some}");
    // A braced suite takes the clause inside its braces, at its members' indent.
    let braced = fixed("braced.bynk").expect("a braced suite changes");
    assert!(
        braced.starts_with("suite shop as unit {\n  consumes bynk { Claim }\n  case"),
        "{braced}"
    );
    // A bare `consumes bynk` is the whole-module form: nothing to fix.
    assert!(fixed("ok.bynk").is_none());
    assert!(fixed("shop.bynk").is_none());
    for f in &plan.files {
        std::fs::write(&f.path, &f.fixed).unwrap();
    }
    assert!(fix(&dir).files.is_empty());
}

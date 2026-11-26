//! #4126: `bynk explain` on a misspelt code names the nearest real one, and still fails.

use std::process::Command;

fn explain(code: &str) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_bynk"))
        .args(["explain", code])
        .output()
        .expect("runs bynk");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn a_misspelt_code_is_refused_with_the_nearest_real_one() {
    // Built at run time: a literal slip would read as a code the source emits.
    let slip = "bynk.types.not_orderable".replace("able", "abel");
    let (ok, err) = explain(&slip);
    assert!(!ok, "an unknown code still exits non-zero");
    assert!(
        err.contains("did you mean `bynk.types.not_orderable`?"),
        "{err}"
    );
}

#[test]
fn a_code_far_from_any_real_one_gets_no_suggestion() {
    let (ok, err) = explain("nothing.like.a.code");
    assert!(!ok);
    assert!(!err.contains("did you mean"), "{err}");
    assert!(err.contains("is not a diagnostic code"), "{err}");
}

#[test]
fn a_real_code_still_explains() {
    let (ok, _) = explain("bynk.types.not_orderable");
    assert!(ok);
}

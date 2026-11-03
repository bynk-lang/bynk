//! #2838: a nested `match` staircase on `Option`/`Result` is offered the flat
//! `let … else` form as a hint with a machine-applicable rewrite, and the
//! shapes that must stay quiet get no hint. The behaviour of the rewrite is
//! pinned by the `2838_let_else_staircase_behaviour` positive fixture.

use bynk_syntax::Severity;

const CODE: &str = "bynk.parse.let_else_staircase";

/// The hints `source` raises, checked as one `commons` file.
fn hints(source: &str) -> Vec<bynk_syntax::CompileError> {
    let (Ok(all) | Err(all)) = bynk_emit::check_with_warnings(source, "t.bynk");
    all.into_iter().filter(|e| e.category == CODE).collect()
}

/// `source` with the only hint's fix applied.
fn fixed(source: &str) -> String {
    let found = hints(source);
    let [hint] = found.as_slice() else {
        panic!("expected one hint, got {}", found.len());
    };
    let edits = bynk_syntax::fix::edits(source, [hint]);
    bynk_syntax::fix::apply(source, &edits)
}

fn commons(body: &str) -> String {
    format!("commons demo.steps\n\ntype Problem = enum {{ Missing, Late }}\n\n{body}\n")
}

const READ: &str = "\
fn read(a: Option[Int], b: Option[Int], c: Result[Int, String]) -> Result[Int, Problem] {
  match a {
    None => Err(Missing),
    Some(x) => match b {
      None => Err(Late),
      Some(y) => match c {
        Err(_) => Err(Missing),
        Ok(z) => Ok(x + y + z),
      },
    },
  }
}";

#[test]
fn a_three_step_staircase_gets_one_hint() {
    let found = hints(&commons(READ));
    assert_eq!(found.len(), 1, "one hint, not one per step: {found:?}");
    assert_eq!(Severity::for_error(&found[0]), Severity::Hint);
    assert!(
        found[0].message.contains("3 nested"),
        "{}",
        found[0].message
    );
}

#[test]
fn the_fix_is_the_flat_form_and_is_clean() {
    let flat = fixed(&commons(READ));
    assert!(
        flat.contains(
            "  let Some(x) = a else { Err(Missing) }\n  \
             let Some(y) = b else { Err(Late) }\n  \
             let Ok(z) = c else { Err(Missing) }\n  \
             Ok(x + y + z)\n"
        ),
        "{flat}"
    );
    assert!(hints(&flat).is_empty(), "the flat form is quiet");
    assert!(
        bynk_emit::check_with_warnings(&flat, "t.bynk").is_ok(),
        "the flat form still checks"
    );
}

#[test]
fn a_binding_fallback_becomes_else_match_and_statements_are_kept() {
    let src = commons(
        "\
fn sum(a: Result[Int, String], b: Result[Int, String]) -> Result[Int, String] {
  match a {
    Err(why) => Err(why),
    Ok(x) => {
      let twice = x * 2
      match b {
        Err(e) => Err(e),
        Ok(y) => Ok(twice + y),
      }
    },
  }
}",
    );
    let flat = fixed(&src);
    assert!(
        flat.contains("  let Ok(x) = a else match {\n")
            || flat.contains("let Ok(x) = a else match { Err(why) => Err(why) }"),
        "{flat}"
    );
    assert!(flat.contains("let twice = x * 2"), "{flat}");
    assert!(
        bynk_emit::check_with_warnings(&flat, "t.bynk").is_ok(),
        "{flat}"
    );
}

#[test]
fn a_multi_line_fallback_is_reindented_with_the_flat_form() {
    let src = commons(
        "\
fn f(a: Option[Int], b: Option[Int]) -> Result[Int, Problem] {
  match a {
    None => {
      let code = 7
      Err(Missing)
    },
    Some(x) => match b {
      None => Err(Late),
      Some(y) => Ok(x + y),
    },
  }
}",
    );
    let flat = fixed(&src);
    assert!(
        flat.contains(
            "  let Some(x) = a else {\n    let code = 7\n    Err(Missing)\n  }\n  let Some(y) = b else { Err(Late) }\n"
        ),
        "{flat}"
    );
    assert!(
        bynk_emit::check_with_warnings(&flat, "t.bynk").is_ok(),
        "{flat}"
    );
}

#[test]
fn nested_hints_never_apply_overlapping_edits() {
    // A fallback that is itself a staircase: both are hinted, and a fix run
    // takes one at a time rather than applying overlapping rewrites.
    let src = commons(
        "\
fn f(a: Option[Int], b: Option[Int], c: Option[Int], d: Option[Int]) -> Int {
  match a {
    None => match b {
      None => 0,
      Some(x) => match c {
        None => 1,
        Some(y) => x + y,
      },
    },
    Some(z) => match d {
      None => 2,
      Some(w) => z + w,
    },
  }
}",
    );
    let found = hints(&src);
    let edits = bynk_syntax::fix::edits(&src, found.iter());
    let out = bynk_syntax::fix::apply(&src, &edits);
    assert!(
        bynk_emit::check_with_warnings(&out, "t.bynk").is_ok(),
        "{out}"
    );
}

#[test]
fn a_wildcard_fallback_and_a_blank_line_come_out_fmt_clean() {
    let src = commons(
        "\
fn f(a: Option[Int], b: Option[Int]) -> Int {
  match a {
    Some(x) => {
      let twice = x * 2

      let more = twice + 1
      match b {
        Some(y) => more + y,
        _ => 1,
      }
    },
    _ => 0,
  }
}",
    );
    let flat = fixed(&src);
    assert!(
        flat.contains(
            "  let Some(x) = a else { 0 }\n  let twice = x * 2\n\n  let more = twice + 1\n  let Some(y) = b else { 1 }\n  more + y\n"
        ),
        "{flat:?}"
    );
    assert!(
        flat.lines().all(|l| l == l.trim_end()),
        "no whitespace-only or trailing-space lines: {flat:?}"
    );
    assert!(
        bynk_emit::check_with_warnings(&flat, "t.bynk").is_ok(),
        "{flat}"
    );
}

#[test]
fn a_guarded_arm_gets_no_hint() {
    // `Some(y) if y > 0 => …, _ => …` has no `let … else` equivalent, so the
    // staircase must not be offered one.
    let src = commons(
        "\
fn f(a: Option[Int], b: Option[Int]) -> Int {
  match a {
    None => 0,
    Some(x) => match b {
      Some(y) if y > 0 => x + y,
      _ => 1,
    },
  }
}",
    );
    assert!(hints(&src).is_empty());
}

#[test]
fn a_two_arm_match_gets_no_hint() {
    let src = commons(
        "fn f(a: Option[Int]) -> Int {\n  match a {\n    None => 0,\n    Some(x) => x,\n  }\n}",
    );
    assert!(hints(&src).is_empty());
}

#[test]
fn a_match_with_three_meaningful_arms_gets_no_hint() {
    let src = commons(
        "\
fn f(a: Option[Int], b: Result[Int, String]) -> Int {
  match a {
    None => 0,
    Some(1) => 1,
    Some(x) => match b {
      Err(_) => 2,
      Ok(y) => x + y,
    },
  }
}",
    );
    assert!(hints(&src).is_empty());
}

#[test]
fn a_match_inside_an_expression_gets_no_hint() {
    let src = commons(
        "\
fn f(a: Option[Int], b: Option[Int], c: Option[Int]) -> Int {
  let v = match a {
    None => 0,
    Some(x) => match b {
      None => 1,
      Some(y) => match c {
        None => 2,
        Some(z) => x + y + z,
      },
    },
  }
  v + 1
}",
    );
    assert!(hints(&src).is_empty());
}

#[test]
fn an_already_flat_let_else_chain_gets_no_hint() {
    let src = commons(
        "\
fn f(a: Option[Int], b: Option[Int]) -> Int {
  let Some(x) = a else { 0 }
  let Some(y) = b else { 1 }
  x + y
}",
    );
    assert!(hints(&src).is_empty());
}

#[test]
fn a_staircase_with_a_comment_is_hinted_without_a_fix() {
    let src = commons(
        "\
fn f(a: Option[Int], b: Option[Int]) -> Int {
  match a {
    None => 0, // nothing
    Some(x) => match b {
      None => 1,
      Some(y) => x + y,
    },
  }
}",
    );
    let found = hints(&src);
    assert_eq!(found.len(), 1);
    assert!(
        found[0].suggestions.is_empty(),
        "a rewrite would drop the comment"
    );
}

#[test]
fn two_matches_in_a_row_name_the_rule() {
    let src = commons(
        "\
fn f(a: Option[Int], b: Option[Int]) -> Int {
  match a {
    None => 0,
    Some(x) => x,
  }
  match b {
    None => 0,
    Some(y) => y,
  }
}",
    );
    let Err(errors) = bynk_emit::check_with_warnings(&src, "t.bynk") else {
        panic!("two consecutive `match`es must not check");
    };
    let err = errors
        .iter()
        .find(|e| e.category == "bynk.parse.match_ends_block")
        .unwrap_or_else(|| panic!("{errors:?}"));
    assert!(err.message.contains("ends the block"), "{}", err.message);
    assert!(err.notes.iter().any(|n| n.contains("let _ = match")));
}

#[test]
fn flat_map_points_at_and_then() {
    let src = commons("fn f(a: Option[Int]) -> Option[Int] {\n  a.flatMap((x) => Some(x))\n}");
    let Err(errors) = bynk_emit::check_with_warnings(&src, "t.bynk") else {
        panic!("`flatMap` is not an `Option` method");
    };
    assert!(
        errors
            .iter()
            .any(|e| e.category == "bynk.types.method_not_found"
                && e.notes.iter().any(|n| n.contains("andThen"))),
        "{errors:?}"
    );
}

#[test]
fn hints_do_not_fail_a_check() {
    assert!(bynk_emit::check_with_warnings(&commons(READ), "t.bynk").is_ok());
}

#[test]
fn the_single_file_compile_does_not_print_the_hint() {
    let dir = std::env::temp_dir().join(format!("bynk-staircase-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("steps.bynk");
    std::fs::write(&input, commons(READ)).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_bynkc"))
        .arg("compile")
        .arg(&input)
        .arg("-o")
        .arg(dir.join("steps.ts"))
        .output()
        .expect("bynkc runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(!stderr.contains("let_else_staircase"), "{stderr}");
}

/// A project holding a staircase and an unrelated type error: the build fails
/// on the error and the terminal output does not carry the hint.
#[test]
fn a_failed_project_build_does_not_print_the_hint() {
    let dir = std::env::temp_dir().join(format!("bynk-staircase-fail-{}", std::process::id()));
    let src = dir.join("src/demo");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("steps.bynk"),
        commons(&format!("{READ}\n\nfn broken() -> Int {{ \"nope\" }}")),
    )
    .unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_bynkc"))
        .arg("compile")
        .arg(dir.join("src"))
        .arg("-o")
        .arg(dir.join("out"))
        .output()
        .expect("bynkc runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{stderr}");
    assert!(
        stderr.contains("return_mismatch"),
        "the real error shows: {stderr}"
    );
    assert!(!stderr.contains("let_else_staircase"), "{stderr}");
}

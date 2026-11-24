use super::*;
use bynk_model::entry::{EntryHandler, EntryShape};

fn entries(files: &[(&str, &str)]) -> Vec<EntryHandler> {
    match compile_files_in_memory(files, BuildTarget::Bundle, Default::default(), false) {
        Ok(out) => out.entry_handlers,
        Err(f) => panic!(
            "fixture should compile:\n{}",
            f.flatten()
                .iter()
                .map(|e| format!("{}: {}\n", e.category, e.message))
                .collect::<String>()
        ),
    }
}

fn only_field(h: &EntryHandler) -> &EntryShape {
    match &h.params[0].shape {
        EntryShape::Record { fields, .. } => &fields[0].1,
        other => panic!("expected a record, got {other:?}"),
    }
}

/// #2695 review: a field resolves in its declaring unit's scope, so a
/// same-named local type in the entry context cannot shadow it.
#[test]
fn a_used_records_field_is_not_shadowed_by_a_local_type() {
    let es = entries(&[
        (
            "lib/ids.bynk",
            "commons lib.ids\n\ntype Id = Int where InRange(0, 9)\ntype User = { id: Id }\n",
        ),
        (
            "app/web.bynk",
            "context app.web\n\nuses lib.ids\n\ntype Id = String where NonEmpty\n\n\
             service s {\n  on call(u: User) -> Effect[Int] by Caller { 1 }\n}\n",
        ),
    ]);
    assert_eq!(es.len(), 1);
    match only_field(&es[0]) {
        EntryShape::Refined { name, base } => {
            assert_eq!((name.as_str(), *base), ("Id", "Int"));
        }
        other => panic!("expected lib's Id (Int), got {other:?}"),
    }
}

/// #2695 review: a field typed from a unit the declaring unit uses (two
/// `uses` hops from the entry) resolves rather than coming out as `?`.
#[test]
fn a_field_two_uses_hops_away_resolves() {
    let es = entries(&[
        (
            "lib/a.bynk",
            "commons lib.a\n\ntype Code = Int where InRange(0, 9)\n",
        ),
        (
            "lib/b.bynk",
            "commons lib.b\n\nuses lib.a\n\ntype Rec = { code: Code }\n",
        ),
        (
            "app/web.bynk",
            "context app.web\n\nuses lib.b\n\nservice s {\n  on call(r: Rec) -> Effect[Int] by Caller { 1 }\n}\n",
        ),
    ]);
    assert_eq!(es.len(), 1);
    match only_field(&es[0]) {
        EntryShape::Refined { name, base } => {
            assert_eq!((name.as_str(), *base), ("Code", "Int"));
        }
        other => panic!("expected Code, got {other:?}"),
    }
}

/// A sum payload typed from a used unit resolves in the sum's own scope.
#[test]
fn a_sum_payload_from_a_used_unit_resolves() {
    let es = entries(&[
        (
            "lib/a.bynk",
            "commons lib.a\n\ntype Code = Int where InRange(0, 9)\n",
        ),
        (
            "lib/b.bynk",
            "commons lib.b\n\nuses lib.a\n\ntype Ev = | Empty | Coded(code: Code)\n",
        ),
        (
            "app/web.bynk",
            "context app.web\n\nuses lib.b\n\nservice s {\n  on call(e: Ev) -> Effect[Int] by Caller { 1 }\n}\n",
        ),
    ]);
    match &es[0].params[0].shape {
        EntryShape::Sum { variants, .. } => match &variants[1].1[0].1 {
            EntryShape::Refined { name, base } => {
                assert_eq!((name.as_str(), *base), ("Code", "Int"));
            }
            other => panic!("expected Code, got {other:?}"),
        },
        other => panic!("expected a sum, got {other:?}"),
    }
}

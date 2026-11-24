//! #3921 (DECISION D): a service whose body-taking routes rely on the default
//! 1 MiB request-body cap gets one hint listing them, and a route or service
//! that declares its own cap does not.

use bynk_syntax::Severity;

const CODE: &str = "bynk.http.body_cap_defaulted";

/// The hints `source` raises, compiled as a one-file project (the rule is
/// project-wide, so a single-file check does not run it).
fn hints(source: &str) -> Vec<bynk_syntax::CompileError> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "body-cap-hint-{}",
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("api.bynk"), source).unwrap();
    let out =
        bynk_driver::project::compile_project(&bynk_testkit::compile_options_single(root.clone()))
            .map_err(bynk_driver::project::ProjectFailure::flatten)
            .expect("the source compiles");
    let _ = std::fs::remove_dir_all(&root);
    out.warnings
        .into_iter()
        .map(|w| w.error)
        .filter(|e| e.category == CODE)
        .collect()
}

fn api(header: &str, annotation: &str) -> String {
    format!(
        "context api\n\ntype Note = {{ text: String }}\n\nservice api from http {{\n  {header}\n  \
         on POST(\"/notes\") (body: Note) -> Effect[HttpResult[String]] by v: Visitor {{ Created(body.text) }}\n  \
         {annotation}\n  \
         on PUT(\"/notes/:id\") (id: String, body: Note) -> Effect[HttpResult[String]] by v: Visitor {{ Ok(body.text) }}\n  \
         on GET(\"/status\") () -> Effect[HttpResult[String]] by v: Visitor {{ Ok(\"ok\") }}\n}}\n"
    )
}

#[test]
fn one_hint_lists_every_route_on_the_default() {
    let found = hints(&api("", ""));
    let [hint] = found.as_slice() else {
        panic!("expected one hint, got {found:?}");
    };
    assert_eq!(Severity::for_error(hint), Severity::Hint);
    assert!(
        hint.message.contains("POST /notes") && hint.message.contains("PUT /notes/:id"),
        "{}",
        hint.message
    );
    assert!(
        !hint.message.contains("GET"),
        "a bodyless route has no cap: {}",
        hint.message
    );
    assert!(
        hint.message.contains(" rely on "),
        "several routes rely: {}",
        hint.message
    );
}

#[test]
fn a_route_with_its_own_cap_is_not_listed() {
    let found = hints(&api("", "@limit(maxBody: 2_000_000)"));
    let [hint] = found.as_slice() else {
        panic!("expected one hint, got {found:?}");
    };
    assert!(
        hint.message.contains("POST /notes") && !hint.message.contains("PUT"),
        "{}",
        hint.message
    );
    assert!(
        hint.message.contains(" relies on "),
        "one route relies: {}",
        hint.message
    );
}

#[test]
fn a_service_with_limits_has_no_hint() {
    assert!(hints(&api("limits { maxBody: 2_000_000 }", "")).is_empty());
}

//! #4504: a `cors` origin a browser can never send is refused at the entry, with the form to write.
//!
//! A negative fixture's `expected_error.txt` matches the code only, so the entry's span, the
//! suggestion and the `null` rule are pinned here, in the way `diagnostic_hints.rs` pins notes.

use bynk_driver::project::{ProjectFailure, compile_project};
use bynk_syntax::error::CompileError;

fn source(origins: &str, extra: &str) -> String {
    format!(
        "context api\n\nservice api from http {{\n  cors {{ origins: {origins}{extra} }}\n\n  \
         on GET(\"/ping\") () -> Effect[HttpResult[String]] by v: Visitor {{\n    Ok(\"pong\")\n  }}\n}}\n"
    )
}

/// Compile `src` as a one-file `context api` project: its errors, empty when it compiles. A
/// context needs a project, which the single-file `compile` API does not accept.
fn compile(tag: &str, src: &str) -> Vec<CompileError> {
    // Tests run in parallel: a directory of its own for each compile.
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root =
        std::env::temp_dir().join(format!("bynk_cors_origin_{}_{tag}_{n}", std::process::id()));
    let dir = root.join("src");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("api.bynk"), src).unwrap();
    let out = match compile_project(&bynk_testkit::compile_options_single(dir)) {
        Ok(_) => Vec::new(),
        Err(f) => ProjectFailure::flatten(f),
    };
    let _ = std::fs::remove_dir_all(&root);
    out
}

fn errors(src: &str) -> Vec<CompileError> {
    let errors = compile("errors", src);
    assert!(!errors.is_empty(), "expected a compile failure for:\n{src}");
    errors
}

fn sole(errors: &[CompileError], category: &str) -> CompileError {
    let matching: Vec<_> = errors.iter().filter(|e| e.category == category).collect();
    assert_eq!(
        matching.len(),
        1,
        "expected exactly one `{category}`; got: {:?}",
        errors
            .iter()
            .map(|e| (e.category, &e.message))
            .collect::<Vec<_>>()
    );
    matching[0].clone()
}

/// The error for the one bad entry in `["…"]`: it points at that entry and names its repair.
fn refuses(entry: &str, want: Option<&str>) {
    let src = source(&format!("[\"{entry}\"]"), "");
    let e = sole(&errors(&src), "bynk.http.cors_invalid_origins");
    assert_eq!(
        &src[e.span.start..e.span.end],
        format!("\"{entry}\""),
        "the error is at the entry: {e:?}"
    );
    assert!(e.message.contains(&format!("`{entry}`")), "{}", e.message);
    let suggestion = e
        .notes
        .iter()
        .find_map(|n| n.strip_prefix("write `").and_then(|r| r.strip_suffix('`')));
    assert_eq!(suggestion, want, "`{entry}`: {:?}", e.notes);
}

#[test]
fn each_shape_a_browser_never_sends_is_refused_at_the_entry_with_its_repair() {
    refuses("https://app.example.com/", Some("https://app.example.com"));
    refuses(
        "https://app.example.com/app",
        Some("https://app.example.com"),
    );
    refuses("https://App.example.com", Some("https://app.example.com"));
    refuses("HTTPS://app.example.com", Some("https://app.example.com"));
    refuses("app.example.com", Some("https://app.example.com"));
    refuses(
        "https://app.example.com:443",
        Some("https://app.example.com"),
    );
    refuses("https://App.example.com/", Some("https://app.example.com"));
    refuses("https://*.example.com", None);
}

/// Only the bad entry of a list is refused, and at its own span.
#[test]
fn a_good_entry_beside_a_bad_one_is_not_refused() {
    let src = source(
        "[\"https://app.example.com\", \"http://localhost:8787/\"]",
        "",
    );
    let e = sole(&errors(&src), "bynk.http.cors_invalid_origins");
    assert_eq!(&src[e.span.start..e.span.end], "\"http://localhost:8787/\"");
}

#[test]
fn an_origin_a_browser_sends_compiles() {
    for origins in [
        "[\"http://localhost:8787\"]",
        "[\"http://[::1]:8787\", \"capacitor://localhost\"]",
        "[\"*\"]",
        "[\"https://app.example.com\", \"null\"]",
    ] {
        let src = source(origins, "");
        let errors = compile("accepts", &src);
        assert!(errors.is_empty(), "`{origins}` should compile: {errors:?}");
    }
}

/// `"null"` is the `Origin` of a sandboxed frame or a `data:` page, which any site can create, so
/// beside credentials it is as open as the wildcard. Without credentials it is an ordinary entry.
#[test]
fn null_beside_credentials_is_refused_and_without_them_is_not() {
    let src = source(
        "[\"https://app.example.com\", \"null\"]",
        ", credentials: true",
    );
    let e = sole(&errors(&src), "bynk.http.cors_wildcard_credentials");
    assert_eq!(&src[e.span.start..e.span.end], "\"null\"");
    assert!(e.message.contains("\"null\""), "{}", e.message);
    // The shape check does not also fire on it: `null` is an origin a browser sends.
    assert!(
        !errors(&src)
            .iter()
            .any(|e| e.category == "bynk.http.cors_invalid_origins")
    );
    let alone = compile("null-alone", &source("[\"null\"]", ""));
    assert!(alone.is_empty(), "`null` alone compiles: {alone:?}");
}

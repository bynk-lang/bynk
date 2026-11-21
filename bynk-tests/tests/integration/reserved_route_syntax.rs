//! #3753: `*`, `{` and `}` are reserved in a route, so a rest-segment form
//! (#3752) can be added later without changing what an existing route means.
//! The refusals are pinned by the `3753_http_reserved_route_*` negative
//! fixtures; this pins the other half, that ordinary routes are unaffected.

fn compile_route(route: &str, params: &str) -> Result<(), Vec<bynk_syntax::CompileError>> {
    let source = format!(
        "context demo\n\nservice api from http {{\n  on GET(\"{route}\") ({params}) -> Effect[HttpResult[Int]] by Visitor {{\n    Ok(1)\n  }}\n}}\n"
    );
    // Each call gets its own directory: the two tests run in parallel.
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("reserved-route-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("demo.bynk"), source).unwrap();
    let result =
        bynk_driver::project::compile_project(&bynk_testkit::compile_options_single(root.clone()))
            .map(|_| ())
            .map_err(bynk_driver::project::ProjectFailure::flatten);
    let _ = std::fs::remove_dir_all(&root);
    result
}

/// A `:name` parameter and a `+` in a literal segment still compile: `+` is
/// common in real paths and is not reserved.
#[test]
fn ordinary_routes_still_compile() {
    for (route, params) in [
        ("/files/:key", "key: String"),
        ("/a+b", ""),
        ("/files/:key/raw", "key: String"),
    ] {
        let result = compile_route(route, params);
        assert!(result.is_ok(), "`{route}` must compile, got: {result:#?}");
    }
}

/// Each reserved character is refused, on its own or inside a `:name`, with
/// the one dedicated diagnostic.
#[test]
fn reserved_characters_are_refused_once() {
    for route in [
        "/files/*key",
        "/files/:key*",
        "/files/{key}",
        "/files/{...key}",
        "/files/:{key}",
        "/files/key}",
    ] {
        let errors = compile_route(route, "").expect_err("a reserved character must be refused");
        let codes: Vec<&str> = errors.iter().map(|e| e.category).collect();
        assert_eq!(
            codes,
            ["bynk.http.reserved_route_syntax"],
            "`{route}` must be refused once, with no cascade"
        );
    }
}

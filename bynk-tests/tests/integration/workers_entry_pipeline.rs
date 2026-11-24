//! 1B.2b (#4028): the Worker entry tests that drove the driver's in-memory pipeline, moved from
//! `bynk-emit` (`emitter/workers_entry.rs`). The entry's own pure helpers keep their unit tests there.

#[path = "../support/emit_pipeline.rs"]
mod emit_pipeline;
use emit_pipeline::{emit_project_with_tests, emit_workers};

/// #2136: the test build of a context a `system` suite drives stores each
/// cron run's `Result` on the execution context; the deployable build
/// (suites stripped) is byte-identical to a project with no suite at all.
#[test]
fn scheduled_result_hook_is_test_build_only() {
    const JOBS: &str = "context ops.jobs\n\nservice ticker from cron {\n\ton schedule(\"*/5 * * * *\") (at: Instant) -> Effect[Result[(), String]] {\n\t\tOk(())\n\t}\n}\n\nservice inbox from queue(\"jobs\") {\n\ton message(n: Int) -> Effect[QueueResult] {\n\t\tAck\n\t}\n}\n";
    const SUITE: &str = "suite ops.jobs as system {\n\tcase \"ticks\" {\n\t\tlet r <- ticker.schedule(\"*/5 * * * *\", Instant.fromEpochMillis(0))\n\t\texpect r is Ok(_)\n\t}\n}\n";
    let entry = "workers/ops-jobs/index.ts";
    let with_suite = [("ops/jobs.bynk", JOBS), ("tests/jobs.bynk", SUITE)];
    let test_build = emit_project_with_tests(
        &with_suite,
        bynk_check::project_model::BuildTarget::Workers,
        true,
    );
    assert!(
        test_build
            .text(entry)
            .contains("ctx.__bynkResult = result;")
    );
    let deploy = emit_project_with_tests(
        &with_suite,
        bynk_check::project_model::BuildTarget::Workers,
        false,
    );
    let bare = emit_workers(&[("ops/jobs.bynk", JOBS)]);
    assert_eq!(deploy.text(entry), bare.text(entry));
    assert!(!deploy.text(entry).contains("__bynkResult"));
}

/// #2137: the test build of a context a `system` harness stands up reads
/// the harness clock (`env.__bynkClock`) in its `compose` and its agents'
/// deps; the deployable build is byte-identical to a project with no suite.
#[test]
fn harness_clock_seam_is_test_build_only() {
    const HOLDS: &str = "context ck.holds\n\nconsumes bynk { Clock }\n\nagent Hold {\n\tkey id: String\n\tstore n: Cell[Int]\n\n\ton call at() -> Effect[Instant] given Clock {\n\t\tClock.now()\n\t}\n}\n\nservice api from http {\n\ton GET(\"/now\") () -> Effect[HttpResult[Bool]] by Visitor given Clock {\n\t\tlet t <- Clock.now()\n\t\tlet a <- Hold(\"h\").at()\n\t\tOk(t == a)\n\t}\n}\n";
    const SUITE: &str = "suite ck.holds as system {\n\tcase \"now\" {\n\t\tadvance Clock by 1.minute\n\t\tlet r <- api.GET(\"/now\") by Visitor\n\t\texpect r is Ok(true)\n\t}\n}\n";
    let with_suite = [("ck/holds.bynk", HOLDS), ("tests/holds.bynk", SUITE)];
    let target = bynk_check::project_model::BuildTarget::Workers;
    let test_build = emit_project_with_tests(&with_suite, target, true);
    let deploy = emit_project_with_tests(&with_suite, target, false);
    let bare = emit_workers(&[("ck/holds.bynk", HOLDS)]);
    for file in [
        "workers/ck-holds/compose.ts",
        "workers/ck-holds/handlers.ts",
    ] {
        assert!(
            test_build
                .text(file)
                .contains(".__bynkClock ?? new bynk__binding.ClockProvider()"),
            "{file}"
        );
        assert_eq!(deploy.text(file), bare.text(file), "{file}");
        assert!(!deploy.text(file).contains("__bynkClock"), "{file}");
    }
}

/// A context with an `on call` service, a GET route with a path
/// parameter, and a POST route.
const SHOP: &str = "context demo.shop

service api from http {
  on GET(\"/items/:id\") (id: String) -> Effect[HttpResult[String]] by v: Visitor {
    Ok(id)
  }
  on POST(\"/items\") (body: String) -> Effect[HttpResult[String]] by v: Visitor {
    Ok(body)
  }
}

service calc {
  on call(n: Int) -> Effect[Int] by Caller {
    Effect.pure(n)
  }
}
";

fn shop_index() -> String {
    emit_workers(&[("demo/shop.bynk", SHOP)])
        .text("workers/demo-shop/index.ts")
        .to_string()
}

#[test]
fn each_call_service_is_dispatched_under_the_internal_prefix() {
    let ts = shop_index();
    assert!(
        ts.contains("if (path.startsWith(\"/_bynk/call/\"))"),
        "{ts}"
    );
    assert!(ts.contains("case \"calc\": {"), "{ts}");
    assert!(ts.contains("await surface.calc(n)"), "{ts}");
    // An unknown internal service is a 404, not a fall-through to the routes.
    assert!(
        ts.contains(
            "default:\n            return new globalThis.Response(\"Not found\", { status: 404 });"
        ),
        "{ts}"
    );
}

#[test]
fn a_call_whose_contract_hash_differs_is_refused_with_409() {
    let ts = shop_index();
    assert!(
        ts.contains("request.headers.get(\"X-Bynk-Contract\")"),
        "{ts}"
    );
    assert!(
        ts.contains("kind: \"ContractMismatch\", service: \"calc\""),
        "{ts}"
    );
    assert!(ts.contains("{ status: 409,"), "{ts}");
    // #1826: logged before the refusal, the sender's header bounded.
    assert!(
        ts.contains("globalThis.console.error(\"ContractMismatch demo.shop call calc\""),
        "{ts}"
    );
    assert!(
        ts.contains("actual: globalThis.String(__contract).slice(0, 16)"),
        "{ts}"
    );
}

#[test]
fn a_call_argument_that_fails_to_decode_is_a_400() {
    let ts = shop_index();
    assert!(
            ts.contains("if (__r_n.tag === \"Err\") return new globalThis.Response(globalThis.JSON.stringify(__r_n.error), { status: 400,"),
            "{ts}"
        );
}

#[test]
fn a_get_route_also_answers_head_with_the_body_stripped() {
    let ts = shop_index();
    assert!(
        ts.contains("if ((method === \"GET\" || method === \"HEAD\") && __m) {"),
        "{ts}"
    );
    assert!(
        ts.contains("return method === \"HEAD\" ? __headResponse(__response) : __response;"),
        "{ts}"
    );
}

#[test]
fn a_post_route_with_unparseable_json_is_a_400_malformed_json() {
    let ts = shop_index();
    assert!(ts.contains("kind: \"MalformedJson\""), "{ts}");
}

#[test]
fn a_known_path_with_an_undeclared_method_is_a_405_listing_the_allowed_ones() {
    let ts = shop_index();
    assert!(
        ts.contains("if (__matchPath(\"/items\", path) !== null) {"),
        "{ts}"
    );
    assert!(ts.contains("headers: { allow: \"OPTIONS, POST\" }"), "{ts}");
    assert!(
        ts.contains("if (__matchPath(\"/items/:id\", path) !== null) {"),
        "{ts}"
    );
    assert!(
        ts.contains("headers: { allow: \"GET, HEAD, OPTIONS\" }"),
        "{ts}"
    );
    assert!(
        ts.contains("const __status = method === \"OPTIONS\" ? 204 : 405;"),
        "{ts}"
    );
}

#[test]
fn exact_paths_are_tried_before_parameterised_ones_and_unknown_paths_are_404() {
    let ts = shop_index();
    let exact = ts
        .find("__matchPath(\"/items\", path) !== null) {")
        .expect("exact route");
    let param = ts
        .find("__matchPath(\"/items/:id\", path);")
        .expect("param route");
    assert!(exact < param, "{ts}");
    assert!(
        ts.contains("return new globalThis.Response(\"Not Found\", { status: 404 });"),
        "{ts}"
    );
}

#[test]
fn every_route_response_carries_the_service_security_headers() {
    let ts = shop_index();
    assert!(
        ts.contains(
            "const __security_api: __SecurityPolicy = { nosniff: true, hstsMaxAgeSecs: null };"
        ),
        "{ts}"
    );
    assert!(
            ts.contains("return __applySecurityHeaders(__applyChallenge(__httpResultToResponse(result, (__v: string) => __v as __JsonValue), request, [\"Bynk realm=\\\"demo.shop\\\"\"]), __security_api);"),
            "{ts}"
        );
}

#[test]
fn a_context_with_only_call_services_emits_no_route_table() {
    let src = "context demo.calc\n\nservice calc {\n  on call(n: Int) -> Effect[Int] by Caller {\n    Effect.pure(n)\n  }\n}\n";
    let ts = emit_workers(&[("demo/calc.bynk", src)])
        .text("workers/demo-calc/index.ts")
        .to_string();
    assert!(!ts.contains("__matchPath"), "{ts}");
    assert!(!ts.contains("__applySecurityHeaders"), "{ts}");
}

#[test]
fn a_body_limit_rejects_an_oversized_request_with_413_before_reading_it() {
    let src = "context demo.up

service api from http {
  limits { maxBody: 1024 }

  on POST(\"/upload\") (body: String) -> Effect[HttpResult[String]] by v: Visitor {
    Ok(body)
  }
}
";
    let ts = emit_workers(&[("demo/up.bynk", src)])
        .text("workers/demo-up/index.ts")
        .to_string();
    let limit = ts.find("PayloadTooLarge").expect("a 413 branch");
    let read = ts.find("await __readBodyCapped").expect("the body read");
    assert!(
        limit < read,
        "the cap is checked before the body is read:\n{ts}"
    );
    assert!(ts.contains("1024"), "{ts}");
    assert!(ts.contains("status: 413"), "{ts}");
}

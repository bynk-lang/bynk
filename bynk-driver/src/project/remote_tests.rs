//! #2138: a remote test build (`CompileOptions::remote_tests`).
//!
//! A pipeline test, so it stays in the driver when the test-harness generation moves into
//! `bynk-emit::testgen` (1B.2a, #4028). The test module is inline so that the probes, which
//! scan whole files, skip it as they skipped it in `tests_emit.rs`.

#[cfg(test)]
mod tests {
    use crate::project::{BuildTarget, RemoteTests, compile_files_in_memory_remote};

    const API: &str = "context shop.api\n\nconsumes bynk { Clock }\n\nservice api from http {\n  on GET(\"/n\") () -> Effect[HttpResult[Int]] by Visitor {\n    Ok(1)\n  }\n}\n\nservice tick from cron {\n  on schedule(\"*/5 * * * *\") () -> Effect[Result[(), String]] {\n    Ok(())\n  }\n}\n\nservice inbox from queue(\"q\") {\n  on message(n: Int) -> Effect[QueueResult] {\n    Ack\n  }\n}\n";

    fn suite(body: &str) -> String {
        format!("suite shop.api as system {{\n  case \"c\" {{\n{body}\n  }}\n}}\n")
    }

    fn build(
        body: &str,
        skip: bool,
    ) -> Result<crate::project::ProjectOutput, crate::project::ProjectFailure> {
        let s = suite(body);
        compile_files_in_memory_remote(
            &[("shop/api.bynk", API), ("tests/api.bynk", &s)],
            BuildTarget::Workers,
            Default::default(),
            true,
            Some(&RemoteTests {
                skip_unsupported: skip,
                ..RemoteTests::default()
            }),
            None,
        )
    }

    fn refusal(body: &str) -> String {
        match build(body, false) {
            Ok(_) => panic!("`{body}` should be refused remotely"),
            Err(f) => f
                .flatten()
                .iter()
                .map(|e| e.category.to_string())
                .collect::<Vec<_>>()
                .join(","),
        }
    }

    #[test]
    fn what_a_remote_run_cannot_honour_is_refused() {
        for (body, code) in [
            (
                "    advance Clock by 1.minutes\n    let n <- api.GET(\"/n\") by Visitor\n    expect n is Ok(1)",
                "bynk.test.remote_unsupported_advance",
            ),
            (
                "    let r <- tick.schedule(\"*/5 * * * *\")\n    expect r is Ok(_)",
                "bynk.test.remote_unsupported_schedule",
            ),
            (
                "    let r <- inbox.message(1)\n    expect r is Ack",
                "bynk.test.remote_unsupported_queue",
            ),
        ] {
            assert_eq!(refusal(body), code, "{body}");
        }
    }

    #[test]
    fn a_case_env_that_disagrees_with_the_target_vars_is_refused() {
        let suite = "suite shop.api as system {\n  env { PAYMENTS = \"fake\" }\n  case \"c\" {\n    let n <- api.GET(\"/n\") by Visitor\n    expect n is Ok(1)\n  }\n}\n";
        let build = |vars: &[(&str, &str)]| {
            compile_files_in_memory_remote(
                &[("shop/api.bynk", API), ("tests/api.bynk", suite)],
                BuildTarget::Workers,
                Default::default(),
                true,
                Some(&RemoteTests {
                    vars: vars
                        .iter()
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect(),
                    ..RemoteTests::default()
                }),
                None,
            )
        };
        assert!(
            build(&[("PAYMENTS", "fake")]).is_ok(),
            "agreeing values run"
        );
        let codes = match build(&[("PAYMENTS", "real")]) {
            Ok(_) => panic!("a target deployed differently is refused"),
            Err(f) => f
                .flatten()
                .iter()
                .map(|e| e.category.to_string())
                .collect::<Vec<_>>(),
        };
        assert_eq!(codes, ["bynk.test.remote_unsupported_env"]);
    }

    #[test]
    fn a_dev_build_groups_cases_by_env_and_builds_one_group() {
        let suite = "suite shop.api as system {\n  env { P = \"a\" }\n  case \"one\" {\n    let n <- api.GET(\"/n\") by Visitor\n    expect n is Ok(1)\n  }\n  case \"two\" {\n    env { P = \"b\" }\n    let n <- api.GET(\"/n\") by Visitor\n    expect n is Ok(1)\n  }\n}\n";
        let build = |only: Option<&str>| {
            let out = compile_files_in_memory_remote(
                &[("shop/api.bynk", API), ("tests/api.bynk", suite)],
                BuildTarget::Workers,
                Default::default(),
                true,
                Some(&RemoteTests {
                    dev: true,
                    only_env: only.map(|v| [("P".to_string(), v.to_string())].into()),
                    ..RemoteTests::default()
                }),
                None,
            )
            .unwrap_or_else(|_| panic!("a dev build applies env rather than refusing it"));
            out.discovered
                .into_iter()
                .find(|s| s.name == "shop.api")
                .expect("the system suite")
        };
        let all = build(None);
        assert_eq!(all.envs.len(), 2, "two distinct env maps");
        assert_eq!(all.cases.len(), 2);
        let b = build(Some("b"));
        assert_eq!(
            b.cases.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            ["two"],
            "only the group's cases are built"
        );
    }

    /// #2175: the clock warning follows what the run produced. Instants
    /// read from an address's result (a relation, a call on one) stay quiet;
    /// a constant instant warns wherever its `expect` sits — top level, in an
    /// `if` arm, or in the tail a `let … else` makes of the rest of the case.
    #[test]
    fn the_clock_warning_tracks_run_derived_instants_through_the_case() {
        const HOLDS: &str = "context shop.api\n\ntype Hold = { created: Instant, expires: Instant, ms: Int }\n\nservice api from http {\n  on GET(\"/h\") () -> Effect[HttpResult[Hold]] by Visitor {\n    Ok(Hold { created: Instant.fromEpochMillis(0), expires: Instant.fromEpochMillis(1), ms: 2 })\n  }\n}\n";
        let warned = |body: &str| {
            let s = suite(body);
            let out = compile_files_in_memory_remote(
                &[("shop/api.bynk", HOLDS), ("tests/api.bynk", &s)],
                BuildTarget::Workers,
                Default::default(),
                true,
                Some(&RemoteTests::default()),
                None,
            )
            .unwrap_or_else(|f| panic!("`{body}` builds remotely: {:?}", f.flatten()));
            out.warnings
                .iter()
                .any(|w| w.error.category == "bynk.test.remote_clock_dependent")
        };
        let read =
            "    let r <- api.GET(\"/h\") by Visitor\n    let Ok(h) = r else { expect false }\n";
        for quiet in [
            "    expect h.expires > h.created",
            "    expect Instant.fromEpochMillis(h.ms) > h.created",
            "    let later = h.created + 5.minutes\n    expect later > h.expires",
            // #2181: a binder rebinds a constant's name to a run value.
            "    let t = Instant.fromEpochMillis(0)\n    expect match Some(h.expires) { Some(t) => t > h.created, None => true }",
            "    let t = Instant.fromEpochMillis(0)\n    if Some(h.expires) is Some(t) {\n      expect t > h.created\n    } else {\n      expect false\n    }",
            "    let t = Instant.fromEpochMillis(0)\n    expect Some(h.expires) is Some(t) && t > h.created",
            "    let t = Instant.fromEpochMillis(0)\n    expect [h.expires].all((t) => t > h.created)",
            "    let t = Instant.fromEpochMillis(0)\n    match Some(h.expires) {\n      Some(t) => { expect t > h.created }\n      None => { expect false }\n    }",
        ] {
            assert!(!warned(&format!("{read}{quiet}")), "run-derived: {quiet}");
        }
        for loud in [
            // In the tail a `let … else` makes of the rest of the case.
            "    expect h.created > Instant.fromEpochMillis(0)",
            // A `let` bound to a constant.
            "    let epoch = Instant.fromEpochMillis(0)\n    expect h.created >= epoch",
            // In an `if` arm.
            "    if h.ms == 2 {\n      expect h.expires < Instant.fromEpochMillis(5)\n    } else {\n      expect false\n    }",
        ] {
            assert!(warned(&format!("{read}{loud}")), "constant: {loud}");
        }
    }

    #[test]
    fn an_absolute_instant_in_an_expect_warns_in_a_remote_build() {
        let warned = |body: &str| {
            let out = build(body, false)
                .unwrap_or_else(|_| panic!("a warning doesn't fail the build: {body}"));
            out.warnings
                .iter()
                .any(|w| w.error.category == "bynk.test.remote_clock_dependent")
        };
        assert!(warned(
            "    let n <- api.GET(\"/n\") by Visitor\n    let t = Instant.fromEpochMillis(0)\n    expect t < Instant.fromEpochMillis(1)"
        ));
        assert!(!warned(
            "    let n <- api.GET(\"/n\") by Visitor\n    expect n is Ok(1)"
        ));
    }

    #[test]
    fn an_asset_fetch_is_refused() {
        const WEB: &str = "context t.web\n\nasset style = \"assets/board.css\" at \"/board.css\"\n\nservice api from http {\n  on GET(\"/\") () -> Effect[HttpResult[String]] by Visitor {\n    Ok(\"hi\")\n  }\n}\n";
        const SUITE: &str = "suite t.web as system {\n  case \"the stylesheet is served\" {\n    let css <- api.GET(style)\n    expect css is Raw(_, _)\n  }\n}\n";
        let folder = crate::project::assets::AssetFolder::new(
            std::collections::BTreeSet::from(["assets/board.css".to_string()]),
            std::sync::Arc::new(|_: &str| Ok(b"body {}".to_vec())),
        );
        let refused = compile_files_in_memory_remote(
            &[("t/web.bynk", WEB), ("tests/web.bynk", SUITE)],
            BuildTarget::Workers,
            Default::default(),
            true,
            Some(&RemoteTests::default()),
            Some(&folder),
        );
        let codes = match refused {
            Ok(_) => panic!("an asset fetch should be refused remotely"),
            Err(f) => f
                .flatten()
                .iter()
                .map(|e| e.category.to_string())
                .collect::<Vec<_>>(),
        };
        assert_eq!(codes, ["bynk.test.remote_unsupported_asset"]);
    }

    #[test]
    fn an_on_call_address_is_refused() {
        const PAY: &str = "context shop.pay\n\nservice charge {\n  on call(n: Int) -> Effect[Int] {\n    n\n  }\n}\n";
        const ORDERS: &str = "context shop.orders\n\nconsumes shop.pay\n\nservice place {\n  on call(n: Int) -> Effect[Int] {\n    let r <- shop.pay.charge(n)\n    r\n  }\n}\n";
        const SUITE: &str = "suite shop.orders as system {\n  case \"places across the wire\" {\n    let r <- shop.orders.place(1)\n    expect r == 1\n  }\n}\n";
        let refused = compile_files_in_memory_remote(
            &[
                ("shop/pay.bynk", PAY),
                ("shop/orders.bynk", ORDERS),
                ("tests/orders.bynk", SUITE),
            ],
            BuildTarget::Workers,
            Default::default(),
            true,
            Some(&RemoteTests::default()),
            None,
        );
        let codes = match refused {
            Ok(_) => panic!("an `on call` address should be refused remotely"),
            Err(f) => f
                .flatten()
                .iter()
                .map(|e| e.category.to_string())
                .collect::<Vec<_>>(),
        };
        assert_eq!(codes, ["bynk.test.remote_unsupported_call"]);
    }

    /// #2138 (track §3.6): a target with a safe and an unsafe route, for the
    /// `--settle` segmentation tests.
    const ORDERS: &str = "context shop.api\n\ntype Pair = { a: Int, b: Int }\n\nservice api from http {\n  on GET(\"/n\") () -> Effect[HttpResult[Int]] by Visitor {\n    Ok(1)\n  }\n\n  on POST(\"/n\") (body: Int) -> Effect[HttpResult[Int]] by Visitor {\n    Ok(body)\n  }\n\n  on GET(\"/p\") () -> Effect[HttpResult[Pair]] by Visitor {\n    Ok(Pair { a: 1, b: 2 })\n  }\n}\n";

    /// The system module a remote build emits for `body`.
    fn settle_ts(body: &str) -> String {
        let s = suite(body);
        let out = compile_files_in_memory_remote(
            &[("shop/api.bynk", ORDERS), ("tests/api.bynk", &s)],
            BuildTarget::Workers,
            Default::default(),
            true,
            Some(&RemoteTests::default()),
            None,
        )
        .unwrap_or_else(|f| panic!("`{body}` builds remotely: {:?}", f.flatten()));
        let (_, doc) = out
            .artefacts
            .docs
            .iter()
            .find(|(p, _)| {
                p.to_string_lossy()
                    .ends_with("integration_shop_api.test.ts")
            })
            .expect("the system module");
        doc.text().to_string()
    }

    /// The text of each `__bynkSettle` attempt in `ts`.
    /// (The case scaffold indents the body, so an attempt ends at the first
    /// line that is only `});`.)
    fn attempts(ts: &str) -> Vec<&str> {
        ts.match_indices("__bynkSettle(async () => {\n")
            .map(|(at, m)| {
                let body = &ts[at + m.len()..];
                let mut end = 0;
                for line in body.split_inclusive('\n') {
                    if line.trim() == "});" {
                        break;
                    }
                    end += line.len();
                }
                assert!(end < body.len(), "an attempt ends: {body}");
                &body[..end]
            })
            .collect()
    }

    /// #2138 review: classification fails closed. An address inside an
    /// effectful statement (an `if` or `match` around a `POST`) ends the
    /// segment, so it is never inside a replayed attempt.
    #[test]
    fn an_address_nested_in_an_effectful_statement_is_never_replayed() {
        for wrapped in [
            "if a is Ok(1) { api.POST(\"/n\", 2) } else { api.GET(\"/n\") }",
            "match a { Ok(_) => api.POST(\"/n\", 2), _ => api.GET(\"/n\") }",
        ] {
            let ts = settle_ts(&format!(
                "    let a <- api.GET(\"/n\") by Visitor\n    let b <- {wrapped} by Visitor\n    let c <- api.GET(\"/n\") by Visitor\n    expect c is Ok(1)"
            ));
            let tries = attempts(&ts);
            assert_eq!(tries.len(), 1, "{ts}");
            assert!(
                tries[0].contains("GET_n"),
                "the later read is retried: {ts}"
            );
            assert!(
                tries.iter().all(|t| !t.contains("POST_n")),
                "the write is not replayed: {ts}"
            );
        }
    }

    /// #2138 review: the attempt returns its bindings by name from the AST,
    /// as emitted — a name the harness already binds (`deps`) is renamed, and
    /// the attempt returns the renamed one.
    #[test]
    fn a_settle_attempt_returns_its_bindings_as_emitted() {
        let ts = settle_ts(
            "    let deps <- api.GET(\"/n\") by Visitor\n    let q = 1\n    expect deps is Ok(q)",
        );
        let tries = attempts(&ts);
        assert_eq!(tries.len(), 1, "{ts}");
        let ret = tries[0]
            .lines()
            .find_map(|l| l.trim().strip_prefix("return { "))
            .and_then(|r| r.strip_suffix(" };"))
            .unwrap_or_else(|| panic!("the attempt returns its bindings: {ts}"));
        let names: Vec<&str> = ret.split(", ").collect();
        assert_eq!(names.len(), 2, "{ts}");
        assert_ne!(names[0], "deps", "the harness's `deps` is renamed: {ts}");
        for n in names {
            assert!(
                tries[0].contains(&format!("const {n} = ")),
                "`{n}` is bound in the attempt: {ts}"
            );
        }
    }

    /// #2138 review: a `let … else` desugars into a hidden binding and a
    /// `match` tail; after a segment, the hidden binding and the pattern's
    /// own binding stay outside the attempt and read its returned names.
    #[test]
    fn a_let_else_and_a_pattern_binding_after_a_segment_see_its_bindings() {
        let ts = settle_ts(
            "    let a <- api.GET(\"/p\") by Visitor\n    let q = 1\n    expect a is Ok(_)\n    let Ok(p) = a else { expect false }\n    expect p.a == q",
        );
        let tries = attempts(&ts);
        assert_eq!(tries.len(), 1, "{ts}");
        assert!(tries[0].contains("return { a, q };"), "{ts}");
        assert!(ts.contains("const { a, q } = await __bynkSettle("), "{ts}");
        assert!(!tries[0].contains("__letElse"), "{ts}");

        let ts = settle_ts(
            "    let r <- api.POST(\"/n\", 2) by Visitor\n    let a <- api.GET(\"/n\") by Visitor\n    expect a is Ok(1)\n    let Ok(n) <- api.GET(\"/n\") by Visitor else { expect false }\n    expect n == 1",
        );
        let tries = attempts(&ts);
        assert_eq!(tries.len(), 1, "{ts}");
        assert!(tries[0].contains("return { a };"), "{ts}");
        assert!(
            !tries[0].contains("POST_n") && !tries[0].contains("__letElse"),
            "{ts}"
        );
    }

    #[test]
    fn an_http_case_is_driven_at_the_target_url_with_the_environment_s_secrets() {
        let out = build(
            "    let n <- api.GET(\"/n\") by Visitor\n    expect n is Ok(1)",
            false,
        )
        .unwrap_or_else(|_| panic!("an http-only case builds remotely"));
        let (_, doc) = out
            .artefacts
            .docs
            .iter()
            .find(|(p, _)| {
                p.to_string_lossy()
                    .ends_with("integration_shop_api.test.ts")
            })
            .expect("the system module");
        let ts = doc.text();
        assert!(
            ts.contains("rootEnv.SHOP_API = __bynkRemoteBinding(\"shop-api\");"),
            "{ts}"
        );
        assert!(ts.contains("BYNK_TEST_TARGETS"), "{ts}");
        assert!(
            !ts.contains("__makeIntegrationDoNamespace("),
            "no in-process Worker: {ts}"
        );
        assert!(
            !ts.contains("\"__bynk_test_secret\""),
            "no fixed test secret: {ts}"
        );
    }

    #[test]
    fn skip_unsupported_drops_the_case_and_says_why() {
        let out = build("    let r <- inbox.message(1)\n    expect r is Ack", true)
            .unwrap_or_else(|_| panic!("a skipped case does not fail the build"));
        let suite = out
            .discovered
            .iter()
            .find(|s| s.name == "shop.api")
            .expect("the system suite");
        assert!(suite.cases.is_empty(), "the case is not run");
        assert_eq!(suite.skipped.len(), 1);
        assert_eq!(suite.skipped[0].code, "bynk.test.remote_unsupported_queue");
        assert!(suite.skipped[0].reason.contains("delivers a queue message"));
    }
}

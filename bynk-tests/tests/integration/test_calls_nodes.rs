//! 1A.26 (#3727): the test-case and system-tier service calls (a local call, an HTTP, cron, queue
//! or event address in a `bynk test` case, the `system`-tier HTTP, cron and queue drivers, and the
//! asset fetch) lower to nodes, with the call-site principal (`by Alice(...)`) carried as a node.
//! Each project is emitted in memory and the exact line is pinned. The builders' own forms are
//! unit tests in `lower_call/test_calls.rs`.
//!

fn fixture_dir(fixture: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("positive")
        .join(fixture)
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        if entry.file_name() == "expected" {
            continue;
        }
        if src.is_dir() {
            copy_dir(&src, &dst);
        } else {
            std::fs::copy(&src, &dst).unwrap();
        }
    }
}

/// `every emitted document joined` for a fixture project, built as the fixture suite
/// builds it. `rewrite` replaces (or adds) files of a temporary copy, so a suite's own unrelated
/// constructs can be taken out of the count.
fn emit(fixture: &str, rewrite: &[(&str, &str)]) -> String {
    let original = fixture_dir(fixture);
    let (dir, copy) = if rewrite.is_empty() {
        (original, None)
    } else {
        // Unique per call: tests sharing a fixture run in parallel, and one removing the copy
        // while another compiles it was the flake (#4405).
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp =
            std::env::temp_dir().join(format!("bynk_tcalls_{}_{n}_{fixture}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        copy_dir(&original, &tmp);
        for (path, text) in rewrite {
            std::fs::write(tmp.join(path), text).unwrap();
        }
        (tmp.clone(), Some(tmp))
    };
    let result = super::e2e::compile_fixture(&dir, super::e2e::fixture_target(&dir));
    if let Some(tmp) = copy {
        let _ = std::fs::remove_dir_all(tmp);
    }
    let out = match result {
        Ok(out) => out,
        Err(failure) => panic!(
            "{fixture} should compile, got:\n{}",
            bynk_driver::render::render_project_errors(&failure.flatten())
        ),
    };

    out.artefacts
        .docs
        .values()
        .map(|d| d.text())
        .collect::<Vec<_>>()
        .join("\n")
}

/// A project of one context `r` (`src`) and one suite `r` (`suite`), built on the smallest fixture;
/// `workers` selects the Workers target a `system` suite needs.
fn project(src: &str, suite: &str, workers: bool) -> String {
    let mut files = vec![("src/r.bynk", src), ("tests/r.bynk", suite)];
    if workers {
        files.push(("target.txt", "workers"));
    }
    emit("2020_test_actor_sum_principal", &files)
}

/// The lines of `text` equal to `line` once indentation is trimmed.
fn assert_line(text: &str, line: &str) {
    assert!(
        text.lines().any(|l| l.trim_start() == line),
        "missing line {line:?} in\n{text}"
    );
}

fn assert_contains(text: &str, part: &str) {
    assert!(text.contains(part), "missing {part:?} in\n{text}");
}

/// An http context with a sum handler, a single-actor handler, a public one and one with a path
/// parameter. The identity is opaque, so a case builds it from a `let` (a node), not a literal.
const HTTP_CONTEXT: &str = r#"context r

type UserId = opaque String

actor User { auth = Bearer(secret = "AUTH_JWT_SECRET"), identity = UserId }

service api from http {
	on GET("/") () -> Effect[HttpResult[String]] by who: User | Visitor {
		match who {
			User(u) => Ok("user"),
			Visitor => Ok("anyone"),
		}
	}

	on GET("/plain") () -> Effect[HttpResult[String]] by u: User { Ok("plain") }

	on GET("/open") () -> Effect[HttpResult[String]] by Visitor { Ok("open") }

	on GET("/item/:id") (id: String) -> Effect[HttpResult[String]] by Visitor { Ok(id) }
}
"#;

/// Every principal form on an http address: a sum actor, a unit actor, an identity, none, and a
/// path argument. Each is a node, and the principal does not leak into the call
/// that follows it (the save and restore around a `by`).
#[test]
fn an_http_address_carries_each_principal_form_as_a_node() {
    let text = project(
        HTTP_CONTEXT,
        r#"suite r

case "principals" {
	let id = UserId.unsafe("ada")
	let a <- api.GET("/") by User(id)
	let b <- api.GET("/plain") by User(id)
	let c <- api.GET("/") by Visitor
	let d <- api.GET("/open")
	let e <- api.GET("/item/:id", "7")
	let f <- api.GET("/") by User(UserId.unsafe("bob"))
	let g <- api.GET("/open")
}
"#,
        false,
    );
    for line in [
        r#"const a = await api.http_GET({ ...deps, who: { tag: "User", identity: id } });"#,
        "const b = await api.http_GET_plain({ ...deps, identity: (id as any), claims: __tokenClaims([]) });",
        r#"const c = await api.http_GET({ ...deps, who: { tag: "Visitor" } });"#,
        "const d = await api.http_GET_open(deps);",
        r#"const e = await api.http_GET_item_Param_id("7", deps);"#,
        r#"const f = await api.http_GET({ ...deps, who: { tag: "User", identity: UserId.unsafe("bob") } });"#,
        "const g = await api.http_GET_open(deps);",
    ] {
        assert_line(&text, line);
    }
}

/// An `Oidc` handler also reads the principal's claims: its own, or none (#2380).
#[test]
fn an_oidc_handler_reads_its_claims_as_a_node() {
    let src = r#"context r

type UserId = opaque String

actor User {
	auth = Oidc(
		issuer = "https://issuer.example.com",
		audience = "my-api",
		jwks = "https://issuer.example.com/.well-known/jwks.json"
	),
	identity = UserId
}

service api from http {
	on GET("/me") () -> Effect[HttpResult[String]] by u: User { Ok("me") }
}
"#;
    let text = project(
        src,
        r#"suite r {
	case "claims" {
		let id = UserId.unsafe("ada")
		let a <- api.GET("/me") by User(id)
		let b <- api.GET("/me") by User(id, TokenClaims.of(["scope: read"]))
	}
}
"#,
        false,
    );
    assert_line(
        &text,
        "const a = await api.http_GET_me({ ...deps, identity: (id as any), claims: __tokenClaims([]) });",
    );
    assert_line(
        &text,
        r#"const b = await api.http_GET_me({ ...deps, identity: (id as any), claims: __tokenClaims(["scope: read"]) });"#,
    );
}

/// A branded principal (a refined literal's brand cast) is a node and prints byte for
/// byte as before.
#[test]
fn a_branded_principal_is_a_node_and_prints_byte_for_byte() {
    let text = emit("2020_test_actor_sum_principal", &[]);
    for line in [
        r#"const r = await api.http_GET({ ...deps, who: { tag: "User", identity: ("ada" as any) } });"#,
        r#"const r = await api.http_GET_plain({ ...deps, identity: (("bo" as any) as any), claims: __tokenClaims([]) });"#,
        r#"const v = await api.http_GET_greet({ ...deps, who: { tag: "Visitor" } });"#,
    ] {
        assert_line(&text, line);
    }
}

/// The plain `call`, cron and queue addresses, .
#[test]
fn plain_cron_and_queue_addresses_are_nodes() {
    let text = emit(
        "1934_case_outcome_typed_by_handler",
        &[(
            "tests/app/web.bynk",
            r#"suite app.web {
  case "addresses" {
    let a <- api.GET("/name") by Visitor
    let b <- calc.call(3)
    let c <- sweeper.schedule("0 0 * * *")
    let d <- intake.message(Item { sku: "w" })
    let e <- api.GET("/name")
  }
}
"#,
        )],
    );
    for line in [
        "const a = await api.http_GET_name(deps);",
        "const b = await calc.call(3, deps);",
        "const c = await sweeper.cron_sweeper_0(deps);",
        r#"const d = await intake.queue_intake_0({ sku: "w" }, deps);"#,
        "const e = await api.http_GET_name(deps);",
    ] {
        assert_line(&text, line);
    }
}

/// A target whose handlers send settles each addressed call, and virtual time drives then settles
/// it; a target with neither passes the bare call (above).
#[test]
fn an_addressed_call_is_settled_on_a_sending_or_a_virtual_time_target() {
    let sends = emit("1994_test_route_with_send", &[]);
    assert_line(
        &sends,
        "const r = await __bynkSettle(deps, api.http_GET(deps));",
    );
    let virtual_time = emit("2683_within_harness", &[]);
    assert_line(
        &virtual_time,
        "const r = await __bynkSettleVirtual(deps, quotes.call(1, deps));",
    );
}

/// An event address, with an envelope and without one (`undefined`).
#[test]
fn an_event_address_is_a_node_with_and_without_an_envelope() {
    let text = emit("2052_test_event_address", &[]);
    assert_line(
        &text,
        "const __r0 = await __bynkEvent_OnPlaced(e, env, deps);",
    );
    assert_line(
        &text,
        "const __r0 = await __bynkEvent_OnPlaced(e, undefined, deps);",
    );
    assert_line(
        &text,
        r#"const __r0 = await __bynkEvent_OnDomestic({ id: "o1", region: Region.International }, undefined, deps);"#,
    );
}

/// An inbound-email queue message reads the envelope sender with no DMARC verdict, with no principal
/// and with a named one that carries no identity (`by Sender`).
#[test]
fn an_inbound_email_message_is_built_with_and_without_a_principal() {
    for call in [
        r#"support.message(InboundEmail { from: "a@b.example", to: "support@shop.example" })"#,
        r#"support.message(InboundEmail { from: "a@b.example", to: "support@shop.example" }) by Sender"#,
    ] {
        let suite = format!(
            "suite shop.support {{\n  consumes bynk.cloudflare {{ EmailVerdict, InboundEmail }}\n  case \"a\" {{\n    let r <- {call}\n  }}\n}}\n"
        );
        let text = emit(
            "3328_email_catch_all_sender",
            &[("tests/support.bynk", suite.as_str())],
        );
        assert_contains(
            &text,
            r#"const r = await ((__m: any) => support.queue_support_0(__m, { ...deps, identity: { address: __m.from, dmarc: __dmarcVerdictOf(__m.headers) } }))({ from: "a@b.example""#,
        );
    }
}

/// The three websocket lifecycle addresses stay text (`bynk_ts` has no `typeof` or indexed-access
/// type), unchanged.
#[test]
fn the_websocket_lifecycle_addresses_stay_text() {
    let text = emit("3051_test_websocket_address", &[]);
    assert_contains(
        &text,
        r#"(async () => { const __c = alice; const __a: Parameters<typeof ChatGateway.open> = [__c, room, { ...deps, identity: (UserId.unsafe("alice") as any) }]; __c.remember([room], __a[__a.length - 1]); return ChatGateway.open(...__a); })()"#,
    );
    assert_contains(&text, "const __o = __c.opened();");
}

/// `svc.GET(asset)`: the argument is a plain identifier, so the redundant brackets go.
#[test]
fn an_asset_fetch_is_a_node() {
    let text = emit("1883_asset_suites_unit", &[]);
    assert_line(&text, "const r = await __bynkAssetGet(style.url);");
    assert_line(&text, "const r = await __bynkAssetGet(favicon.url);");
}

/// A `system` HTTP call on each driver the principal picks: none, `Visitor`, `Nobody`, `Forged`; a
/// path argument; the wrong method.
#[test]
fn a_system_http_call_drives_the_driver_its_principal_picks() {
    let text = project(
        HTTP_CONTEXT,
        r#"suite r as system {
	case "drivers" {
		let c <- api.GET("/") by Visitor
		let d <- api.GET("/open")
		let e <- api.GET("/item/:id", "7")
		let f <- api.GET("/plain") by Nobody
		let g <- api.GET("/plain") by Forged
		let i <- api.POST("/open")
	}
}
"#,
        true,
    );
    for line in [
        r#"const c = await __sysdrive_api_http_GET(undefined, "", "Visitor");"#,
        r#"const d = await __sysdrive_api_http_GET_open(undefined, "");"#,
        r#"const e = await __sysdrive_api_http_GET_item_Param_id("7", undefined, "");"#,
        r#"const f = await __sysdrive_noauth_api_http_GET_plain(undefined, "");"#,
        r#"const g = await __sysdrive_forged_api_http_GET_plain(undefined, "");"#,
        r#"const i = await __sysdrive_wrongmethod_api("POST", "/open");"#,
    ] {
        assert_line(&text, line);
    }
}

/// The raw driver converts each typed argument to its string slot: a typed body (JSON), a `@form`
/// body, a query record and a cookies record go through the wire codec, a typed path argument
/// through `String(...)`.
#[test]
fn a_raw_system_call_converts_each_typed_argument_to_its_slot() {
    let src = r#"context r

type Theme = enum { Light, Dark }
type Note = { text: String }
type Session = { theme: Theme = Light, visits: Int = 0 }
type Filter = { limit: Int = 50, fresh: Bool = false }
type Form = { q: String }

service api from http {
	on PUT("/note/:id") (id: String, body: Note) -> Effect[HttpResult[String]] by v: Visitor { Ok(id) }

	on GET("/me/:id") (id: String, cookies: Session) -> Effect[HttpResult[String]] by v: Visitor { Ok(id) }

	on GET("/find/:id") (id: String, query: Filter) -> Effect[HttpResult[String]] by v: Visitor { Ok(id) }

	@form
	on PUT("/form/:id") (id: String, body: Form) -> Effect[HttpResult[String]] by v: Visitor { Ok(id) }
}
"#;
    let text = project(
        src,
        r#"suite r as system {
	case "slots" {
		let a <- api.PUT("/note/:id", Wire("n1"), Note { text: "hi" })
		let b <- api.GET("/me/:id", Wire("n1"), Session { theme: Theme.Dark, visits: 3 })
		let c <- api.GET("/find/:id", Wire("n1"), Filter { limit: 7, fresh: true })
		let d <- api.PUT("/form/:id", Wire("n1"), Form { q: "a b" })
		let e <- api.PUT("/note/:id", "n1", Wire("{}"))
	}
}
"#,
        true,
    );
    for line in [
        r#"const a = await __sysdrive_raw_api_http_PUT_note_Param_id("n1", globalThis.JSON.stringify(__ns_r.__serialise_Note(({ text: "hi" } as any))), undefined, "");"#,
        r#"const b = await __sysdrive_raw_api_http_GET_me_Param_id("n1", __cookieString(__ns_r.__serialise_Session(({ theme: Theme.Dark, visits: 3 } as any))), undefined, "");"#,
        r#"const c = await __sysdrive_raw_api_http_GET_find_Param_id("n1", __queryString(__ns_r.__serialise_Filter(({ limit: 7, fresh: true } as any))), undefined, "");"#,
        r#"const d = await __sysdrive_raw_api_http_PUT_form_Param_id("n1", __queryString(__ns_r.__serialise_Form(({ q: "a b" } as any))).slice(1), undefined, "");"#,
        r#"const e = await __sysdrive_raw_api_http_PUT_note_Param_id(String("n1"), "{}", undefined, "");"#,
    ] {
        assert_line(&text, line);
    }
}

/// A wrong-method call writes the verb and the path as strings; a path with `"`, `\` and non-ASCII
/// text is escaped as Rust's `{:?}` escaped it.
#[test]
fn a_wrong_method_call_escapes_its_path_as_before() {
    let src = r#"context r

service api from http {
	on GET("/café") () -> Effect[HttpResult[String]] by Visitor { Ok("a") }
	on GET("/q\"x\\y") () -> Effect[HttpResult[String]] by Visitor { Ok("b") }
}
"#;
    let text = project(
        src,
        r#"suite r as system {
	case "paths" {
		let a <- api.POST("/café")
		let b <- api.POST("/q\"x\\y")
	}
}
"#,
        true,
    );
    assert_line(
        &text,
        r#"const a = await __sysdrive_wrongmethod_api("POST", "/café");"#,
    );
    assert_line(
        &text,
        r#"const b = await __sysdrive_wrongmethod_api("POST", "/q\"x\\y");"#,
    );
}

/// The system principal's claims and acting actor reach the signing drivers: a refinement actor's
/// witness claims (a table built in `bynk-driver`, kept as text), an `Oidc` call's claims lines, and
/// an actor-sum route's actor name; the no-auth and forged drivers sign nothing.
#[test]
fn a_system_call_passes_its_claims_and_its_acting_actor() {
    let refinement = emit("2019_system_refinement_actor_principal", &[]);
    assert_line(
        &refinement,
        r#"const r = await __sysdrive_api_http_GET_admin(undefined, ("root" as any), { "admin": true });"#,
    );
    assert_line(
        &refinement,
        r#"const r = await __sysdrive_api_http_GET_admin(undefined, ("root" as any), {});"#,
    );
    let oidc = emit("3052_system_oidc_refusals", &[]);
    assert_line(
        &oidc,
        r#"const r = await __sysdrive_api_http_GET_who(undefined, ("wid" as any), __bynkClaimLines(["repository: acme/money"]));"#,
    );
    assert_line(
        &oidc,
        r#"const r = await __sysdrive_raw_api_http_POST_release("\"v1\"", undefined, ("wid" as any), __bynkClaimLines([]));"#,
    );
    let sum = emit("2155_system_sum_route_credential", &[]);
    assert_line(
        &sum,
        r#"const r = await __sysdrive_api_http_POST_notify("hi", undefined, "", "Processor");"#,
    );
    assert_line(
        &sum,
        r#"const forged = await __sysdrive_forged_api_http_POST_notify("hi", undefined, "");"#,
    );
}

/// A system cron or queue address drives the participant Worker's real entry.
#[test]
fn a_system_cron_or_queue_address_drives_its_driver() {
    let text = emit("2136_system_cron_queue", &[]);
    assert_line(&text, "const a = await __sysdrive_message_deliveries(d);");
    let text = emit(
        "2131_cron_queue_address_in_process",
        &[(
            "tests/jobs_system.bynk",
            r#"suite cr.jobs as system {
	case "a" {
		let r <- sweeper.schedule("*/5 * * * *", Instant.fromEpochMillis(960000))
		let d <- deliveries.message(Delivery { id: "c" })
	}
}
"#,
        )],
    );
    assert_line(
        &text,
        "const r = await __sysdrive_schedule_sweeper_0(960000);",
    );
    assert_line(
        &text,
        r#"const d = await __sysdrive_message_deliveries({ id: "c" });"#,
    );
}

/// `extra headers` (#2910) take a slot ahead of the identity.
#[test]
fn a_system_call_with_extra_headers_fills_the_slot_ahead_of_sub() {
    let text = emit("2931_system_extra_headers_credentialed", &[]);
    assert_line(
        &text,
        r#"const r = await __sysdrive_api_http_GET_items({ limit: 7 }, __requestHeaders(["X-Trace: 1"]), "");"#,
    );
}

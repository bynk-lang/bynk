//! 1A.25 (#3726): the cross-context service call (the bundle `deps.surface.<key>.<method>(args)`
//! and the Workers `__callService(…)`) and its boundary casts lower to nodes. Each project is
//! emitted in memory and the exact line is pinned; the text-exit counter is read where it is real
//! (the driver links a non-test copy of `bynk-emit`, see `text_exit_counters.rs`). The checker-
//! agreement panics and the cast builder are unit tests in `lower_call/cross_context.rs`.
//!
use bynk_emit::emitter::text_counters;

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

/// `(text exits, every emitted document joined)` for a fixture project, built as the fixture suite
/// builds it. `rewrite` replaces files of a temporary copy (`(path in the fixture, new text)`), so
/// a suite's own unrelated constructs can be taken out of the count.
fn emit(fixture: &str, rewrite: &[(&str, &str)]) -> (usize, String) {
    let original = fixture_dir(fixture);
    let (dir, copy) = if rewrite.is_empty() {
        (original, None)
    } else {
        let tmp =
            std::env::temp_dir().join(format!("bynk_platform_{}_{fixture}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        copy_dir(&original, &tmp);
        for (path, text) in rewrite {
            std::fs::write(tmp.join(path), text).unwrap();
        }
        (tmp.clone(), Some(tmp))
    };
    text_counters::reset();
    let result = super::e2e::compile_fixture(&dir, super::e2e::fixture_target(&dir));
    let exits = text_counters::exits();
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
    let text = out
        .artefacts
        .docs
        .values()
        .map(|d| d.text())
        .collect::<Vec<_>>()
        .join("\n");
    (exits, text)
}

/// Every line in `text` equals one of `lines` (trimmed of indentation), in order, as a block.
fn assert_block(text: &str, block: &[&str]) {
    let lines: Vec<&str> = text.lines().map(str::trim_start).collect();
    assert!(
        lines.windows(block.len()).any(|w| w == block),
        "missing block {block:#?} in\n{text}"
    );
}

fn assert_line(text: &str, line: &str) {
    assert_block(text, &[line]);
}

const PAYMENT_BOOL: &str = "context commerce.payment

service authorise {
  on call(ok: Bool) -> Effect[Result[Int, Int]] {
    Ok(1)
  }
}
";

const ORDERS_TEXT_ARG: &str = "context commerce.orders

consumes commerce.payment

service placeOrder {
  on call(total: Int) -> Effect[Result[Int, Int]] {
    let _ <- commerce.payment.authorise(\"a\".splitRegex(\",\").length() > 0)
    Ok(total)
  }
}
";

/// A bundle call with no argument, one argument and one cast argument: a node, with no text exit.
#[test]
fn a_bundle_call_is_a_node_with_zero_one_and_a_cast_argument() {
    for (fixture, line) in [
        (
            "1807_uses_reaches_hidden_type",
            "const t = await deps.surface.Vault.fetch();",
        ),
        (
            "97_simple_cross_context_call",
            "const __r0 = await deps.surface.payment.authorise(total);",
        ),
        (
            "99_cross_context_with_money",
            "const __r0 = await deps.surface.payment.authorise((total as unknown as commerce_payment.Money));",
        ),
    ] {
        let (exits, ts) = emit(fixture, &[]);
        assert_line(&ts, line);
        // `1807`'s own suite carries unrelated text constructs; the call is pinned by its line.
        if fixture != "1807_uses_reaches_hidden_type" {
            assert_eq!(exits, 0, "{fixture}: no cross-context call left as text");
        }
    }
}

/// Many arguments are cast one by one, in order, each to the callee's own named type.
#[test]
fn a_bundle_call_casts_each_argument_in_order() {
    let (exits, ts) = emit("2802_stronger_sender_crosses_boundary", &[]);
    assert_line(
        &ts,
        "const __r0 = await deps.surface.Provider.describe((label as unknown as boundary_provider.Label), (qty as unknown as boundary_provider.Qty), (ratio as unknown as boundary_provider.Ratio), (price as unknown as boundary_provider.Price), (slug as unknown as boundary_provider.Slug));",
    );
    assert_eq!(exits, 0);
}

/// A literal the checker admitted to the callee's refined type (`holds_callee_typed`) and a value
/// at a generic parameter are cast to the callee's type through `unknown`; the aliased surface key
/// is the alias.
#[test]
fn a_literal_and_a_generic_argument_are_cast_to_the_callee_type() {
    let (_, ts) = emit("2887_consumes_alias_service_bundle", &[]);
    assert_line(
        &ts,
        "const flipped = await deps.surface.lib.flip((LibCode.Alpha as unknown as lib.Code));",
    );
    assert_line(
        &ts,
        "const l = await deps.surface.lib.ping(((3 as lib.Level) as unknown as lib.Level));",
    );
}

/// #2825, #2828: a result that reaches a refined or opaque type the caller binds is cast to the
/// caller's declaration, through `unknown`; one that does not is the bare call.
#[test]
fn a_bundle_result_is_cast_only_where_it_reaches_a_caller_bound_brand() {
    let (exits, ts) = emit("2825_same_name_refined_return", &[]);
    for line in [
        "const r = await (deps.surface.Provider.reply(n) as unknown as globalThis.Promise<Result<Reply, number>>);",
        "const rs = await (deps.surface.Provider.replies(n) as unknown as globalThis.Promise<readonly Reply[]>);",
        "const r = await (deps.surface.Provider.mixed(n) as unknown as globalThis.Promise<Result<Reply, boundary_provider.Fault>>);",
        "const h = await (deps.surface.Provider.handle(n) as unknown as globalThis.Promise<Result<Handle, number>>);",
    ] {
        assert_line(&ts, line);
    }
    assert_eq!(exits, 0);
    let (exits, ts) = emit("2828_shared_commons_and_nested_brand_return", &[]);
    assert_line(&ts, "const r = await deps.surface.Provider.tagged(n);");
    assert_eq!(exits, 0);
}

/// Workers: a zero-argument call sends `{}` (it was `{  }`), one argument is serialised bare,
/// many are an object of the parameter names. The contract hash stamped on the call is unchanged.
#[test]
fn a_workers_call_is_a_node_with_zero_one_and_many_arguments() {
    let (exits, ts) = emit("172_integration_with_capability", &[]);
    assert!(
        ts.contains("__callService(deps.env.OPS_JOBS, \"tick\", {}, "),
        "{ts}"
    );
    assert!(ts.contains("\"12c6f9c0fb6f00af\", deps.env, \"ops.jobs\")"));
    assert!(!ts.contains("{  }"), "no two-space payload");
    assert_eq!(exits, 0, "the zero-argument call is a node");

    let (exits, ts) = emit("118_workers_target_with_consume", &[]);
    assert_line(
        &ts,
        "const auth = await __callService(deps.env.COMMERCE_PAYMENT, \"authorise\", __serialise_Money((total as unknown as commerce_payment.Money)), __deserialise_Result_AuthId_PaymentError, \"commerce.orders\", \"d7dff23f6fd4c9e4\", deps.env, \"commerce.payment\");",
    );
    assert_eq!(exits, 0, "the one-argument call is a node");

    let (_, ts) = emit("1818_agent_reads_its_key", &[]);
    assert!(
        ts.contains("__callService(deps.env.SHOP_ONE, \"sum\", { x: ((v: number) =>"),
        "{ts}"
    );
    assert!(ts.contains(", y: ((v: number) =>"), "{ts}");
}

/// A literal admitted to the callee's type, where the consumer renamed it, is cast to what the
/// codec takes (`Parameters<typeof codec>[0]`, the one arm `bynk_ts` has no type for).
#[test]
fn a_workers_renamed_literal_is_cast_to_the_codec_parameter() {
    let (_, ts) = emit("2039_nested_refined_literal", &[]);
    assert!(
        ts.contains("__serialise_Option_t_pay__Cents((Some((250 as Cents)) as unknown as globalThis.Parameters<typeof __serialise_Option_t_pay__Cents>[0]))"),
        "{ts}"
    );
    assert!(
        ts.contains("__serialise_List_t_pay__Cents(([(10 as Cents), (20 as Cents)] as unknown as globalThis.Parameters<typeof __serialise_List_t_pay__Cents>[0]))"),
        "{ts}"
    );
}

/// A test scaffold reaches the callee's codecs through its namespace, and a production module
/// reaches them locally.
#[test]
fn a_test_scaffold_call_is_namespaced_and_a_production_call_is_not() {
    let (_, ts) = emit("172_integration_with_capability", &[]);
    assert!(
        ts.contains("__callService(deps.env.OPS_JOBS, \"tick\", {}, __ns_ops_jobs.__deserialise_Result_Int_TickError, \"integration\", "),
        "{ts}"
    );
    let (_, ts) = emit("118_workers_target_with_consume", &[]);
    assert!(
        !ts.contains("__ns_commerce_payment.__deserialise"),
        "a production call names the codecs locally"
    );
}

/// #1961: a local that shadows the consumed alias is a UFCS call, not a cross-context call.
#[test]
fn a_local_shadowing_the_alias_falls_through_to_ufcs() {
    let (exits, ts) = emit("1203_cross_context_call_shadowed_by_local", &[]);
    assert_line(
        &ts,
        "const r = await LocalLedger.authorise(Ledger, amount);",
    );
    assert!(!ts.contains("__callService(deps.env.SHOP_LEDGER"), "{ts}");
    assert_eq!(exits, 0);
}

/// A traced build wraps the call in a trace step and its binding; an untraced build is the bare
/// call. Both are nodes.
#[test]
fn a_traced_and_an_untraced_call_are_both_nodes() {
    let fixture = fixture_dir("174_integration_full_checkout");
    for traced in [false, true] {
        let options = bynk_testkit::compile_options_single(fixture.join("src"))
            .trace_requests(traced)
            .target(bynk_driver::project::BuildTarget::Workers)
            .tests(false);
        let out = bynk_driver::project::compile_project(&options)
            .map_err(bynk_driver::project::ProjectFailure::flatten)
            .unwrap_or_else(|e| panic!("compile failed: {e:?}"));
        let ts = out
            .artefacts
            .docs
            .iter()
            .filter(|(p, _)| p.to_string_lossy().ends_with("shop-orders/handlers.ts"))
            .map(|(_, d)| d.text())
            .collect::<String>();
        if traced {
            assert!(
                ts.contains("__traceStep(\"call\", \"shop.payment.authorise\", { file: \"shop/orders.bynk\", line: 23, col: 14 })(__callService(__traceBinding(deps.env.SHOP_PAYMENT), \"authorise\", "),
                "{ts}"
            );
        } else {
            assert!(
                ts.contains("await __callService(deps.env.SHOP_PAYMENT, \"authorise\", "),
                "{ts}"
            );
            assert!(!ts.contains("__traceStep"));
        }
    }
}

/// An argument that is still text keeps its text, byte for byte, in the cast and the call, on
/// both targets.
#[test]
fn a_text_argument_falls_back_byte_for_byte() {
    for (fixture, call) in [
        (
            "97_simple_cross_context_call",
            "await deps.surface.payment.authorise((((__s: string, __r: globalThis.RegExp): readonly string[] => { if (__s.length === 0) { return __r.exec(__s) === null ? [\"\"] : []; } const __out: string[] = []; let __p = 0; let __q = 0; while (__q < __s.length) { __r.lastIndex = __q; const __hit = __r.exec(__s); const __e = __r.lastIndex; if (__hit === null || __e === __p) { __q++; } else { __out.push(__s.slice(__p, __q)); __p = __e; __q = __p; } } __out.push(__s.slice(__p)); return __out; })(\"a\", new globalThis.RegExp(\",\", \"y\"))).length > 0)",
        ),
        (
            "118_workers_target_with_consume",
            "__callService(deps.env.COMMERCE_PAYMENT, \"authorise\", ",
        ),
    ] {
        let (exits, ts) = emit(
            fixture,
            &[
                ("src/commerce/payment.bynk", PAYMENT_BOOL),
                ("src/commerce/orders.bynk", ORDERS_TEXT_ARG),
            ],
        );
        assert!(ts.contains(call), "{fixture}: {ts}");
        assert!(exits > 0, "{fixture}: the text argument is a text exit");
    }
}

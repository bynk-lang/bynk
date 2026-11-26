//! 1A.17 (#3718): the test-domain constructs (`expect`, `advance`, `Val`,
//! observations, `trace` and fault claims) lower to nodes.
//!
//! Each project is emitted in memory and the exact lines are pinned.

use bynk_check::project_model::BuildTarget;

const PAYMENT: &str =
    include_str!("../fixtures/positive/246_observation_surface/src/commerce/payment.bynk");
const VAULT: &str = include_str!("../fixtures/positive/1706_expect_faults/src/demo/vault.bynk");
const MONEY: &str = include_str!("../fixtures/positive/144_mock_refined/src/commerce/money.bynk");
const SHAPES: &str = include_str!("../fixtures/positive/145_mock_bare_kinds/src/demo/shapes.bynk");
const HOLDS: &str = include_str!("../fixtures/positive/2137_harness_clock/src/ck/holds.bynk");

/// `the test module's TypeScript` for a project of `ctx` and one
/// suite.
fn emit(ctx_path: &str, ctx: &str, suite: &str) -> String {
    let out = bynk_driver::project::compile_files_in_memory(
        &[(ctx_path, ctx), ("tests/t.bynk", suite)],
        BuildTarget::Bundle,
        Default::default(),
        true,
    )
    .unwrap_or_else(|_| panic!("fixture should compile:\n{suite}"));
    let text: String = out
        .artefacts
        .docs
        .iter()
        .filter(|(p, _)| p.to_string_lossy().starts_with("tests/"))
        .map(|(_, d)| d.text())
        .collect::<Vec<_>>()
        .join("\n");
    text
}

/// The suite with each `lines` entry as one `expect` in a case, and the
/// baseline with every one replaced by `expect true`.
fn suite(header: &str, prelude: &str, lines: &[&str], name: &str) -> (String, String) {
    let build = |f: &dyn Fn(&str) -> String| {
        let body: String = lines.iter().map(|l| format!("    {}\n", f(l))).collect();
        format!("suite {name} {{\n{header}\n  case \"c\" {{\n{prelude}{body}  }}\n}}\n")
    };
    (
        build(&|l| l.to_string()),
        build(&|_| "expect true".to_string()),
    )
}

fn emit_suite(
    ctx_path: &str,
    ctx: &str,
    header: &str,
    prelude: &str,
    lines: &[&str],
    name: &str,
) -> String {
    let (with, _) = suite(header, prelude, lines, name);
    emit(ctx_path, ctx, &with)
}

const PAYMENT_HEADER: &str = "  stub Logger.log(_) returns ()\n  stub Logger.warn(_) returns ()";
const PAYMENT_PRELUDE: &str = "    let r <- authorise.call(100)\n";

#[test]
fn observations_and_trace_are_nodes() {
    let text = emit_suite(
        "commerce/payment.bynk",
        PAYMENT,
        PAYMENT_HEADER,
        PAYMENT_PRELUDE,
        &[
            "expect Logger.log never called",
            "expect Logger.log called",
            "expect Logger.log called 2 times",
            "expect Logger.warn called once",
            "expect Logger.log before Logger.warn",
            "expect Logger.log called with msg == \"start\"",
            "expect Logger.log called 1 times with msg == \"checking\"",
        ],
        "commerce.payment",
    );
    assert!(
        text.contains("(__obs.log[\"Logger.log\"] ?? []).length === 0)"),
        "{text}"
    );
    assert!(text.contains(".length >= 1)"), "{text}");
    assert!(text.contains(".length === (2))"), "{text}");
    assert!(text.contains("[0].order < "), "{text}");
    // The `Cap.op` key prints through the printer's escaper exactly as the
    // old `{key:?}` did: no escapes for an identifier pair.
    assert!(text.contains("__obs.log[\"Logger.warn\"]"), "{text}");
    // The `with` predicate's arrow body is the multi-line form.
    assert!(
        text.contains(".filter((__c: { args: unknown[] }) => {\n      const { msg } = { msg: __c.args[0] } as __Logger_log_Call;\n      return (msg === \"start\");\n    }).length"),
        "{text}"
    );

    let with = suite(
        PAYMENT_HEADER,
        PAYMENT_PRELUDE,
        &["let calls = trace(Logger.log)\n    expect true"],
        "commerce.payment",
    )
    .0;
    let trace = emit("commerce/payment.bynk", PAYMENT, &with);
    assert!(
        trace.contains("(((__obs.log[\"Logger.log\"] ?? []).map((__c: { args: unknown[] }) => ({ msg: __c.args[0] }))) as __Logger_log_Call[])"),
        "{trace}"
    );
}

#[test]
fn a_fault_claim_is_a_try_catch_arrow_that_rethrows_expectation_errors() {
    // A pure call, so the claim's child is a node.
    const BOOM: &str = "commons demo.boom\n\nfn boom(x: Int) -> Effect[Int] { Effect.pure(x) }\n";
    let text = emit_suite(
        "demo/boom.bynk",
        BOOM,
        "",
        "",
        &["expect boom(1) faults"],
        "demo.boom",
    );
    for want in [
        "(await (async () => {\n      try {\n        await (boom(1));\n        return false;\n      } catch (__e) {\n        if (__e instanceof ExpectationError) {\n          throw __e;\n        }\n        return true;\n      }\n    })())",
        "expect boom(1) faults\\n  the call returned without faulting",
    ] {
        assert!(text.contains(want), "missing {want:?} in:\n{text}");
    }
}

#[test]
fn expect_is_a_one_line_if_over_a_not_of_the_parenthesised_condition() {
    let text = emit_suite(
        "demo/vault.bynk",
        VAULT,
        "  stub Vault.open() returns 1",
        "    let r <- box.call()\n",
        &["expect r == 2", "expect r > 0"],
        "demo.vault",
    );
    // The two layout probes: `!(cond)` takes no extra brackets, and the
    // `throw` stays on the `if`'s own line.
    assert!(
        text.contains("if (!(r === 2)) { throw __bynkExpectFailure(\"tests/t.bynk:"),
        "{text}"
    );
    assert!(
        text.contains("\"expect r == 2\\n  expected: r == 2\\n  actual:   \" + __bynkShow((r)) + \" == \" + __bynkShow((2)));"),
        "{text}"
    );
}

#[test]
fn val_builds_nodes_for_every_shape() {
    let text = emit(
        "commerce/money.bynk",
        MONEY,
        "suite commerce.money {\n  case \"c\" {\n    let d = Val[Quantity]\n    let q = Val[Quantity](50)\n    expect d == q\n  }\n}\n",
    );
    assert!(text.contains("const d = (1 as any);"), "{text}");
    assert!(text.contains("const q = (50 as any);"), "{text}");

    let shapes = emit(
        "demo/shapes.bynk",
        SHAPES,
        "suite demo.shapes {\n  case \"c\" {\n    let s = Val[Status]\n    let c = Val[Cart]\n    expect s is Active(_)\n  }\n}\n",
    );
    assert!(shapes.contains("const s = Status.Active(0);"), "{shapes}");
    assert!(
        shapes.contains("const c = { id: OrderId.unsafe(\"mock\"), count: 0 };"),
        "{shapes}"
    );
}

#[test]
fn advance_is_a_call_statement() {
    let text = emit(
        "ck/holds.bynk",
        HOLDS,
        "suite ck.holds {\n  case \"c\" {\n    advance Clock by 16.minutes\n    let back = 0.minutes - 1.minute\n    advance Clock by back\n  }\n}\n",
    );
    assert!(text.contains("__bynkAdvanceClock(960000);"), "{text}");
    assert!(text.contains("__bynkAdvanceClock(back);"), "{text}");
}

/// Hoisting order (#3718, Risks). `expect len(Day.of("x")?) == 1` hoists the
/// `?` guard out of the comparison's left operand. The hoisted statements must
/// land before the `if (!(…))`. Each operand is lowered once and bound to a
/// temporary that the condition and the failure detail both read, so
/// `Day.of(` runs once, not once for the condition and once for the detail.
#[test]
fn a_hoisting_operand_runs_once_before_the_expect_and_its_detail_reads_the_temporary() {
    const DAY: &str = "commons demo.day\n\ntype Day = String where NonEmpty\n\nfn len(d: Day) -> Int { d.length() }\n";
    let text = emit(
        "demo/day.bynk",
        DAY,
        "suite demo.day\n\ncase \"q\" {\n  expect len(Day.of(\"x\")?) == 1\n}\n",
    );
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    let at = |needle: &str| {
        lines
            .iter()
            .position(|l| l.contains(needle))
            .unwrap_or_else(|| panic!("no line with {needle:?} in:\n{text}"))
    };
    let temp = at("const __r0 = Day.of(");
    let guard = at("tag === \"Err\"");
    let left = at("const __r1 = len(__r0.value);");
    let right = at("const __r2 = 1;");
    let expect = lines
        .iter()
        .position(|l| l.starts_with("if (!("))
        .unwrap_or_else(|| panic!("no expect in:\n{text}"));
    assert!(
        temp < guard && guard < left && left < right && right < expect,
        "hoisted statements precede the expect:\n{text}"
    );
    let line = lines[expect];
    assert!(line.starts_with("if (!(__r1 === __r2))"), "{line}");
    assert!(line.contains("__bynkShow((__r1))"), "{line}");
    assert!(line.contains("__bynkShow((__r2))"), "{line}");
    assert_eq!(
        lines.iter().filter(|l| l.contains("= Day.of(")).count(),
        1,
        "{text}"
    );
}

/// When only the right operand hoists, the left one is still bound after the
/// right one's hoists, so the order matches the single condition it replaces
/// (`1 === len(__r0.value)` read the left operand after the guard too).
#[test]
fn a_hoisting_right_operand_binds_both_temporaries_after_its_hoists() {
    const DAY: &str = "commons demo.day\n\ntype Day = String where NonEmpty\n\nfn len(d: Day) -> Int { d.length() }\n";
    let text = emit(
        "demo/day.bynk",
        DAY,
        "suite demo.day\n\ncase \"q\" {\n  expect 1 == len(Day.of(\"x\")?)\n}\n",
    );
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    let at = |needle: &str| {
        lines
            .iter()
            .position(|l| l.contains(needle))
            .unwrap_or_else(|| panic!("no line with {needle:?} in:\n{text}"))
    };
    let temp = at("const __r0 = Day.of(");
    let guard = at("tag === \"Err\"");
    let left = at("const __r1 = 1;");
    let right = at("const __r2 = len(__r0.value);");
    let expect = at("if (!(__r1 === __r2))");
    assert!(
        temp < guard && guard < left && left < right && right < expect,
        "{text}"
    );
}

/// An operand with no hoist is not bound: the condition and the detail read
/// the operand itself, as before.
#[test]
fn a_comparison_whose_operands_hoist_nothing_binds_no_temporary() {
    const DAY: &str = "commons demo.day\n\nfn len(d: String) -> Int { d.length() }\n";
    let text = emit(
        "demo/day.bynk",
        DAY,
        "suite demo.day\n\ncase \"q\" {\n  expect len(\"x\") == 1\n}\n",
    );
    assert!(!text.contains("const __r"), "{text}");
    assert!(text.contains("if (!(len(\"x\") === 1))"), "{text}");
}

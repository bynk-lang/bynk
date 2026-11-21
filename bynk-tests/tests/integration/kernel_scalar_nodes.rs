//! 1A.29 (#3730): the scalar kernels (`Int`, `Float`, `Decimal`, `Duration`, `Instant`, `Bytes`,
//! `String`) lower to nodes. A project is emitted through the driver and the exact lines are pinned.
//! The arms' own forms are unit tests in `lower_call/kernel_scalar.rs`.

/// `every emitted document joined` for the one-context project `src` on Workers.
fn mem(src: &str) -> String {
    let out = bynk_driver::project::compile_files_in_memory(
        &[("ops/jobs.bynk", src)],
        bynk_check::project_model::BuildTarget::Workers,
        Default::default(),
        true,
    )
    .unwrap_or_else(|e| {
        panic!(
            "should compile:\n{src}\n{}",
            bynk_driver::render::render_project_errors(&e.flatten())
        )
    });

    out.artefacts
        .docs
        .values()
        .map(|d| d.text())
        .collect::<Vec<_>>()
        .join("\n")
}

fn assert_line(text: &str, line: &str) {
    assert!(
        text.lines().any(|l| l.trim_start() == line),
        "missing line {line:?} in\n{text}"
    );
}

/// Every scalar family over identifiers, in one context: the exact lines.
#[test]
fn the_scalar_kernels_print_as_nodes() {
    let src = r#"context ops.jobs

fn ints(n: Int, m: Int) -> Int { n.abs().max(m).clamp(0, 9) }
fn floors(x: Float) -> Int { x.floor() }
fn render(n: Int) -> String { n.toString() }
fn widths(s: String, b: Bytes) -> Int { s.trim().length() + b.length() }
fn parts(s: String, t: String) -> List[String] { s.split(t) }
fn letters(s: String) -> List[String] { s.chars() }
fn where_(s: String, t: String) -> Option[Int] { s.indexOf(t) }
fn swap(s: String, t: String, u: String) -> String { s.replace(t, u) }
fn digits(s: String) -> Bool { s.matchesRegex("\\d+") }
fn tidy(s: String) -> String { s.replaceRegex("\\s+", " ") }
fn hex(b: Bytes) -> String { b.toHex() }
fn bigger(d: Decimal, e: Decimal) -> Decimal { d.max(e) }
fn rounded(d: Decimal) -> Decimal { d.round(2, RoundingMode.HalfUp) }
fn stamp(i: Instant) -> String { i.toIso() }
"#;
    let text = mem(src);
    for line in [
        "return globalThis.Math.min(globalThis.Math.max(globalThis.Math.max(globalThis.Math.abs(n), m), 0), 9);",
        r#"return __bynkToInt(globalThis.Math.floor(x), "floor");"#,
        "return String(n);",
        "return s.trim().length + b.length;",
        "return s.split(t);",
        "return [...s];",
        "return ((__i: number) => __i < 0 ? None : Some(__i))(s.indexOf(t));",
        "return s.replaceAll(t, () => u);",
        r#"return new globalThis.RegExp("(?<![\\s\\S])(?:\\d+)(?![\\s\\S])", "").test(s);"#,
        r#"return s.replace(new globalThis.RegExp("\\s+", "g"), " ");"#,
        "return __bynkBytesToHex(b);",
        "return __bynkDecimalMax(d, e);",
        r#"return __bynkDecimalRound(d, 2, "HalfUp");"#,
        "return __bynkInstantToIso(i);",
    ] {
        assert_line(&text, line);
    }
    assert!(
        !text.contains("(s).length") && !text.contains("(b).length"),
        "{text}"
    );
}

/// The identity arms behind #3770 are nodes: the operand keeps its
/// brackets under the tighter `*`.
#[test]
fn the_3770_identity_arms_bracket_their_operand() {
    let src = r#"context ops.jobs

fn f(a: Int, b: Int) -> Int { Duration.millis(a + b).toMillis() * 2 }
"#;
    let text = mem(src);
    assert_line(&text, "return (a + b) * 2;");
}

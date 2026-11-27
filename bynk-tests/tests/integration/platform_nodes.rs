//! 1A.24 (#3725): the capability and platform calls (a capability call in a handler, a provider
//! body, a test case and through a `D1.peek` handle; a decoded reply under the `fault` policy;
//! Postgres and MySQL `select`/`firstAs`) lower to nodes. Each project is emitted in memory and the
//! exact line is pinned.
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
/// builds it. `rewrite` replaces files of a temporary copy (`(path in the fixture, new text)`), so
/// a suite's own unrelated constructs can be taken out of the count.
fn emit(fixture: &str, rewrite: &[(&str, &str)]) -> String {
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

/// A capability call in a service handler reaches `deps`, and an encoded argument is serialised.
#[test]
fn a_capability_call_in_a_handler_is_a_node() {
    // The suite's own record comparisons are not under test, so a trivial suite stands in for it.
    let ts = emit(
        "3458_typed_capability_reads",
        &[(
            "tests/demo/store.bynk",
            "suite demo.store as integration\n\ncase \"trivial\" {\n\texpect true\n}\n",
        )],
    );
    assert_line(
        &ts,
        "return deps.TypedReads.write(key, __serialise_Receipt(receipt));",
    );
}

/// A decoded reply wraps the call in `__bynkCapabilityReply`: a miss for an `Option` payload, and
/// a fault for the payload that declares no miss.
#[test]
fn a_decoded_reply_is_pinned_under_each_policy() {
    let ts = emit("3458_typed_capability_reads", &[]);
    assert_line(
        &ts,
        "return __bynkCapabilityReply(deps.TypedReads.readOpt(key), { encode: __serialise_Option_Receipt, decode: __deserialise_Option_Receipt }, { capability: \"TypedReads\", operation: \"readOpt\", type: \"Option[Receipt]\" }, \"miss\");",
    );
    assert_line(
        &ts,
        "return __bynkCapabilityReply(deps.TypedReads.read(key), { encode: __serialise_Receipt, decode: __deserialise_Receipt }, { capability: \"TypedReads\", operation: \"read\", type: \"Receipt\" }, \"fault\");",
    );
}

/// A capability call in a provider body reaches `this.deps`.
#[test]
fn a_capability_call_in_a_provider_body_is_a_node() {
    let ts = emit("1976_secrets_provider_body_workers", &[]);
    assert_line(&ts, "const t = await this.deps.Secrets.get(\"TONE\");");
}

/// A capability call in a test `case` reaches the case's own `deps`.
#[test]
fn a_capability_call_in_a_test_case_is_a_node() {
    let ts = emit("1975_suite_on_adapter", &[]);
    assert_line(&ts, "const s = await deps.Greeter.hello(\"ann\");");
}

/// `D1.peek` binds the participant's capability over its own database, read-only, and a read
/// through the handle is a block-bodied IIFE on `deps`.
#[test]
fn a_d1_peek_handle_and_its_reads_are_nodes() {
    let ts = emit("3147_d1_peek_system", &[]);
    assert_line(
        &ts,
        "const db = (deps.__peek_shop_orders_Orders = new bynk_cloudflare__binding.WorkersD1({ D1_ORDERS: deps.env___ns_shop_orders.D1_ORDERS.readOnly() }, \"D1_ORDERS\"));",
    );
    assert_block(
        &ts,
        &[
            "if (__r.tag === \"Err\") return Err({ tag: \"Sql\" as const, error: __r.error });",
            "const __d = __decode(__r.value);",
            "return __d.tag === \"Ok\" ? Ok(__d.value) : Err({ tag: \"Decode\" as const, error: __d.error });",
        ],
    );
    // The suite's own pattern assertions are not under test: a suite of the handle and one read
    // is the count that says the handle and the read are nodes.
    let small = emit(
        "3147_d1_peek_system",
        &[(
            "tests/orders.test.bynk",
            "suite shop.orders as system {\n  case \"peek\" {\n    let db = D1.peek(shop.orders.Orders)\n    let rows <- db.select[Order](\"FROM orders WHERE owner = ?1\", [Text(\"bob\")])\n    let one <- db.firstAs[Order](\"FROM orders WHERE id = ?1\", [Text(\"o1\")])\n    let raw <- db.all(\"SELECT id FROM orders\", [])\n    expect true\n  }\n}\n",
        )],
    );
    // A plain capability call through the handle is the ordinary call on `deps.<local>`.
    assert_line(
        &small,
        "const raw = await deps.__peek_shop_orders_Orders.all(\"SELECT id FROM orders\", []);",
    );
}

/// Postgres and MySQL `select`/`firstAs`: the error tags are the engine's own, `as const`, and
/// the quoting is the engine's.
#[test]
fn postgres_and_mysql_selects_are_pinned() {
    for (fixture, ident, quote, sql_tag, decode_tag) in [
        ("3320_postgres_typed", "Main", "\\\"", "PgSql", "PgDecode"),
        ("3322_mysql_typed", "Legacy", "`", "MySqlSql", "MySqlDecode"),
    ] {
        let ts = emit(fixture, &[]);
        let cols = [
            "id", "name", "placed", "wait", "total", "status", "note", "paid",
        ]
        .map(|c| format!("{quote}{c}{quote}"))
        .join(", ");
        assert!(
            ts.contains(&format!(
                "const __r = await deps.{ident}.all(\"SELECT {cols} FROM orders WHERE id >"
            )),
            "{fixture}: {ts}"
        );
        assert_line(
            &ts,
            &format!(
                "if (__r.tag === \"Err\") return Err({{ tag: \"{sql_tag}\" as const, error: __r.error }});"
            ),
        );
        assert_line(
            &ts,
            &format!(
                "return __d.tag === \"Ok\" ? Ok(__d.value) : Err({{ tag: \"{decode_tag}\" as const, error: __d.error }});"
            ),
        );
        assert!(
            ts.contains(&format!(
                "const __r = await deps.{ident}.first(\"SELECT {cols} FROM orders ORDER BY placed DESC LIMIT 1\", []);"
            )),
            "{fixture}: {ts}"
        );
        assert_block(
            &ts,
            &[
                "if (__r.value.tag === \"None\") return Ok(None);",
                "const __d = __decode(__r.value.value);",
                &format!(
                    "return __d.tag === \"Ok\" ? Ok(Some(__d.value)) : Err({{ tag: \"{decode_tag}\" as const, error: __d.error }});"
                ),
            ],
        );
    }
}

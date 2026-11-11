//! #3108: the D1 SQL schema check (#3070) through the editor's entry point,
//! `analyse_project`, which builds the migrations' schemas itself: the
//! diagnostics arrive, a `[check] d1_sql = false` project reports none, and a
//! `let`-bound parameter list is counted like a list literal.

use std::collections::HashMap;
use std::path::PathBuf;

use bynk_check::analysis;
use bynk_project::{Roots, SchemaLock};

const MIGRATION: &str = "CREATE TABLE orders (id TEXT PRIMARY KEY, total INTEGER NOT NULL);";

fn source(statements: &str) -> String {
    format!(
        "context demo\n\nconsumes bynk.cloudflare {{ D1 }}\n\ncapability Orders = D1\n\n\
         type Order = {{ id: String, total: Int, note: Option[String] }}\n\n\
         const HIDDEN: String = \"SELECT id, total AS amount FROM orders\"\n\n\
         service probe {{\n  on call(id: String) -> Effect[Bool] given Orders {{\n{statements}    true\n  }}\n}}\n"
    )
}

/// The categories `analyse_project` reports for a project whose manifest is
/// `manifest` and whose one `demo` unit holds `statements`.
fn categories(test: &str, manifest: &str, statements: &str) -> Vec<&'static str> {
    categories_with(test, manifest, statements, true)
}

/// As [`categories`], with the migrations directory written only when `migrated`.
fn categories_with(
    test: &str,
    manifest: &str,
    statements: &str,
    migrated: bool,
) -> Vec<&'static str> {
    let root =
        std::env::temp_dir().join(format!("bynk-check-d1-sql-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let write = |rel: &str, text: &str| {
        let p: PathBuf = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    };
    write("bynk.toml", manifest);
    let unit = source(statements);
    write("demo.bynk", &unit);
    if migrated {
        write("migrations/demo-orders/0001_create.sql", MIGRATION);
    }
    let analysed = analysis::analyse_project(
        &Roots::Single(root.clone()),
        &HashMap::from([(root.join("demo.bynk"), unit)]),
        &SchemaLock::Off,
    );
    let out = analysed
        .errors
        .iter()
        .map(|e| e.error.category)
        .collect::<Vec<_>>();
    let _ = std::fs::remove_dir_all(&root);
    out
}

const BAD_COLUMN: &str =
    "    let _ <- Orders.all(\"SELECT id FROM orders WHERE stauts = ?1\", [Text(id)])\n";

#[test]
fn the_editor_path_reports_the_schema_check() {
    assert_eq!(categories("lsp", "", BAD_COLUMN), ["bynk.d1.sql_invalid"]);
}

#[test]
fn the_opt_out_silences_the_editor_path_too() {
    assert_eq!(
        categories("optout", "[check]\nd1_sql = false\n", BAD_COLUMN),
        Vec::<&str>::new()
    );
}

#[test]
fn a_let_bound_list_is_counted() {
    let two_for_one = "    let args = [Text(id)]\n    \
                       let _ <- Orders.all(\"SELECT id FROM orders WHERE id = ?1 AND total = ?2\", args)\n";
    assert_eq!(
        categories("let-bad", "", two_for_one),
        ["bynk.d1.sql_param_count"]
    );
    let right = "    let args = [Text(id)]\n    \
                 let _ <- Orders.all(\"SELECT id FROM orders WHERE id = ?1\", args)\n";
    assert!(categories("let-ok", "", right).is_empty());
}

#[test]
fn a_shadowed_let_list_is_not_counted() {
    // The inner `args` has no length the text fixes (a call), so the outer
    // literal's length (1) must not carry over: the SQL takes two parameters.
    let shadowed = "    let args = [Text(id)]\n    \
                    let args = args.map((v) => v)\n    \
                    let _ <- Orders.all(\"SELECT id FROM orders WHERE id = ?1 AND id = ?2\", args)\n";
    assert!(categories("let-shadow", "", shadowed).is_empty());
}

/// #3108: a literal D1 statement in a test body is prepared on the editor path
/// too (the suite's target is `demo`, whose binding names the database), and its
/// error points at the refused token in the suite's source.
#[test]
fn a_statement_in_a_test_body_is_prepared() {
    let root = std::env::temp_dir().join(format!("bynk-check-d1-sql-suite-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let unit = source("");
    let suite = "suite demo\n\ncase \"reads a missing column\" {\n  \
                 let _ <- Orders.all(\"SELECT id FROM orders WHERE stauts = ?1\", [Text(\"a\")])\n}\n\n\
                 property \"a property reads one too\" {\n  for all n: Int {\n    \
                 let _ <- Orders.all(\"SELECT id FROM orders WHERE stauts = ?1\", [Text(\"a\")])\n    \
                 expect n == n\n  }\n}\n";
    let mut overlay = HashMap::new();
    for (rel, text) in [
        ("demo.bynk", unit.as_str()),
        ("demo_test.bynk", suite),
        ("migrations/demo-orders/0001_create.sql", MIGRATION),
    ] {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, text).unwrap();
        overlay.insert(p, text.to_string());
    }
    let analysed =
        analysis::analyse_project(&Roots::Single(root.clone()), &overlay, &SchemaLock::Off);
    let _ = std::fs::remove_dir_all(&root);
    let spans: Vec<_> = analysed
        .errors
        .iter()
        .filter(|e| e.error.category == "bynk.d1.sql_invalid")
        .map(|e| &suite[e.error.span.start..e.error.span.end])
        .collect();
    // One in the case body, one in the property body, each on the token.
    assert_eq!(spans, ["stauts", "stauts"]);
}

/// #3108: the error points at the token SQLite refused, inside the literal.
#[test]
fn the_error_points_inside_the_sql_literal() {
    let root = std::env::temp_dir().join(format!("bynk-check-d1-sql-span-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let unit = source(BAD_COLUMN);
    let mut overlay = HashMap::new();
    for (rel, text) in [
        ("demo.bynk", unit.as_str()),
        ("migrations/demo-orders/0001_create.sql", MIGRATION),
    ] {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, text).unwrap();
        overlay.insert(p, text.to_string());
    }
    let analysed =
        analysis::analyse_project(&Roots::Single(root.clone()), &overlay, &SchemaLock::Off);
    let _ = std::fs::remove_dir_all(&root);
    let err = analysed
        .errors
        .iter()
        .find(|e| e.error.category == "bynk.d1.sql_invalid")
        .expect("reported");
    let span = err.error.span;
    assert_eq!(&unit[span.start..span.end], "stauts");
}

/// #3108: for `select[T]` the error offset is shifted past the compiler's
/// `SELECT <columns>` prefix, so it still lands on the author's token.
#[test]
fn a_select_tail_error_points_into_the_tail() {
    let root = std::env::temp_dir().join(format!("bynk-check-d1-sql-sel-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let unit = source(
        "    let _ <- Orders.select[Row](\"  FROM orders WHERE stauts = ?1\", [Text(id)])\n",
    )
    .replace(
        "service probe",
        "type Row = { id: String, total: Int }\n\nservice probe",
    );
    let mut overlay = HashMap::new();
    for (rel, text) in [
        ("demo.bynk", unit.as_str()),
        ("migrations/demo-orders/0001_create.sql", MIGRATION),
    ] {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, text).unwrap();
        overlay.insert(p, text.to_string());
    }
    let analysed =
        analysis::analyse_project(&Roots::Single(root.clone()), &overlay, &SchemaLock::Off);
    let _ = std::fs::remove_dir_all(&root);
    let err = analysed
        .errors
        .iter()
        .find(|e| e.error.category == "bynk.d1.sql_invalid")
        .expect("reported");
    let span = err.error.span;
    assert_eq!(&unit[span.start..span.end], "stauts");
}

/// #3135: the cases of a `suite … as integration` are test bodies like a unit
/// suite's, so they are prepared too (the check is the one of #3133); a valid
/// statement stays clean, and the opt-out silences a bad one. (A `suite … as
/// system { … }` can't call a capability directly, so it has no statement to
/// check.)
#[test]
fn a_statement_in_an_integration_case_is_prepared() {
    let suite = |sql: &str| {
        format!(
            "suite demo as integration\n\ncase \"reads\" {{\n  \
             let _ <- Orders.all(\"{sql}\", [Text(\"a\")])\n}}\n"
        )
    };
    let run = |test: &str, manifest: &str, sql: &str| {
        let root =
            std::env::temp_dir().join(format!("bynk-check-d1-sql-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let unit = source("");
        let suite = suite(sql);
        let mut overlay = HashMap::new();
        for (rel, text) in [
            ("bynk.toml", manifest),
            ("demo.bynk", unit.as_str()),
            ("demo_integration.bynk", suite.as_str()),
            ("migrations/demo-orders/0001_create.sql", MIGRATION),
        ] {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, text).unwrap();
            overlay.insert(p, text.to_string());
        }
        let analysed =
            analysis::analyse_project(&Roots::Single(root.clone()), &overlay, &SchemaLock::Off);
        let _ = std::fs::remove_dir_all(&root);
        analysed
            .errors
            .iter()
            .filter(|e| e.error.category.starts_with("bynk.d1."))
            .map(|e| suite[e.error.span.start..e.error.span.end].to_string())
            .collect::<Vec<_>>()
    };
    assert!(run("int-ok", "", "SELECT id FROM orders WHERE id = ?1").is_empty());
    assert_eq!(
        run("int-bad", "", "SELECT id FROM orders WHERE stauts = ?1"),
        ["stauts"]
    );
    assert!(
        run(
            "int-off",
            "[check]\nd1_sql = false\n",
            "SELECT id FROM orders WHERE stauts = ?1"
        )
        .is_empty()
    );
}

/// #3126: a binding with no migrations has no schema, so `sql_invalid` has
/// nothing to check against, but the parameter count, a property of the SQL text
/// and the list, is still reported on the editor path; `[check] d1_sql = false`
/// silences it, and a migrated binding is unchanged.
#[test]
fn a_binding_without_migrations_is_checked_for_its_parameter_count() {
    let counted =
        "    let _ <- Orders.all(\"SELECT id FROM orders WHERE id = ?1\", [Text(id), Text(id)])\n";
    assert_eq!(
        categories_with("nomig-count", "", counted, false),
        ["bynk.d1.sql_param_count"]
    );
    assert!(
        categories_with("nomig-bad-column", "", BAD_COLUMN, false).is_empty(),
        "no schema, so the column is not checked"
    );
    assert!(
        categories_with("nomig-optout", "[check]\nd1_sql = false\n", counted, false).is_empty()
    );
    assert_eq!(
        categories("mig-count", "", counted),
        ["bynk.d1.sql_param_count"],
        "a migrated binding is unchanged"
    );
}

/// #3108: a context-level `fn … given Orders` is checked like a handler: all
/// four forms (a literal, a `const`, a parameter count, a `batch`) report on the
/// editor path, and a valid body reports nothing.
fn given_fn_categories(test: &str, body: &str, called: bool) -> Vec<&'static str> {
    let root =
        std::env::temp_dir().join(format!("bynk-check-d1-sql-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let write = |rel: &str, text: &str| {
        let p: PathBuf = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    };
    write("bynk.toml", "");
    // An uncalled fn leaves the handler with nothing to be `given`.
    let (given, call) = if called {
        (" given Orders", "lookup(id)")
    } else {
        ("", "true")
    };
    let unit = format!(
        "context demo\n\nconsumes bynk.cloudflare {{ D1 }}\n\ncapability Orders = D1\n\n\
         const MISSPELT: String = \"SELECT id FROM orders WHERE stauts = ?1\"\n\n\
         fn lookup(id: String) -> Effect[Bool] given Orders {{\n{body}  true\n}}\n\n\
         service probe {{\n  on call(id: String) -> Effect[Bool]{given} {{\n    {call}\n  }}\n}}\n"
    );
    write("demo.bynk", &unit);
    write("migrations/demo-orders/0001_create.sql", MIGRATION);
    let analysed = analysis::analyse_project(
        &Roots::Single(root.clone()),
        &HashMap::from([(root.join("demo.bynk"), unit)]),
        &SchemaLock::Off,
    );
    let out = analysed
        .errors
        .iter()
        .map(|e| e.error.category)
        .collect::<Vec<_>>();
    let _ = std::fs::remove_dir_all(&root);
    out
}

#[test]
fn a_given_fn_is_checked_like_a_handler() {
    let bad = "  let _ <- Orders.all(\"SELECT id FROM orders WHERE stauts = ?1\", [Text(id)])\n  \
               let _ <- Orders.all(MISSPELT, [Text(id)])\n  \
               let _ <- Orders.all(\"SELECT id FROM orders WHERE id = ?1\", [Text(id), Text(id)])\n  \
               let _ <- Orders.batch([SqlStatement { sql: \"SELECT id FROM orders WHERE stauts = ?1\", params: [Text(id)] }])\n";
    let expected = [
        "bynk.d1.sql_invalid",
        "bynk.d1.sql_invalid",
        "bynk.d1.sql_param_count",
        "bynk.d1.sql_invalid",
    ];
    // Checked where the fn is declared, so a fn no handler calls is checked too.
    assert_eq!(given_fn_categories("given-bad", bad, true), expected);
    assert_eq!(
        given_fn_categories("given-bad-uncalled", bad, false),
        expected
    );
    let good = "  let _ <- Orders.all(\"SELECT id FROM orders WHERE id = ?1\", [Text(id)])\n";
    assert!(given_fn_categories("given-ok", good, true).is_empty());
}

/// #3139: `Json.decodeSql[T](text)` of a literal D1 read's rows is checked
/// against the statement's columns on the editor path too: a required field with
/// no column is reported; an `Option` field with none, an extra column, a name the
/// check can't follow (copied, shadowed) and the opt-out are all silent.
#[test]
fn a_decode_is_checked_against_the_statements_columns() {
    let decode = |sql: &str, arm: &str| {
        format!(
            "    let rows <- Orders.all(\"{sql}\", [])\n    \
             let _ = match rows {{\n      Ok(text) => {arm}\n      Err(_) => false\n    }}\n"
        )
    };
    let linked = "Json.decodeSql[List[Order]](text) is Ok(_)";
    // `total` is hidden behind an alias.
    let bad = decode("SELECT id, total AS amount FROM orders", linked);
    assert_eq!(
        categories("dec-bad", "", &bad),
        ["bynk.d1.select_field_missing"]
    );
    assert!(
        categories("dec-optout", "[check]\nd1_sql = false\n", &bad).is_empty(),
        "the opt-out covers it"
    );
    // An `Option` field with no column, and an extra column.
    let ok = decode("SELECT id, total, 1 AS extra FROM orders", linked);
    assert!(categories("dec-ok", "", &ok).is_empty());
    // A copy of the text is not followed.
    let copied = decode(
        "SELECT id FROM orders",
        "{ let copy = text\n        Json.decodeSql[List[Order]](copy) is Ok(_) }",
    );
    assert!(categories("dec-copy", "", &copied).is_empty());
    // A shadowing binding is not the statement's text.
    let shadowed = decode(
        "SELECT id FROM orders",
        "{ let text = \"[]\"\n        Json.decodeSql[List[Order]](text) is Ok(_) }",
    );
    assert!(categories("dec-shadow", "", &shadowed).is_empty());
    // A `const` statement is linked like a literal.
    let by_const = "    let rows <- Orders.all(HIDDEN, [])\n    \
                    let _ = match rows {\n      Ok(text) => Json.decodeSql[List[Order]](text) is Ok(_)\n      Err(_) => false\n    }\n";
    assert_eq!(
        categories("dec-const", "", by_const),
        ["bynk.d1.select_field_missing"]
    );
    // Without migrations there are no columns to read, so a decode stays silent
    // (the parameter count is all such a binding is checked for, #3126).
    assert!(categories_with("dec-nomig", "", &bad, false).is_empty());
    // `first` and `Ok(Some(text))`.
    let first = "    let row <- Orders.first(\"SELECT id FROM orders\", [])\n    \
                 let _ = match row {\n      Ok(Some(text)) => Json.decodeSql[Order](text) is Ok(_)\n      _ => false\n    }\n";
    assert_eq!(
        categories("dec-first", "", first),
        ["bynk.d1.select_field_missing"]
    );
}

/// #3197: a batch's statements are checked in order, each against the schema
/// the earlier ones leave, so the issue's `DROP` → `CREATE` → `INSERT` batch is
/// valid as written.
#[test]
fn a_batch_may_drop_then_recreate_a_table() {
    let batch = "    let _ <- Orders.batch([\n\
        \x20     SqlStatement { sql: \"DROP TABLE orders\", params: [] },\n\
        \x20     SqlStatement { sql: \"CREATE TABLE orders (id, total, note)\", params: [] },\n\
        \x20     SqlStatement { sql: \"INSERT INTO orders VALUES ('x', 10, 'n')\", params: [] }\n\
        \x20   ])\n";
    assert_eq!(categories("batch-recreate", "", batch), Vec::<&str>::new());
}

/// #3197: a table created earlier in the batch exists for a later statement.
#[test]
fn a_batch_may_use_a_table_it_creates() {
    let batch = "    let _ <- Orders.batch([\n\
        \x20     SqlStatement { sql: \"CREATE TABLE audit (id TEXT, note TEXT)\", params: [] },\n\
        \x20     SqlStatement { sql: \"INSERT INTO audit VALUES (?1, 'seen')\", params: [Text(id)] }\n\
        \x20   ])\n";
    assert_eq!(categories("batch-create", "", batch), Vec::<&str>::new());
}

/// #3197: an invalid statement after DDL is still refused.
#[test]
fn an_invalid_statement_after_ddl_is_still_refused() {
    let batch = "    let _ <- Orders.batch([\n\
        \x20     SqlStatement { sql: \"CREATE TABLE audit (id TEXT)\", params: [] },\n\
        \x20     SqlStatement { sql: \"INSERT INTO audit (nope) VALUES (1)\", params: [] }\n\
        \x20   ])\n";
    assert_eq!(
        categories("batch-invalid", "", batch),
        ["bynk.d1.sql_invalid"]
    );
}

/// #3197: the batch's DDL does not leak out: a later, separate `batch` or
/// single statement is checked against the migrated schema, where `orders`
/// still has its original columns and `audit` does not exist.
#[test]
fn a_following_statement_sees_the_original_schema() {
    let statements = "    let _ <- Orders.batch([\n\
        \x20     SqlStatement { sql: \"DROP TABLE orders\", params: [] },\n\
        \x20     SqlStatement { sql: \"CREATE TABLE audit (id)\", params: [] }\n\
        \x20   ])\n\
        \x20   let _ <- Orders.all(\"SELECT id, total FROM orders\", [])\n\
        \x20   let _ <- Orders.run(\"INSERT INTO audit VALUES (1)\", [])\n\
        \x20   let _ <- Orders.batch([\n\
        \x20     SqlStatement { sql: \"UPDATE orders SET total = 1\", params: [] }\n\
        \x20   ])\n";
    // Only the `INSERT INTO audit` (no such table outside the first batch) fails.
    assert_eq!(
        categories("batch-isolated", "", statements),
        ["bynk.d1.sql_invalid"]
    );
}

/// #3243 S2.3b: the language server's memo must not replay a stale SQL
/// diagnostic after a migration changes. The migrations are not in the project's
/// `.bynk` text, so a digest that missed them would keep answering from the old
/// schema until the unit itself was edited.
#[test]
fn a_memoised_round_sees_an_edited_migration() {
    use bynk_check::memo::{AnalysisMemo, analyse_project_memoised};
    use bynk_check::oracle::canonical_report;

    let root = std::env::temp_dir().join(format!("bynk-check-d1-memo-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let write = |rel: &str, text: &str| {
        let p: PathBuf = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    };
    let unit = source(BAD_COLUMN);
    write("bynk.toml", "");
    write("demo.bynk", &unit);
    write("migrations/demo-orders/0001_create.sql", MIGRATION);
    let overlay = HashMap::from([(root.join("demo.bynk"), unit)]);
    let roots = Roots::Single(root.clone());
    let mut memo = AnalysisMemo::new();
    let round = |memo: &mut AnalysisMemo| {
        let memoised = analyse_project_memoised(&roots, &overlay, &SchemaLock::Off, memo);
        let fresh = analysis::analyse_project(&roots, &overlay, &SchemaLock::Off);
        assert_eq!(
            canonical_report(&memoised),
            canonical_report(&fresh),
            "a memoised round must answer what a fresh one does"
        );
        memoised
            .errors
            .iter()
            .map(|e| e.error.category)
            .collect::<Vec<_>>()
    };

    assert_eq!(
        round(&mut memo),
        ["bynk.d1.sql_invalid"],
        "the column is missing"
    );
    // The migration gains the column the statement names. No `.bynk` file changed.
    write(
        "migrations/demo-orders/0002_status.sql",
        "ALTER TABLE orders ADD COLUMN stauts TEXT;",
    );
    assert_eq!(
        round(&mut memo),
        Vec::<&str>::new(),
        "the new migration makes the statement valid"
    );
    // And an unchanged round after that replays everything.
    assert_eq!(round(&mut memo), Vec::<&str>::new());
    assert_eq!(memo.stats().misses, 0, "{:?}", memo.stats());
    let _ = std::fs::remove_dir_all(&root);
}

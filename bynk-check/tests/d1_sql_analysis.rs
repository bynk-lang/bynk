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
         service probe {{\n  on call(id: String) -> Effect[Bool] given Orders {{\n{statements}    true\n  }}\n}}\n"
    )
}

/// The categories `analyse_project` reports for a project whose manifest is
/// `manifest` and whose one `demo` unit holds `statements`.
fn categories(test: &str, manifest: &str, statements: &str) -> Vec<&'static str> {
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

/// #3108: a context-level `fn … given Orders` is checked like a handler: all
/// four forms (a literal, a `const`, a parameter count, a `batch`) report on the
/// editor path, and a valid body reports nothing.
fn given_fn_categories(test: &str, body: &str) -> Vec<&'static str> {
    let root =
        std::env::temp_dir().join(format!("bynk-check-d1-sql-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let write = |rel: &str, text: &str| {
        let p: PathBuf = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    };
    write("bynk.toml", "");
    let unit = format!(
        "context demo\n\nconsumes bynk.cloudflare {{ D1 }}\n\ncapability Orders = D1\n\n\
         const MISSPELT: String = \"SELECT id FROM orders WHERE stauts = ?1\"\n\n\
         fn lookup(id: String) -> Effect[Bool] given Orders {{\n{body}  true\n}}\n\n\
         service probe {{\n  on call(id: String) -> Effect[Bool] given Orders {{\n    lookup(id)\n  }}\n}}\n"
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
    assert_eq!(
        given_fn_categories("given-bad", bad),
        [
            "bynk.d1.sql_invalid",
            "bynk.d1.sql_invalid",
            "bynk.d1.sql_param_count",
            "bynk.d1.sql_invalid"
        ]
    );
    let good = "  let _ <- Orders.all(\"SELECT id FROM orders WHERE id = ?1\", [Text(id)])\n";
    assert!(given_fn_categories("given-ok", good).is_empty());
}

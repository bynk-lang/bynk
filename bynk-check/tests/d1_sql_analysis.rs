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

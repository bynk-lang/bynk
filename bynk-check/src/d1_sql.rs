//! #3070: a `D1` call's literal SQL, checked against the schema its binding's
//! migrations build.
//!
//! [`D1Sql::build`] applies each database's migrations, in order, to a fresh
//! in-memory SQLite (a bundled one, so every machine and CI agree), once per
//! run. The checker records each literal statement as a
//! [`D1SqlSite`](crate::checker::D1SqlSite) (a [`Callee::D1Sql`](crate::checker::Callee::D1Sql) entry
//! under the SQL argument's id); [`D1Sql::check_unit`] then `prepare`s each one
//! against its database. `prepare` executes nothing, so the check is cheap and
//! has no side effects. Reported:
//!
//! - `bynk.d1.sql_invalid`: the statement doesn't prepare (a syntax error, an
//!   unknown table or column);
//! - `bynk.d1.sql_param_count`: the SQL's parameter count
//!   (`sqlite3_bind_parameter_count`, the rule the test D1 refuses a mismatch
//!   by, #3003) differs from the list literal passed;
//! - `bynk.d1.select_field_missing`: a `select[T]` field with no column;
//! - `bynk.d1.migration_invalid` (warning): a migration that doesn't apply; the
//!   database's statements are then not checked, since each would only echo it.
//!
//! A database with no migrations directory is not checked: no schema is known.
//! Never stricter than the test tier is the aim, since that tier runs the same
//! migrations on Node's own SQLite.
//!
//! On `wasm32` (the playground) there is no migrations folder and no SQLite:
//! [`D1Sql::build`] returns `None`.

#[cfg(not(target_arch = "wasm32"))]
pub use native::D1Sql;
#[cfg(target_arch = "wasm32")]
pub use stub::D1Sql;

#[cfg(target_arch = "wasm32")]
mod stub {
    use std::collections::HashMap;
    use std::sync::Arc;

    use bynk_syntax::ast::ExprId;
    use bynk_syntax::error::CompileError;

    use crate::checker::Callee;
    use crate::d1_migrations::D1Migrations;
    use crate::project_model::ErrorSink;
    use crate::symbols::UnitTable;

    #[derive(Debug)]
    pub struct D1Sql;

    impl D1Sql {
        pub fn build(_: &D1Migrations, _: &mut ErrorSink) -> Option<Arc<D1Sql>> {
            None
        }
        pub fn check_unit(
            &self,
            _: &str,
            _: &UnitTable,
            _: &HashMap<ExprId, Callee>,
        ) -> Vec<CompileError> {
            Vec::new()
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::collections::{BTreeMap, HashMap};
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use bynk_project::assets::PROJECT_MIGRATIONS_DIR;
    use bynk_syntax::ast::ExprId;
    use bynk_syntax::error::CompileError;
    use bynk_syntax::span::Span;
    use rusqlite::{Connection, config::DbConfig};

    use crate::checker::Callee;
    use crate::checker::D1SqlSite;
    use crate::d1_migrations::{D1Migrations, d1_database_name, is_d1};
    use crate::project_model::ErrorSink;
    use crate::symbols::UnitTable;

    /// The schema each migrated database builds, ready to `prepare` against.
    pub struct D1Sql {
        /// Database name → its connection. A database whose migrations failed,
        /// or that has none, has no entry.
        pub(super) schemas: HashMap<String, Mutex<Connection>>,
    }

    impl std::fmt::Debug for D1Sql {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("D1Sql")
                .field("databases", &self.schemas.keys().collect::<Vec<_>>())
                .finish()
        }
    }

    /// Wrangler's tracking table, which a statement could read (`bynk revert`
    /// does), created before the migrations run as `wrangler d1 migrations`
    /// does.
    const TRACKING_TABLE: &str = "CREATE TABLE IF NOT EXISTS d1_migrations(\
        id INTEGER PRIMARY KEY AUTOINCREMENT, \
        name TEXT UNIQUE, \
        applied_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP NOT NULL)";

    impl D1Sql {
        /// Apply every database's migrations to its own in-memory SQLite. One
        /// that fails is reported as a warning at its file, and the database is
        /// left out. `None` when no database has migrations (nothing to check).
        pub fn build(migrations: &D1Migrations, errors: &mut ErrorSink) -> Option<Arc<D1Sql>> {
            let mut schemas = HashMap::new();
            for (database, files) in migrations {
                if files.is_empty() {
                    continue;
                }
                let Ok(conn) = Connection::open_in_memory() else {
                    continue;
                };
                if conn.execute_batch(TRACKING_TABLE).is_err() {
                    continue;
                }
                let mut ok = true;
                for file in files {
                    if let Err(e) = conn.execute_batch(&file.sql) {
                        let key = format!("{PROJECT_MIGRATIONS_DIR}/{database}/{}", file.name);
                        errors.push_for(
                            Some(&PathBuf::from(&key)),
                            CompileError::new(
                                "bynk.d1.migration_invalid",
                                Span::default(),
                                format!("migration `{key}` doesn't apply: {e}"),
                            )
                            .with_note(format!(
                                "applied in order to an empty in-memory SQLite; the SQL of `{database}` is not checked against its schema until this applies"
                            )),
                        );
                        ok = false;
                        break;
                    }
                }
                // A double-quoted name that matches no column silently becomes a
                // string literal (SQLite's `DQS` fallback), so a misspelt column
                // would prepare, and `select[T]`'s quoted column list would
                // never miss. Off for statements; migrations ran with it on.
                let _ = conn.set_db_config(DbConfig::SQLITE_DBCONFIG_DQS_DML, false);
                if ok {
                    schemas.insert(database.clone(), Mutex::new(conn));
                }
            }
            (!schemas.is_empty()).then(|| Arc::new(D1Sql { schemas }))
        }

        /// Check every literal statement the unit's call sites recorded.
        pub fn check_unit(
            &self,
            unit: &str,
            table: &UnitTable,
            callees: &HashMap<ExprId, Callee>,
        ) -> Vec<CompileError> {
            // A deterministic order: by position in the source.
            let mut sites: BTreeMap<(usize, usize), &D1SqlSite> = BTreeMap::new();
            for callee in callees.values() {
                // A `const` SQL argument's entry is `Const`, which keeps the site.
                let site: &D1SqlSite = match callee {
                    Callee::D1Sql(site) => site,
                    Callee::Const(_, Some(site)) => site,
                    _ => continue,
                };
                sites.insert((site.span.start, site.span.end), site);
            }
            let worker = bynk_project::worker_dir_name(unit);
            let mut out = Vec::new();
            for site in sites.values() {
                let Some(instance) = table.derived.get(&site.cap).filter(|d| is_d1(d)) else {
                    continue;
                };
                let database = d1_database_name(&worker, &instance.binding);
                let Some(conn) = self.schemas.get(&database) else {
                    continue;
                };
                let Ok(conn) = conn.lock() else { continue };
                if let Some(err) = check_site(&conn, site, &database) {
                    out.push(err);
                }
            }
            out
        }
    }

    /// The column a `no such column: x` message names, bare of any `table.` or
    /// quotes (SQLite adds a hint after a double-quoted one).
    fn missing_column(msg: &str) -> Option<&str> {
        let name = msg.strip_prefix("no such column: ")?;
        let name = name.split(" - ").next().unwrap_or(name).trim();
        let name = name.rsplit('.').next().unwrap_or(name);
        Some(name.trim_matches('"'))
    }

    fn check_site(conn: &Connection, site: &D1SqlSite, database: &str) -> Option<CompileError> {
        let what = format!("`{}.{}`", site.cap, site.op);
        let stmt = match conn.prepare(&site.text) {
            Ok(stmt) => stmt,
            Err(e) => {
                let msg = match &e {
                    rusqlite::Error::SqlInputError { msg, .. } => msg.clone(),
                    other => other.to_string(),
                };
                if let Some((row, fields)) = &site.row
                    && let Some(col) = missing_column(&msg)
                    && fields.iter().any(|f| f == col)
                {
                    return Some(
                        CompileError::new(
                            "bynk.d1.select_field_missing",
                            site.span,
                            format!("`{row}` has a field `{col}`, but the query has no column `{col}`"),
                        )
                        .with_note(format!(
                            "{what} selects every field of `{row}` by name; rename the column in the tail (`AS {col}`), rename the field, or use `all` and `Json.decodeSql` to write the columns yourself"
                        )),
                    );
                }
                return Some(
                    CompileError::new(
                        "bynk.d1.sql_invalid",
                        site.span,
                        format!("the SQL of {what} doesn't prepare: {msg}"),
                    )
                    .with_note(format!(
                        "checked against the migrations of `{database}`, applied to an empty in-memory SQLite"
                    )),
                );
            }
        };
        let used = stmt.parameter_count();
        let passed = site.passed?;
        (used != passed).then(|| {
            CompileError::new(
                "bynk.d1.sql_param_count",
                site.span,
                format!(
                    "the SQL of {what} takes {used} parameter{}, but the list passes {passed}",
                    if used == 1 { "" } else { "s" }
                ),
            )
            .with_note(
                "`?N` names the Nth value (the highest `N` is the count) and a bare `?` the next; D1 refuses a statement bound to a different number of values",
            )
        })
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::d1_migrations::{D1Migrations, MigrationFile};
    use crate::project_model::ErrorSink;
    use rusqlite::Connection;

    fn migrations(files: &[(&str, &str)]) -> D1Migrations {
        let mut m = D1Migrations::new();
        m.insert(
            "demo-orders".to_string(),
            files
                .iter()
                .map(|(name, sql)| MigrationFile {
                    name: name.to_string(),
                    sql: sql.to_string(),
                })
                .collect(),
        );
        m
    }

    #[test]
    fn a_migration_that_fails_warns_once_and_drops_the_database() {
        let mut errors = ErrorSink::new();
        let built = D1Sql::build(
            &migrations(&[
                ("0001_a.sql", "CREATE TABLE t (id TEXT);"),
                ("0002_b.sql", "ALTER TABLE t ADD COLUMN;"),
                ("0003_c.sql", "CREATE TABLE u (id TEXT);"),
            ]),
            &mut errors,
        );
        assert!(built.is_none(), "no database left to check against");
        assert!(errors.is_empty(), "a warning, not an error");
        let warnings = errors.into_warnings();
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].error.category, "bynk.d1.migration_invalid");
    }

    #[test]
    fn migrations_see_the_tracking_table_and_later_files_see_earlier_ones() {
        let mut errors = ErrorSink::new();
        let built = D1Sql::build(
            &migrations(&[
                ("0001_a.sql", "CREATE TABLE t (id TEXT);"),
                (
                    "0002_b.sql",
                    "INSERT INTO t VALUES (\"a\"); SELECT name FROM d1_migrations;",
                ),
            ]),
            &mut errors,
        );
        assert!(built.is_some());
        assert!(errors.into_warnings().is_empty());
    }

    /// The parameter count is SQLite's own (`sqlite3_bind_parameter_count`),
    /// the rule the test D1's `sqlParameters` mirrors (#3003), so the two agree
    /// on the cases #3043 pinned.
    #[test]
    fn the_parameter_count_follows_sqlites_rule() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE t (a, b, c)").unwrap();
        let count = |sql: &str| conn.prepare(sql).unwrap().parameter_count();
        assert_eq!(count("SELECT * FROM t WHERE a = ?1 AND b = ?2"), 2);
        assert_eq!(
            count("SELECT * FROM t WHERE a = ?1 OR b = ?1"),
            1,
            "repeated ?1"
        );
        assert_eq!(count("SELECT * FROM t WHERE a = ?3"), 3, "the highest ?N");
        assert_eq!(count("SELECT * FROM t WHERE a = ? AND b = ?"), 2, "bare ?");
        assert_eq!(
            count("SELECT * FROM t WHERE a = ?2 AND b = ?"),
            3,
            "bare ? after ?2"
        );
        assert_eq!(count("SELECT '?1' FROM t"), 0, "quoted");
        assert_eq!(count("SELECT * FROM t -- ?1\nWHERE a = ?1"), 1, "comment");
        assert_eq!(
            count("SELECT * FROM t /* ?5 */ WHERE a = ?1"),
            1,
            "block comment"
        );
    }

    #[test]
    fn a_double_quoted_unknown_name_is_not_a_string() {
        let mut errors = ErrorSink::new();
        let built = D1Sql::build(
            &migrations(&[("0001_a.sql", "CREATE TABLE t (id TEXT);")]),
            &mut errors,
        )
        .expect("applies");
        let conn = built.schemas["demo-orders"].lock().unwrap();
        assert!(conn.prepare("SELECT \"nope\" FROM t").is_err());
        assert!(conn.prepare("SELECT \"id\" FROM t").is_ok());
    }
}

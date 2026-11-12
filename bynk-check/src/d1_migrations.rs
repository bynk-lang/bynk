//! Standard-library P4b (#2608): the project's `migrations/` folder, which
//! holds each `D1` instance's schema migrations as
//! `migrations/<database>/NNNN_<name>.sql`, where `<database>` is the
//! instance's undecorated database name ([`d1_database_name`]).
//!
//! The checker stays disk-free: the folder reaches it from the caller as a
//! [`bynk_project::assets::AssetFolder`], and [`phase_d1_migrations`] checks
//! the file names, reads every file of a declared database, and warns about a
//! directory no instance owns. Wrangler applies the files and records which
//! ran in the database itself, so nothing here tracks that.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use bynk_syntax::error::CompileError;
use bynk_syntax::span::Span;

use crate::project_model::ErrorSink;
use crate::symbols::UnitTable;
use bynk_project::UnitKind;
use bynk_project::assets::PROJECT_MIGRATIONS_DIR;

/// One migration file: its name (`0001_create_orders.sql`) and its SQL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationFile {
    pub name: String,
    pub sql: String,
}

/// Whether the listed `key` is hidden (`.DS_Store`, an editor's
/// `.0001_create.sql.swp`, anything under a `.archive/` directory) or an
/// editor backup (`0001_create.sql~`), which the folder holds without meaning
/// to: it's neither checked nor copied.
pub fn is_ignored(key: &str) -> bool {
    key.split('/').any(|part| part.starts_with('.')) || key.ends_with('~')
}

/// Every declared database's migrations, in order, keyed by database name.
/// A database without a directory has no entry.
pub type D1Migrations = BTreeMap<String, Vec<MigrationFile>>;

/// W30 (#2840): the binding of a Worker's implicit idempotency database, which
/// holds `Idempotency.claim`'s claims. A Worker that calls `claim` declares it
/// with no author action; its database is `<worker>-idempotency`
/// ([`d1_database_name`]), and its schema is [`implicit_migrations`].
pub const IDEMPOTENCY_BINDING: &str = "D1_IDEMPOTENCY";

/// W30 (#2840): the migration files the compiler owns for an implicit database
/// (the author writes no SQL), by its `binding`; `None` for any other binding.
/// The claims table holds one row per claimed key and the time it lapses.
pub fn implicit_migrations(binding: &str) -> Option<Vec<MigrationFile>> {
    (binding == IDEMPOTENCY_BINDING).then(|| {
        vec![MigrationFile {
            name: "0001_idempotency_claims.sql".to_string(),
            sql: "CREATE TABLE idempotency_claims (\n  key TEXT PRIMARY KEY NOT NULL,\n  expires_at INTEGER NOT NULL\n);\n\nCREATE INDEX idempotency_claims_expires_at ON idempotency_claims (expires_at);\n".to_string(),
        }]
    })
}

/// The migration files of database `database`, bound as `binding`: the
/// project's own (`migrations/<database>/`), or the compiler's for an implicit
/// database ([`implicit_migrations`]).
pub fn migrations_of(
    migrations: &D1Migrations,
    database: &str,
    binding: &str,
) -> Vec<MigrationFile> {
    implicit_migrations(binding)
        .or_else(|| migrations.get(database).cloned())
        .unwrap_or_default()
}

/// #3321: the key the opt-in `Postgres` test double's schema rides under in a
/// [`D1Migrations`] on its way to the test emitters. Not a database name (those
/// are `<worker>-<capability>`, never starting `@`), so it cannot collide with
/// one, and nothing that looks a database up finds it. Present exactly when the
/// project has `[test] postgres = "pglite"`; its one file is `schema.sql`, or
/// none when the project has none.
pub const PG_TEST_KEY: &str = "@postgres";

/// Whether the project opts in to the PGlite `Postgres` test double (#3321).
pub fn pglite_on(migrations: &D1Migrations) -> bool {
    migrations.contains_key(PG_TEST_KEY)
}

/// The `Postgres` test double's database, as test emitters look it up: the
/// project's `schema.sql` for a `Postgres` instance, the migrations of a `D1`
/// database named `d1_database` otherwise (#3321, P4b #2608).
pub fn double_files<'a>(
    migrations: &'a D1Migrations,
    cap: &str,
    d1_database: Option<String>,
) -> Option<&'a [MigrationFile]> {
    let key = if cap == "Postgres" {
        Some(PG_TEST_KEY.to_string())
    } else {
        d1_database
    };
    migrations.get(&key?).map(Vec::as_slice)
}

/// P4a (#2602): the account-facing name of a `D1` instance's database,
/// `<worker>-<capability, kebab>`: `shop-cart` and `D1_ORDERS` give
/// `shop-cart-orders`. D1 database names are scoped to the account, as R2
/// bucket names are. `bynk deploy` environment-qualifies it.
pub fn d1_database_name(worker: &str, binding: &str) -> String {
    let prefix = format!("{}_", crate::firstparty::D1_BINDING_PREFIX);
    let stem = binding.strip_prefix(&prefix).unwrap_or(binding);
    format!("{worker}-{}", stem.to_ascii_lowercase().replace('_', "-"))
}

/// Whether `instance` (a context's derived capability) is a `D1` database.
pub fn is_d1(instance: &crate::symbols::DerivedInstance) -> bool {
    instance.base_unit == crate::firstparty::CLOUDFLARE_UNIT && instance.base_cap == "D1"
}

/// Every `D1` instance's database name, from each context's derived
/// capabilities.
pub fn declared_databases(
    kinds: &BTreeMap<String, UnitKind>,
    unit_tables: &HashMap<String, UnitTable>,
) -> Vec<String> {
    let mut out = Vec::new();
    for (unit, kind) in kinds {
        if *kind != UnitKind::Context {
            continue;
        }
        let Some(table) = unit_tables.get(unit) else {
            continue;
        };
        let worker = bynk_project::worker_dir_name(unit);
        for instance in table.derived.values().filter(|d| is_d1(d)) {
            out.push(d1_database_name(&worker, &instance.binding));
        }
    }
    out.sort();
    out
}

/// The number of a well-formed migration file name, `NNNN_<name>.sql`: four
/// ASCII digits, an underscore, then one or more letters, digits, `_` or `-`.
/// Wrangler applies a directory's `.sql` files in name order, so the fixed
/// width keeps that order numeric.
pub fn migration_number(file: &str) -> Option<&str> {
    let stem = file.strip_suffix(".sql")?;
    let (number, name) = (stem.get(..4)?, stem.get(4..)?);
    let name = name.strip_prefix('_')?;
    (number.bytes().all(|b| b.is_ascii_digit())
        && !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'))
    .then_some(number)
}

fn file_error(errors: &mut ErrorSink, key: &str, code: &'static str, message: String, note: &str) {
    errors.push_for(
        Some(&PathBuf::from(key)),
        CompileError::new(code, Span::default(), message).with_note(note.to_string()),
    );
}

/// Check the listed `migrations/` folder against the declared databases, and
/// read the files of each. `None` means no filesystem was consulted, so there
/// is nothing to check or read.
///
/// - `bynk.d1.migration_name`: a file that isn't `migrations/<database>/NNNN_<name>.sql`,
///   or a second file with the same number in one directory;
/// - `bynk.d1.migration_unreadable`: a file that can't be read, or isn't UTF-8;
/// - `bynk.d1.migrations_unused` (warning): a directory no `D1` instance's
///   database name matches, usually a renamed capability.
pub fn phase_d1_migrations(
    folder: Option<&bynk_project::assets::AssetFolder>,
    databases: &[String],
    errors: &mut ErrorSink,
) -> D1Migrations {
    let mut out = D1Migrations::new();
    let Some(folder) = folder else {
        return out;
    };
    let mut warned: Vec<&str> = Vec::new();
    let mut numbers: BTreeMap<(String, String), String> = BTreeMap::new();
    for key in folder.files() {
        if is_ignored(key) {
            continue;
        }
        let parts: Vec<&str> = key.split('/').collect();
        let [dir, database, file] = parts.as_slice() else {
            file_error(
                errors,
                key,
                "bynk.d1.migration_name",
                format!("`{key}` isn't in a database's migrations directory"),
                "a migration is `migrations/<database>/NNNN_<name>.sql`, directly in its database's directory",
            );
            continue;
        };
        debug_assert_eq!(*dir, PROJECT_MIGRATIONS_DIR);
        if !databases.iter().any(|d| d == database) {
            if !warned.contains(database) {
                warned.push(database);
                let known = if databases.is_empty() {
                    "the project declares no `D1` instance".to_string()
                } else {
                    format!(
                        "the declared databases are {}",
                        databases
                            .iter()
                            .map(|d| format!("`{d}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                };
                file_error(
                    errors,
                    &format!("{PROJECT_MIGRATIONS_DIR}/{database}"),
                    "bynk.d1.migrations_unused",
                    format!(
                        "`{PROJECT_MIGRATIONS_DIR}/{database}/` matches no `D1` instance's database, so its migrations are never applied"
                    ),
                    &format!(
                        "{known}; a database is `<worker>-<capability>`, and a renamed capability is a new, empty database"
                    ),
                );
            }
            continue;
        }
        let Some(number) = migration_number(file) else {
            file_error(
                errors,
                key,
                "bynk.d1.migration_name",
                format!("migration `{key}` isn't named `NNNN_<name>.sql`"),
                "four digits, an underscore, then letters, digits, `_` or `-`, as in `0001_create_orders.sql`",
            );
            continue;
        };
        if let Some(first) =
            numbers.insert((database.to_string(), number.to_string()), file.to_string())
        {
            file_error(
                errors,
                key,
                "bynk.d1.migration_name",
                format!("migration `{key}` has the same number as `{first}`"),
                "each migration of a database needs its own number, since they are applied in order",
            );
            continue;
        }
        let sql = match folder.read(key) {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(sql) => sql,
                Err(_) => {
                    file_error(
                        errors,
                        key,
                        "bynk.d1.migration_unreadable",
                        format!("migration `{key}` isn't UTF-8 text"),
                        "save the file as UTF-8",
                    );
                    continue;
                }
            },
            Err(e) => {
                file_error(
                    errors,
                    key,
                    "bynk.d1.migration_unreadable",
                    format!("migration `{key}` can't be read: {e}"),
                    "a migration must be a regular file; symbolic links are not followed",
                );
                continue;
            }
        };
        out.entry(database.to_string())
            .or_default()
            .push(MigrationFile {
                name: file.to_string(),
                sql,
            });
    }
    for files in out.values_mut() {
        files.sort_by(|a, b| a.name.cmp(&b.name));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_migration_name_is_four_digits_an_underscore_and_a_name() {
        assert_eq!(migration_number("0001_create_orders.sql"), Some("0001"));
        assert_eq!(migration_number("0042_add-index.sql"), Some("0042"));
        for bad in [
            "1_init.sql",
            "00001_init.sql",
            "0001init.sql",
            "0001_.sql",
            "0001_init.SQL",
            "0001_init.txt",
            "0001_in it.sql",
            "abcd_init.sql",
            "README.md",
        ] {
            assert_eq!(migration_number(bad), None, "{bad}");
        }
    }

    #[test]
    fn hidden_files_and_editor_backups_are_ignored() {
        for key in [
            "migrations/shop-orders/.DS_Store",
            "migrations/shop-orders/.0001_create.sql.swp",
            "migrations/shop-orders/0001_create.sql~",
            "migrations/.DS_Store",
            "migrations/shop-orders/.archive/0001_old.sql",
            "migrations/.trash/0001_create.sql",
        ] {
            assert!(is_ignored(key), "{key}");
        }
        assert!(!is_ignored("migrations/shop-orders/0001_create.sql"));
        assert!(!is_ignored("migrations/README.md"));
    }

    /// Review of #2609: a listed entry whose read is refused (a symbolic
    /// link, which `migrations_on_disk` lists unfollowed) is reported rather
    /// than dropped, and nothing of that database's later files is lost.
    #[test]
    fn a_refused_read_is_reported_not_dropped() {
        let files = [
            "migrations/shop-orders/0001_init.sql",
            "migrations/shop-orders/0002_linked.sql",
            "migrations/shop-orders/.archive/0001_old.sql",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        let folder = bynk_project::assets::AssetFolder::new(
            files,
            std::sync::Arc::new(|key: &str| {
                if key.ends_with("linked.sql") {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "not a regular file",
                    ))
                } else {
                    Ok(b"CREATE TABLE t (id INTEGER);".to_vec())
                }
            }),
        );
        let mut errors = ErrorSink::new();
        let out = phase_d1_migrations(Some(&folder), &["shop-orders".to_string()], &mut errors);
        let names: Vec<&str> = out["shop-orders"].iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["0001_init.sql"]);
        let all = errors.into_all();
        let categories: Vec<&str> = all.iter().map(|e| e.error.category).collect();
        assert_eq!(categories, ["bynk.d1.migration_unreadable"]);
        assert!(
            all[0].error.message.contains("0002_linked.sql"),
            "{}",
            all[0].error.message
        );
    }

    #[test]
    fn the_database_name_is_the_worker_and_the_capability_in_kebab_case() {
        assert_eq!(
            d1_database_name("shop-cart", "D1_ORDERS"),
            "shop-cart-orders"
        );
        assert_eq!(
            d1_database_name("shop", "D1_ORDER_LINES"),
            "shop-order-lines"
        );
    }

    /// W30 (#2840): the idempotency database is `<worker>-idempotency`, its
    /// migration is the compiler's, and no other binding gets one.
    #[test]
    fn the_implicit_idempotency_database_is_named_and_migrated_by_the_compiler() {
        assert_eq!(
            d1_database_name("shop-hooks", IDEMPOTENCY_BINDING),
            "shop-hooks-idempotency"
        );
        let files = implicit_migrations(IDEMPOTENCY_BINDING).expect("the compiler owns it");
        assert_eq!(files.len(), 1);
        assert!(files[0].sql.contains("CREATE TABLE idempotency_claims"));
        assert!(implicit_migrations("D1_ORDERS").is_none());
    }

    /// A project's own database keeps its folder's files; the implicit one
    /// ignores the folder.
    #[test]
    fn migrations_of_reads_the_folder_for_a_project_database_only() {
        let mut folder = D1Migrations::new();
        folder.insert(
            "shop-orders".to_string(),
            vec![MigrationFile {
                name: "0001_a.sql".to_string(),
                sql: "SELECT 1;".to_string(),
            }],
        );
        assert_eq!(migrations_of(&folder, "shop-orders", "D1_ORDERS").len(), 1);
        assert!(migrations_of(&folder, "shop-cart", "D1_CART").is_empty());
        assert_eq!(
            migrations_of(&folder, "shop-idempotency", IDEMPOTENCY_BINDING)[0].name,
            "0001_idempotency_claims.sql"
        );
    }
}

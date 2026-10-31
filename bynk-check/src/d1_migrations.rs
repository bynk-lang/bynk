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

/// Whether the listed `key` is a hidden file (`.DS_Store`, an editor's
/// `.0001_create.sql.swp`) or an editor backup (`0001_create.sql~`), which the
/// folder holds without meaning to: it's neither checked nor copied.
pub fn is_ignored(key: &str) -> bool {
    let name = key.rsplit('/').next().unwrap_or(key);
    name.starts_with('.') || name.ends_with('~')
}

/// Every declared database's migrations, in order, keyed by database name.
/// A database without a directory has no entry.
pub type D1Migrations = BTreeMap<String, Vec<MigrationFile>>;

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
        ] {
            assert!(is_ignored(key), "{key}");
        }
        assert!(!is_ignored("migrations/shop-orders/0001_create.sql"));
        assert!(!is_ignored("migrations/README.md"));
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
}

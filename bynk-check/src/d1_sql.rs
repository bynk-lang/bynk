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
//! A database with no migrations directory has no schema, so `sql_invalid` and
//! `select_field_missing` have nothing to check against; but the parameter
//! count is a property of the SQL text, so `parameter_count` (a scanner that
//! mirrors SQLite's) still checks it (#3126). A database whose migrations
//! failed is skipped entirely, as each statement would only echo that warning.
//! #3197: the statements of a literal `batch` list run in order, so each is
//! prepared against the schema the earlier ones leave: inside a savepoint on the
//! schema connection, a statement that prepares and is DDL (`CREATE`, `DROP`,
//! `ALTER`) is also executed, and the savepoint is rolled back when the batch
//! has been checked, on every path, so the shared schema is never left changed.
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
            _: Option<&str>,
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
    use rusqlite::{Connection, DropBehavior, config::DbConfig};

    use crate::checker::Callee;
    use crate::checker::{D1DecodeSite, D1SqlSite};
    use crate::d1_migrations::{D1Migrations, d1_database_name, is_d1};
    use crate::project_model::ErrorSink;
    use crate::symbols::UnitTable;

    use super::parameter_count;

    /// The schema each migrated database builds, ready to `prepare` against.
    pub struct D1Sql {
        /// Database name → its connection. A database whose migrations failed,
        /// or that has none, has no entry.
        pub(super) schemas: HashMap<String, Arc<Mutex<Connection>>>,
        /// The databases that have migration files, whether or not they applied.
        /// A database in neither map has no migrations, and only its parameter
        /// counts are checked (#3126).
        pub(super) migrated: std::collections::HashSet<String>,
    }

    impl std::fmt::Debug for D1Sql {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("D1Sql")
                .field("databases", &self.schemas.keys().collect::<Vec<_>>())
                .finish()
        }
    }

    /// One database's schema: its migrations applied in order to a fresh
    /// in-memory SQLite. `None` (with a `migration_invalid` warning) when one
    /// doesn't apply.
    fn build_database(
        database: &str,
        files: &[crate::d1_migrations::MigrationFile],
        errors: &mut ErrorSink,
    ) -> Option<Connection> {
        let conn = Connection::open_in_memory().ok()?;
        conn.execute_batch(TRACKING_TABLE).ok()?;
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
                return None;
            }
        }
        // A double-quoted name that matches no column silently becomes a
        // string literal (SQLite's `DQS` fallback), so a misspelt column
        // would prepare, and `select[T]`'s quoted column list would
        // never miss. Off for statements; migrations ran with it on.
        let _ = conn.set_db_config(DbConfig::SQLITE_DBCONFIG_DQS_DML, false);
        Some(conn)
    }

    /// Wrangler's tracking table, which a statement could read (`bynk revert`
    /// does), created before the migrations run as `wrangler d1 migrations`
    /// does.
    const TRACKING_TABLE: &str = "CREATE TABLE IF NOT EXISTS d1_migrations(\
        id INTEGER PRIMARY KEY AUTOINCREMENT, \
        name TEXT UNIQUE, \
        applied_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP NOT NULL)";

    /// #3108: the schemas built so far in this process, by a hash of the database's
    /// name and migration contents. The language server analyses the project on
    /// every edit; a schema is rebuilt only when its migrations changed. A
    /// connection is only ever prepared against (never written), so sharing one
    /// between analyses is safe. Bounded: past [`CACHE_LIMIT`] entries it starts
    /// over, so edited migrations' stale schemas don't pile up.
    static SCHEMA_CACHE: Mutex<Option<HashMap<u64, CachedSchema>>> = Mutex::new(None);

    /// A cached schema with the migration contents it was built from, compared
    /// on a hit so a hash collision can't reuse another database's schema.
    struct CachedSchema {
        contents: Vec<(String, String)>,
        conn: Arc<Mutex<Connection>>,
    }

    fn migration_contents(files: &[crate::d1_migrations::MigrationFile]) -> Vec<(String, String)> {
        files
            .iter()
            .map(|f| (f.name.clone(), f.sql.clone()))
            .collect()
    }
    const CACHE_LIMIT: usize = 64;

    fn migrations_key(database: &str, files: &[crate::d1_migrations::MigrationFile]) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        database.hash(&mut h);
        for file in files {
            file.name.hash(&mut h);
            file.sql.hash(&mut h);
        }
        h.finish()
    }

    impl D1Sql {
        /// Apply every database's migrations to its own in-memory SQLite. One
        /// that fails is reported as a warning at its file, and the database is
        /// left out. Always `Some`: a database with no migrations still has its
        /// parameter counts checked (#3126).
        pub fn build(migrations: &D1Migrations, errors: &mut ErrorSink) -> Option<Arc<D1Sql>> {
            let mut schemas = HashMap::new();
            let mut migrated = std::collections::HashSet::new();
            for (database, files) in migrations {
                if files.is_empty() {
                    continue;
                }
                migrated.insert(database.clone());
                let key = migrations_key(database, files);
                let contents = migration_contents(files);
                // A poisoned lock (a panic elsewhere) is recovered, not treated as
                // a miss forever: the cache holds only complete entries.
                let cached = SCHEMA_CACHE
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .as_ref()
                    .and_then(|c| c.get(&key))
                    .filter(|hit| hit.contents == contents)
                    .map(|hit| hit.conn.clone());
                if let Some(conn) = cached {
                    schemas.insert(database.clone(), conn);
                    continue;
                }
                let Some(conn) = build_database(database, files, errors) else {
                    continue;
                };
                let conn = Arc::new(Mutex::new(conn));
                {
                    let mut cache = SCHEMA_CACHE.lock().unwrap_or_else(|e| e.into_inner());
                    let cache = cache.get_or_insert_with(HashMap::new);
                    if cache.len() >= CACHE_LIMIT {
                        cache.clear();
                    }
                    cache.insert(
                        key,
                        CachedSchema {
                            contents,
                            conn: conn.clone(),
                        },
                    );
                }
                schemas.insert(database.clone(), conn);
            }
            Some(Arc::new(D1Sql { schemas, migrated }))
        }

        /// Check every literal statement the unit's call sites recorded.
        pub fn check_unit(
            &self,
            unit: &str,
            table: &UnitTable,
            callees: &HashMap<ExprId, Callee>,
            // The source of the file the sites are in, to point an error inside
            // the SQL literal; `None` reports at the whole argument.
            source: Option<&str>,
        ) -> Vec<CompileError> {
            // A deterministic order: by position in the source.
            let mut sites: BTreeMap<(usize, usize), &D1SqlSite> = BTreeMap::new();
            let mut decodes: BTreeMap<(usize, usize), &D1DecodeSite> = BTreeMap::new();
            for callee in callees.values() {
                // #3139: a `Json.decodeSql` linked to a statement's rows.
                if let Callee::D1Decode(d) = callee {
                    decodes.insert((d.span.start, d.span.end), d);
                    continue;
                }
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
            // #3197: a batch is checked as a whole, the first time one of its
            // statements is reached (they are contiguous in source order).
            let mut batches_done: std::collections::HashSet<(String, usize)> =
                std::collections::HashSet::new();
            for site in sites.values() {
                let Some(instance) = table.derived.get(&site.cap).filter(|d| is_d1(d)) else {
                    continue;
                };
                let database = d1_database_name(&worker, &instance.binding);
                let Some(conn) = self.schemas.get(&database) else {
                    // No schema: checked for its parameter count alone, unless
                    // the database has migrations that failed (their warning
                    // already stands for every statement).
                    if !self.migrated.contains(&database)
                        && let Some(err) =
                            parameter_count(&site.text).and_then(|used| count_error(site, used))
                    {
                        out.push(err);
                    }
                    continue;
                };
                let mut conn = conn.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(position) = site.batch {
                    if batches_done.insert((database.clone(), position.group)) {
                        let members: Vec<&D1SqlSite> = sites
                            .values()
                            .copied()
                            .filter(|s| s.batch.is_some_and(|b| b.group == position.group))
                            .filter(|s| s.cap == site.cap)
                            .collect();
                        out.extend(check_batch(&mut conn, &members, &database, source));
                    }
                    continue;
                }
                if let Some(err) = check_site(&conn, site, &database, source) {
                    out.push(err);
                }
            }
            // #3139: each linked decode's required fields against the columns
            // of the statement the rows came from (nothing to check without a
            // schema, or when the statement itself doesn't prepare: that is
            // already reported at its site).
            for d in decodes.values() {
                let Some(instance) = table.derived.get(&d.site.cap).filter(|i| is_d1(i)) else {
                    continue;
                };
                let database = d1_database_name(&worker, &instance.binding);
                let Some(conn) = self.schemas.get(&database) else {
                    continue;
                };
                let conn = conn.lock().unwrap_or_else(|e| e.into_inner());
                out.extend(check_decode(&conn, d));
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

    /// #3108: the span of the token SQLite refused, inside the SQL literal, or
    /// `None` (the whole argument is reported) when the offset is unknown, lies
    /// in the compiler's generated `select` prefix, or can't be mapped back
    /// into the source with certainty.
    pub(super) fn error_span(source: &str, site: &D1SqlSite, offset: i32) -> Option<Span> {
        let offset = usize::try_from(offset).ok()?;
        let (decoded, origin) = match &site.literal {
            Some((lit, origin)) => (lit.as_str(), *origin),
            None => (site.text.as_str(), 0),
        };
        let at = usize::try_from(offset as isize - origin).ok()?;
        super::literal_subspan(source, site.span, decoded, at)
    }

    /// #3197: one literal `batch` list's statements, in order, each prepared
    /// against the schema the earlier ones leave. The work happens inside a
    /// savepoint whose guard rolls back (and releases) when it drops, so the
    /// cached schema connection comes out unchanged on every path, an early
    /// return or a panic included. Only DDL is executed: DML can fail on its
    /// parameters or a constraint, and never changes the schema. A statement
    /// that doesn't prepare is reported with its position in the list and
    /// leaves the schema as it was; checking goes on with the next one.
    ///
    /// A DDL statement that doesn't prepare isn't applied, so a later statement
    /// that depends on it (a failed `CREATE`, then an `INSERT` into the table it
    /// would have made) is reported too. That is accepted rather than
    /// suppressed: at run time the batch stops at the first failure, so the
    /// first error is the one to fix, and later ones go once it is fixed.
    pub(super) fn check_batch(
        conn: &mut Connection,
        members: &[&D1SqlSite],
        database: &str,
        source: Option<&str>,
    ) -> Vec<CompileError> {
        let mut out = Vec::new();
        let Ok(mut savepoint) = conn.savepoint() else {
            // No savepoint to work in: check each against the migrated schema.
            for site in members {
                out.extend(check_site(conn, site, database, source));
            }
            return out;
        };
        savepoint.set_drop_behavior(DropBehavior::Rollback);
        for site in members {
            match check_site(&savepoint, site, database, source) {
                Some(err) => {
                    let index = site.batch.map_or(0, |b| b.index) + 1;
                    out.push(err.with_note(format!(
                        "statement {index} of the batch; a batch's statements run in order, so each is checked against the schema the earlier ones leave"
                    )));
                }
                None => apply_ddl(&savepoint, &site.text),
            }
        }
        out
    }

    /// Whether `sql` begins (past whitespace and comments) with a schema
    /// statement.
    pub(super) fn is_ddl(sql: &str) -> bool {
        let mut rest = sql;
        loop {
            rest = rest.trim_start();
            if let Some(after) = rest.strip_prefix("--") {
                rest = after.split_once('\n').map_or("", |(_, tail)| tail);
            } else if let Some(after) = rest.strip_prefix("/*") {
                rest = after.split_once("*/").map_or("", |(_, tail)| tail);
            } else {
                break;
            }
        }
        let word: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphabetic())
            .collect::<String>()
            .to_ascii_uppercase();
        matches!(word.as_str(), "CREATE" | "DROP" | "ALTER")
    }

    /// Execute `sql` when it is DDL, so the next statement of the batch sees its
    /// effect. A failure to execute leaves the schema as it was.
    fn apply_ddl(conn: &Connection, sql: &str) {
        if !is_ddl(sql) {
            return;
        }
        if let Ok(mut stmt) = conn.prepare(sql)
            && stmt.parameter_count() == 0
        {
            let _ = stmt.execute([]);
        }
    }

    fn check_site(
        conn: &Connection,
        site: &D1SqlSite,
        database: &str,
        source: Option<&str>,
    ) -> Option<CompileError> {
        let what = format!("`{}.{}`", site.cap, site.op);
        let stmt = match conn.prepare(&site.text) {
            Ok(stmt) => stmt,
            Err(e) => {
                let (msg, offset) = match &e {
                    rusqlite::Error::SqlInputError { msg, offset, .. } => (msg.clone(), *offset),
                    other => (other.to_string(), -1),
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
                let span = source
                    .and_then(|src| error_span(src, site, offset))
                    .unwrap_or(site.span);
                return Some(
                    CompileError::new(
                        "bynk.d1.sql_invalid",
                        span,
                        format!("the SQL of {what} doesn't prepare: {msg}"),
                    )
                    .with_note(format!(
                        "checked against the migrations of `{database}`, applied to an empty in-memory SQLite"
                    )),
                );
            }
        };
        count_error(site, stmt.parameter_count())
    }

    /// #3139: the required fields of the decoded row with no result column in the
    /// statement the rows came from, one `select_field_missing` each.
    fn check_decode(conn: &Connection, d: &D1DecodeSite) -> Vec<CompileError> {
        let Ok(stmt) = conn.prepare(&d.site.text) else {
            return Vec::new();
        };
        let columns: Vec<&str> = stmt.column_names().collect();
        let what = format!("`{}.{}`", d.site.cap, d.site.op);
        d.required
            .iter()
            .filter(|f| !columns.contains(&f.as_str()))
            .map(|f| {
                CompileError::new(
                    "bynk.d1.select_field_missing",
                    d.span,
                    format!(
                        "`{}` has a field `{f}`, but the statement of {what} has no column `{f}`",
                        d.row
                    ),
                )
                .with_note(
                    "`Json.decodeSql` reads columns by name; alias the column in the SQL (`… AS name`) or rename the field",
                )
            })
            .collect()
    }

    /// `bynk.d1.sql_param_count` when the SQL uses `used` parameters and the
    /// list literal passes a different number.
    fn count_error(site: &D1SqlSite, used: usize) -> Option<CompileError> {
        let what = format!("`{}.{}`", site.cap, site.op);
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

/// #3126: how many parameters `sql` binds, by SQLite's rule
/// (`sqlite3_bind_parameter_count`), without a schema: `?NNN` is `NNN`, a bare
/// `?` the next number after the highest so far, the count the highest index,
/// and a named parameter (`:a`, `@a`, `$a`) takes the next number the first
/// time its name is seen. `None` for an index SQLite refuses (`?0`, past 32766). Quoted text (`'…'`, `"…"`, `` `…` ``, `[…]`) and
/// comments hold none. It mirrors `sqlParameters` in the test tier's runtime
/// (`bynk-emit/runtime/src/storage.ts`, #3043), which is the baseline for the
/// named forms; the `agrees_with_sqlite` test pins it against `rusqlite`.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn parameter_count(sql: &str) -> Option<usize> {
    let b = sql.as_bytes();
    let n = b.len();
    let mut count = 0usize;
    let mut names: std::collections::HashSet<&[u8]> = std::collections::HashSet::new();
    let mut i = 0;
    // Skip a quoted run starting at `i`; `doubled` when the close doubles to escape.
    let skip = |i: &mut usize, close: u8, doubled: bool| {
        *i += 1;
        while *i < n {
            if b[*i] == close {
                if doubled && b.get(*i + 1) == Some(&close) {
                    *i += 2;
                } else {
                    break;
                }
            } else {
                *i += 1;
            }
        }
        *i += 1;
    };
    // SQLite's tokenizer takes every byte >= 0x80 as part of an identifier.
    let is_word = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c == b'$' || c >= 0x80;
    while i < n {
        let c = b[i];
        match c {
            b'\'' | b'"' | b'`' => skip(&mut i, c, true),
            b'[' => skip(&mut i, b']', false),
            b'-' if b.get(i + 1) == Some(&b'-') => {
                while i < n && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i = match sql[i + 2..].find("*/") {
                    Some(end) => i + 2 + end + 2,
                    None => n,
                };
            }
            b'?' => {
                i += 1;
                let start = i;
                while i < n && b[i].is_ascii_digit() {
                    i += 1;
                }
                if i > start {
                    // SQLite refuses `?0` and an index past its limit (32766) as
                    // a syntax error, so there is no count to compare.
                    match sql[start..i].parse::<usize>() {
                        Ok(index) if (1..=32766).contains(&index) => count = count.max(index),
                        _ => return None,
                    }
                } else {
                    count = count.saturating_add(1);
                }
            }
            // `$` continues an identifier (`a$b`); `:` and `@` always start a parameter.
            b':' | b'@' | b'$' if c != b'$' || !(i > 0 && is_word(b[i - 1])) => {
                let start = i;
                i += 1;
                while i < n && is_word(b[i]) {
                    i += 1;
                }
                if i - start > 1 && names.insert(&b[start..i]) {
                    count = count.saturating_add(1);
                }
            }
            _ => i += 1,
        }
    }
    Some(count)
}

/// The source span of the token starting at byte `at` of a string literal's
/// decoded value `decoded`, for the literal token `lit` in `source`.
///
/// The decoded bytes are aligned with the lexeme's: an escape (`\\n`, `\\"`, `\\u{…}`)
/// stands for one decoded character, and bytes the decoder drops (a multi-line
/// literal's indent) are skipped. The alignment is *verified*: the lexeme text
/// at the mapped range must equal the decoded token, so a form the alignment
/// gets wrong (or can't handle: raw-looking oddities, a `const`'s use site,
/// which isn't a literal) gives `None`, never a wrong span.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn literal_subspan(
    source: &str,
    lit: bynk_syntax::span::Span,
    decoded: &str,
    at: usize,
) -> Option<bynk_syntax::span::Span> {
    use bynk_syntax::span::Span;
    let lexeme = source.get(lit.start..lit.end)?;
    let bytes = lexeme.as_bytes();
    // Where the value's text begins: past the opening quote (and a raw literal's
    // `r` and hashes), or, for a multi-line literal, past the opener's line.
    let raw = bytes.first() == Some(&b'r');
    let quote = lexeme.find('"')?;
    if bytes[..quote].iter().any(|b| !matches!(b, b'r' | b'#')) {
        return None;
    }
    let mut i = if lexeme[quote..].starts_with("\"\"\"") {
        quote + lexeme[quote..].find('\n')? + 1
    } else {
        quote + 1
    };
    // The token: a quoted name, or a run of word characters, or one character.
    let tok_end = {
        let rest = decoded.get(at..)?;
        let mut chars = rest.char_indices();
        let (_, first) = chars.next()?;
        let len = if first == '"' {
            rest[1..].find('"').map_or(1, |n| n + 2)
        } else if first.is_alphanumeric() || first == '_' {
            rest.char_indices()
                .find(|(_, c)| !(c.is_alphanumeric() || *c == '_'))
                .map_or(rest.len(), |(n, _)| n)
        } else {
            first.len_utf8()
        };
        at + len
    };
    let token = decoded.get(at..tok_end)?;
    let mut start = None;
    let mut end = 0;
    for (idx, c) in decoded.char_indices() {
        if idx >= tok_end {
            break;
        }
        // Advance to the lexeme byte that spells `c`.
        let found = loop {
            let rest = lexeme.get(i..)?;
            let mut it = rest.chars();
            let head = it.next()?;
            if head == '\\' && !raw {
                let (decoded_char, len) = unescape(rest)?;
                if decoded_char == c {
                    break (i, len);
                }
                i += len;
            } else if head == c {
                break (i, head.len_utf8());
            } else {
                i += head.len_utf8();
            }
        };
        if idx == at {
            start = Some(found.0);
        }
        end = found.0 + found.1;
        i = end;
    }
    let (start, end) = (start?, end);
    (lexeme.get(start..end)? == token)
        .then(|| Span::new_in(lit.file, lit.start + start, lit.start + end))
}

/// The character an escape at the head of `s` (which starts with `\`) denotes,
/// and the escape's length in bytes.
#[cfg(not(target_arch = "wasm32"))]
fn unescape(s: &str) -> Option<(char, usize)> {
    let b = s.as_bytes();
    Some(match *b.get(1)? {
        b'n' => ('\n', 2),
        b't' => ('\t', 2),
        b'r' => ('\r', 2),
        b'0' => ('\0', 2),
        b'\\' => ('\\', 2),
        b'"' => ('"', 2),
        b'\'' => ('\'', 2),
        b'u' => {
            let close = s.find('}')?;
            let hex = s.get(3..close)?;
            (
                char::from_u32(u32::from_str_radix(hex, 16).ok()?)?,
                close + 1,
            )
        }
        _ => return None,
    })
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
        let built = built.expect("built");
        assert!(
            built.schemas.is_empty(),
            "no database left to check against"
        );
        assert_eq!(
            built.migrated.len(),
            1,
            "but it is still known to be migrated"
        );
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

    /// #3126: the scanner agrees with SQLite's own count on a corpus (the
    /// cases #3043 pinned, plus the quoting forms and named parameters).
    #[test]
    fn the_scanner_agrees_with_sqlite() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE t (a, b, c)").unwrap();
        let corpus = [
            "SELECT * FROM t",
            "SELECT * FROM t WHERE a = ?1 AND b = ?2",
            "SELECT * FROM t WHERE a = ?1 OR b = ?1",
            "SELECT * FROM t WHERE a = ?3",
            "SELECT * FROM t WHERE a = ? AND b = ?",
            "SELECT * FROM t WHERE a = ?2 AND b = ?",
            "SELECT * FROM t WHERE a = ? AND b = ?5 AND c = ?",
            "SELECT '?1' FROM t",
            "SELECT 'it''s ?1' FROM t WHERE a = ?1",
            "SELECT \"?1\" AS \"?2\" FROM t",
            "SELECT `?1` FROM (SELECT 1 AS `?1`) WHERE 1 = ?1",
            "SELECT [?1] FROM (SELECT 1 AS [?1]) WHERE 1 = ?1",
            "SELECT * FROM t -- ?1\nWHERE a = ?1",
            "SELECT * FROM t /* ?5 */ WHERE a = ?1",
            "SELECT * FROM t WHERE a = ?1 /* unterminated ?9",
            "SELECT * FROM t WHERE a = :x AND b = :x AND c = :y",
            "SELECT * FROM t WHERE a = @x AND b = $y",
            "SELECT * FROM t WHERE a = ?1 AND b = :x",
            "SELECT * FROM t WHERE a = :x AND b = ?",
            "SELECT * FROM t WHERE a = ?01 AND b = ?1",
            "INSERT INTO t VALUES (?, ?, ?)",
            "INSERT INTO t VALUES (?1, ?2, ?3) -- ?4",
            "SELECT * FROM t WHERE a = :café AND b = :cafè",
            "SELECT * FROM t WHERE a = :é",
        ];
        for sql in corpus {
            let want = conn.prepare(sql).unwrap().parameter_count();
            assert_eq!(super::parameter_count(sql), Some(want), "{sql}");
        }
        // An index SQLite refuses has no count (it is a syntax error there).
        for sql in [
            "SELECT ?0",
            "SELECT ?32767",
            "SELECT ?99999999999999999999999",
        ] {
            assert!(conn.prepare(sql).is_err(), "{sql}");
            assert_eq!(super::parameter_count(sql), None, "{sql}");
        }
    }

    #[test]
    fn a_database_without_migrations_is_checked_for_its_parameter_count_alone() {
        let mut errors = ErrorSink::new();
        let built = D1Sql::build(&migrations(&[]), &mut errors).expect("always built");
        assert!(built.schemas.is_empty() && built.migrated.is_empty());
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

    /// #3108: the token at a decoded offset, found again in the literal's source.
    fn sub(source: &str, decoded: &str, needle: &str) -> Option<String> {
        let lit_start = source.find(['"', 'r']).unwrap();
        let lit_end = source.rfind(['"', '#']).unwrap() + 1;
        let at = decoded.find(needle).unwrap();
        let lit = bynk_syntax::span::Span::new(lit_start, lit_end);
        literal_subspan(source, lit, decoded, at).map(|s| source[s.start..s.end].to_string())
    }

    #[test]
    fn a_token_is_found_in_a_plain_literal_with_escapes() {
        let src = r#"f("SELECT a,\n b FROM \"t\" WHERE stauts = ?1")"#;
        let decoded = "SELECT a,\n b FROM \"t\" WHERE stauts = ?1";
        assert_eq!(sub(src, decoded, "stauts").as_deref(), Some("stauts"));
        assert_eq!(sub(src, decoded, "b FROM").as_deref(), Some("b"));
    }

    #[test]
    fn a_token_is_found_in_a_multi_line_literal_past_its_indent_and_tag() {
        let src = "f(\"\"\"sql\n\t\tselect id\n\t\tFROM orders\n\t\tWHERE stauts = ?1\n\t\t\"\"\")";
        let decoded = "select id\nFROM orders\nWHERE stauts = ?1";
        assert_eq!(sub(src, decoded, "stauts").as_deref(), Some("stauts"));
        assert_eq!(sub(src, decoded, "select").as_deref(), Some("select"));
    }

    #[test]
    fn a_raw_literal_maps_through_unescaped() {
        let src = r##"f(r#"SELECT \n FROM t WHERE stauts = 1"#)"##;
        let decoded = r"SELECT \n FROM t WHERE stauts = 1";
        assert_eq!(sub(src, decoded, "stauts").as_deref(), Some("stauts"));
    }

    #[test]
    fn a_mapping_that_cannot_be_verified_falls_back() {
        // Not a literal at all (a `const`'s use site): no quote to start from.
        let lit = bynk_syntax::span::Span::new(2, 8);
        assert_eq!(literal_subspan("f(CONSTX)", lit, "SELECT x", 0), None);
    }

    /// #3108: an unchanged database's schema is built once and shared; a changed
    /// migration builds a new one.
    #[test]
    fn a_schema_is_cached_until_its_migrations_change() {
        let build = |sql: &str| {
            let mut errors = ErrorSink::new();
            D1Sql::build(&migrations(&[("0001_a.sql", sql)]), &mut errors).expect("applies")
        };
        let a = build("CREATE TABLE cache_probe (id TEXT);");
        let b = build("CREATE TABLE cache_probe (id TEXT);");
        assert!(
            std::sync::Arc::ptr_eq(&a.schemas["demo-orders"], &b.schemas["demo-orders"]),
            "the same migrations reuse the built schema"
        );
        let c = build("CREATE TABLE cache_probe (id TEXT, more TEXT);");
        assert!(!std::sync::Arc::ptr_eq(
            &a.schemas["demo-orders"],
            &c.schemas["demo-orders"]
        ));
        assert!(
            c.schemas["demo-orders"]
                .lock()
                .unwrap()
                .prepare("SELECT more FROM cache_probe")
                .is_ok(),
            "the edited migration's columns are there"
        );
    }

    /// A migration that fails is not cached: it warns on every build.
    #[test]
    fn a_failing_migration_still_warns_each_time() {
        for _ in 0..2 {
            let mut errors = ErrorSink::new();
            let built = D1Sql::build(
                &migrations(&[("0001_a.sql", "NOT SQL AT ALL;")]),
                &mut errors,
            );
            assert!(built.expect("built").schemas.is_empty());
            assert_eq!(errors.into_warnings().len(), 1);
        }
    }

    fn site(
        text: &str,
        literal: Option<(&str, isize)>,
        span: (usize, usize),
    ) -> crate::checker::D1SqlSite {
        crate::checker::D1SqlSite {
            cap: "Orders".into(),
            op: "select".into(),
            text: text.into(),
            span: bynk_syntax::span::Span::new(span.0, span.1),
            row: None,
            passed: None,
            literal: literal.map(|(l, o)| (l.to_string(), o)),
            batch: None,
        }
    }

    /// An offset in the compiler's generated `SELECT <columns>` prefix isn't in
    /// the author's literal: the whole argument is reported.
    #[test]
    fn an_error_in_the_generated_select_prefix_falls_back() {
        let source = r#"f("FROM t WHERE x = 1")"#;
        let text = "SELECT \"id\" FROM t WHERE x = 1";
        let site = site(text, Some(("FROM t WHERE x = 1", 12)), (2, 22));
        // Offset 7 is the `"id"` the compiler wrote; offset 25 is `x` in the tail.
        assert_eq!(super::native::error_span(source, &site, 7), None);
        let span = super::native::error_span(source, &site, 25).expect("in the tail");
        assert_eq!(&source[span.start..span.end], "x");
    }

    /// A quoted identifier written with escaped quotes can't be mapped
    /// verbatim; it falls back rather than point at the wrong text.
    #[test]
    fn an_escaped_quoted_identifier_falls_back() {
        let source = r#"f("SELECT \"nope\" FROM t")"#;
        let decoded = "SELECT \"nope\" FROM t";
        let lit = bynk_syntax::span::Span::new(2, source.len() - 1);
        assert_eq!(literal_subspan(source, lit, decoded, 7), None);
    }

    /// #3197: a statement of a `batch`, at `index`, in the batch at `group`.
    fn batch_site(text: &str, index: usize) -> crate::checker::D1SqlSite {
        crate::checker::D1SqlSite {
            op: "batch".into(),
            batch: Some(crate::checker::BatchPosition { group: 7, index }),
            // A distinct span per statement, as in source.
            ..site(text, None, (index * 100, index * 100 + 50))
        }
    }

    /// Check one batch's statements against a database built from `migration`;
    /// returns the diagnostics and the (shared, cached) schema connection.
    fn check_batch_of(
        migration: &str,
        statements: &[&str],
    ) -> (
        Vec<bynk_syntax::error::CompileError>,
        std::sync::Arc<std::sync::Mutex<Connection>>,
    ) {
        let mut errors = ErrorSink::new();
        let built =
            D1Sql::build(&migrations(&[("0001_a.sql", migration)]), &mut errors).expect("applies");
        let conn = built.schemas["demo-orders"].clone();
        let sites: Vec<_> = statements
            .iter()
            .enumerate()
            .map(|(i, text)| batch_site(text, i))
            .collect();
        let members: Vec<&crate::checker::D1SqlSite> = sites.iter().collect();
        let out = super::native::check_batch(
            &mut conn.lock().unwrap_or_else(|e| e.into_inner()),
            &members,
            "demo-orders",
            None,
        );
        (out, conn)
    }

    fn tables(conn: &std::sync::Arc<std::sync::Mutex<Connection>>) -> Vec<String> {
        let conn = conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_schema WHERE type = 'table' ORDER BY name")
            .unwrap();
        stmt.query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    /// #3197: `DROP`, then `CREATE` of the same table, then an `INSERT` into it
    /// is a valid batch: the `CREATE` runs after the `DROP`.
    #[test]
    fn a_batch_may_drop_then_recreate_a_table() {
        let (out, _) = check_batch_of(
            "CREATE TABLE b3197_books (isbn, title);",
            &[
                "DROP TABLE b3197_books",
                "CREATE TABLE b3197_books (isbn, title, author, pages, finished)",
                "INSERT INTO b3197_books VALUES ('x', 'Notes', 'Anon', 10, 2)",
            ],
        );
        assert!(out.is_empty(), "{out:?}");
    }

    /// #3197: a table created earlier in the batch exists for a later statement.
    #[test]
    fn a_batch_may_use_a_table_it_creates() {
        let (out, _) = check_batch_of(
            "CREATE TABLE b3197_other (id);",
            &[
                "CREATE TABLE b3197_fresh (id, name)",
                "INSERT INTO b3197_fresh VALUES (1, 'a')",
                "ALTER TABLE b3197_fresh ADD COLUMN extra",
                "UPDATE b3197_fresh SET extra = 'x' WHERE id = ?1",
            ],
        );
        assert!(out.is_empty(), "{out:?}");
    }

    /// #3197: a genuinely invalid statement after DDL is still refused, at that
    /// statement's own span, naming its position, and a later valid one is not.
    #[test]
    fn an_invalid_statement_after_ddl_is_refused_at_its_element() {
        let (out, _) = check_batch_of(
            "CREATE TABLE b3197_t (id);",
            &[
                "CREATE TABLE b3197_u (id)",
                "INSERT INTO b3197_u (nope) VALUES (1)",
                "INSERT INTO b3197_u (id) VALUES (1)",
            ],
        );
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].category, "bynk.d1.sql_invalid");
        assert!(out[0].message.contains("nope"), "{}", out[0].message);
        assert_eq!((out[0].span.start, out[0].span.end), (100, 150));
        assert!(
            out[0]
                .notes
                .iter()
                .any(|n| n.contains("statement 2 of the batch")),
            "{:?}",
            out[0].notes
        );
    }

    /// #3197: a statement that doesn't prepare doesn't change the schema, so a
    /// later statement still sees the table as it was.
    #[test]
    fn a_failing_ddl_statement_leaves_the_schema_alone() {
        let (out, _) = check_batch_of(
            "CREATE TABLE b3197_k (id);",
            &[
                "CREATE TABLE b3197_k (id)",      // already exists: refused
                "INSERT INTO b3197_k VALUES (1)", // still the migrated table
            ],
        );
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(
            out[0].message.contains("already exists"),
            "{}",
            out[0].message
        );
    }

    /// #3197: the cached schema connection comes out of a batch unchanged, so
    /// the next batch, or a single statement, sees the migrated schema: the
    /// dropped table is back and the created one is gone. Rolled back even
    /// when a statement is refused part-way.
    #[test]
    fn the_cached_schema_is_unchanged_after_a_batch() {
        let migration = "CREATE TABLE b3197_keep (id, name);";
        let before = {
            let (_, conn) = check_batch_of(migration, &[]);
            tables(&conn)
        };
        assert!(before.contains(&"b3197_keep".to_string()), "{before:?}");
        let (out, conn) = check_batch_of(
            migration,
            &[
                "DROP TABLE b3197_keep",
                "CREATE TABLE b3197_made (id)",
                "SELECT nope FROM b3197_made", // refused part-way
                "ALTER TABLE b3197_made ADD COLUMN more",
            ],
        );
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(tables(&conn), before, "the migrated schema is untouched");
        // A separate statement sees the original schema, and a second batch
        // starts from it too.
        let c = conn.lock().unwrap_or_else(|e| e.into_inner());
        assert!(c.prepare("SELECT name FROM b3197_keep").is_ok());
        assert!(c.prepare("SELECT id FROM b3197_made").is_err());
        drop(c);
        let (again, _) = check_batch_of(migration, &["INSERT INTO b3197_keep VALUES (1, 'a')"]);
        assert!(again.is_empty(), "{again:?}");
    }

    /// DML is never executed; only DDL is recognised, past comments.
    #[test]
    fn only_schema_statements_are_executed() {
        use super::native::is_ddl;
        for ddl in [
            "CREATE TABLE t (a)",
            "  drop table t",
            "-- note\nALTER TABLE t ADD c",
            "/* x */ CREATE INDEX i ON t (a)",
        ] {
            assert!(is_ddl(ddl), "{ddl}");
        }
        for other in [
            "INSERT INTO t VALUES (1)",
            "UPDATE t SET a = 1",
            "SELECT 1",
            "PRAGMA x",
            "",
        ] {
            assert!(!is_ddl(other), "{other}");
        }
    }
}

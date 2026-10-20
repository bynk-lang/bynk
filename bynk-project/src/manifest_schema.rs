//! #2096: the JSON Schema types for the `bynk.toml` tables this crate checks.
//!
//! `[project]`, `[paths]`, `[workers]`, `[unstable]` and the packaging tables
//! are read by hand, from a `toml::Table`, against
//! [`MANIFEST_TABLES`](crate::MANIFEST_TABLES) and
//! [`MANIFEST_MAP_TABLES`](crate::MANIFEST_MAP_TABLES), so there is no serde
//! struct to derive a schema from. These types describe them for an
//! editor instead: each field's doc comment is the description a TOML extension
//! shows on hover. `[lsp]` is here too, because its reader lives in `bynk-lsp`,
//! which nothing that assembles the schema can depend on; `bynk-lsp`'s own test
//! holds its `LspSection` to this one. `[fmt]` is not here: `bynk-fmt` derives it
//! from the struct that reads it (`bynk_fmt::fmt_table_schema`).
//!
//! `cargo xtask manifest-schema` assembles the whole schema, and its test fails
//! when a table or key here disagrees with
//! [`MANIFEST_TABLES`](crate::MANIFEST_TABLES) or the committed
//! schema is stale. Only built with the `schema` feature.

use schemars::{JsonSchema, Schema, SchemaGenerator};

/// A list of paths, or one path on its own (`include = "src"`).
#[derive(JsonSchema)]
#[serde(untagged)]
#[allow(dead_code)]
enum PathList {
    One(String),
    Many(Vec<String>),
}

/// Project metadata. Neither key affects compilation: both are for display
/// only, and have no default.
#[derive(JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct ProjectTable {
    /// Display name for the project.
    name: Option<String>,
    /// Version string for the project.
    version: Option<String>,
}

/// The project's source trees. Each path is relative to the directory holding
/// `bynk.toml`. A conventional `src/` (and `tests/`) project, or a flat one with
/// its `.bynk` files at the root, needs no `[paths]` table.
#[derive(JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct PathsTable {
    /// Trees to compile. Default: the conventional roots that exist (`src`, and
    /// `tests` when present), or the project root itself when neither does.
    include: Option<PathList>,
    /// Subtrees to skip during discovery (monorepo, vendored or generated
    /// `.bynk`). The tool's own `out` and `node_modules` caches and
    /// dot-directories are always skipped. Default: `[]`.
    #[schemars(extend("default" = []))]
    exclude: Option<PathList>,
}

/// Language-server settings, read by `bynkc-lsp`.
#[derive(JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct LspTable {
    /// When diagnostics are computed: `"live"` (as you type) or `"on_save"`.
    /// Default: `"live"`.
    #[schemars(extend("enum" = ["live", "on_save"], "default" = "live"))]
    diagnostics_mode: Option<String>,
    /// Debounce interval, in milliseconds, for live diagnostics. Default: `300`.
    #[schemars(extend("default" = 300))]
    diagnostics_debounce_ms: Option<u64>,
}

/// Settings for the Cloudflare Workers a Workers build emits (`bynkc compile
/// --target workers`, `bynk dev`, `bynk deploy`).
#[derive(JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct WorkersTable {
    /// Extra Cloudflare compatibility flags for every Worker's `wrangler.toml`,
    /// after the default set (`global_fetch_strictly_public`). Names aren't
    /// checked; a flag listed twice, or one already on by default, is dropped
    /// with a warning. Default: `[]`.
    #[schemars(extend("default" = []))]
    compatibility_flags: Option<Vec<String>>,
}

/// Opts in to experimental standard-library items, which carry no
/// compatibility promise and can't be used until they are listed here.
#[derive(JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct UnstableTable {
    /// Item paths to allow: a unit (`bynk.cloudflare`), an item it exports
    /// (`bynk.cloudflare.Kv`), or a capability operation
    /// (`bynk.cloudflare.Kv.list`). An entry covers its path and everything
    /// under it. Default: `[]`.
    #[schemars(extend("default" = []))]
    allow: Option<Vec<String>>,
}

/// The deployments `bynk test --against <name>` may run the `system` suites
/// against (end-to-end testing). A bare URL is never accepted on the command
/// line: a target has to be named here.
#[derive(JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct TestTable {
    /// Named targets, as `[test.targets.<name>]`. `dev` is reserved for the
    /// local `bynk dev` the runner starts itself.
    targets: Option<std::collections::BTreeMap<String, TestTargetTable>>,
}

/// One deployment `bynk test --against <name>` runs against.
#[derive(JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct TestTargetTable {
    /// Each Worker's base URL, keyed by its worker-directory name
    /// (`url.shop-orders = "https://shop-orders-preview.example.workers.dev"`).
    url: std::collections::BTreeMap<String, String>,
    /// The names of the secrets the deployed Workers verify with. Their values
    /// are read from the environment when the suites run, never from this
    /// file. Default: `[]`.
    #[schemars(extend("default" = []))]
    secrets: Option<Vec<String>>,
    /// The non-secret values the target was deployed with. A case runs
    /// against the target only if its `env` agrees with them. Default: `{}`.
    vars: Option<std::collections::BTreeMap<String, String>>,
    /// Marks a production deployment, which `--against` refuses. Default:
    /// `false`.
    #[schemars(extend("default" = false))]
    production: Option<bool>,
}

/// A shareable project's identity. A project has `[package]` or `[project]`,
/// never both.
#[derive(JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct PackageTable {
    /// The package's name, `organisation/package` (`acme/money`). The
    /// organisation matches `[a-z][a-z0-9-]*`; the package matches
    /// `[a-z][a-z0-9_]*` and isn't a Bynk keyword, since a dependency's
    /// alias defaults to it. The organisation `bynk` is reserved.
    #[schemars(extend("pattern" = "^[a-z][a-z0-9-]*/[a-z][a-z0-9_]*$"))]
    name: String,
    /// The package's version, in strict semver (`1.4.0`). Publishing will
    /// require it.
    version: Option<String>,
    /// A one-line description of the package.
    description: Option<String>,
    /// The package's licence, as an SPDX expression (`MIT`).
    license: Option<String>,
    /// The URL of the package's source repository.
    repository: Option<String>,
    /// The units the package shares: commons and adapters it declares, never
    /// a context. Default: every commons and adapter.
    exports: Option<Vec<String>>,
}

/// One dependency: `package`, and exactly one of `version`, `git` (with
/// `rev`) or `path`. `path` dependencies resolve; `git` and `version` are
/// refused until later packaging slices.
#[derive(JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct DependencyEntry {
    /// The package, as `organisation/package`.
    package: String,
    /// A pinned version range from the registry (`^1.4`). Refused until
    /// packaging slice 4.
    version: Option<String>,
    /// A git repository URL. Needs `rev`. Refused until packaging slice 2b.
    git: Option<String>,
    /// The tag or commit of `git` to use.
    rev: Option<String>,
    /// A directory holding the package's `bynk.toml`, relative to this
    /// project.
    path: Option<String>,
}

/// The packages this project depends on, keyed by alias: the name source
/// reaches each one's units by (`<alias>.<unit>`). An alias matches
/// `[a-z][a-z0-9_]*`, isn't a Bynk keyword, and isn't `bynk`. Only a
/// dependency's exported commons and adapters are reachable.
#[derive(JsonSchema)]
#[schemars(extend("propertyNames" = { "pattern": "^[a-z][a-z0-9_]*$" }))]
#[allow(dead_code)]
struct DependenciesTable(std::collections::BTreeMap<String, DependencyEntry>);

/// The packages only this project's tests depend on, keyed by alias, as in
/// `[dependencies]`. Not resolved yet: a non-empty table is refused until
/// packaging slice 2b.
#[derive(JsonSchema)]
#[schemars(extend("propertyNames" = { "pattern": "^[a-z][a-z0-9_]*$" }))]
#[allow(dead_code)]
struct DevDependenciesTable(std::collections::BTreeMap<String, DependencyEntry>);

/// One package registry. Credentials never go here: a token is read from
/// `BYNK_REGISTRY_<NAME>_TOKEN` or the user credentials file.
#[derive(JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct RegistryEntry {
    /// The registry's index URL: `https://`, or `http://` to `localhost` only.
    index: String,
    /// The organisations whose packages resolve on this registry. An
    /// organisation is listed under one registry at most, and never `bynk`.
    organisations: Option<Vec<String>>,
    /// The registry this one mirrors: `bynk` (the first-party registry) or
    /// another registry here that isn't itself a mirror. Not with
    /// `organisations`.
    #[serde(rename = "mirror-of")]
    mirror_of: Option<String>,
    /// The registry's pinned signing key, `ed25519:<base64>`.
    key: Option<String>,
}

/// Package registries, keyed by name (`[a-z][a-z0-9-]*`, not `bynk`). Checked
/// now, used once packages are fetched. In a workspace, only the root declares
/// them.
#[derive(JsonSchema)]
#[schemars(extend("propertyNames" = { "pattern": "^[a-z][a-z0-9-]*$" }))]
#[allow(dead_code)]
struct RegistriesTable(std::collections::BTreeMap<String, RegistryEntry>);

/// Makes this directory a workspace root: a group of projects that `bynk
/// check`, `fmt` and `test` run across. A workspace root isn't a project:
/// its `bynk.toml` holds only `[workspace]` and `[registries]`.
#[derive(JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct WorkspaceTable {
    /// The member directories, relative to the root. A component may use `*`
    /// and `?`, each `*` within one directory level (`packages/*`). Each
    /// member holds its own `bynk.toml`.
    members: Vec<String>,
    /// Directories `members` matches that aren't members, as patterns.
    /// Default: `[]`.
    #[schemars(extend("default" = []))]
    exclude: Option<Vec<String>>,
}

/// The schema of the `bynk.toml` table `name`, or `None` for a table this
/// crate doesn't describe (`fmt`, which is `bynk-fmt`'s, or an unknown name).
pub fn table_schema(name: &str, generator: &mut SchemaGenerator) -> Option<Schema> {
    Some(match name {
        "project" => generator.subschema_for::<ProjectTable>(),
        "paths" => generator.subschema_for::<PathsTable>(),
        "lsp" => generator.subschema_for::<LspTable>(),
        "workers" => generator.subschema_for::<WorkersTable>(),
        "unstable" => generator.subschema_for::<UnstableTable>(),
        "test" => generator.subschema_for::<TestTable>(),
        "package" => generator.subschema_for::<PackageTable>(),
        "workspace" => generator.subschema_for::<WorkspaceTable>(),
        "dependencies" => generator.subschema_for::<DependenciesTable>(),
        "dev-dependencies" => generator.subschema_for::<DevDependenciesTable>(),
        "registries" => generator.subschema_for::<RegistriesTable>(),
        _ => return None,
    })
}

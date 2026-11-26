//! #3941: the per-environment values a `const X: T = env(NAME[, default])`
//! takes from `bynk.toml`, at build time:
//!
//! ```toml
//! [vars]                                   # every environment, `default` included
//! APP_ORIGIN = "https://app.shop.example"
//!
//! [env.staging.vars]                       # this environment, over `[vars]`
//! APP_ORIGIN = "https://staging.app.shop.example"
//! ```
//!
//! These are build-time configuration, committed with the project, and are not
//! Wrangler's run-time `[vars]`. The value for an environment is its own
//! `[env.<name>.vars]`, then `[vars]`; the constant's own default comes last,
//! in the checker.

use crate::discovery::read_source;
use crate::paths::ProjectPathsError;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

/// The name of the environment a build without `--env` uses.
pub const DEFAULT_ENVIRONMENT: &str = "default";

/// Whether `name` is a variable name: `[A-Z][A-Z0-9_]*`.
pub fn is_var_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_uppercase())
        && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// One variable's value, as `bynk.toml` wrote it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VarValue {
    Str(String),
    Int(i64),
    Bool(bool),
    /// An array of strings.
    List(Vec<String>),
    /// Anything else (a float, a table, a mixed array), kept by its kind so the
    /// checker can say what the value is.
    Other(&'static str),
}

impl VarValue {
    /// The value as a literal is written (`"x"`, `3`, `true`, `["a", "b"]`).
    pub fn shown(&self) -> String {
        match self {
            Self::Str(s) => format!("{s:?}"),
            Self::Int(n) => n.to_string(),
            Self::Bool(b) => b.to_string(),
            Self::List(items) => {
                let parts: Vec<String> = items.iter().map(|i| format!("{i:?}")).collect();
                format!("[{}]", parts.join(", "))
            }
            Self::Other(kind) => (*kind).to_string(),
        }
    }

    fn from_toml(v: &toml::Value) -> Self {
        match v {
            toml::Value::String(s) => Self::Str(s.clone()),
            toml::Value::Integer(n) => Self::Int(*n),
            toml::Value::Boolean(b) => Self::Bool(*b),
            toml::Value::Array(items) => items
                .iter()
                .map(|i| i.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
                .map_or(Self::Other("an array that is not all strings"), Self::List),
            toml::Value::Float(_) => Self::Other("a float"),
            toml::Value::Datetime(_) => Self::Other("a date-time"),
            toml::Value::Table(_) => Self::Other("a table"),
        }
    }

    /// What this value is, for a message: `a string`, `an integer`, …
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Str(_) => "a string",
            Self::Int(_) => "an integer",
            Self::Bool(_) => "a boolean",
            Self::List(_) => "a list of strings",
            Self::Other(what) => what,
        }
    }
}

/// Where a resolved variable's value came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// `[env.<name>.vars]`.
    EnvVars(String),
    /// `[vars]`.
    Vars,
}

impl Origin {
    /// `[env.staging.vars]` or `[vars]`, as the plan prints it.
    pub fn table(&self) -> String {
        match self {
            Self::EnvVars(name) => format!("[env.{name}.vars]"),
            Self::Vars => "[vars]".to_string(),
        }
    }
}

/// What a build resolved one `env(…)` constant to, and where the value came
/// from: what the deploy plan lists (#3941 decision H).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedConst {
    /// The constant's name.
    pub constant: String,
    /// The `bynk.toml` variable it reads.
    pub variable: String,
    /// The value, as a literal is written.
    pub value: String,
    /// `[env.<name>.vars]`, `[vars]`, or the constant's own default.
    pub source: ValueSource,
}

/// Where a resolved `env(…)` constant's value came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueSource {
    /// `[env.<name>.vars]`.
    EnvVars(String),
    /// `[vars]`.
    Vars,
    /// The default given to `env(NAME, default)`.
    Default,
}

impl ValueSource {
    /// `[env.staging.vars]`, `[vars]` or `default`, as the plan prints it.
    pub fn label(&self) -> String {
        match self {
            Self::EnvVars(name) => format!("[env.{name}.vars]"),
            Self::Vars => "[vars]".to_string(),
            Self::Default => "the constant's default".to_string(),
        }
    }
}

impl From<Origin> for ValueSource {
    fn from(o: Origin) -> Self {
        match o {
            Origin::EnvVars(name) => Self::EnvVars(name),
            Origin::Vars => Self::Vars,
        }
    }
}

/// The project's `[vars]` and `[env.<name>.vars]` tables.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectVars {
    /// `[vars]`.
    pub shared: BTreeMap<String, VarValue>,
    /// `[env.<name>.vars]`, by environment name.
    pub environments: BTreeMap<String, BTreeMap<String, VarValue>>,
}

impl ProjectVars {
    /// Every variable name `bynk.toml` defines, in any table.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.shared
            .keys()
            .chain(self.environments.values().flat_map(BTreeMap::keys))
            .map(String::as_str)
    }

    /// The value of `name` in `environment`, with where it came from: the
    /// environment's own table, else `[vars]`.
    pub fn resolve(&self, environment: &str, name: &str) -> Option<(&VarValue, Origin)> {
        if let Some(v) = self
            .environments
            .get(environment)
            .and_then(|vars| vars.get(name))
        {
            return Some((v, Origin::EnvVars(environment.to_string())));
        }
        self.shared.get(name).map(|v| (v, Origin::Vars))
    }
}

fn bad(reason: String) -> ProjectPathsError {
    ProjectPathsError::BadVars { reason }
}

fn vars_table(
    place: &str,
    table: &toml::Table,
) -> Result<BTreeMap<String, VarValue>, ProjectPathsError> {
    table
        .iter()
        .map(|(name, value)| {
            if !is_var_name(name) {
                return Err(bad(format!(
                    "`{place}` names `{name}`, which is not a variable name: capital letters, digits and `_`, starting with a capital letter (`APP_ORIGIN`)"
                )));
            }
            Ok((name.clone(), VarValue::from_toml(value)))
        })
        .collect()
}

/// The `[vars]` and `[env.<name>.vars]` tables of `doc`.
pub fn vars_from(doc: &toml::Table) -> Result<ProjectVars, ProjectPathsError> {
    let mut out = ProjectVars::default();
    match doc.get("vars") {
        None => {}
        Some(toml::Value::Table(t)) => out.shared = vars_table("[vars]", t)?,
        Some(_) => return Err(bad("`[vars]` must be a table of variables".to_string())),
    }
    match doc.get("env") {
        None => {}
        Some(toml::Value::Table(envs)) => {
            for (env, entry) in envs {
                let toml::Value::Table(entry) = entry else {
                    return Err(bad(format!("`[env.{env}]` must be a table")));
                };
                if let Some(key) = entry.keys().find(|k| k.as_str() != "vars") {
                    return Err(bad(format!(
                        "`[env.{env}]` has no key named `{key}` (its only key is `vars`)"
                    )));
                }
                if let Some(vars) = entry.get("vars") {
                    let toml::Value::Table(vars) = vars else {
                        return Err(bad(format!(
                            "`[env.{env}.vars]` must be a table of variables"
                        )));
                    };
                    out.environments
                        .insert(env.clone(), vars_table(&format!("[env.{env}.vars]"), vars)?);
                }
            }
        }
        Some(_) => return Err(bad("`[env]` must be a table of environments".to_string())),
    }
    Ok(out)
}

/// Read `bynk.toml`'s variables, honouring `overlay` for the manifest the way
/// [`crate::paths::try_read_workers_config_with`] does. No manifest is none.
pub fn try_read_vars_with(
    project_root: &Path,
    overlay: &HashMap<PathBuf, String>,
) -> Result<ProjectVars, ProjectPathsError> {
    // Like the worker groups, an overlay without the manifest falls back to the
    // disk: the CLI's source map holds `.bynk` files only.
    let manifest = project_root.join("bynk.toml");
    let content = match read_source(&manifest, overlay) {
        Ok(text) => text,
        Err(_) => match std::fs::read_to_string(&manifest) {
            Ok(text) => text,
            Err(_) => return Ok(ProjectVars::default()),
        },
    };
    let doc = content
        .parse::<toml::Table>()
        .map_err(|_| ProjectPathsError::Malformed)?;
    vars_from(&doc)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(src: &str) -> Result<ProjectVars, ProjectPathsError> {
        vars_from(&src.parse::<toml::Table>().unwrap())
    }

    #[test]
    fn an_environment_overrides_the_shared_table_and_says_where_a_value_came_from() {
        let v = read(
            "[vars]\nAPP_ORIGIN = \"https://app\"\nTAGS = [\"a\"]\n\n[env.staging.vars]\nAPP_ORIGIN = \"https://staging\"\nLIMIT = 3\n",
        )
        .unwrap();
        let (value, origin) = v.resolve("staging", "APP_ORIGIN").unwrap();
        assert_eq!(value, &VarValue::Str("https://staging".into()));
        assert_eq!(origin.table(), "[env.staging.vars]");
        let (value, origin) = v.resolve("default", "APP_ORIGIN").unwrap();
        assert_eq!(value, &VarValue::Str("https://app".into()));
        assert_eq!(origin.table(), "[vars]");
        assert_eq!(v.resolve("staging", "TAGS").unwrap().1, Origin::Vars);
        assert!(v.resolve("default", "LIMIT").is_none());
        let mut names: Vec<&str> = v.names().collect();
        names.sort();
        assert_eq!(names, ["APP_ORIGIN", "APP_ORIGIN", "LIMIT", "TAGS"]);
    }

    #[test]
    fn the_shapes_are_checked() {
        assert!(read("[vars]\nlower = 1\n").is_err());
        assert!(read("vars = 3\n").is_err());
        assert!(read("[env.staging]\nnope = 1\n").is_err());
        assert!(read("[env.staging.vars]\nA-B = 1\n").is_err());
        assert_eq!(
            read("[vars]\nF = 1.5\n").unwrap().shared["F"].kind(),
            "a float"
        );
    }
}

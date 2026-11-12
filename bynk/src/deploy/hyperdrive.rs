//! #3319: Hyperdrive configs a deploy makes, updates or adopts.
//!
//! A `Postgres` instance (`capability Main = Postgres`) binds a Hyperdrive
//! config by id. `bynk.toml` says where that id comes from, in one of two ways
//! per environment ([`bynk_project::hyperdrive`]):
//!
//! - **`connection_env`**: the name of an environment variable holding the
//!   database's connection string. The deploy reads it, makes the config
//!   through Cloudflare's API, and updates the config when the string changes.
//! - **`config_id`**: the id of a config that already exists. It is adopted as
//!   it is, never updated or deleted, and no connection string is needed.
//!
//! **The connection string is a secret** (R-16). It is read from the
//! environment, parsed, and sent to Cloudflare over TLS, and that is all. It
//! never reaches the ledger (which holds an id and a name, and nothing derived
//! from the string), the plan or its JSON, a generated file, a log or an error
//! message: [`Secret`] and [`Origin`] print as `<redacted>`, every message here
//! is built from names and never from the value, and what Cloudflare answers
//! is [`scrub`]bed before it is shown.

use super::*;
use crate::migrate::live::{self, Credentials, Rest};
use bynk_project::hyperdrive::HyperdriveDeployConfig;

/// A value that must never be printed: it has no `Display`, and its `Debug`
/// hides it, so a stray `{:?}` of a struct holding one cannot leak it.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Secret(String);

impl Secret {
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<redacted>")
    }
}

/// A database connection as Cloudflare's API takes it: the parts of a
/// connection string, not the string.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Origin {
    pub(crate) scheme: String,
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) database: String,
    pub(crate) user: String,
    pub(crate) password: Secret,
    /// The TLS mode of the connection from Hyperdrive to the database, as
    /// Cloudflare's API spells it (`REQUIRED`), for a `MySql` config (#3322).
    /// `None` leaves it to Cloudflare, as a `Postgres` config always does.
    pub(crate) sslmode: Option<&'static str>,
}

impl std::fmt::Debug for Origin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The host, user and database are part of what a connection string
        // reveals; none of it is for a log.
        f.write_str("Origin(<redacted>)")
    }
}

/// Why a connection string could not be read. Each names a part, never a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParseProblem {
    Scheme,
    Credentials,
    Password,
    Host,
    Port,
    Database,
}

impl ParseProblem {
    fn describe(self, kind: HyperdriveKind) -> &'static str {
        match self {
            Self::Scheme if kind == HyperdriveKind::MySql => "it must start with `mysql://`",
            Self::Scheme => "it must start with `postgres://` or `postgresql://`",
            Self::Credentials => "it has no user name before the `@`",
            Self::Password => "it has no password after the user name",
            Self::Host => "it names no host after the `@`",
            Self::Port => "its port is not a number from 1 to 65535",
            Self::Database => "it names no database after the host (`/<database>`)",
        }
    }
}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = text.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Read `postgres://user:password@host:port/database`. A query string
/// (`?sslmode=…`) is ignored: Hyperdrive makes its own TLS choices. The port
/// defaults to 5432; the user, password and database are percent-decoded.
#[cfg(test)]
pub(crate) fn parse_connection_string(raw: &str) -> Result<Origin, ParseProblem> {
    parse_connection_string_for(HyperdriveKind::Postgres, raw)
}

/// [`parse_connection_string`] for a kind of database (#3322): a `MySql`
/// instance's is `mysql://user:password@host:3306/database`, the port defaulting
/// to 3306. Nothing else differs, so there is one parser.
pub(crate) fn parse_connection_string_for(
    kind: HyperdriveKind,
    raw: &str,
) -> Result<Origin, ParseProblem> {
    let raw = raw.trim();
    let (scheme, rest) = raw.split_once("://").ok_or(ParseProblem::Scheme)?;
    let scheme_ok = match kind {
        HyperdriveKind::Postgres => scheme == "postgres" || scheme == "postgresql",
        HyperdriveKind::MySql => scheme == "mysql",
    };
    if !scheme_ok {
        return Err(ParseProblem::Scheme);
    }
    // The last `@` separates the credentials, so a password may hold a raw `@`,
    // `#` or `?`; the query or fragment is then cut from the location alone.
    // (Percent-encoding the password is still the safe way to write one.)
    let (credentials, location) = rest.rsplit_once('@').ok_or(ParseProblem::Credentials)?;
    let location = location.split(['?', '#']).next().unwrap_or_default();
    let (user, password) = credentials.split_once(':').ok_or(ParseProblem::Password)?;
    if user.is_empty() {
        return Err(ParseProblem::Credentials);
    }
    if password.is_empty() {
        return Err(ParseProblem::Password);
    }
    let (authority, database) = location.split_once('/').ok_or(ParseProblem::Database)?;
    let (host, port) = match authority.strip_prefix('[') {
        // An IPv6 literal.
        Some(v6) => {
            let (host, tail) = v6.split_once(']').ok_or(ParseProblem::Host)?;
            (host, tail.strip_prefix(':'))
        }
        None => match authority.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        },
    };
    if host.is_empty() {
        return Err(ParseProblem::Host);
    }
    let port = match port {
        None | Some("") => match kind {
            HyperdriveKind::Postgres => 5432,
            HyperdriveKind::MySql => 3306,
        },
        Some(p) => p
            .parse::<u16>()
            .ok()
            .filter(|p| *p != 0)
            .ok_or(ParseProblem::Port)?,
    };
    let database = percent_decode(database).ok_or(ParseProblem::Database)?;
    if database.is_empty() {
        return Err(ParseProblem::Database);
    }
    Ok(Origin {
        scheme: match kind {
            HyperdriveKind::Postgres => "postgres",
            HyperdriveKind::MySql => "mysql",
        }
        .to_string(),
        sslmode: None,
        host: host.to_string(),
        port,
        database,
        user: percent_decode(user).ok_or(ParseProblem::Credentials)?,
        password: Secret(percent_decode(password).ok_or(ParseProblem::Password)?),
    })
}

/// `message` with each of `secrets` removed. Callers pass the whole raw string
/// and the decoded password; a secret of fewer than three characters is left,
/// since removing it would mangle the message and it cannot identify anything.
pub(crate) fn scrub(message: &str, secrets: &[&str]) -> String {
    let mut out = message.to_string();
    for secret in secrets.iter().filter(|s| s.len() >= 3) {
        out = out.replace(secret, "<redacted>");
    }
    out
}

/// What a deploy wants of one `Postgres` binding's config.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Want {
    /// Use the config `id` as it is.
    Adopt { id: String },
    /// Make (or update) a config from the connection string.
    Connection {
        /// The environment variable it was read from: a name, safe to print.
        variable: String,
        origin: Origin,
        /// The whole string, kept only to scrub it from what an API answers.
        raw: Secret,
    },
}

impl std::fmt::Debug for Want {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Want::Adopt { id } => write!(f, "Adopt({id})"),
            Want::Connection { variable, .. } => write!(f, "Connection({variable}, <redacted>)"),
        }
    }
}

/// Each `(worker, binding)`'s [`Want`], read before the plan is shown.
pub(crate) type Wants = BTreeMap<(String, String), Want>;

/// The account-facing name of the config of `worker`'s `decl`:
/// `<worker>-<capability, kebab>`, environment-qualified.
pub(crate) fn config_name(environment: &str, worker: &str, decl: &HyperdriveDecl) -> String {
    env_qualify(environment, &format!("{worker}-{}", decl.kebab()))
}

/// Read what every `Postgres` binding of `order` wants, in `environment`, from
/// `settings` and `lookup` (the process environment, or a test's). Every
/// problem is collected, not just the first: a missing variable is one
/// `export` away, and three are better found together.
pub(crate) fn resolve_wants(
    order: &[String],
    resources: &BTreeMap<String, Resources>,
    settings: &HyperdriveDeployConfig,
    environment: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<Wants, Vec<String>> {
    let mut wants = Wants::new();
    let mut problems = Vec::new();
    for worker in order {
        for decl in &resources[worker].hyperdrive {
            let entry = settings
                .iter()
                .find(|(capability, _)| decl.matches_capability(capability))
                .map(|(_, entry)| entry);
            let what = format!(
                "the `{}` capability behind `{}` of `{worker}`",
                decl.kind.capability(),
                decl.binding
            );
            let Some(entry) = entry else {
                problems.push(format!(
                    "{what} has no Hyperdrive settings: add `[deploy.hyperdrive.<Capability>]` to bynk.toml with a `connection_env` naming the environment variable that holds its connection string, or a `config_id` to adopt a config that exists"
                ));
                continue;
            };
            // An environment's own `config_id` adopts: no string needed.
            if let Some(id) = entry.config_id.get(environment) {
                wants.insert(
                    (worker.clone(), decl.binding.clone()),
                    Want::Adopt { id: id.clone() },
                );
                continue;
            }
            let Some(variable) = &entry.connection_env else {
                problems.push(format!(
                    "{what} has no `connection_env` and no `config_id` for environment `{environment}` in bynk.toml's `[deploy.hyperdrive]`"
                ));
                continue;
            };
            let Some(raw) = lookup(variable).filter(|v| !v.trim().is_empty()) else {
                problems.push(format!(
                    "the environment variable `{variable}` is not set, and {what} reads its connection string from it (set it, or give environment `{environment}` a `config_id` in `[deploy.hyperdrive]` to adopt a config that exists)"
                ));
                continue;
            };
            // Hyperdrive H5 (#3322, DECISION B): the `verify` modes check the
            // server's certificate against a CA certificate the account holds,
            // which this deploy cannot upload, so such a config is made by hand
            // and adopted. Said before the string is read: it needs none.
            if let Some(mode) = decl.tls.as_deref().filter(|m| *m != "required") {
                problems.push(format!(
                    "{what} has `@tls(\"{mode}\")`, which needs a CA certificate the deploy does not upload: upload it (`wrangler cert upload certificate-authority`), make the config with `wrangler hyperdrive create … --ca-certificate-id <id> --sslmode {}`, and adopt it with `config_id` for environment `{environment}` in `[deploy.hyperdrive]`",
                    if mode == "verify-ca" { "VERIFY_CA" } else { "VERIFY_IDENTITY" },
                ));
                continue;
            }
            match parse_connection_string_for(decl.kind, &raw) {
                Ok(mut origin) => {
                    if decl.kind == HyperdriveKind::MySql {
                        origin.sslmode = Some("REQUIRED");
                    }
                    wants.insert(
                        (worker.clone(), decl.binding.clone()),
                        Want::Connection {
                            variable: variable.clone(),
                            origin,
                            raw: Secret(raw),
                        },
                    );
                }
                Err(problem) => problems.push(format!(
                    "the connection string in `{variable}`, for {what}, is not a {} connection URL (`{}://user:password@host:{}/database`): {}",
                    decl.kind.capability().replace("MySql", "MySQL"),
                    if decl.kind == HyperdriveKind::MySql { "mysql" } else { "postgres" },
                    if decl.kind == HyperdriveKind::MySql { "3306" } else { "5432" },
                    problem.describe(decl.kind)
                )),
            }
        }
    }
    if problems.is_empty() {
        Ok(wants)
    } else {
        Err(problems)
    }
}

/// What a deploy will do to one binding's config, from the ledger and what is
/// wanted. Pure, so the plan shows it before anything is asked of the account.
///
/// - `create`: no config of this name is recorded.
/// - `sync`: one is, and it is updated from the variable again (every deploy).
/// - `reuse`: one is, and there is no string to sync (a recorded config with
///   nothing wanted).
/// - `adopt`: the project names a config that already exists.
pub(crate) fn action(
    recorded: Option<&HyperdriveRecord>,
    physical: &str,
    want: Option<&Want>,
) -> &'static str {
    match want {
        Some(Want::Adopt { id }) => {
            if recorded.is_some_and(|r| r.adopted && &r.id == id) {
                "reuse"
            } else {
                "adopt"
            }
        }
        // The config is synced from the variable on every deploy: nothing
        // derived from the string is kept to tell whether it changed, since
        // the ledger is committed and a digest of a secret there could be
        // guessed offline. A repeat sync is idempotent.
        Some(Want::Connection { .. }) => match recorded {
            Some(r) if !r.adopted && r.name == physical => "sync",
            _ => "create",
        },
        None => {
            if recorded.is_some() {
                "reuse"
            } else {
                "create"
            }
        }
    }
}

/// The first binding of `order` whose config this run would make or update
/// and whose id is not in the ledger: CI must not make one it cannot commit
/// (the id Cloudflare mints would be lost, and the next run would make a
/// second config).
pub(crate) fn first_unrecorded<'a>(
    order: &'a [String],
    resources: &'a BTreeMap<String, Resources>,
    lock: &DeployLock,
    environment: &str,
    wants: &Wants,
) -> Option<(&'a str, &'a str)> {
    order.iter().find_map(|worker| {
        resources[worker].hyperdrive.iter().find_map(|decl| {
            let want = wants.get(&(worker.clone(), decl.binding.clone()));
            let physical = config_name(environment, worker, decl);
            let recorded = recorded_hyperdrive(lock, worker, &decl.binding, environment);
            (action(recorded, &physical, want) == "create"
                && matches!(want, Some(Want::Connection { .. })))
            .then_some((worker.as_str(), decl.binding.as_str()))
        })
    })
}

/// Cloudflare's Hyperdrive configs, as a deploy needs them. A trait so the
/// run is tested against a fake and the REST client against a stub.
pub(crate) trait HyperdriveApi {
    /// Make a config, returning its id.
    fn create(&self, name: &str, origin: &Origin) -> Result<String, String>;
    /// Replace config `id`'s name and origin.
    fn update(&self, id: &str, name: &str, origin: &Origin) -> Result<(), String>;
    /// The name of config `id`, or `None` when the account has none.
    fn name_of(&self, id: &str) -> Result<Option<String>, String>;
    /// Delete config `id`; already gone is success.
    fn delete(&self, id: &str) -> Result<(), String>;
}

/// The permission a token needs to make, update and read configs.
pub(crate) const TOKEN_SCOPE: &str = "Hyperdrive: Edit";

/// Cloudflare's REST API for Hyperdrive (`/accounts/{id}/hyperdrive/configs`).
pub(crate) struct RestHyperdrive {
    pub(crate) rest: Rest,
}

impl RestHyperdrive {
    /// A client for the account in `CLOUDFLARE_ACCOUNT_ID`, or a message naming
    /// the missing variable and the permission the token needs.
    pub(crate) fn from_env() -> Result<Self, String> {
        let credentials = Credentials::from_env_for(
            "create or update a Hyperdrive config",
            &format!(
                "an API token with \"{TOKEN_SCOPE}\" (a `wrangler login` session is not enough: the deploy calls Cloudflare's API directly)"
            ),
        )?;
        Ok(Self {
            rest: Rest {
                agent: live::agent()?,
                credentials,
                base: live::API.to_string(),
            },
        })
    }
}

fn origin_json(name: &str, origin: &Origin) -> serde_json::Value {
    let mut body = serde_json::json!({
        "name": name,
        "origin": {
            "scheme": origin.scheme,
            "host": origin.host,
            "port": origin.port,
            "database": origin.database,
            "user": origin.user,
            "password": origin.password.expose(),
        },
    });
    // #3322: a `MySql` config's TLS mode (`@tls("required")`, the default).
    if let Some(sslmode) = origin.sslmode {
        body["mtls"] = serde_json::json!({ "sslmode": sslmode });
    }
    body
}

impl HyperdriveApi for RestHyperdrive {
    fn create(&self, name: &str, origin: &Origin) -> Result<String, String> {
        let body = self
            .rest
            .post("/hyperdrive/configs", &origin_json(name, origin))?;
        body["result"]["id"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| "Cloudflare made the config but returned no id".to_string())
    }

    fn update(&self, id: &str, name: &str, origin: &Origin) -> Result<(), String> {
        self.rest
            .put(
                &format!("/hyperdrive/configs/{id}"),
                &origin_json(name, origin),
            )
            .map(drop)
    }

    fn name_of(&self, id: &str) -> Result<Option<String>, String> {
        match self.rest.get(&format!("/hyperdrive/configs/{id}")) {
            Ok(body) => Ok(Some(
                body["result"]["name"].as_str().unwrap_or("").to_string(),
            )),
            Err(e) if e.contains("answered 404") => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn delete(&self, id: &str) -> Result<(), String> {
        match self.rest.delete(&format!("/hyperdrive/configs/{id}")) {
            Ok(_) => Ok(()),
            Err(e) if e.contains("answered 404") => Ok(()),
            Err(e) => Err(e),
        }
    }
}

/// What a deploy run carries for Hyperdrive: what each binding wants, and the
/// API to ask (absent when nothing needs it, so a project without `Postgres`
/// needs no token).
pub(crate) struct HyperdriveRun<'a> {
    pub(crate) wants: &'a Wants,
    pub(crate) api: Option<&'a dyn HyperdriveApi>,
}

#[cfg(test)]
impl HyperdriveRun<'static> {
    /// A run with no `Postgres` bindings.
    pub(crate) fn none() -> HyperdriveRun<'static> {
        static EMPTY: Wants = Wants::new();
        HyperdriveRun {
            wants: &EMPTY,
            api: None,
        }
    }
}

/// Make, update or adopt each `Postgres` binding's config, record what was
/// done before the push (an interrupted run never makes a second config), and
/// write each id into the Worker's generated config.
impl HyperdriveRun<'_> {
    /// [`provision`], over this run.
    pub(crate) fn provision(
        &self,
        worker: &str,
        declared: &Resources,
        lock: &mut DeployLock,
        lock_path: &Path,
        config: &Path,
        environment: &str,
    ) -> Result<(), DeployFailure> {
        provision(self, worker, declared, lock, lock_path, config, environment)
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn provision(
    run: &HyperdriveRun<'_>,
    worker: &str,
    declared: &Resources,
    lock: &mut DeployLock,
    lock_path: &Path,
    config: &Path,
    environment: &str,
) -> Result<(), DeployFailure> {
    let mut ids = BTreeMap::new();
    for decl in &declared.hyperdrive {
        let binding = &decl.binding;
        let physical = config_name(environment, worker, decl);
        let want = run.wants.get(&(worker.to_string(), binding.clone()));
        let recorded = recorded_hyperdrive(lock, worker, binding, environment).cloned();
        let todo = action(recorded.as_ref(), &physical, want);
        let id = match (todo, want) {
            ("reuse", _) => match &recorded {
                Some(record) => record.id.clone(),
                None => {
                    return Err(DeployFailure::driver(format!(
                        "no Hyperdrive config is recorded for `{worker}`'s `{binding}`"
                    )));
                }
            },
            ("adopt", Some(Want::Adopt { id })) => {
                let api = need_api(run, worker, binding)?;
                let name = api
                    .name_of(id)
                    .map_err(|e| {
                        DeployFailure::driver(format!(
                            "could not look up the Hyperdrive config `{id}` for `{worker}`'s `{binding}`: {e}"
                        ))
                    })?
                    .ok_or_else(|| {
                        DeployFailure::driver(format!(
                            "the Hyperdrive config `{id}` that bynk.toml names for `{worker}`'s `{binding}` does not exist on this account"
                        ))
                    })?;
                record(
                    lock,
                    lock_path,
                    environment,
                    worker,
                    binding,
                    HyperdriveRecord {
                        name,
                        id: id.clone(),
                        adopted: true,
                        superseded: Vec::new(),
                    },
                )?;
                id.clone()
            }
            ("create", Some(Want::Connection { origin, raw, .. })) => {
                let api = need_api(run, worker, binding)?;
                let id = api.create(&physical, origin).map_err(|e| {
                    DeployFailure::driver(format!(
                        "could not create the Hyperdrive config `{physical}` for `{worker}`'s `{binding}`: {}",
                        scrub(&e, &[raw.expose(), origin.password.expose()])
                    ))
                })?;
                record(
                    lock,
                    lock_path,
                    environment,
                    worker,
                    binding,
                    HyperdriveRecord {
                        name: physical.clone(),
                        id: id.clone(),
                        adopted: false,
                        superseded: Vec::new(),
                    },
                )?;
                id
            }
            ("sync", Some(Want::Connection { origin, raw, .. })) => {
                let api = need_api(run, worker, binding)?;
                let Some(old) = recorded else {
                    return Err(DeployFailure::driver(format!(
                        "no Hyperdrive config is recorded for `{worker}`'s `{binding}`"
                    )));
                };
                api.update(&old.id, &physical, origin).map_err(|e| {
                    DeployFailure::driver(format!(
                        "could not sync the Hyperdrive config `{physical}` for `{worker}`'s `{binding}`: {}",
                        scrub(&e, &[raw.expose(), origin.password.expose()])
                    ))
                })?;
                record(lock, lock_path, environment, worker, binding, old.clone())?;
                old.id
            }
            _ => {
                return Err(DeployFailure::driver(format!(
                    "no Hyperdrive settings were read for `{worker}`'s `{binding}`"
                )));
            }
        };
        ids.insert(binding.clone(), id);
    }
    if !ids.is_empty() && !materialise_hyperdrive_ids(config, &ids) {
        return Err(DeployFailure::driver(format!(
            "could not materialise the Hyperdrive config ids into `{worker}`'s generated configuration"
        )));
    }
    Ok(())
}

fn need_api<'a>(
    run: &HyperdriveRun<'a>,
    worker: &str,
    binding: &str,
) -> Result<&'a dyn HyperdriveApi, DeployFailure> {
    run.api.ok_or_else(|| {
        DeployFailure::driver(format!(
            "no Cloudflare API client for `{worker}`'s `{binding}`'s Hyperdrive config"
        ))
    })
}

fn record(
    lock: &mut DeployLock,
    lock_path: &Path,
    environment: &str,
    worker: &str,
    binding: &str,
    record: HyperdriveRecord,
) -> Result<(), DeployFailure> {
    let name = record.name.clone();
    lock.record_hyperdrive(environment, worker, binding, record);
    write_lock(lock_path, lock).map_err(|e| {
        DeployFailure::driver(format!(
            "changed the Hyperdrive config `{name}` but could not record it in {}: {e}",
            lock_path.display()
        ))
    })
}

/// The run's Hyperdrive inputs, or `None` after printing why not (a missing
/// variable, an unreadable string, a binding with no settings). Reads the
/// process environment; `bynk.toml` is read from `project_root`. Asks nothing
/// of the account.
pub(crate) fn wants_for_run(
    project_root: &Path,
    order: &[String],
    resources: &BTreeMap<String, Resources>,
    environment: &str,
) -> Option<Wants> {
    if !order.iter().any(|w| !resources[w].hyperdrive.is_empty()) {
        return Some(Wants::new());
    }
    let settings = match bynk_project::hyperdrive::try_read_hyperdrive_deploy(project_root) {
        Ok(settings) => settings,
        Err(e) => {
            eprintln!("bynk: {e}");
            return None;
        }
    };
    let lookup = |name: &str| std::env::var(name).ok();
    match resolve_wants(order, resources, &settings, environment, &lookup) {
        Ok(wants) => Some(wants),
        Err(problems) => {
            for problem in problems {
                eprintln!("bynk: {problem}");
            }
            None
        }
    }
}

/// Printing why, `true` when this run would make a config it cannot commit
/// ([`first_unrecorded`]). Called only in CI.
pub(crate) fn refused_in_ci(
    order: &[String],
    resources: &BTreeMap<String, Resources>,
    lock: &DeployLock,
    environment: &str,
    wants: &Wants,
) -> bool {
    let Some((worker, binding)) = first_unrecorded(order, resources, lock, environment, wants)
    else {
        return false;
    };
    eprintln!(
        "bynk: the Hyperdrive config of `{binding}` of `{worker}` is unrecorded; deploy locally first and commit {LOCK_FILE}, or name an existing config with `config_id` in `[deploy.hyperdrive]`"
    );
    true
}

/// The Hyperdrive API client of a run, `None` when nothing needs it (so a
/// project without `Postgres` needs no token), or `Err` after printing why it
/// cannot be had. Needed to make, update or adopt a config, and to delete an
/// orphan under `--prune`.
pub(crate) fn api_for_run(
    order: &[String],
    resources: &BTreeMap<String, Resources>,
    lock: &DeployLock,
    environment: &str,
    wants: &Wants,
    orphans: &Orphans,
    prune: bool,
) -> Result<Option<RestHyperdrive>, ()> {
    let deletes = prune && orphans.hyperdrive.iter().any(|o| !o.adopted);
    if !deletes && !needs_api(order, resources, lock, environment, wants) {
        return Ok(None);
    }
    RestHyperdrive::from_env().map(Some).map_err(|e| {
        eprintln!("bynk: {e}");
    })
}

/// Delete the orphaned configs `--prune` named, if the run holds an API.
pub(crate) fn prune_run(
    run: &HyperdriveRun<'_>,
    lock: &mut DeployLock,
    lock_path: &Path,
    environment: &str,
    orphans: &Orphans,
) -> Result<(), DeployFailure> {
    match run.api {
        Some(api) => prune(api, lock, lock_path, environment, &orphans.hyperdrive),
        None => Ok(()),
    }
}

impl<'a> HyperdriveRun<'a> {
    /// A run over `wants`, asking `api`.
    pub(crate) fn new(wants: &'a Wants, api: Option<&'a RestHyperdrive>) -> Self {
        Self {
            wants,
            api: api.map(|api| api as &dyn HyperdriveApi),
        }
    }
}

/// Whether this run needs the Cloudflare API for Hyperdrive: a config to make,
/// update or adopt, or an orphan to delete.
pub(crate) fn needs_api(
    order: &[String],
    resources: &BTreeMap<String, Resources>,
    lock: &DeployLock,
    environment: &str,
    wants: &Wants,
) -> bool {
    order.iter().any(|worker| {
        resources[worker].hyperdrive.iter().any(|decl| {
            let recorded = recorded_hyperdrive(lock, worker, &decl.binding, environment);
            let physical = config_name(environment, worker, decl);
            let want = wants.get(&(worker.clone(), decl.binding.clone()));
            action(recorded, &physical, want) != "reuse"
        })
    })
}

/// Delete each orphaned config `--prune` just named (never an adopted one: it
/// is not the project's), and strip it from the ledger. Already gone is
/// success, as for a queue (DECISION E): the ledger entry goes either way.
pub(crate) fn prune(
    api: &dyn HyperdriveApi,
    lock: &mut DeployLock,
    lock_path: &Path,
    environment: &str,
    orphans: &[OrphanHyperdrive],
) -> Result<(), DeployFailure> {
    for orphan in orphans.iter().filter(|o| !o.adopted) {
        api.delete(&orphan.id).map_err(|e| {
            DeployFailure::driver(format!(
                "could not delete the orphaned Hyperdrive config `{}` of `{}`'s `{}`: {e}",
                orphan.config, orphan.worker, orphan.binding
            ))
        })?;
        lock.forget_hyperdrive(environment, &orphan.worker, &orphan.binding, &orphan.id);
        write_lock(lock_path, lock).map_err(|e| {
            DeployFailure::driver(format!(
                "deleted the orphaned Hyperdrive config `{}` but could not record it in {}: {e}",
                orphan.config,
                lock_path.display()
            ))
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;

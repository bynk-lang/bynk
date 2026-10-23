//! `bynk login`, `bynk logout` and `bynk org` (first-party registry R3.1,
//! #2378): the client side of the account API the hosted registry serves
//! beside its publish API (R3, #2363).
//!
//! `login` runs GitHub's device flow through the registry and keeps the
//! account token (`bynka_…`) in the credentials file's `[accounts.<name>]`,
//! apart from the publish token `bynk publish` reads from
//! `[registries.<name>] token`. `org token mint --save` writes a new publish
//! token into that slot.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bynk_project::lock::FIRST_PARTY_REGISTRY;
use serde_json::{Value, json};

use super::Client;
use crate::fetch::{FetchError, fail};

pub(crate) const ACCOUNT_REFUSED: &str = "bynk.registry.account_refused";

/// RFC 8628 §3.5: a `slow_down` adds five seconds to the interval.
const SLOW_DOWN: u64 = 5;

/// Which registry a command talks to: `--registry`, else the one the
/// organisation routes to in the project at `project` (`login` and `logout`
/// have no organisation, and default to the first-party registry).
#[derive(Debug, Clone)]
pub struct Target {
    pub project: PathBuf,
    pub registry: Option<String>,
}

/// The account API, behind a seam the unit tests stub.
pub(crate) trait Api {
    /// `method` on `path` (relative to the account base), with a JSON body.
    fn call(
        &mut self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<(u16, Vec<u8>), FetchError>;
}

/// The live API: a registry opened through its verified `config.json`, and
/// the account token, when there is one.
struct Live {
    client: Client,
    registry: String,
    bearer: Option<String>,
}

impl Api for Live {
    fn call(
        &mut self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<(u16, Vec<u8>), FetchError> {
        self.client
            .account_call(&self.registry, method, path, body, self.bearer.as_deref())
    }
}

/// A client over the project's `[registries]`, and the registry the command
/// talks to.
fn open(target: &Target, org: Option<&str>) -> (Client, String) {
    let root = bynk_project::lock::lock_root(&target.project);
    let client = Client::new(super::commands::registries_of(&root), None);
    let registry = match (&target.registry, org) {
        (Some(r), _) => r.clone(),
        (None, Some(o)) => client.route(o),
        (None, None) => FIRST_PARTY_REGISTRY.to_string(),
    };
    (client, registry)
}

/// The live API for `org`'s registry, signed in.
fn signed_in(target: &Target, org: &str) -> Result<Live, FetchError> {
    let (client, registry) = open(target, Some(org));
    let session = session(&registry).ok_or_else(|| {
        fail(
            ACCOUNT_REFUSED,
            format!(
                "you aren't signed in to registry `{registry}`: run `bynk login --registry {registry}`"
            ),
        )
    })?;
    if session.expires_at.is_some_and(|t| t <= now_millis()) {
        return Err(fail(
            ACCOUNT_REFUSED,
            format!(
                "your session on registry `{registry}` expired: run `bynk login --registry {registry}`"
            ),
        ));
    }
    Ok(Live {
        client,
        registry,
        bearer: Some(session.token),
    })
}

// ---------------------------------------------------------------------------
// Signing in and out
// ---------------------------------------------------------------------------

/// What `POST /account/login` answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeviceStart {
    pub poll: String,
    pub user_code: String,
    pub verification_uri: String,
    pub interval: u64,
    pub expires_in: u64,
}

/// What a successful poll answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SignedIn {
    pub token: String,
    pub login: String,
    pub expires_at: i64,
}

/// Sign in to `target`'s registry through GitHub's device flow, and keep the
/// account token.
pub fn login(target: &Target) -> Result<(), FetchError> {
    let (client, registry) = open(target, None);
    let mut api = Live {
        client,
        registry: registry.clone(),
        bearer: None,
    };
    let start = start(&mut api, &registry)?;
    eprintln!(
        "To sign in to registry `{registry}`, open {} and enter the code {}.\nNever enter a code you didn't request. Waiting…",
        start.verification_uri, start.user_code
    );
    let began = Instant::now();
    let signed = poll(&mut api, &registry, &start, &mut |d| {
        std::thread::sleep(d);
        began.elapsed()
    })?;
    let path = credentials_file()?;
    let mut doc = read_doc(&path).map_err(|e| fail(ACCOUNT_REFUSED, e))?;
    set_session(&mut doc, &registry, &signed);
    write_doc(&path, &doc).map_err(|e| fail(ACCOUNT_REFUSED, e))?;
    eprintln!(
        "Signed in to `{registry}` as {} (until {}). Saved to {}.",
        signed.login,
        date(signed.expires_at),
        path.display()
    );
    if std::env::var_os(account_token_var(&registry)).is_some() {
        eprintln!(
            "bynk: warning: `{}` is set, and is used instead of the saved session",
            account_token_var(&registry)
        );
    }
    Ok(())
}

/// Sign out of `target`'s registry: end the session on the server, and forget
/// it here.
pub fn logout(target: &Target) -> Result<(), FetchError> {
    let (client, registry) = open(target, None);
    let Some(session) = session(&registry) else {
        eprintln!("bynk: you aren't signed in to registry `{registry}`");
        return Ok(());
    };
    let mut api = Live {
        client,
        registry: registry.clone(),
        bearer: Some(session.token),
    };
    let (status, body) = api.call("DELETE", "session", None)?;
    // A `401` means the session is already over: forget it all the same.
    if !(200..300).contains(&status) && status != 401 {
        return Err(refused(&registry, status, &body));
    }
    if let Some(path) = super::credentials_path() {
        let mut doc = read_doc(&path).map_err(|e| fail(ACCOUNT_REFUSED, e))?;
        if remove_session(&mut doc, &registry) {
            write_doc(&path, &doc).map_err(|e| fail(ACCOUNT_REFUSED, e))?;
        }
    }
    eprintln!("bynk: signed out of `{registry}`");
    if session.from_env {
        eprintln!(
            "bynk: note: `{}` still holds the token; unset it",
            account_token_var(&registry)
        );
    }
    Ok(())
}

/// `POST /account/login`.
pub(crate) fn start(api: &mut dyn Api, registry: &str) -> Result<DeviceStart, FetchError> {
    let (status, body) = api.call("POST", "login", None)?;
    if status != 200 {
        return Err(refused(registry, status, &body));
    }
    let v = parse(registry, &body)?;
    let text = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
    let num = |k: &str| v.get(k).and_then(Value::as_u64);
    match (
        text("poll"),
        text("userCode"),
        text("verificationUri"),
        num("interval"),
        num("expiresIn"),
    ) {
        (Some(poll), Some(user_code), Some(verification_uri), Some(interval), Some(expires_in)) => {
            Ok(DeviceStart {
                poll,
                user_code,
                verification_uri,
                interval,
                expires_in,
            })
        }
        _ => Err(malformed(registry)),
    }
}

/// Poll until the person approves, declines, or the code expires (decision
/// C). `wait` sleeps for the interval and answers the time since the flow
/// started; the client stops at `expiresIn` whatever the server says.
pub(crate) fn poll(
    api: &mut dyn Api,
    registry: &str,
    start: &DeviceStart,
    wait: &mut dyn FnMut(Duration) -> Duration,
) -> Result<SignedIn, FetchError> {
    let mut interval = start.interval;
    let deadline = Duration::from_secs(start.expires_in);
    let expired = || {
        fail(
            ACCOUNT_REFUSED,
            "the code expired before it was approved: run `bynk login` again",
        )
    };
    loop {
        if wait(Duration::from_secs(interval)) >= deadline {
            return Err(expired());
        }
        let (status, body) =
            api.call("POST", "login/poll", Some(&json!({ "poll": start.poll })))?;
        if status == 200 {
            let v = parse(registry, &body)?;
            return match (
                v.get("token").and_then(Value::as_str),
                v.get("login").and_then(Value::as_str),
                v.get("expiresAt").and_then(Value::as_i64),
            ) {
                (Some(token), Some(login), Some(expires_at)) => Ok(SignedIn {
                    token: token.to_owned(),
                    login: login.to_owned(),
                    expires_at,
                }),
                _ => Err(malformed(registry)),
            };
        }
        // The `error` code decides, not the status: `400` is also a bad
        // request, and `403` every permission refusal.
        match error_code(&body).as_deref() {
            Some("authorization_pending") => {}
            Some("slow_down") => interval += SLOW_DOWN,
            Some("expired_token") => return Err(expired()),
            Some("access_denied") => {
                return Err(fail(ACCOUNT_REFUSED, "the sign-in was declined at GitHub"));
            }
            _ => return Err(refused(registry, status, &body)),
        }
    }
}

// ---------------------------------------------------------------------------
// Organisations, members and tokens
// ---------------------------------------------------------------------------

/// Claim `org`, first come (`block` holds it without publishing; an operator
/// names the `owner`).
pub fn claim(
    target: &Target,
    org: &str,
    block: bool,
    owner: Option<&str>,
) -> Result<(), FetchError> {
    let mut api = signed_in(target, org)?;
    let mut body = json!({ "block": block });
    if let Some(o) = owner {
        body["owner"] = json!(o);
    }
    let (status, answer) = api.call("POST", &orgs(&[org])?, Some(&body))?;
    let registry = &api.registry;
    match status {
        201 => eprintln!(
            "bynk: claimed `{org}` on `{registry}`{}",
            if block { " (blocked)" } else { "" }
        ),
        200 => eprintln!("bynk: `{org}` on `{registry}` is already yours"),
        _ => return Err(refused(registry, status, &answer)),
    }
    Ok(())
}

/// Print `org`: its owner, members, whether it is blocked, and its tokens
/// (never the tokens themselves). `json` prints the API's answer as it is.
pub fn show(target: &Target, org: &str, json: bool) -> Result<(), FetchError> {
    let mut api = signed_in(target, org)?;
    let view = view(&mut api, org)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&view).expect("a JSON value")
        );
    } else {
        print!("{}", render(&api.registry, &view));
    }
    Ok(())
}

/// Block or unblock `org`.
pub fn set_blocked(target: &Target, org: &str, blocked: bool) -> Result<(), FetchError> {
    let mut api = signed_in(target, org)?;
    acknowledged(
        &mut api,
        "PUT",
        &orgs(&[org, "blocked"])?,
        Some(&json!({ "blocked": blocked })),
    )?;
    eprintln!(
        "bynk: {} `{org}` on `{}`",
        if blocked { "blocked" } else { "unblocked" },
        api.registry
    );
    Ok(())
}

/// Add the GitHub user `login` to `org`.
pub fn member_add(target: &Target, org: &str, login: &str) -> Result<(), FetchError> {
    let mut api = signed_in(target, org)?;
    acknowledged(&mut api, "PUT", &orgs(&[org, "members", login])?, None)?;
    eprintln!("bynk: added {login} to `{org}` on `{}`", api.registry);
    Ok(())
}

/// Remove a member from `org`, by GitHub login or numeric id.
pub fn member_remove(target: &Target, org: &str, who: &str) -> Result<(), FetchError> {
    let mut api = signed_in(target, org)?;
    let id = member_id(&mut api, org, who)?;
    acknowledged(&mut api, "DELETE", &orgs(&[org, "members", &id])?, None)?;
    eprintln!("bynk: removed {who} from `{org}` on `{}`", api.registry);
    Ok(())
}

/// What `token mint` is asked for.
#[derive(Debug, Clone)]
pub struct Mint {
    pub can: Vec<String>,
    pub days: u32,
    pub note: String,
    /// Write it to `[registries.<name>] token` instead of printing it.
    pub save: bool,
    /// Let `save` replace a different token.
    pub force: bool,
}

/// Mint a publish token for `org`: printed once on stdout, or saved where
/// `bynk publish` reads it (decision E).
pub fn token_mint(target: &Target, org: &str, mint: &Mint) -> Result<(), FetchError> {
    let mut api = signed_in(target, org)?;
    let registry = api.registry.clone();
    // Check the slot before minting, so a refused `--save` mints nothing.
    let saving = if mint.save {
        let path = credentials_file()?;
        let doc = read_doc(&path).map_err(|e| fail(ACCOUNT_REFUSED, e))?;
        check_publish_slot(&doc, &registry, None, mint.force)
            .map_err(|e| fail(ACCOUNT_REFUSED, e))?;
        Some((path, doc))
    } else {
        None
    };
    let body = json!({ "can": mint.can, "days": mint.days, "note": mint.note });
    let (status, answer) = api.call("POST", &orgs(&[org, "tokens"])?, Some(&body))?;
    if status != 201 && status != 200 {
        return Err(refused(&registry, status, &answer));
    }
    let v = parse(&registry, &answer)?;
    let token = v
        .get("token")
        .and_then(Value::as_str)
        .ok_or_else(|| malformed(&registry))?;
    let info = v.get("info").cloned().unwrap_or(Value::Null);
    let id = info.get("id").and_then(Value::as_str).unwrap_or("?");
    let can = info
        .get("can")
        .and_then(Value::as_array)
        .map(|c| {
            c.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    let until = info
        .get("expiresAt")
        .and_then(Value::as_i64)
        .map(date)
        .unwrap_or_default();
    eprintln!(
        "bynk: minted token {} for `{org}` on `{registry}` ({can}; until {until})",
        short_id(id)
    );
    match saving {
        Some((path, mut doc)) => {
            check_publish_slot(&doc, &registry, Some(token), mint.force)
                .map_err(|e| fail(ACCOUNT_REFUSED, e))?;
            set_publish_token(&mut doc, &registry, token);
            write_doc(&path, &doc).map_err(|e| fail(ACCOUNT_REFUSED, e))?;
            eprintln!(
                "bynk: saved as `[registries.{registry}] token` in {}",
                path.display()
            );
            let var = bynk_project::manifest::registry_token_var(&registry);
            if std::env::var_os(&var).is_some() {
                eprintln!(
                    "bynk: warning: `{var}` is set, and `bynk publish` uses it instead of the saved token"
                );
            }
        }
        None => {
            eprintln!("bynk: it is shown once, below; keep it somewhere safe");
            println!("{token}");
        }
    }
    Ok(())
}

/// Revoke one of `org`'s publish tokens, by its id (or a unique prefix of it,
/// as `org show` prints it).
pub fn token_revoke(target: &Target, org: &str, id: &str) -> Result<(), FetchError> {
    let mut api = signed_in(target, org)?;
    let id = token_id(&mut api, org, id)?;
    acknowledged(&mut api, "DELETE", &orgs(&[org, "tokens", &id])?, None)?;
    eprintln!(
        "bynk: revoked token {} for `{org}` on `{}`",
        short_id(&id),
        api.registry
    );
    Ok(())
}

/// `GET /account/orgs/<org>`.
fn view(api: &mut Live, org: &str) -> Result<Value, FetchError> {
    let (status, body) = api.call("GET", &orgs(&[org])?, None)?;
    if status != 200 {
        return Err(refused(&api.registry, status, &body));
    }
    parse(&api.registry, &body)
}

/// A change the API answers with `{"ok": true}`.
fn acknowledged(
    api: &mut Live,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Result<(), FetchError> {
    let (status, answer) = api.call(method, path, body)?;
    if (200..300).contains(&status) {
        Ok(())
    } else {
        Err(refused(&api.registry, status, &answer))
    }
}

/// A member's GitHub id: `who` itself when it is one, else the id of the
/// member whose login it is (the API removes members by id, since logins can
/// be reassigned).
fn member_id(api: &mut Live, org: &str, who: &str) -> Result<String, FetchError> {
    if !who.is_empty() && who.bytes().all(|b| b.is_ascii_digit()) {
        return Ok(who.to_owned());
    }
    let v = view(api, org)?;
    find_member(&v, who)
        .ok_or_else(|| fail(ACCOUNT_REFUSED, format!("`{org}` has no member `{who}`")))
}

/// The id of the member of `view` whose login is `login` (GitHub logins are
/// case-insensitive).
pub(crate) fn find_member(view: &Value, login: &str) -> Option<String> {
    view.get("members")?
        .as_array()?
        .iter()
        .find(|m| {
            m.get("login")
                .and_then(Value::as_str)
                .is_some_and(|l| l.eq_ignore_ascii_case(login))
        })
        .and_then(|m| m.get("id")?.as_i64())
        .map(|id| id.to_string())
}

/// A token's full id from `prefix`, which must name exactly one.
fn token_id(api: &mut Live, org: &str, prefix: &str) -> Result<String, FetchError> {
    let v = view(api, org)?;
    find_token(&v, prefix).map_err(|why| fail(ACCOUNT_REFUSED, format!("`{org}` {why}")))
}

/// The one token id in `view` that starts with `prefix`.
pub(crate) fn find_token(view: &Value, prefix: &str) -> Result<String, String> {
    let ids: Vec<&str> = view
        .get("tokens")
        .and_then(Value::as_array)
        .map(|ts| {
            ts.iter()
                .filter_map(|t| t.get("id")?.as_str())
                .filter(|id| !prefix.is_empty() && id.starts_with(prefix))
                .collect()
        })
        .unwrap_or_default();
    match ids.as_slice() {
        [one] => Ok((*one).to_owned()),
        [] => Err(format!("has no token `{prefix}` (see `bynk org show`)")),
        _ => Err(format!(
            "has {} tokens starting `{prefix}`: give more of the id",
            ids.len()
        )),
    }
}

/// `orgs/<segment>/…`, refusing a segment that isn't a plain name, so nothing
/// typed can reach another path.
fn orgs(segments: &[&str]) -> Result<String, FetchError> {
    let mut path = String::from("orgs");
    for s in segments {
        let plain = !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        if !plain {
            return Err(fail(
                ACCOUNT_REFUSED,
                format!("`{s}` isn't a name the account API takes"),
            ));
        }
        path.push('/');
        path.push_str(s);
    }
    Ok(path)
}

/// `org show`'s text.
pub(crate) fn render(registry: &str, view: &Value) -> String {
    let user = |u: &Value| {
        format!(
            "{} (#{})",
            u.get("login").and_then(Value::as_str).unwrap_or("?"),
            u.get("id").and_then(Value::as_i64).unwrap_or_default()
        )
    };
    let members: Vec<&Value> = view
        .get("members")
        .and_then(Value::as_array)
        .map(|m| m.iter().collect())
        .unwrap_or_default();
    let login_of = |id: i64| {
        members
            .iter()
            .copied()
            .chain(view.get("owner"))
            .find(|m| m.get("id").and_then(Value::as_i64) == Some(id))
            .and_then(|m| m.get("login")?.as_str())
            .map_or_else(
                || {
                    if id == 0 {
                        "an operator".to_owned()
                    } else {
                        format!("#{id}")
                    }
                },
                str::to_owned,
            )
    };
    let name = view.get("name").and_then(Value::as_str).unwrap_or("?");
    let blocked = view.get("blocked").and_then(Value::as_bool) == Some(true);
    let mut out = format!("{name} on `{registry}`\n");
    out.push_str(&format!(
        "  owner:   {}\n",
        view.get("owner").map(user).unwrap_or_default()
    ));
    out.push_str(&format!(
        "  members: {}\n",
        members
            .iter()
            .map(|m| user(m))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    out.push_str(&format!(
        "  blocked: {}\n",
        if blocked { "yes" } else { "no" }
    ));
    let tokens: Vec<&Value> = view
        .get("tokens")
        .and_then(Value::as_array)
        .map(|t| t.iter().collect())
        .unwrap_or_default();
    if tokens.is_empty() {
        out.push_str("  tokens:  none\n");
    } else {
        out.push_str("  tokens:\n");
        for t in tokens {
            let id = t.get("id").and_then(Value::as_str).unwrap_or("?");
            let can = t
                .get("can")
                .and_then(Value::as_array)
                .map(|c| {
                    c.iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            let until = t
                .get("expiresAt")
                .and_then(Value::as_i64)
                .map(date)
                .unwrap_or_default();
            let by = login_of(t.get("mintedBy").and_then(Value::as_i64).unwrap_or(-1));
            let note = t.get("note").and_then(Value::as_str).unwrap_or_default();
            out.push_str(&format!(
                "    {}  {can}  until {until}  minted by {by}",
                short_id(id)
            ));
            if !note.is_empty() {
                out.push_str(&format!("  {note:?}"));
            }
            out.push('\n');
        }
    }
    out
}

/// The first 12 characters of a token id: enough for `token revoke`.
fn short_id(id: &str) -> &str {
    id.get(..12).unwrap_or(id)
}

// ---------------------------------------------------------------------------
// Answers
// ---------------------------------------------------------------------------

/// A refusal's JSON `error` code.
fn error_code(body: &[u8]) -> Option<String> {
    serde_json::from_slice::<Value>(body)
        .ok()?
        .get("error")?
        .as_str()
        .map(str::to_owned)
}

fn parse(registry: &str, body: &[u8]) -> Result<Value, FetchError> {
    serde_json::from_slice(body).map_err(|_| malformed(registry))
}

fn malformed(registry: &str) -> FetchError {
    fail(
        ACCOUNT_REFUSED,
        format!("registry `{registry}` answered the account API with something unexpected"),
    )
}

/// A refusal from the account API, with its JSON `error` and `message`.
pub(crate) fn refused(registry: &str, status: u16, body: &[u8]) -> FetchError {
    let v = serde_json::from_slice::<Value>(body).ok();
    let field = |k: &str| v.as_ref()?.get(k)?.as_str().map(str::to_owned);
    let mut message = match (field("error"), field("message")) {
        (Some(code), Some(why)) => {
            format!("registry `{registry}` refused it (HTTP {status}, `{code}`): {why}")
        }
        _ if status == 404 => format!(
            "registry `{registry}` has no account API (HTTP 404) — only the first-party registry service has accounts"
        ),
        _ => format!("registry `{registry}` refused it (HTTP {status})"),
    };
    if status == 401 {
        message.push_str(&format!(
            " — sign in again with `bynk login --registry {registry}`"
        ));
    }
    fail(ACCOUNT_REFUSED, message)
}

// ---------------------------------------------------------------------------
// Credentials (decisions D and E)
// ---------------------------------------------------------------------------

/// `BYNK_ACCOUNT_<NAME>_TOKEN`. Not `BYNK_REGISTRY_<NAME>_ACCOUNT_TOKEN`,
/// which is registry `<name>-account`'s publish token.
pub fn account_token_var(name: &str) -> String {
    format!(
        "BYNK_ACCOUNT_{}_TOKEN",
        name.to_ascii_uppercase().replace('-', "_")
    )
}

/// A saved (or exported) account session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Session {
    pub token: String,
    pub expires_at: Option<i64>,
    /// It came from `BYNK_ACCOUNT_<NAME>_TOKEN`.
    pub from_env: bool,
}

/// The account token for `registry`: `BYNK_ACCOUNT_<NAME>_TOKEN`, else
/// `[accounts.<name>]` in the credentials file.
fn session(registry: &str) -> Option<Session> {
    if let Some(t) = std::env::var(account_token_var(registry))
        .ok()
        .filter(|t| !t.trim().is_empty())
    {
        return Some(Session {
            token: t,
            expires_at: None,
            from_env: true,
        });
    }
    let doc = read_doc(&super::credentials_path()?).ok()?;
    saved_session(&doc, registry)
}

/// `[accounts.<name>]` in `doc`.
pub(crate) fn saved_session(doc: &toml_edit::DocumentMut, registry: &str) -> Option<Session> {
    let entry = doc.get("accounts")?.get(registry)?;
    Some(Session {
        token: entry.get("token")?.as_str()?.to_owned(),
        expires_at: entry.get("expires-at").and_then(|v| v.as_integer()),
        from_env: false,
    })
}

fn credentials_file() -> Result<PathBuf, FetchError> {
    super::credentials_path().ok_or_else(|| {
        fail(
            ACCOUNT_REFUSED,
            "there is no home directory to keep the credentials file in (set `XDG_CONFIG_HOME`)",
        )
    })
}

/// The credentials file, or an empty one when there is none yet.
pub(crate) fn read_doc(path: &Path) -> Result<toml_edit::DocumentMut, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => text
            .parse()
            .map_err(|e| format!("`{}` isn't valid TOML: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(toml_edit::DocumentMut::new()),
        Err(e) => Err(format!("could not read `{}`: {e}", path.display())),
    }
}

/// Write the credentials file: private from its first byte (a temporary file
/// created `0600`, then renamed into place), keeping the rest of it as the
/// person wrote it.
pub(crate) fn write_doc(path: &Path, doc: &toml_edit::DocumentMut) -> Result<(), String> {
    let dir = path
        .parent()
        .ok_or("the credentials file has no directory")?;
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("could not create `{}`: {e}", dir.display()))?;
    let tmp = dir.join(format!(".credentials.toml.{}", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let written = (|| {
        use std::io::Write;
        let mut f = options.open(&tmp)?;
        f.write_all(doc.to_string().as_bytes())?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    written.map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("could not write `{}`: {e}", path.display())
    })
}

/// `[<table>.<name>]`, created (with `<table>` implicit) when missing.
fn entry<'d>(
    doc: &'d mut toml_edit::DocumentMut,
    table: &str,
    name: &str,
) -> &'d mut toml_edit::Table {
    let outer = doc
        .entry(table)
        .or_insert_with(|| {
            let mut t = toml_edit::Table::new();
            t.set_implicit(true);
            toml_edit::Item::Table(t)
        })
        .as_table_mut()
        .expect("a table");
    outer
        .entry(name)
        .or_insert_with(toml_edit::table)
        .as_table_mut()
        .expect("a table")
}

/// Keep `signed` as `[accounts.<name>]`.
pub(crate) fn set_session(doc: &mut toml_edit::DocumentMut, registry: &str, signed: &SignedIn) {
    let t = entry(doc, "accounts", registry);
    t["token"] = toml_edit::value(signed.token.as_str());
    t["login"] = toml_edit::value(signed.login.as_str());
    t["expires-at"] = toml_edit::value(signed.expires_at);
}

/// Forget `[accounts.<name>]`; whether there was one.
pub(crate) fn remove_session(doc: &mut toml_edit::DocumentMut, registry: &str) -> bool {
    let Some(accounts) = doc.get_mut("accounts").and_then(|a| a.as_table_mut()) else {
        return false;
    };
    let removed = accounts.remove(registry).is_some();
    if accounts.is_empty() {
        doc.remove("accounts");
    }
    removed
}

/// Whether `token` (or, before minting, any token) may go in
/// `[registries.<name>] token`: refused when it holds a different one,
/// unless `force`.
pub(crate) fn check_publish_slot(
    doc: &toml_edit::DocumentMut,
    registry: &str,
    token: Option<&str>,
    force: bool,
) -> Result<(), String> {
    let held = doc
        .get("registries")
        .and_then(|r| r.get(registry))
        .and_then(|r| r.get("token"))
        .and_then(|t| t.as_str());
    match held {
        Some(h) if !force && token != Some(h) => Err(format!(
            "`[registries.{registry}] token` holds a token already (perhaps another organisation's): `--force` replaces it"
        )),
        _ => Ok(()),
    }
}

/// `[registries.<name>] token = token`.
pub(crate) fn set_publish_token(doc: &mut toml_edit::DocumentMut, registry: &str, token: &str) {
    entry(doc, "registries", registry)["token"] = toml_edit::value(token);
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// Epoch milliseconds as a UTC date, `2026-11-22`.
fn date(millis: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp(millis.div_euclid(1000))
        .map_or_else(|_| millis.to_string(), |t| t.date().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// Canned answers, and the requests that asked for them.
    struct Stub {
        answers: VecDeque<(u16, Value)>,
        asked: Vec<(String, String, Option<Value>)>,
    }

    impl Stub {
        fn new(answers: &[(u16, Value)]) -> Stub {
            Stub {
                answers: answers.iter().cloned().collect(),
                asked: Vec::new(),
            }
        }
    }

    impl Api for Stub {
        fn call(
            &mut self,
            method: &str,
            path: &str,
            body: Option<&Value>,
        ) -> Result<(u16, Vec<u8>), FetchError> {
            self.asked
                .push((method.to_owned(), path.to_owned(), body.cloned()));
            let (status, v) = self.answers.pop_front().expect("an answer left");
            Ok((status, v.to_string().into_bytes()))
        }
    }

    fn problem(code: &str) -> Value {
        json!({ "error": code, "message": "…" })
    }

    fn device(interval: u64, expires_in: u64) -> DeviceStart {
        DeviceStart {
            poll: "p1".into(),
            user_code: "WDJB-MJHT".into(),
            verification_uri: "https://github.com/login/device".into(),
            interval,
            expires_in,
        }
    }

    /// Runs `poll`, recording each wait, with time passing only by waits.
    fn run(stub: &mut Stub, start: &DeviceStart) -> (Result<SignedIn, FetchError>, Vec<u64>) {
        let mut waits = Vec::new();
        let mut elapsed = Duration::ZERO;
        let got = poll(stub, "bynk", start, &mut |d| {
            waits.push(d.as_secs());
            elapsed += d;
            elapsed
        });
        (got, waits)
    }

    #[test]
    fn the_start_is_read_from_its_json() {
        let mut stub = Stub::new(&[(
            200,
            json!({ "poll": "p1", "userCode": "WDJB-MJHT", "verificationUri": "https://github.com/login/device", "interval": 5, "expiresIn": 900 }),
        )]);
        assert_eq!(start(&mut stub, "bynk").unwrap(), device(5, 900));
        assert_eq!(stub.asked[0].0, "POST");
        assert_eq!(stub.asked[0].1, "login");
    }

    #[test]
    fn polling_waits_the_interval_and_slows_down_by_five_seconds() {
        let mut stub = Stub::new(&[
            (400, problem("authorization_pending")),
            (429, problem("slow_down")),
            (400, problem("authorization_pending")),
            (
                200,
                json!({ "token": "bynka_x", "login": "ann", "expiresAt": 1_795_392_000_000_i64 }),
            ),
        ]);
        let (got, waits) = run(&mut stub, &device(2, 900));
        assert_eq!(
            got.unwrap(),
            SignedIn {
                token: "bynka_x".into(),
                login: "ann".into(),
                expires_at: 1_795_392_000_000,
            }
        );
        assert_eq!(waits, [2, 2, 7, 7]);
        assert!(stub.asked.iter().all(|(m, p, b)| m == "POST"
            && p == "login/poll"
            && b == &Some(json!({ "poll": "p1" }))));
    }

    #[test]
    fn polling_stops_on_expiry_denial_and_the_local_deadline() {
        let (got, _) = run(
            &mut Stub::new(&[(410, problem("expired_token"))]),
            &device(1, 900),
        );
        assert!(got.unwrap_err().message.contains("expired"));
        let (got, _) = run(
            &mut Stub::new(&[(403, problem("access_denied"))]),
            &device(1, 900),
        );
        assert!(got.unwrap_err().message.contains("declined"));
        // A server that never says `expired_token` can't keep the client
        // waiting past `expiresIn`.
        let pending = vec![(400, problem("authorization_pending")); 3];
        let mut stub = Stub::new(&pending);
        let (got, waits) = run(&mut stub, &device(5, 12));
        assert!(got.unwrap_err().message.contains("expired"));
        assert_eq!(waits, [5, 5, 5]);
        assert_eq!(stub.asked.len(), 2);
    }

    #[test]
    fn polling_branches_on_the_error_code_not_the_status() {
        // A `400` that isn't `authorization_pending` is a refusal, not a wait.
        let (got, _) = run(
            &mut Stub::new(&[(400, problem("bad_request"))]),
            &device(0, 900),
        );
        let e = got.unwrap_err();
        assert_eq!(e.code, ACCOUNT_REFUSED);
        assert!(e.message.contains("`bad_request`"), "{}", e.message);
    }

    #[test]
    fn a_401_says_to_sign_in_again() {
        let e = refused("bynk", 401, problem("unauthorised").to_string().as_bytes());
        assert!(
            e.message.contains("bynk login --registry bynk"),
            "{}",
            e.message
        );
        let e = refused("acme", 404, b"not json");
        assert!(e.message.contains("no account API"), "{}", e.message);
    }

    #[test]
    fn sessions_are_kept_apart_from_publish_tokens() {
        let mut doc: toml_edit::DocumentMut =
            "# mine\n[registries.bynk]\ntoken = \"bynk_old\" # laptop\n"
                .parse()
                .unwrap();
        set_session(
            &mut doc,
            "bynk",
            &SignedIn {
                token: "bynka_new".into(),
                login: "ann".into(),
                expires_at: 7,
            },
        );
        let text = doc.to_string();
        assert!(
            text.starts_with("# mine\n[registries.bynk]\ntoken = \"bynk_old\" # laptop\n"),
            "{text}"
        );
        assert!(
            text.contains(
                "[accounts.bynk]\ntoken = \"bynka_new\"\nlogin = \"ann\"\nexpires-at = 7\n"
            ),
            "{text}"
        );
        assert_eq!(
            saved_session(&doc, "bynk"),
            Some(Session {
                token: "bynka_new".into(),
                expires_at: Some(7),
                from_env: false,
            })
        );
        assert!(remove_session(&mut doc, "bynk"));
        assert!(!remove_session(&mut doc, "bynk"));
        assert_eq!(
            doc.to_string(),
            "# mine\n[registries.bynk]\ntoken = \"bynk_old\" # laptop\n"
        );
    }

    #[test]
    fn saving_a_publish_token_never_replaces_another_without_force() {
        let mut doc = toml_edit::DocumentMut::new();
        assert!(check_publish_slot(&doc, "bynk", None, false).is_ok());
        set_publish_token(&mut doc, "bynk", "bynk_a");
        assert_eq!(doc.to_string(), "[registries.bynk]\ntoken = \"bynk_a\"\n");
        assert!(check_publish_slot(&doc, "bynk", None, false).is_err());
        assert!(check_publish_slot(&doc, "bynk", Some("bynk_b"), false).is_err());
        assert!(check_publish_slot(&doc, "bynk", Some("bynk_a"), false).is_ok());
        assert!(check_publish_slot(&doc, "bynk", Some("bynk_b"), true).is_ok());
        assert!(check_publish_slot(&doc, "other", None, false).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn the_credentials_file_is_written_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("bynk-account-{}", std::process::id()));
        let path = dir.join("bynk").join("credentials.toml");
        let mut doc = read_doc(&path).unwrap();
        set_publish_token(&mut doc, "bynk", "bynk_a");
        write_doc(&path, &doc).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(read_doc(&path).unwrap().to_string(), doc.to_string());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn members_and_tokens_are_found_by_login_and_prefix() {
        let view = json!({
            "name": "acme",
            "owner": { "id": 1, "login": "ann" },
            "members": [{ "id": 1, "login": "ann" }, { "id": 2, "login": "Bob" }],
            "blocked": false,
            "tokens": [
                { "id": "7f3a00", "can": ["publish"], "expiresAt": 0, "note": "", "mintedBy": 2 },
                { "id": "7f3b00", "can": ["publish", "yank"], "expiresAt": 0, "note": "ci", "mintedBy": 0 },
            ],
        });
        assert_eq!(find_member(&view, "bob").as_deref(), Some("2"));
        assert_eq!(find_member(&view, "cy"), None);
        assert_eq!(find_token(&view, "7f3a").unwrap(), "7f3a00");
        assert!(find_token(&view, "7f3").unwrap_err().contains("2 tokens"));
        assert!(find_token(&view, "00").is_err());
        assert!(find_token(&view, "").is_err());
        assert_eq!(
            render("bynk", &view),
            "acme on `bynk`\n  owner:   ann (#1)\n  members: ann (#1), Bob (#2)\n  blocked: no\n  tokens:\n    7f3a00  publish  until 1970-01-01  minted by Bob\n    7f3b00  publish, yank  until 1970-01-01  minted by an operator  \"ci\"\n"
        );
    }

    #[test]
    fn a_path_segment_must_be_a_plain_name() {
        assert_eq!(
            orgs(&["acme", "members", "bob"]).unwrap(),
            "orgs/acme/members/bob"
        );
        assert!(orgs(&["..", "x"]).is_err());
        assert!(orgs(&["acme/x"]).is_err());
        assert!(orgs(&[""]).is_err());
    }

    #[test]
    fn the_account_variable_cannot_collide_with_a_publish_variable() {
        assert_eq!(
            account_token_var("acme-internal"),
            "BYNK_ACCOUNT_ACME_INTERNAL_TOKEN"
        );
        assert_ne!(
            account_token_var("foo"),
            bynk_project::manifest::registry_token_var("foo-account")
        );
        assert_eq!(date(1_795_392_000_000), "2026-11-23");
    }
}

//! Cloudflare's REST API (1D.3c, #4043 Decisions B, E and H): one client for the run, and typed
//! errors whose text is what bynk printed before, byte for byte.
//!
//! Moved from `migrate::live`, which kept the deploy depending on the migration sweep for its
//! client. Each consumer used to build its own client, so one `bynk deploy` loaded the system's
//! TLS roots up to eight times. Now [`for_purpose`] builds the agent once and reads the two
//! credentials once per process, which is once per command run. A missing credential is still
//! reported by the purpose that first needs it, in that purpose's words: the variables are read
//! once, and each purpose renders the same absence its own way.
//!
//! The twelve narrow traits (`HyperdriveApi`, `Listings`, `SecretsStores`, …) stay as their
//! callers' seams, each over a [`Rest`] the client hands out, as Decision B ruled.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use super::error::CloudflareError;
use super::retry::{Idempotent, Policy};

pub(crate) mod d1;
pub(crate) mod kv;
pub(crate) mod r2;
pub(crate) mod vectorize;

pub(crate) const API: &str = "https://api.cloudflare.com/client/v4";

/// The credentials the REST calls need, from the variables Wrangler itself reads in CI.
/// Wrangler's OAuth login cannot be reused here.
#[derive(Debug, Clone)]
pub(crate) struct Credentials {
    pub(crate) token: String,
    pub(crate) account: String,
}

impl Credentials {
    /// Both variables from the environment, or which of them is missing.
    fn from_env() -> Result<Self, MissingCredentials> {
        let get = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
        match (get("CLOUDFLARE_API_TOKEN"), get("CLOUDFLARE_ACCOUNT_ID")) {
            (Some(token), Some(account)) => Ok(Credentials { token, account }),
            (token, account) => Err(MissingCredentials {
                token: token.is_none(),
                account: account.is_none(),
            }),
        }
    }
}

/// Which of the two variables a run is missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MissingCredentials {
    pub(crate) token: bool,
    pub(crate) account: bool,
}

impl MissingCredentials {
    /// `set X [and Y] to {doing}: {needs} and the account it belongs to`: what each purpose has
    /// always said.
    pub(crate) fn message(self, doing: &str, needs: &str) -> String {
        let missing: Vec<&str> = [
            self.token.then_some("CLOUDFLARE_API_TOKEN"),
            self.account.then_some("CLOUDFLARE_ACCOUNT_ID"),
        ]
        .into_iter()
        .flatten()
        .collect();
        format!(
            "set {} to {doing}: {needs} and the account it belongs to",
            missing.join(" and ")
        )
    }
}

/// An HTTPS agent over rustls (ring) with the operating system's roots.
pub(crate) fn agent() -> Result<ureq::Agent, String> {
    let roots = rustls_native_certs::load_native_certs();
    let certs: Vec<ureq::tls::Certificate<'static>> = roots
        .certs
        .iter()
        .map(|c| ureq::tls::Certificate::from_der(c.as_ref()).to_owned())
        .collect();
    if certs.is_empty() {
        return Err(format!(
            "could not load the system's TLS certificate roots{}",
            roots
                .errors
                .first()
                .map(|e| format!(": {e}"))
                .unwrap_or_default()
        ));
    }
    let tls = ureq::tls::TlsConfig::builder()
        .provider(ureq::tls::TlsProvider::Rustls)
        .root_certs(ureq::tls::RootCerts::new_with_certs(&certs))
        .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .build();
    Ok(ureq::Agent::config_builder()
        .tls_config(tls)
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(60)))
        .build()
        .new_agent())
}

/// The run's credentials and agent, each built once.
struct Shared {
    credentials: Result<Credentials, MissingCredentials>,
    agent: OnceLock<Result<ureq::Agent, String>>,
}

fn shared() -> &'static Shared {
    static SHARED: OnceLock<Shared> = OnceLock::new();
    SHARED.get_or_init(|| Shared {
        credentials: Credentials::from_env(),
        agent: OnceLock::new(),
    })
}

/// Every object in `errors`, at any depth, for `visit`: an entry's `error_chain` (which
/// Cloudflare sends for some failures) included. Before 1D.3c the predicates searched the whole
/// printed `errors` text, so a nested code or message counted; walking the tree keeps that,
/// while reading each code whole.
fn walk(
    errors: &serde_json::Value,
    visit: &mut impl FnMut(&serde_json::Map<String, serde_json::Value>),
) {
    match errors {
        serde_json::Value::Object(entry) => {
            visit(entry);
            for value in entry.values() {
                walk(value, visit);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                walk(item, visit);
            }
        }
        _ => {}
    }
}

/// The run's REST client, for a purpose that says what it is `doing` and what token it `needs`
/// (both in the message when a credential is missing). The credentials are read and the agent
/// built the first time any purpose asks; a purpose asking later reuses them.
pub(crate) fn for_purpose(doing: &str, needs: &str) -> Result<Rest, CloudflareError> {
    let shared = shared();
    let credentials = shared
        .credentials
        .clone()
        .map_err(|missing| CloudflareError::Credentials(missing.message(doing, needs)))?;
    let agent = shared
        .agent
        .get_or_init(agent)
        .clone()
        .map_err(CloudflareError::Other)?;
    Ok(Rest {
        agent,
        credentials,
        base: API.to_string(),
        policy: Policy::RULED,
    })
}

/// A resource kind whose account operations run over REST (1D.4, #4056 Decision E): what its
/// missing-credential message says the run is doing, and the permission its token needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Purpose {
    Kv,
    R2,
    D1,
    Vectorize,
}

impl Purpose {
    /// What the run is doing, in the message's `set X to {doing}: …`.
    pub(crate) fn doing(self) -> &'static str {
        match self {
            Purpose::Kv => "create or delete the account's KV namespaces",
            Purpose::R2 => "create the account's R2 buckets",
            Purpose::D1 => "create the account's D1 databases",
            Purpose::Vectorize => "check, create or delete the account's Vectorize indexes",
        }
    }

    /// The permission the token needs, as Cloudflare's dashboard names it.
    pub(crate) fn permission(self) -> &'static str {
        match self {
            Purpose::Kv => "Workers KV Storage: Edit",
            Purpose::R2 => "Workers R2 Storage: Edit",
            Purpose::D1 => "D1: Edit",
            Purpose::Vectorize => "Vectorize: Edit",
        }
    }

    /// What the message says the run needs: the permission, and why a `wrangler login` session
    /// will not do (the wording Hyperdrive and the linked kinds already use).
    pub(crate) fn needs(self) -> String {
        format!(
            "an API token with \"{}\" (a `wrangler login` session is not enough: the deploy calls Cloudflare's API directly)",
            self.permission()
        )
    }
}

/// The run's REST client for a resource kind's operations ([`for_purpose`] in its words).
pub(crate) fn for_kind(purpose: Purpose) -> Result<Rest, CloudflareError> {
    for_purpose(purpose.doing(), &purpose.needs())
}

/// How a [`Request`] is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Method {
    Get,
    Post,
    Delete,
}

/// One account call as data (#4056 Decision C): the method, the path under the account, the
/// JSON body, and whether it may be sent again. A kind's request module builds these and decodes
/// the answers; [`Rest::request`] sends them.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Request {
    pub(crate) method: Method,
    pub(crate) path: String,
    pub(crate) body: Option<serde_json::Value>,
    /// One more request header (an R2 call's `cf-r2-jurisdiction`).
    pub(crate) header: Option<(&'static str, String)>,
    pub(crate) idempotent: Idempotent,
}

/// Cloudflare's REST API, over the run's agent: cheap to clone (the agent is shared).
#[derive(Clone)]
pub(crate) struct Rest {
    agent: ureq::Agent,
    credentials: Credentials,
    /// [`API`], or a local stub in tests.
    base: String,
    policy: Policy,
}

/// A failed REST call: the method and the path it asked, and what went wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RestError {
    method: &'static str,
    path: String,
    failure: RestFailure,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RestFailure {
    /// The request did not complete, or its body could not be read: the transport's own text.
    Transport(String),
    /// The body was not JSON: the parser's text.
    Unreadable(String),
    /// Cloudflare answered, but not with a success: the status and the `errors` array, as it
    /// printed (`errors` is the empty string when the answer had none), and how many seconds a
    /// `Retry-After` asked for.
    Answered {
        status: u16,
        errors: String,
        retry_after: Option<u64>,
    },
}

impl RestError {
    /// The status Cloudflare answered with, when it answered.
    pub(crate) fn status(&self) -> Option<u16> {
        match self.failure {
            RestFailure::Answered { status, .. } => Some(status),
            _ => None,
        }
    }

    /// The codes in Cloudflare's `errors` array.
    pub(crate) fn codes(&self) -> Vec<u64> {
        let mut codes = Vec::new();
        walk(&self.errors(), &mut |entry| {
            codes.extend(entry.get("code").and_then(serde_json::Value::as_u64));
        });
        codes
    }

    /// Whether Cloudflare's errors carry `code` (exactly: `10007` is not `100070`).
    pub(crate) fn has_code(&self, code: u64) -> bool {
        self.codes().contains(&code)
    }

    /// Whether one of Cloudflare's error messages mentions `text` (a D1 query's `no such table`).
    pub(crate) fn message_mentions(&self, text: &str) -> bool {
        let mut mentioned = false;
        walk(&self.errors(), &mut |entry| {
            mentioned |= entry
                .get("message")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|m| m.contains(text));
        });
        mentioned
    }

    /// Whether the token lacks the permission: a 401 or 403, or one of Cloudflare's
    /// authentication error codes (10000 "Authentication error", 9109 "Invalid access token",
    /// 9106 "Missing authentication headers").
    pub(crate) fn is_denied(&self) -> bool {
        matches!(self.status(), Some(401 | 403))
            || [10000, 9109, 9106].iter().any(|c| self.has_code(*c))
    }

    /// Cloudflare's `errors`, as it sent them.
    fn errors(&self) -> serde_json::Value {
        match &self.failure {
            RestFailure::Answered { errors, .. } => {
                serde_json::from_str(errors).unwrap_or(serde_json::Value::Null)
            }
            _ => serde_json::Value::Null,
        }
    }

    /// A failure of `method path` that Cloudflare answered with `status` and `errors` (JSON,
    /// as it prints), for a test's fake.
    #[cfg(test)]
    pub(crate) fn answered(method: &'static str, path: &str, status: u16, errors: &str) -> Self {
        RestError {
            method,
            path: path.to_string(),
            failure: RestFailure::Answered {
                status,
                errors: errors.to_string(),
                retry_after: None,
            },
        }
    }

    /// This failure, as one whose answer asked for a retry after `seconds`.
    #[cfg(test)]
    pub(crate) fn retrying_after(mut self, seconds: u64) -> Self {
        if let RestFailure::Answered { retry_after, .. } = &mut self.failure {
            *retry_after = Some(seconds);
        }
        self
    }

    /// How long Cloudflare asked to wait before a retry (`Retry-After`, in seconds).
    pub(crate) fn retry_after(&self) -> Option<Duration> {
        match self.failure {
            RestFailure::Answered {
                retry_after: Some(seconds),
                ..
            } => Some(Duration::from_secs(seconds)),
            _ => None,
        }
    }

    /// Whether a retry could help: the request did not complete, or Cloudflare answered 429 or
    /// a 5xx.
    pub(crate) fn is_transient(&self) -> bool {
        match self.failure {
            RestFailure::Transport(_) => true,
            RestFailure::Answered { status, .. } => status == 429 || status >= 500,
            RestFailure::Unreadable(_) => false,
        }
    }
}

impl std::fmt::Display for RestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (method, path) = (self.method, &self.path);
        match &self.failure {
            RestFailure::Transport(e) => write!(f, "{method} {path}: {e}"),
            RestFailure::Unreadable(e) => write!(f, "{method} {path}: unreadable answer ({e})"),
            RestFailure::Answered { status, errors, .. } => {
                write!(f, "{method} {path}: Cloudflare answered {status} {errors}")
            }
        }
    }
}

impl std::error::Error for RestError {}

impl Rest {
    /// A client for `base` with these credentials: a test's stub.
    #[cfg(test)]
    pub(crate) fn at(base: String, agent: ureq::Agent, credentials: Credentials) -> Self {
        Rest {
            agent,
            credentials,
            base,
            policy: Policy::RULED,
        }
    }

    /// This client with another retry policy (a test's).
    #[cfg(test)]
    pub(crate) fn with_policy(mut self, policy: Policy) -> Self {
        self.policy = policy;
        self
    }

    /// The account the calls are under (`CLOUDFLARE_ACCOUNT_ID`).
    pub(crate) fn account(&self) -> &str {
        &self.credentials.account
    }

    /// The agent, for the migration sweep's HTTPS wake, which is not an API call.
    pub(crate) fn agent(&self) -> &ureq::Agent {
        &self.agent
    }

    pub(crate) fn get(&self, path: &str) -> Result<serde_json::Value, RestError> {
        let url = format!("{}/accounts/{}{path}", self.base, self.credentials.account);
        self.fetch(&url, path, None)
    }

    /// [`Rest::get`] with one more request header (#3494: an R2 listing names its jurisdiction
    /// in `cf-r2-jurisdiction`).
    pub(crate) fn get_with_header(
        &self,
        path: &str,
        header: (&str, &str),
    ) -> Result<serde_json::Value, RestError> {
        let url = format!("{}/accounts/{}{path}", self.base, self.credentials.account);
        self.fetch(&url, path, Some(header))
    }

    /// A `GET` of a path that is not under the account (`/zones/…`), answered as [`Rest::get`]
    /// is (#3302: a zone's Email Sending subdomains).
    pub(crate) fn get_global(&self, path: &str) -> Result<serde_json::Value, RestError> {
        self.fetch(&format!("{}{path}", self.base), path, None)
    }

    fn fetch(
        &self,
        url: &str,
        path: &str,
        header: Option<(&str, &str)>,
    ) -> Result<serde_json::Value, RestError> {
        self.policy.run(Idempotent::Yes, || {
            let mut request = self.agent.get(url).header(
                "Authorization",
                &format!("Bearer {}", self.credentials.token),
            );
            if let Some((name, value)) = header {
                request = request.header(name, value);
            }
            answered("GET", path, request.call())
        })
    }

    /// A `POST` with a JSON body, answered as [`Rest::get`] is. Not idempotent unless the caller
    /// says so: the D1 query that reads a database's applied migrations (#3073) is a `SELECT`.
    pub(crate) fn post(
        &self,
        path: &str,
        body: &serde_json::Value,
        idempotent: Idempotent,
    ) -> Result<serde_json::Value, RestError> {
        self.send("POST", path, Some(body), idempotent)
    }

    /// A `PUT` with a JSON body, answered as [`Rest::get`] is (#3319: replacing a Hyperdrive
    /// config).
    pub(crate) fn put(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value, RestError> {
        self.send("PUT", path, Some(body), Idempotent::Yes)
    }

    /// A `DELETE`, answered as [`Rest::get`] is (#3319: pruning a Hyperdrive config).
    pub(crate) fn delete(&self, path: &str) -> Result<serde_json::Value, RestError> {
        self.send("DELETE", path, None, Idempotent::Yes)
    }

    /// Send `request`, answered as [`Rest::get`] is.
    pub(crate) fn request(&self, request: &Request) -> Result<serde_json::Value, RestError> {
        let path = &request.path;
        let header = request.header.as_ref().map(|(n, v)| (*n, v.as_str()));
        match request.method {
            Method::Get => match header {
                Some(header) => self.get_with_header(path, header),
                None => self.get(path),
            },
            Method::Post => self.send_with(
                "POST",
                path,
                Some(request.body.as_ref().unwrap_or(&serde_json::Value::Null)),
                request.idempotent,
                header,
            ),
            Method::Delete => self.send_with("DELETE", path, None, Idempotent::Yes, header),
        }
    }

    /// A write to an account path. Errors name the method and path and what Cloudflare
    /// answered, never the request body: a body can carry a secret.
    fn send(
        &self,
        method: &'static str,
        path: &str,
        body: Option<&serde_json::Value>,
        idempotent: Idempotent,
    ) -> Result<serde_json::Value, RestError> {
        self.send_with(method, path, body, idempotent, None)
    }

    /// [`Rest::send`] with one more request header on a `POST` or `DELETE`.
    fn send_with(
        &self,
        method: &'static str,
        path: &str,
        body: Option<&serde_json::Value>,
        idempotent: Idempotent,
        header: Option<(&str, &str)>,
    ) -> Result<serde_json::Value, RestError> {
        let url = format!("{}/accounts/{}{path}", self.base, self.credentials.account);
        let auth = format!("Bearer {}", self.credentials.token);
        let (extra, value) = header.unwrap_or(("", ""));
        self.policy.run(idempotent, || {
            let response = match (method, body) {
                ("POST", Some(b)) => {
                    let mut request = self
                        .agent
                        .post(&url)
                        .header("Authorization", &auth)
                        .header("Content-Type", "application/json");
                    if !extra.is_empty() {
                        request = request.header(extra, value);
                    }
                    request.send(b.to_string())
                }
                ("PUT", Some(b)) => self
                    .agent
                    .put(&url)
                    .header("Authorization", &auth)
                    .header("Content-Type", "application/json")
                    .send(b.to_string()),
                _ => {
                    let mut request = self.agent.delete(&url).header("Authorization", &auth);
                    if !extra.is_empty() {
                        request = request.header(extra, value);
                    }
                    request.call()
                }
            };
            answered(method, path, response)
        })
    }
}

/// What a response means: its JSON when Cloudflare says it succeeded, else the failure.
fn answered(
    method: &'static str,
    path: &str,
    response: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
) -> Result<serde_json::Value, RestError> {
    let fail = |failure| RestError {
        method,
        path: path.to_string(),
        failure,
    };
    let mut response = response.map_err(|e| fail(RestFailure::Transport(e.to_string())))?;
    let status = response.status().as_u16();
    let retry_after = response
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse().ok());
    let text = response
        .body_mut()
        .read_to_string()
        .map_err(|e| fail(RestFailure::Transport(e.to_string())))?;
    let body: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| fail(RestFailure::Unreadable(e.to_string())))?;
    if status != 200 || body.get("success") != Some(&serde_json::Value::Bool(true)) {
        let errors = body
            .get("errors")
            .map(serde_json::Value::to_string)
            .unwrap_or_default();
        return Err(fail(RestFailure::Answered {
            status,
            errors,
            retry_after,
        }));
    }
    Ok(body)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A client for a local stub at `base`.
    pub(crate) fn rest(base: String) -> Rest {
        Rest::at(
            base,
            ureq::Agent::config_builder()
                .http_status_as_error(false)
                .build()
                .new_agent(),
            Credentials {
                token: "tok".into(),
                account: "acct".into(),
            },
        )
    }

    #[test]
    fn a_missing_credential_is_named_in_the_purposes_words() {
        let both = MissingCredentials {
            token: true,
            account: true,
        };
        assert_eq!(
            both.message("sweep", "a token"),
            "set CLOUDFLARE_API_TOKEN and CLOUDFLARE_ACCOUNT_ID to sweep: a token and the account it belongs to"
        );
        let token = MissingCredentials {
            token: true,
            account: false,
        };
        assert_eq!(
            token.message("revert", "a token"),
            "set CLOUDFLARE_API_TOKEN to revert: a token and the account it belongs to"
        );
        let account = MissingCredentials {
            token: false,
            account: true,
        };
        assert!(
            account
                .message("x", "y")
                .starts_with("set CLOUDFLARE_ACCOUNT_ID to x:")
        );
    }

    #[test]
    fn what_cloudflare_answered_is_read_exactly() {
        let e = RestError::answered(
            "GET",
            "/x",
            404,
            r#"[{"code":10007,"message":"no such table: d1_migrations"}]"#,
        );
        assert_eq!(e.status(), Some(404));
        assert!(e.has_code(10007));
        assert!(
            !e.has_code(1000),
            "a code is matched whole, not as a prefix"
        );
        assert!(e.message_mentions("no such table"));
        assert!(!e.is_denied());
        assert_eq!(
            e.to_string(),
            r#"GET /x: Cloudflare answered 404 [{"code":10007,"message":"no such table: d1_migrations"}]"#
        );
        // A code or message nested in an entry's `error_chain` counts, as it did when the
        // printed text was searched.
        let nested = RestError::answered(
            "POST",
            "/x",
            400,
            r#"[{"code":7500,"message":"failed","error_chain":[{"code":10000,"message":"no such table: t"}]}]"#,
        );
        assert!(nested.has_code(10000) && nested.is_denied(), "{nested}");
        assert!(nested.message_mentions("no such table"));
        let near = RestError::answered("GET", "/x", 400, r#"[{"code":100070}]"#);
        assert!(!near.has_code(10007), "100070 is not 10007");
        for denied in [
            RestError::answered("GET", "/x", 403, "[]"),
            RestError::answered("GET", "/x", 401, ""),
            RestError::answered("GET", "/x", 400, r#"[{"code":9109}]"#),
            RestError::answered("GET", "/x", 200, r#"[{"code":10000}]"#),
        ] {
            assert!(denied.is_denied(), "{denied}");
        }
        assert!(!RestError::answered("GET", "/x", 500, "[]").is_denied());
    }

    /// A stub that answers each request with the next of `answers` and counts what it was sent.
    pub(crate) fn answering_in_turn(
        answers: &'static [(u16, &'static str)],
    ) -> (String, std::thread::JoinHandle<Vec<String>>) {
        use std::io::{BufRead, BufReader, Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for (status, body) in answers {
                let Ok((stream, _)) = listener.accept() else {
                    break;
                };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                seen.push(
                    line.split_whitespace()
                        .take(2)
                        .collect::<Vec<_>>()
                        .join(" "),
                );
                let mut length = 0usize;
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).unwrap_or(0) == 0 || header == "\r\n" {
                        break;
                    }
                    if let Some(n) = header.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = n.trim().parse().unwrap_or(0);
                    }
                }
                let _ = reader.read_exact(&mut vec![0u8; length]);
                let mut stream = stream;
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
            seen
        });
        (base, seen)
    }

    fn no_wait(_: std::time::Duration) {}

    /// The client runs its policy: under backoff an idempotent call is sent again after a 5xx,
    /// and a call that is not idempotent never is. (What ships is one attempt, pinned by the REST
    /// message fixture.)
    #[test]
    fn the_client_retries_only_as_its_policy_and_the_call_allow() {
        let backoff = Policy {
            sleep: no_wait,
            ..Policy::BACKOFF
        };
        let (base, seen) = answering_in_turn(&[
            (503, r#"{"success":false,"errors":[]}"#),
            (200, r#"{"success":true,"result":{"id":"v1"}}"#),
        ]);
        let got = rest(base).with_policy(backoff).get("/workers/scripts/s");
        assert!(got.unwrap().to_string().contains("v1"));
        assert_eq!(seen.join().unwrap().len(), 2);

        let (base, seen) = answering_in_turn(&[(503, r#"{"success":false,"errors":[]}"#)]);
        let got = rest(base).with_policy(backoff).post(
            "/hyperdrive/configs",
            &serde_json::json!({"name": "h"}),
            Idempotent::No,
        );
        assert_eq!(got.unwrap_err().status(), Some(503));
        assert_eq!(
            seen.join().unwrap(),
            ["POST /accounts/acct/hyperdrive/configs"]
        );
    }
}

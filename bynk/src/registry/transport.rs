//! Reading a registry's files (packaging slice 4a, #2274, decision G): a
//! `file://` registry from disk, an `https://` one (or `http://localhost`)
//! over `ureq` with rustls.
//!
//! A token is sent as `Authorization: Bearer …` only to the exact origins
//! the registry's own URLs name (scheme, host and port). Redirects are
//! followed by hand: one to another origin drops the token, and one from
//! `https` to `http` is refused. A token never appears in a message.

use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

use ureq::unversioned::resolver::{DefaultResolver, ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{DefaultConnector, NextTimeout};

use crate::fetch::{FetchError, fail};

pub(crate) const HTTP_FAILED: &str = "bynk.registry.http_failed";

/// The most a single registry response may be: an index file or an archive.
pub(crate) const MAX_RESPONSE: u64 = 64 * 1024 * 1024;

const MAX_REDIRECTS: usize = 5;

/// A registry's location and credentials.
pub(crate) struct Transport {
    /// The registry name, for messages.
    pub name: String,
    /// The origins (`scheme://host[:port]`) a token may be sent to.
    origins: Vec<String>,
    token: Option<String>,
    agent: Option<ureq::Agent>,
}

impl Transport {
    pub(crate) fn new(name: &str, urls: &[&str]) -> Transport {
        Transport {
            name: name.to_owned(),
            origins: urls.iter().filter_map(|u| origin(u)).collect(),
            token: None,
            agent: None,
        }
    }

    /// Send `token` to this registry's origins from now on.
    pub(crate) fn with_token(mut self, token: Option<String>) -> Transport {
        self.token = token;
        self
    }

    /// Also allow `url`'s origin a token (the archive base, once
    /// `config.json` names it).
    pub(crate) fn allow_origin(&mut self, url: &str) {
        if let Some(o) = origin(url)
            && !self.origins.contains(&o)
        {
            self.origins.push(o);
        }
    }

    /// The bytes at `url`; `Ok(None)` for a missing file (404).
    pub(crate) fn get(&mut self, url: &str) -> Result<Option<Vec<u8>>, FetchError> {
        if let Some(path) = file_path(url) {
            return match std::fs::read(&path) {
                Ok(bytes) => Ok(Some(bytes)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(fail(
                    HTTP_FAILED,
                    format!(
                        "could not read `{}` from registry `{}`: {e}",
                        path.display(),
                        self.name
                    ),
                )),
            };
        }
        let agent = match &self.agent {
            Some(a) => a.clone(),
            None => {
                let a = agent().map_err(|e| fail(HTTP_FAILED, e))?;
                self.agent = Some(a.clone());
                a
            }
        };
        let mut current = url.to_owned();
        for _ in 0..=MAX_REDIRECTS {
            let mut req = agent.get(&current);
            let send_token = origin(&current).is_some_and(|o| self.origins.contains(&o));
            if send_token && let Some(token) = &self.token {
                req = req.header("Authorization", &format!("Bearer {token}"));
            }
            let mut resp = req.call().map_err(|e| {
                fail(
                    HTTP_FAILED,
                    format!(
                        "could not reach registry `{}` at `{current}`: {e}",
                        self.name
                    ),
                )
            })?;
            let status = resp.status().as_u16();
            if (300..400).contains(&status) {
                let Some(location) = resp.headers().get("location").and_then(|v| v.to_str().ok())
                else {
                    return Err(fail(
                        HTTP_FAILED,
                        format!("registry `{}` redirected without a location", self.name),
                    ));
                };
                let next = resolve(&current, location);
                if current.starts_with("https://") && !next.starts_with("https://") {
                    return Err(fail(
                        HTTP_FAILED,
                        format!(
                            "registry `{}` redirected from https to `{next}`, which isn't https",
                            self.name
                        ),
                    ));
                }
                current = next;
                continue;
            }
            if status == 404 {
                return Ok(None);
            }
            if status == 401 || status == 403 {
                let why = if self.token.is_some() {
                    format!("the token for `{}` was refused", self.name)
                } else {
                    format!(
                        "registry `{}` needs a token: set `{}` or add it to `~/.config/bynk/credentials.toml`",
                        self.name,
                        bynk_project::manifest::registry_token_var(&self.name)
                    )
                };
                return Err(fail(HTTP_FAILED, format!("{why} (HTTP {status})")));
            }
            if !(200..300).contains(&status) {
                return Err(fail(
                    HTTP_FAILED,
                    format!(
                        "registry `{}` answered HTTP {status} for `{current}`",
                        self.name
                    ),
                ));
            }
            let mut bytes = Vec::new();
            resp.body_mut()
                .as_reader()
                .take(MAX_RESPONSE + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| {
                    fail(
                        HTTP_FAILED,
                        format!("could not read from registry `{}`: {e}", self.name),
                    )
                })?;
            if bytes.len() as u64 > MAX_RESPONSE {
                return Err(fail(
                    HTTP_FAILED,
                    format!(
                        "a response from registry `{}` is larger than {MAX_RESPONSE} bytes",
                        self.name
                    ),
                ));
            }
            return Ok(Some(bytes));
        }
        Err(fail(
            HTTP_FAILED,
            format!(
                "registry `{}` redirected more than {MAX_REDIRECTS} times",
                self.name
            ),
        ))
    }
}

impl Transport {
    /// #2287: a write to the publish API — `PUT` with `body`, or `POST` with
    /// none — returning the status and the response body. A write follows no
    /// redirect: a registry that redirects one is refused, so a token and an
    /// upload never travel anywhere the registry's `api` didn't name.
    pub(crate) fn send(
        &mut self,
        method: &str,
        url: &str,
        body: Option<&[u8]>,
    ) -> Result<(u16, Vec<u8>), FetchError> {
        let agent = match &self.agent {
            Some(a) => a.clone(),
            None => {
                let a = agent().map_err(|e| fail(HTTP_FAILED, e))?;
                self.agent = Some(a.clone());
                a
            }
        };
        let send_token = origin(url).is_some_and(|o| self.origins.contains(&o));
        let auth = self
            .token
            .as_ref()
            .filter(|_| send_token)
            .map(|t| format!("Bearer {t}"));
        let result = match (method, body) {
            ("PUT", Some(bytes)) => {
                let mut req = agent
                    .put(url)
                    .header("Content-Type", "application/vnd.bynk.tree-v1");
                if let Some(a) = &auth {
                    req = req.header("Authorization", a);
                }
                req.send(bytes)
            }
            _ => {
                let mut req = agent.post(url);
                if let Some(a) = &auth {
                    req = req.header("Authorization", a);
                }
                req.send_empty()
            }
        };
        let mut resp = result.map_err(|e| {
            fail(
                HTTP_FAILED,
                format!("could not reach registry `{}` at `{url}`: {e}", self.name),
            )
        })?;
        let status = resp.status().as_u16();
        if (300..400).contains(&status) {
            return Err(fail(
                HTTP_FAILED,
                format!(
                    "registry `{}` redirected a publish request (HTTP {status}), which bynk never follows",
                    self.name
                ),
            ));
        }
        let mut bytes = Vec::new();
        resp.body_mut()
            .as_reader()
            .take(1024 * 1024)
            .read_to_end(&mut bytes)
            .map_err(|e| {
                fail(
                    HTTP_FAILED,
                    format!("could not read from registry `{}`: {e}", self.name),
                )
            })?;
        Ok((status, bytes))
    }
}

impl Transport {
    /// R3.1 (#2378): a request to the account API — any method, an optional
    /// JSON body, and `bearer` (the account token, never the registry's
    /// publish token), sent only to the registry's own origins. Like
    /// [`Transport::send`], it follows no redirect.
    pub(crate) fn json(
        &mut self,
        method: &str,
        url: &str,
        body: Option<&serde_json::Value>,
        bearer: Option<&str>,
    ) -> Result<(u16, Vec<u8>), FetchError> {
        let agent = match &self.agent {
            Some(a) => a.clone(),
            None => {
                let a = agent().map_err(|e| fail(HTTP_FAILED, e))?;
                self.agent = Some(a.clone());
                a
            }
        };
        let mut req = ureq::http::Request::builder().method(method).uri(url);
        if origin(url).is_some_and(|o| self.origins.contains(&o))
            && let Some(t) = bearer
        {
            req = req.header("Authorization", format!("Bearer {t}"));
        }
        let built =
            |e: ureq::http::Error| fail(HTTP_FAILED, format!("could not build the request: {e}"));
        let result = match body {
            Some(v) => agent.run(
                req.header("Content-Type", "application/json")
                    .body(v.to_string().into_bytes())
                    .map_err(built)?,
            ),
            None => agent.run(req.body(()).map_err(built)?),
        };
        let mut resp = result.map_err(|e| {
            fail(
                HTTP_FAILED,
                format!("could not reach registry `{}` at `{url}`: {e}", self.name),
            )
        })?;
        let status = resp.status().as_u16();
        if (300..400).contains(&status) {
            return Err(fail(
                HTTP_FAILED,
                format!(
                    "registry `{}` redirected an account request (HTTP {status}), which bynk never follows",
                    self.name
                ),
            ));
        }
        let mut out = Vec::new();
        resp.body_mut()
            .as_reader()
            .take(1024 * 1024)
            .read_to_end(&mut out)
            .map_err(|e| {
                fail(
                    HTTP_FAILED,
                    format!("could not read from registry `{}`: {e}", self.name),
                )
            })?;
        Ok((status, out))
    }
}

/// #2311: what a conformance request carries as its body.
pub(crate) enum Body<'b> {
    None,
    Bytes(&'b [u8]),
    /// `n` zero bytes, streamed and declared with `Content-Length: n`.
    Zeros(u64),
}

/// #2311: whose token a conformance request carries.
pub(crate) enum Auth<'a> {
    /// None at all.
    Without,
    /// This one, whatever the registry's configured token is.
    Bearer(&'a str),
}

/// #2311: one request exactly as a conformance check needs it — any method,
/// an explicit token (or none), a streamed body, no redirect followed —
/// returning the status, the response's `Content-Type`, and up to 1 MiB of
/// its body. The bytes go only to `url`; no configured token is consulted.
pub(crate) fn request(
    method: &str,
    url: &str,
    body: Body<'_>,
    auth: Auth<'_>,
) -> Result<(u16, Option<String>, Vec<u8>), String> {
    let agent = agent()?;
    let mut req = ureq::http::Request::builder().method(method).uri(url);
    if let Auth::Bearer(t) = auth {
        req = req.header("Authorization", format!("Bearer {t}"));
    }
    let result = match body {
        Body::None => agent.run(
            req.body(())
                .map_err(|e| format!("could not build the request: {e}"))?,
        ),
        Body::Bytes(bytes) => agent.run(
            req.header("Content-Type", "application/vnd.bynk.tree-v1")
                .body(bytes.to_vec())
                .map_err(|e| format!("could not build the request: {e}"))?,
        ),
        Body::Zeros(n) => {
            let reader = std::io::repeat(0).take(n);
            agent.run(
                req.header("Content-Type", "application/vnd.bynk.tree-v1")
                    .header("Content-Length", n.to_string())
                    .body(ureq::SendBody::from_owned_reader(reader))
                    .map_err(|e| format!("could not build the request: {e}"))?,
            )
        }
    };
    let mut resp = result.map_err(|e| format!("could not reach `{url}`: {e}"))?;
    let status = resp.status().as_u16();
    let kind = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let mut bytes = Vec::new();
    resp.body_mut()
        .as_reader()
        .take(1024 * 1024)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("could not read the response from `{url}`: {e}"))?;
    Ok((status, kind, bytes))
}

/// `url` joined to `base` the way a browser would for the forms registries
/// use: absolute, root-relative, or relative to the base's directory.
pub(crate) fn resolve(base: &str, url: &str) -> String {
    if url.contains("://") {
        return url.to_owned();
    }
    if let Some(rest) = url.strip_prefix('/')
        && let Some(o) = origin(base)
    {
        return format!("{o}/{rest}");
    }
    let dir = base.rsplit_once('/').map_or(base, |(d, _)| d);
    format!("{dir}/{url}")
}

/// `base` + `/` + `rest`, without doubling the slash.
pub(crate) fn join(base: &str, rest: &str) -> String {
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        rest.trim_start_matches('/')
    )
}

/// `scheme://host[:port]` of an `http(s)` URL.
fn origin(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    if scheme != "https" && scheme != "http" {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next()?;
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    Some(format!("{scheme}://{}", host_port.to_ascii_lowercase()))
}

/// The local path of a `file://` URL (`file:///C:/x` on Windows).
fn file_path(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    let bytes = rest.as_bytes();
    // `file:///C:/…`: drop the slash before a drive letter.
    if bytes.len() > 3 && bytes[0] == b'/' && bytes[2] == b':' && bytes[1].is_ascii_alphabetic() {
        return Some(PathBuf::from(&rest[1..]));
    }
    Some(PathBuf::from(rest))
}

/// An agent that follows no redirects itself, reports every status, and
/// times out.
fn agent() -> Result<ureq::Agent, String> {
    let mut builder = ureq::Agent::config_builder()
        .max_redirects(0)
        .max_redirects_will_error(false)
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(120)));
    // The system's roots over rustls (ring), as `bynk migrate` uses. Without
    // them, only plain `http://localhost` works, which `index_problem` allows.
    if let Ok(base) = crate::migrate::live::agent() {
        builder = builder.tls_config(base.config().tls_config().clone());
    }
    Ok(ureq::Agent::with_parts(
        builder.build(),
        DefaultConnector::default(),
        LocalhostIpv4First::default(),
    ))
}

/// ureq's resolver, except that `localhost` tries its IPv4 addresses first.
///
/// ureq tries the addresses in the order the system gives them, and Windows
/// gives `::1` first. `bynk registry serve` listens on `127.0.0.1`, so every
/// request to `http://localhost` was first refused on `::1`, which Windows
/// takes about two seconds to report. The conformance suite makes dozens of
/// requests, each on a fresh connection. A server on `::1` alone is still
/// reached, one address later.
#[derive(Debug, Default)]
struct LocalhostIpv4First(DefaultResolver);

impl Resolver for LocalhostIpv4First {
    fn resolve(
        &self,
        uri: &ureq::http::Uri,
        config: &ureq::config::Config,
        timeout: NextTimeout,
    ) -> Result<ResolvedSocketAddrs, ureq::Error> {
        let mut addrs = self.0.resolve(uri, config, timeout)?;
        if uri
            .host()
            .is_some_and(|h| h.eq_ignore_ascii_case("localhost"))
        {
            ipv4_first(&mut addrs);
        }
        Ok(addrs)
    }
}

/// IPv4 addresses before IPv6 ones, each family in its original order.
fn ipv4_first(addrs: &mut [std::net::SocketAddr]) {
    addrs.sort_by_key(|a| a.is_ipv6());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipv4_addresses_are_tried_first_in_their_order() {
        let mut addrs: Vec<std::net::SocketAddr> = [
            "[::1]:8730",
            "127.0.0.1:8730",
            "[::2]:8730",
            "127.0.0.2:8730",
        ]
        .iter()
        .map(|a| a.parse().unwrap())
        .collect();
        ipv4_first(&mut addrs);
        let order: Vec<String> = addrs.iter().map(ToString::to_string).collect();
        assert_eq!(
            order,
            [
                "127.0.0.1:8730",
                "127.0.0.2:8730",
                "[::1]:8730",
                "[::2]:8730"
            ]
        );
    }

    #[test]
    fn origins_and_resolution() {
        assert_eq!(
            origin("https://Reg.Example:8443/index/x").as_deref(),
            Some("https://reg.example:8443")
        );
        assert_eq!(origin("https://u@h/x").as_deref(), Some("https://h"));
        assert_eq!(origin("file:///x"), None);
        assert_eq!(
            resolve("https://a/index/config.json", "dl"),
            "https://a/index/dl"
        );
        assert_eq!(resolve("https://a/index/x", "/other"), "https://a/other");
        assert_eq!(resolve("https://a/x", "https://b/y"), "https://b/y");
        assert_eq!(
            join("https://a/index/", "/acme/money"),
            "https://a/index/acme/money"
        );
        assert_eq!(
            file_path("file:///C:/reg").unwrap(),
            PathBuf::from("C:/reg")
        );
        assert_eq!(
            file_path("file:///tmp/reg").unwrap(),
            PathBuf::from("/tmp/reg")
        );
    }
}

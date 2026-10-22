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
    Ok(builder.build().new_agent())
}

#[cfg(test)]
mod tests {
    use super::*;

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

//! `bynk registry serve` and `bynk registry token` (packaging slice 4c, #2287,
//! decisions A, D and E): a reference server that puts the publish API in
//! front of a static registry directory.
//!
//! It serves the directory's files by `GET` and the API under `/api`:
//!
//! - `PUT /api/<org>/<package>/<version>` with a `bynk-tree-v1` archive body
//!   admits the upload by the same rules as `bynk registry add`, signs its line
//!   with the **server's** key, and answers `201` with the line (`200` with the
//!   existing line when the same tree was already published, #2311);
//! - `POST /api/<org>/<package>/<version>/yank` (or `/unyank`) flips `yanked`.
//!
//! A request carries `Authorization: Bearer <token>`; the tokens file holds
//! only each token's sha256, the organisations it may act for, and what it
//! `can` (`publish`, `yank`). Requests are handled one at a time, so writes to
//! a registry are serialised. It speaks plain HTTP and binds loopback by
//! default: put a TLS-terminating proxy in front to serve beyond this machine.

use std::io::Read;
use std::path::{Component, Path, PathBuf};

use serde_json::json;
use sha2::{Digest, Sha256};

use super::commands::{
    Refusal, admit, read_config, signing_key, write_private, write_replacing, yank_line,
};
use super::protocol::Config;

/// The largest upload accepted.
const MAX_UPLOAD: u64 = 64 * 1024 * 1024;

/// The default address: loopback only.
pub const DEFAULT_LISTEN: &str = "127.0.0.1:8730";

/// One entry of the tokens file.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TokenEntry {
    sha256: String,
    organisations: Vec<String>,
    can: Vec<String>,
}

/// `bynk registry serve`: serve `dir` with the publish API, signing with
/// `key_file` and authorising with `tokens_file`, on `listen`. `ready` is told
/// the address actually bound, once listening. Runs until the process ends.
pub fn serve(
    dir: &Path,
    key_file: &Path,
    tokens_file: &Path,
    listen: &str,
    ready: impl FnOnce(&str),
) -> Result<(), String> {
    for (what, path) in [("signing key", key_file), ("tokens file", tokens_file)] {
        if inside(path, dir) {
            return Err(format!(
                "the {what} `{}` is inside the registry directory, which is served — keep it elsewhere",
                path.display()
            ));
        }
    }
    let mut config = read_config(dir)?;
    let key = signing_key(key_file)?;
    super::commands::current_key(&config, &key, key_file)?;
    if config.api.is_none() {
        config.api = Some("api".to_string());
        write_replacing(&dir.join("config.json"), config.render().as_bytes())
            .map_err(|e| format!("could not record the API in `config.json`: {e}"))?;
    }
    let tokens = read_tokens(tokens_file)?;
    let server = tiny_http::Server::http(listen)
        .map_err(|e| format!("could not listen on `{listen}`: {e}"))?;
    let bound = server
        .server_addr()
        .to_ip()
        .ok_or("the server isn't on an IP address")?;
    if !bound.ip().is_loopback() {
        eprintln!(
            "bynk: warning: serving on {bound}, beyond this machine, over plain HTTP — tokens travel in clear unless a TLS-terminating proxy is in front"
        );
    }
    ready(&format!("http://{bound}"));
    for mut request in server.incoming_requests() {
        let (status, body, kind) = handle(dir, &config, &key, &tokens, &mut request);
        let response = tiny_http::Response::from_data(body)
            .with_status_code(status)
            .with_header(
                tiny_http::Header::from_bytes(&b"Content-Type"[..], kind.as_bytes())
                    .expect("a valid header"),
            );
        let _ = request.respond(response);
    }
    Ok(())
}

/// One request: `(status, body, content type)`.
fn handle(
    dir: &Path,
    config: &Config,
    key: &super::protocol::SigningKey,
    tokens: &[TokenEntry],
    request: &mut tiny_http::Request,
) -> (u16, Vec<u8>, &'static str) {
    let url = request
        .url()
        .split(['?', '#'])
        .next()
        .unwrap_or("/")
        .to_string();
    let method = request.method().clone();
    if let Some(rest) = url.strip_prefix("/api/") {
        let parts: Vec<&str> = rest.split('/').collect();
        let error = |status: u16, code: &str, message: String| {
            (
                status,
                json!({ "error": code, "message": message })
                    .to_string()
                    .into_bytes(),
                "application/json",
            )
        };
        let token = request
            .headers()
            .iter()
            .find(|h| h.field.equiv("Authorization"))
            .and_then(|h| h.value.as_str().strip_prefix("Bearer ").map(str::to_owned));
        let authorise = |org: &str, cap: &str| -> Result<(), (u16, Vec<u8>, &'static str)> {
            let Some(token) = &token else {
                return Err(error(
                    401,
                    "unauthorised",
                    "this registry needs a publish token".into(),
                ));
            };
            let digest = hex(&Sha256::digest(token.as_bytes()));
            let Some(entry) = tokens
                .iter()
                .find(|t| constant_time_eq(t.sha256.as_bytes(), digest.as_bytes()))
            else {
                return Err(error(
                    401,
                    "unauthorised",
                    "the token isn't known to this registry".into(),
                ));
            };
            if !entry.organisations.iter().any(|o| o == org) {
                return Err(error(
                    403,
                    "forbidden",
                    format!("the token can't act for the organisation `{org}`"),
                ));
            }
            if !entry.can.iter().any(|c| c == cap) {
                return Err(error(403, "forbidden", format!("the token can't `{cap}`")));
            }
            Ok(())
        };
        return match (method, parts.as_slice()) {
            (tiny_http::Method::Put, [org, package, version]) => {
                if let Err(e) = authorise(org, "publish") {
                    return e;
                }
                if request.body_length().is_some_and(|n| n as u64 > MAX_UPLOAD) {
                    return error(
                        413,
                        "too_large",
                        format!("an upload is limited to {MAX_UPLOAD} bytes"),
                    );
                }
                let mut upload = Vec::new();
                if request
                    .as_reader()
                    .take(MAX_UPLOAD + 1)
                    .read_to_end(&mut upload)
                    .is_err()
                {
                    return error(400, "invalid_package", "the upload couldn't be read".into());
                }
                if upload.len() as u64 > MAX_UPLOAD {
                    return error(
                        413,
                        "too_large",
                        format!("an upload is limited to {MAX_UPLOAD} bytes"),
                    );
                }
                let name = format!("{org}/{package}");
                match admit(dir, config, key, &upload, Some((&name, version))) {
                    // #2311 (D10): a retry of the same tree is `200` with
                    // the existing line; a new line is `201`.
                    Ok(a) => (
                        if a.fresh { 201 } else { 200 },
                        a.line.render().into_bytes(),
                        "application/json",
                    ),
                    Err(r) => refusal(r),
                }
            }
            (tiny_http::Method::Post, [org, package, version, verb @ ("yank" | "unyank")]) => {
                if let Err(e) = authorise(org, "yank") {
                    return e;
                }
                match yank_line(dir, &format!("{org}/{package}"), version, *verb == "unyank") {
                    Ok(()) => (
                        200,
                        json!({ "ok": true }).to_string().into_bytes(),
                        "application/json",
                    ),
                    // #2330 (E): un-yanking a withdrawn version conflicts with
                    // its state (`409`); publishing it again is `410`.
                    Err(r) if r.code == "withdrawn" => {
                        let (_, body, kind) = refusal(r);
                        (409, body, kind)
                    }
                    Err(r) => refusal(r),
                }
            }
            _ => error(404, "not_found", format!("no API endpoint at `{url}`")),
        };
    }
    if method != tiny_http::Method::Get {
        return (405, Vec::new(), "text/plain");
    }
    match served_path(dir, &url).and_then(|p| std::fs::read(p).ok()) {
        Some(bytes) => (200, bytes, "application/octet-stream"),
        None => (404, Vec::new(), "text/plain"),
    }
}

/// A refusal as an HTTP response.
fn refusal(r: Refusal) -> (u16, Vec<u8>, &'static str) {
    let status = match r.code {
        "invalid_package" => 400,
        "version_exists" => 409,
        "withdrawn" => 410,
        "not_found" => 404,
        "too_large" => 413,
        _ => 500,
    };
    (
        status,
        json!({ "error": r.code, "message": r.message })
            .to_string()
            .into_bytes(),
        "application/json",
    )
}

/// The file under `dir` a `GET` names, if it is a plain path inside it.
fn served_path(dir: &Path, url: &str) -> Option<PathBuf> {
    let rel = url.trim_start_matches('/');
    if rel.is_empty() || rel.contains('\\') {
        return None;
    }
    let path = Path::new(rel);
    if !path.components().all(|c| matches!(c, Component::Normal(_))) {
        return None;
    }
    let full = dir.join(path);
    full.is_file().then_some(full)
}

/// `bynk registry token`: mint a token for `organisations` that `can` the
/// given actions, append its hash to `tokens_file`, and return the token —
/// shown once, never stored.
pub fn mint(
    tokens_file: &Path,
    organisations: &[String],
    can: &[String],
) -> Result<String, String> {
    if organisations.is_empty() {
        return Err("name at least one organisation with `--org`".to_string());
    }
    for org in organisations {
        if let Some(problem) = bynk_project::manifest::org_problem(org) {
            return Err(format!("`{org}` isn't an organisation: {problem}"));
        }
    }
    for c in can {
        if c != "publish" && c != "yank" {
            return Err(format!("`{c}` isn't a capability: use `publish` or `yank`"));
        }
    }
    let mut raw = [0u8; 32];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut raw)
        .map_err(|_| "could not generate a token".to_string())?;
    let token = format!("bynk_{}", hex(&raw));
    let mut text = std::fs::read_to_string(tokens_file).unwrap_or_default();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    let list = |xs: &[String]| {
        xs.iter()
            .map(|x| format!("\"{x}\""))
            .collect::<Vec<_>>()
            .join(", ")
    };
    text.push_str(&format!(
        "\n[[token]]\nsha256 = \"{}\"\norganisations = [{}]\ncan = [{}]\n",
        hex(&Sha256::digest(token.as_bytes())),
        list(organisations),
        list(can),
    ));
    write_private(tokens_file, text.as_bytes())?;
    Ok(token)
}

fn read_tokens(path: &Path) -> Result<Vec<TokenEntry>, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("could not read the tokens file `{}`: {e}", path.display()))?;
    let doc: toml::Table = text
        .parse()
        .map_err(|e| format!("the tokens file `{}` isn't TOML: {e}", path.display()))?;
    let list = |v: Option<&toml::Value>| -> Vec<String> {
        v.and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    let entries = doc
        .get("token")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|t| {
                    Some(TokenEntry {
                        sha256: t.get("sha256")?.as_str()?.to_ascii_lowercase(),
                        organisations: list(t.get("organisations")),
                        can: list(t.get("can")),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(entries)
}

fn inside(path: &Path, dir: &Path) -> bool {
    let abs = |p: &Path| {
        p.canonicalize()
            .unwrap_or_else(|_| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf()))
    };
    abs(path).starts_with(abs(dir))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Equality that takes the same time wherever the inputs differ.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

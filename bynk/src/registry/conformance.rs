//! `bynk registry conformance` (first-party registry track R0, #2311): run the
//! registry protocol's observable behaviour against a registry URL, and report
//! each check.
//!
//! Servers are judged by the client's own code wherever it can be used — the
//! `config.json` and key-chain reading, line verification, archive fetching
//! and checksumming are exactly what a consumer runs. Only requests a client
//! would never make (a bad token, a mismatched URL, a hostile archive, an
//! oversized declared length) go through [`transport::request`] directly.
//!
//! A full run publishes one throwaway package, `<org>/conformance_<utc>_<hex>`,
//! and yanks every version it published: indexes are append-only, so that
//! package stays behind, yanked, and the report names it (decision B).
//! `--read-only` publishes nothing.

use std::path::{Path, PathBuf};

use bynk_project::manifest::RegistryConfig;

use super::protocol::IndexLine;
use super::transport::{self, Auth, Body};
use super::{Client, archive};

/// What to run against.
#[derive(Debug, Clone)]
pub struct Options {
    /// The registry's index URL.
    pub index: String,
    /// The organisation the token may publish and yank under.
    pub org: String,
    /// The registry name the token is found under (`BYNK_REGISTRY_<NAME>_TOKEN`).
    pub name: String,
    /// A key to pin, `ed25519:…`; trusted on first use for the run otherwise.
    pub key: Option<String>,
    /// Publish nothing: check `config.json` and an existing package only.
    pub read_only: bool,
    /// The existing `org/package` a `--read-only` run verifies.
    pub package: Option<String>,
    /// A token that can't act for `org`, for the `auth.scope` check.
    pub narrow_token: Option<String>,
    /// The upload cap to test `413` against.
    pub max_upload: u64,
}

/// A check's outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Pass,
    Fail,
    Skip,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Pass => "pass",
            Outcome::Fail => "fail",
            Outcome::Skip => "skip",
        }
    }
}

/// One check's result.
#[derive(Debug, Clone)]
pub struct Check {
    /// A stable id, `group.name`.
    pub id: &'static str,
    pub outcome: Outcome,
    pub detail: String,
    /// The protocol spec section the check holds a registry to.
    pub spec: &'static str,
}

/// A run's results.
#[derive(Debug, Clone, Default)]
pub struct Report {
    /// The registry's declared `id`, once `config.json` was read.
    pub registry_id: Option<String>,
    /// The throwaway package a full run published (and yanked).
    pub published: Option<String>,
    pub checks: Vec<Check>,
}

impl Report {
    pub fn failed(&self) -> usize {
        self.count(Outcome::Fail)
    }

    pub fn count(&self, outcome: Outcome) -> usize {
        self.checks.iter().filter(|c| c.outcome == outcome).count()
    }

    /// The report as JSON: the registry, the package, and each check.
    pub fn render_json(&self) -> String {
        let checks: Vec<serde_json::Value> = self
            .checks
            .iter()
            .map(|c| {
                serde_json::json!({
                    "id": c.id,
                    "outcome": c.outcome.as_str(),
                    "detail": c.detail,
                    "spec": c.spec,
                })
            })
            .collect();
        let doc = serde_json::json!({
            "registry": self.registry_id,
            "published": self.published,
            "passed": self.count(Outcome::Pass),
            "failed": self.count(Outcome::Fail),
            "skipped": self.count(Outcome::Skip),
            "checks": checks,
        });
        serde_json::to_string_pretty(&doc).expect("a JSON value renders") + "\n"
    }
}

const S_CONFIG: &str = "/docs/registry-protocol/#configjson";
const S_TRUST: &str = "/docs/registry-protocol/#trust";
const S_INDEX: &str = "/docs/registry-protocol/#index-files";
const S_ARCHIVES: &str = "/docs/registry-protocol/#archives";
const S_PUBLISH: &str = "/docs/registry-protocol/#publish-api";

struct Run<'o> {
    opts: &'o Options,
    report: Report,
    work: PathBuf,
}

impl Run<'_> {
    fn record(&mut self, id: &'static str, spec: &'static str, result: Result<String, String>) {
        let (outcome, detail) = match result {
            Ok(d) => (Outcome::Pass, d),
            Err(d) => (Outcome::Fail, d),
        };
        self.report.checks.push(Check {
            id,
            outcome,
            detail,
            spec,
        });
    }

    fn skip(&mut self, id: &'static str, spec: &'static str, why: &str) {
        self.report.checks.push(Check {
            id,
            outcome: Outcome::Skip,
            detail: why.to_string(),
            spec,
        });
    }
}

/// Run the suite.
pub fn run(opts: &Options) -> Report {
    let work = scratch_dir();
    let mut run = Run {
        opts,
        report: Report::default(),
        work: work.clone(),
    };
    checks(&mut run);
    let _ = std::fs::remove_dir_all(&work);
    run.report
}

fn client(opts: &Options) -> Client {
    Client::new(
        vec![RegistryConfig {
            name: opts.name.clone(),
            index: opts.index.clone(),
            organisations: vec![opts.org.clone()],
            mirror_of: None,
            key: opts.key.clone(),
        }],
        None,
    )
}

fn checks(run: &mut Run<'_>) {
    let opts = run.opts;
    // config.*: the client's own reading of `config.json` and its chain.
    let mut reader = client(opts);
    let token = super::token_for(&opts.name, &mut Vec::new());
    let api = match reader.open(&opts.name) {
        Ok(o) => {
            run.report.registry_id = Some(o.id.clone());
            let id = o.id.clone();
            let keys = o.keys.len();
            let api = o.api.clone();
            run.record(
                "config.shape",
                S_CONFIG,
                Ok(format!(
                    "protocol 1, id `{id}`, dl, auth{}",
                    if api.is_some() { ", api" } else { "" }
                )),
            );
            run.record(
                "config.chain",
                S_TRUST,
                Ok(format!(
                    "{keys} key{} trusted, each link verified",
                    if keys == 1 { "" } else { "s" }
                )),
            );
            api
        }
        Err(e) => {
            let chain = e.code == super::KEY_MISMATCH || e.code == super::BAD_SIGNATURE;
            if chain {
                run.record("config.shape", S_CONFIG, Ok("`config.json` parses".into()));
                run.record("config.chain", S_TRUST, Err(e.to_string()));
            } else {
                run.record("config.shape", S_CONFIG, Err(e.to_string()));
                run.skip("config.chain", S_TRUST, "`config.json` couldn't be read");
            }
            return;
        }
    };

    if opts.read_only {
        read_only(run, &mut reader);
        return;
    }
    let Some(api) = api else {
        run.record(
            "publish.api",
            S_PUBLISH,
            Err("`config.json` has no `api`, so the registry takes no publishes over HTTP — run with `--read-only`".into()),
        );
        return;
    };
    let Some(token) = token else {
        run.record(
            "publish.token",
            S_PUBLISH,
            Err(format!(
                "no publish token: set `{}`",
                bynk_project::manifest::registry_token_var(&opts.name)
            )),
        );
        return;
    };
    publishing(run, &mut reader, &api, &token);
}

/// `--read-only`: the lines of an existing package verify, and its newest
/// version's archive is the signed tree.
fn read_only(run: &mut Run<'_>, reader: &mut Client) {
    let name = run.opts.name.as_str();
    let Some(package) = run.opts.package.clone() else {
        run.skip("read.lines", S_INDEX, "no `--package` given to read back");
        run.skip("read.archive", S_ARCHIVES, "no `--package` given");
        return;
    };
    match reader.lines(name, &package) {
        Ok(lines) if !lines.is_empty() => {
            run.record(
                "read.lines",
                S_INDEX,
                Ok(format!("{} line(s) of `{package}` verify", lines.len())),
            );
            let last = lines.last().expect("non-empty").clone();
            let home = run.work.join("home");
            let result = reader
                .fetch(name, &last, &home)
                .map(|_| format!("{} {} is the signed tree", package, last.version))
                .map_err(|e| e.to_string());
            run.record("read.archive", S_ARCHIVES, result);
        }
        Ok(_) => {
            run.record(
                "read.lines",
                S_INDEX,
                Err(format!("`{package}` has no lines")),
            );
            run.skip("read.archive", S_ARCHIVES, "no line to fetch");
        }
        Err(e) => {
            run.record("read.lines", S_INDEX, Err(e.to_string()));
            run.skip("read.archive", S_ARCHIVES, "the index didn't verify");
        }
    }
}

/// A fixture package `name` at `version`, its commons returning `n`, with
/// extra manifest text; packed. `(archive, checksum)`.
fn fixture(
    work: &Path,
    name: &str,
    version: &str,
    n: u32,
    extra: &str,
) -> Result<(Vec<u8>, String), String> {
    let dir = work.join(format!("{}-{version}-{n}", name.replace('/', "_")));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).map_err(|e| e.to_string())?;
    std::fs::write(
        dir.join("bynk.toml"),
        format!("[package]\nname = \"{name}\"\nversion = \"{version}\"\n{extra}"),
    )
    .map_err(|e| e.to_string())?;
    std::fs::write(
        dir.join("src/probe.bynk"),
        format!("commons probe\n\nfn value() -> Int {{\n  {n}\n}}\n"),
    )
    .map_err(|e| e.to_string())?;
    archive::pack(&dir).map_err(|e| format!("could not pack a fixture: {e}"))
}

/// The refusals seen, for the `errors.shape` check: `(check, status, content
/// type, body, expected code)`.
type Seen = Vec<(&'static str, u16, Option<String>, Vec<u8>, &'static str)>;

fn publishing(run: &mut Run<'_>, reader: &mut Client, api: &str, token: &str) {
    let opts = run.opts;
    let name = opts.name.as_str();
    let org = opts.org.clone();
    let package = format!("{org}/conformance_{}_{}", utc_stamp(), hex4());
    run.report.published = Some(package.clone());
    let url = |rest: &str| transport::join(api, rest);
    let as_token = Auth::Bearer(token);
    let mut seen: Seen = Vec::new();

    let Ok((first, first_sum)) = fixture(&run.work, &package, "0.0.1", 1, "") else {
        run.record(
            "publish.created",
            S_PUBLISH,
            Err("could not build the fixture package".into()),
        );
        return;
    };

    // publish.created
    let created = transport::request(
        "PUT",
        &url(&format!("{package}/0.0.1")),
        Body::Bytes(&first),
        Auth::Bearer(token),
    );
    let created_line = match &created {
        Ok((201, _, body)) => match parse_line(body) {
            Ok(line) => {
                run.record(
                    "publish.created",
                    S_PUBLISH,
                    Ok(format!("201 with the signed line for {package} 0.0.1")),
                );
                Some(line)
            }
            Err(e) => {
                run.record(
                    "publish.created",
                    S_PUBLISH,
                    Err(format!("201, but the body isn't an index line: {e}")),
                );
                None
            }
        },
        Ok((status, _, body)) => {
            run.record(
                "publish.created",
                S_PUBLISH,
                Err(format!("expected 201, got {status}: {}", snippet(body))),
            );
            None
        }
        Err(e) => {
            run.record("publish.created", S_PUBLISH, Err(e.clone()));
            None
        }
    };
    if created_line.is_none() {
        run.skip("publish.read-back", S_INDEX, "nothing was published");
        return;
    }

    // publish.read-back: through the verifying client.
    reader.forget(name, &package);
    let read_back = reader
        .lines(name, &package)
        .map_err(|e| e.to_string())
        .and_then(|lines| {
            let line = lines
                .iter()
                .find(|l| l.version == "0.0.1")
                .cloned()
                .ok_or("the index doesn't list 0.0.1")?;
            if line.checksum != first_sum {
                return Err(format!(
                    "the line's checksum is {}, but the tree sent is {first_sum}",
                    line.checksum
                ));
            }
            reader
                .fetch(name, &line, &run.work.join("home"))
                .map_err(|e| e.to_string())?;
            Ok("the line verifies, its checksum is the tree sent, and the archive matches".into())
        });
    run.record("publish.read-back", S_INDEX, read_back);
    let index_url = transport::join(&reader_index(reader, name), &package);
    let index_before = transport::request("GET", &index_url, Body::None, Auth::Without)
        .map(|(_, _, b)| b)
        .unwrap_or_default();

    // publish.retry-same-tree (D10)
    let retry = transport::request(
        "PUT",
        &url(&format!("{package}/0.0.1")),
        Body::Bytes(&first),
        Auth::Bearer(token),
    );
    let result = match retry {
        Ok((200, _, body)) => match (parse_line(&body), &created_line) {
            (Ok(line), Some(orig)) if line == *orig => Ok("200 with the existing line".to_string()),
            (Ok(_), _) => Err("200, but not with the existing line".to_string()),
            (Err(e), _) => Err(format!("200, but the body isn't an index line: {e}")),
        },
        Ok((status, _, body)) => Err(format!(
            "expected 200 with the existing line, got {status}: {}",
            snippet(&body)
        )),
        Err(e) => Err(e),
    };
    run.record("publish.retry-same-tree", S_PUBLISH, result);

    // publish.conflict
    let conflict = fixture(&run.work, &package, "0.0.1", 2, "").and_then(|(other, _)| {
        transport::request(
            "PUT",
            &url(&format!("{package}/0.0.1")),
            Body::Bytes(&other),
            Auth::Bearer(token),
        )
    });
    let after = transport::request("GET", &index_url, Body::None, Auth::Without)
        .map(|(_, _, b)| b)
        .unwrap_or_default();
    let result = expect_refusal(
        &mut seen,
        "publish.conflict",
        conflict,
        409,
        "version_exists",
    )
    .and_then(|d| {
        if after == index_before {
            Ok(format!("{d}; the index is unchanged"))
        } else {
            Err("409, but the index changed".to_string())
        }
    });
    run.record("publish.conflict", S_PUBLISH, result);

    // refuse.*: each a 400 that leaves no line behind.
    let other_name = format!("{package}x");
    let junk_traversal = {
        let mut evil = b"bynk-tree-v1\n../escape.txt\x005\x00hello".to_vec();
        evil.extend_from_slice(b"bynk.toml\x000\x00");
        evil
    };
    let path_dep = fixture(
        &run.work,
        &package,
        "0.0.7",
        7,
        &format!(
            "\n[dependencies]\nhelper = {{ package = \"{org}/helper\", path = \"../helper\" }}\n"
        ),
    );
    let refusals: Vec<(&'static str, String, Result<Vec<u8>, String>)> = vec![
        (
            "refuse.other-package",
            format!("{other_name}/0.0.1"),
            Ok(first.clone()),
        ),
        (
            "refuse.other-version",
            format!("{package}/0.0.4"),
            Ok(first.clone()),
        ),
        (
            "refuse.junk",
            format!("{package}/0.0.5"),
            Ok(b"not an archive".to_vec()),
        ),
        (
            "refuse.traversal",
            format!("{package}/0.0.6"),
            Ok(junk_traversal),
        ),
        (
            "refuse.path-dependency",
            format!("{package}/0.0.7"),
            path_dep.map(|(b, _)| b),
        ),
    ];
    for (id, path, body) in refusals {
        let result = match body {
            Ok(body) => {
                let sent =
                    transport::request("PUT", &url(&path), Body::Bytes(&body), Auth::Bearer(token));
                expect_refusal(&mut seen, id, sent, 400, "invalid_package")
            }
            Err(e) => Err(format!("could not build the upload: {e}")),
        };
        run.record(id, S_PUBLISH, result);
    }
    reader.forget(name, &package);
    let stray = reader
        .lines(name, &package)
        .map(|lines| {
            lines
                .iter()
                .filter(|l| l.version != "0.0.1")
                .map(|l| l.version.clone())
                .collect::<Vec<_>>()
        })
        .map_err(|e| e.to_string());
    let clean = match stray {
        Ok(v) if v.is_empty() => Ok("no refused upload left a line".to_string()),
        Ok(v) => Err(format!("refused uploads left lines: {}", v.join(", "))),
        Err(e) => Err(e),
    };
    run.record("refuse.clean", S_PUBLISH, clean);
    if reader.lines(name, &other_name).is_ok_and(|l| !l.is_empty()) {
        run.record(
            "refuse.other-package-clean",
            S_PUBLISH,
            Err(format!("`{other_name}` gained a line")),
        );
    }

    // auth.*
    let auth_url = url(&format!("{package}/0.0.8"));
    let sent = transport::request("PUT", &auth_url, Body::Bytes(&first), Auth::Without);
    let r = expect_refusal(&mut seen, "auth.missing", sent, 401, "unauthorised");
    run.record("auth.missing", S_PUBLISH, r);
    let bogus = format!("bynk_{}{}", hex4(), "0".repeat(60));
    let sent = transport::request("PUT", &auth_url, Body::Bytes(&first), Auth::Bearer(&bogus));
    let r = expect_refusal(&mut seen, "auth.unknown", sent, 401, "unauthorised");
    run.record("auth.unknown", S_PUBLISH, r);
    match &opts.narrow_token {
        Some(narrow) => {
            let sent =
                transport::request("PUT", &auth_url, Body::Bytes(&first), Auth::Bearer(narrow));
            let r = expect_refusal(&mut seen, "auth.scope", sent, 403, "forbidden");
            run.record("auth.scope", S_PUBLISH, r);
        }
        None => run.skip(
            "auth.scope",
            S_PUBLISH,
            "no narrow token given (`--narrow-token-env`)",
        ),
    }

    // limits.too-large
    let sent = transport::request(
        "PUT",
        &url(&format!("{package}/0.0.9")),
        Body::Zeros(opts.max_upload + 1),
        as_token,
    );
    let r = expect_refusal(&mut seen, "limits.too-large", sent, 413, "too_large");
    run.record("limits.too-large", S_PUBLISH, r);

    // consume: a fresh client and store select and admit the version.
    let consumer_home = run.work.join("consumer");
    let mut consumer = client(opts);
    let consumed = consumer
        .lines(name, &package)
        .map_err(|e| e.to_string())
        .and_then(|lines| {
            let v = semver::Version::parse(env!("CARGO_PKG_VERSION")).expect("our own version");
            let line = super::select::choose(&lines, &["^0.0.1".to_string()], None, &v)
                .map_err(|_| "no version could be chosen".to_string())?
                .clone();
            consumer
                .fetch(name, &line, &consumer_home)
                .map_err(|e| e.to_string())?;
            Ok(format!(
                "a fresh client chose {} and admitted its tree",
                line.version
            ))
        });
    run.record("consume.resolve", S_INDEX, consumed);

    // yank.*
    let yank = |verb: &str| {
        transport::request(
            "POST",
            &url(&format!("{package}/0.0.1/{verb}")),
            Body::None,
            Auth::Bearer(token),
        )
    };
    let mut flip = |run: &mut Run<'_>, id: &'static str, verb: &str, want: bool| {
        let result = match yank(verb) {
            Ok((200, _, _)) => {
                reader.forget(name, &package);
                match reader.lines(name, &package) {
                    Ok(lines) => match lines.iter().find(|l| l.version == "0.0.1") {
                        Some(l) if l.yanked == want => {
                            Ok(format!("200, and the index shows yanked = {want}"))
                        }
                        Some(_) => Err(format!("200, but the index doesn't show yanked = {want}")),
                        None => Err("the line disappeared".to_string()),
                    },
                    Err(e) => Err(e.to_string()),
                }
            }
            Ok((status, _, body)) => Err(format!("expected 200, got {status}: {}", snippet(&body))),
            Err(e) => Err(e),
        };
        run.record(id, S_PUBLISH, result);
    };
    flip(run, "yank.yank", "yank", true);
    flip(run, "yank.unyank", "unyank", false);
    let sent = transport::request(
        "POST",
        &url(&format!("{package}/9.9.9/yank")),
        Body::None,
        Auth::Bearer(token),
    );
    let r = expect_refusal(&mut seen, "yank.missing", sent, 404, "not_found");
    run.record("yank.missing", S_PUBLISH, r);

    // errors.shape: every refusal is JSON `{error, message}` with its code.
    let mut bad = Vec::new();
    for (id, _status, kind, body, code) in &seen {
        let json = kind
            .as_deref()
            .is_some_and(|k| k.starts_with("application/json"));
        let parsed: Option<(String, bool)> = serde_json::from_slice::<serde_json::Value>(body)
            .ok()
            .and_then(|v| {
                Some((
                    v.get("error")?.as_str()?.to_owned(),
                    v.get("message")?.is_string(),
                ))
            });
        match parsed {
            Some((c, true)) if json && c == *code => {}
            Some((c, _)) if c != *code => {
                bad.push(format!("{id}: `error` is `{c}`, expected `{code}`"))
            }
            _ => bad.push(format!("{id}: not a JSON `{{error, message}}` body")),
        }
    }
    let result = if seen.is_empty() {
        Err("no refusal was observed".to_string())
    } else if bad.is_empty() {
        Ok(format!(
            "{} refusals, each JSON with the right `error`",
            seen.len()
        ))
    } else {
        Err(bad.join("; "))
    };
    run.record("errors.shape", S_PUBLISH, result);

    // Leave the throwaway package yanked (decision B).
    let _ = yank("yank");
}

/// Check a response is the refusal `status` with `code`, recording it for
/// `errors.shape`.
fn expect_refusal(
    seen: &mut Seen,
    id: &'static str,
    sent: Result<(u16, Option<String>, Vec<u8>), String>,
    status: u16,
    code: &'static str,
) -> Result<String, String> {
    match sent {
        Ok((got, kind, body)) if got == status => {
            seen.push((id, got, kind, body, code));
            Ok(format!("{status} `{code}`"))
        }
        Ok((got, _, body)) => Err(format!(
            "expected {status} `{code}`, got {got}: {}",
            snippet(&body)
        )),
        Err(e) => Err(e),
    }
}

fn parse_line(body: &[u8]) -> Result<IndexLine, String> {
    let text = std::str::from_utf8(body).map_err(|_| "not UTF-8".to_string())?;
    IndexLine::parse(text.trim())
}

fn reader_index(reader: &mut Client, name: &str) -> String {
    reader
        .open(name)
        .map(|o| o.index.clone())
        .unwrap_or_default()
}

fn snippet(body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    let text = text.trim();
    if text.is_empty() {
        "(empty body)".to_string()
    } else if text.chars().count() > 160 {
        format!("{}…", text.chars().take(160).collect::<String>())
    } else {
        text.to_string()
    }
}

fn scratch_dir() -> PathBuf {
    loop {
        let dir = std::env::temp_dir().join(format!("bynk-conformance-{}{}", hex4(), hex4()));
        if std::fs::create_dir(&dir).is_ok() {
            return dir;
        }
    }
}

fn hex4() -> String {
    let mut raw = [0u8; 2];
    let _ = ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut raw);
    format!("{:02x}{:02x}", raw[0], raw[1])
}

/// The current UTC time as `yyyymmddhhmmss`.
fn utc_stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // Howard Hinnant's civil-from-days.
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}{m:02}{d:02}{:02}{:02}{:02}",
        rem / 3_600,
        rem % 3_600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stamp_is_fourteen_digits() {
        let s = utc_stamp();
        assert_eq!(s.len(), 14, "{s}");
        assert!(s.starts_with("20"), "{s}");
        assert!(s.chars().all(|c| c.is_ascii_digit()));
    }
}

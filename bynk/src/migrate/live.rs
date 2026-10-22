//! The live side of `bynk migrate sweep` (state-migrations S4, #2259
//! Decisions B and D): Cloudflare's REST API and the deployed Workers over
//! HTTPS, and `wrangler secret put`/`delete` for the run's key.

use std::io::Write;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use super::sweep::{Cloudflare, Namespace, Secrets, Wake};
use crate::probe::Provenance;

pub(crate) const API: &str = "https://api.cloudflare.com/client/v4";

/// The credentials the REST calls need, from the variables Wrangler itself
/// reads in CI. Wrangler's OAuth login cannot be reused here.
pub(crate) struct Credentials {
    pub token: String,
    pub account: String,
}

impl Credentials {
    /// Both variables, or a message naming whichever is missing.
    pub(crate) fn from_env() -> Result<Self, String> {
        let get = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
        match (get("CLOUDFLARE_API_TOKEN"), get("CLOUDFLARE_ACCOUNT_ID")) {
            (Some(token), Some(account)) => Ok(Credentials { token, account }),
            (token, account) => {
                let missing: Vec<&str> = [
                    token.is_none().then_some("CLOUDFLARE_API_TOKEN"),
                    account.is_none().then_some("CLOUDFLARE_ACCOUNT_ID"),
                ]
                .into_iter()
                .flatten()
                .collect();
                Err(format!(
                    "set {} to sweep: an API token with \"Workers Scripts: Edit\" (the \
                     listings need read access, and `wrangler` uses the same token to set and \
                     delete the sweep's key) and the account it belongs to",
                    missing.join(" and ")
                ))
            }
        }
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

/// Cloudflare's REST API.
pub(crate) struct Rest {
    pub agent: ureq::Agent,
    pub credentials: Credentials,
    /// [`API`], or a local stub in tests.
    pub base: String,
}

impl Rest {
    fn get(&self, path: &str) -> Result<serde_json::Value, String> {
        let url = format!("{}/accounts/{}{path}", self.base, self.credentials.account);
        let mut response = self
            .agent
            .get(&url)
            .header(
                "Authorization",
                &format!("Bearer {}", self.credentials.token),
            )
            .call()
            .map_err(|e| format!("GET {path}: {e}"))?;
        let status = response.status().as_u16();
        let text = response
            .body_mut()
            .read_to_string()
            .map_err(|e| format!("GET {path}: {e}"))?;
        let body: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| format!("GET {path}: unreadable answer ({e})"))?;
        if status != 200 || body.get("success") != Some(&serde_json::Value::Bool(true)) {
            let errors = body
                .get("errors")
                .map(serde_json::Value::to_string)
                .unwrap_or_default();
            return Err(format!("GET {path}: Cloudflare answered {status} {errors}"));
        }
        Ok(body)
    }
}

impl Cloudflare for Rest {
    fn namespaces(&self) -> Result<Vec<Namespace>, String> {
        let mut out = Vec::new();
        for page in 1.. {
            let body = self.get(&format!(
                "/workers/durable_objects/namespaces?per_page=1000&page={page}"
            ))?;
            let items = body["result"].as_array().cloned().unwrap_or_default();
            let n = items.len();
            out.extend(items.iter().filter_map(|ns| {
                Some(Namespace {
                    id: ns.get("id")?.as_str()?.to_string(),
                    script: ns.get("script")?.as_str()?.to_string(),
                    class: ns.get("class")?.as_str()?.to_string(),
                })
            }));
            let pages = body["result_info"]["total_pages"].as_u64().unwrap_or(1);
            if n == 0 || page >= pages {
                break;
            }
        }
        Ok(out)
    }

    fn stored_objects(&self, namespace: &str) -> Result<Vec<String>, String> {
        let mut out = Vec::new();
        let mut cursor = String::new();
        loop {
            let mut path =
                format!("/workers/durable_objects/namespaces/{namespace}/objects?limit=10000");
            if !cursor.is_empty() {
                path.push_str(&format!("&cursor={cursor}"));
            }
            let body = self.get(&path)?;
            for object in body["result"].as_array().cloned().unwrap_or_default() {
                if object.get("hasStoredData") == Some(&serde_json::Value::Bool(true))
                    && let Some(id) = object.get("id").and_then(serde_json::Value::as_str)
                {
                    out.push(id.to_string());
                }
            }
            cursor = body["result_info"]["cursor"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            if cursor.is_empty() {
                break;
            }
        }
        Ok(out)
    }

    fn workers_dev_url(&self, script: &str) -> Result<Option<String>, String> {
        let enabled =
            self.get(&format!("/workers/scripts/{script}/subdomain"))?["result"]["enabled"]
                .as_bool()
                .unwrap_or(false);
        if !enabled {
            return Ok(None);
        }
        let subdomain = self.get("/workers/subdomain")?["result"]["subdomain"]
            .as_str()
            .map(str::to_string)
            .ok_or("the account has no workers.dev subdomain")?;
        Ok(Some(format!("https://{script}.{subdomain}.workers.dev")))
    }

    fn last_code_deployment(&self, script: &str) -> Result<std::time::SystemTime, String> {
        // The list is paged (10 a page by default), and each sweep adds two
        // `secret` deployments, so a few sweeps push the code deployment off
        // the first page (#2279 review). Page back until one is in view.
        let mut all: Vec<serde_json::Value> = Vec::new();
        for page in 1..=DEPLOYMENT_PAGES {
            let body = self.get(&format!(
                "/workers/scripts/{script}/deployments?per_page=50&page={page}"
            ))?;
            let items = body["result"]["deployments"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let n = items.len();
            all.extend(items);
            let pages = body["result_info"]["total_pages"].as_u64().unwrap_or(1);
            if n == 0 || page >= pages || all.iter().any(|d| !is_secret_change(d)) {
                break;
            }
        }
        last_code_deployment_in(&all)
    }
}

/// How many pages (of 50) of a Worker's deployments a sweep reads back
/// looking for its last code deployment: 1,000 deployments, about 500 sweeps.
const DEPLOYMENT_PAGES: u64 = 20;

/// Whether a deployment was triggered by a secret change rather than code.
fn is_secret_change(d: &serde_json::Value) -> bool {
    d["annotations"]["workers/triggered_by"].as_str() == Some("secret")
}

/// #2276 Decision A: in a deployments list (newest first), the newest one not
/// triggered by a secret change; when every one was, the oldest listed, since
/// the code is at least that old. Any trigger other than `secret`, absent or
/// unknown included, counts as a code deployment.
pub(crate) fn last_code_deployment_in(
    deployments: &[serde_json::Value],
) -> Result<std::time::SystemTime, String> {
    let chosen = deployments
        .iter()
        .find(|d| !is_secret_change(d))
        .or_else(|| deployments.last())
        .ok_or("Cloudflare lists no deployment for it")?;
    let created = chosen["created_on"]
        .as_str()
        .ok_or("its deployment names no creation time")?;
    time::OffsetDateTime::parse(created, &time::format_description::well_known::Rfc3339)
        .map(std::time::SystemTime::from)
        .map_err(|e| format!("unreadable deployment time `{created}`: {e}"))
}

/// The signed wake, over HTTPS.
pub(crate) struct Https(pub ureq::Agent);

impl Wake for Https {
    fn post(
        &self,
        url: &str,
        headers: &[(String, String)],
        body: &str,
    ) -> Result<(u16, String), String> {
        let mut request = self.0.post(url).header("Content-Type", "application/json");
        for (k, v) in headers {
            request = request.header(k.as_str(), v.as_str());
        }
        let mut response = request.send(body).map_err(|e| e.to_string())?;
        let status = response.status().as_u16();
        let text = response
            .body_mut()
            .read_to_string()
            .map_err(|e| e.to_string())?;
        Ok((status, text))
    }
}

/// `wrangler secret put`/`delete` against a deployed Worker by name, run in
/// an empty directory so no project config is picked up.
pub(crate) struct WranglerSecrets {
    pub provenance: Provenance,
    pub scratch: PathBuf,
}

impl WranglerSecrets {
    fn run(&self, verb: &str, script: &str, name: &str, stdin: &str) -> Result<(), String> {
        let Some(mut command) = crate::workers::wrangler_command(&self.provenance, "secret") else {
            return Err("wrangler not found".into());
        };
        std::fs::create_dir_all(&self.scratch).map_err(|e| e.to_string())?;
        command
            .arg(verb)
            .arg(name)
            .arg("--name")
            .arg(script)
            .current_dir(&self.scratch)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|e| e.to_string())?;
        {
            // The value goes on stdin, never argv (ADR 0195 D1); a `delete`
            // sends nothing, so its confirmation takes Wrangler's
            // non-interactive default.
            let mut pipe = child
                .stdin
                .take()
                .ok_or("could not open wrangler's stdin")?;
            pipe.write_all(stdin.as_bytes())
                .map_err(|e| e.to_string())?;
        }
        let output = child.wait_with_output().map_err(|e| e.to_string())?;
        if output.status.success() {
            return Ok(());
        }
        let said = [
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
            String::from_utf8_lossy(&output.stdout).trim().to_string(),
        ]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
        Err(if said.is_empty() {
            format!("wrangler secret {verb} failed")
        } else {
            said
        })
    }
}

impl Secrets for WranglerSecrets {
    fn put(&self, script: &str, name: &str, value: &str) -> Result<(), String> {
        self.run("put", script, name, value)
    }

    fn delete(&self, script: &str, name: &str) -> Result<(), String> {
        self.run("delete", script, name, "")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read};

    /// A one-thread HTTP stub: answers each request by its path from
    /// `routes`, recording the paths and the `Authorization` it saw.
    fn stub(
        routes: Vec<(&'static str, String)>,
    ) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let path = line
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or_default()
                    .to_string();
                let mut auth = String::new();
                loop {
                    let mut h = String::new();
                    if reader.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
                        break;
                    }
                    if h.to_ascii_lowercase().starts_with("authorization:") {
                        auth = h.trim().to_string();
                    }
                }
                log.lock().unwrap().push(format!("{path} {auth}"));
                let body = routes
                    .iter()
                    .find(|(p, _)| path.ends_with(p))
                    .map(|(_, b)| b.clone())
                    .unwrap_or_else(|| r#"{"success":false,"errors":[{"code":7003}]}"#.into());
                let mut s = stream;
                let _ = write!(
                    s,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.read(&mut [0u8; 0]);
            }
        });
        (base, seen)
    }

    fn rest(base: String) -> Rest {
        Rest {
            agent: ureq::Agent::config_builder()
                .http_status_as_error(false)
                .build()
                .new_agent(),
            credentials: Credentials {
                token: "tok".into(),
                account: "acct".into(),
            },
            base,
        }
    }

    /// The key goes to `wrangler secret put` on stdin, never argv, against the
    /// deployed Worker by name; `delete` names the same secret; a failure
    /// carries what wrangler said.
    #[cfg(unix)]
    #[test]
    fn secrets_go_through_wrangler_by_worker_name_on_stdin() {
        use std::os::unix::fs::PermissionsExt;
        let dir =
            std::env::temp_dir().join(format!("bynk-migrate-wrangler-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("log");
        let script = dir.join("wrangler");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\necho \"$@ stdin=$(cat)\" >> {}\n[ \"$3\" = BAD ] && {{ echo nope >&2; exit 1; }}\nexit 0\n",
                log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let secrets = WranglerSecrets {
            provenance: Provenance::Path(script.clone()),
            scratch: dir.join("scratch"),
        };
        secrets
            .put("shop-staging", "BYNK_MIGRATE_KEY", "s3cret")
            .unwrap();
        secrets.delete("shop-staging", "BYNK_MIGRATE_KEY").unwrap();
        let err = secrets.put("shop", "BAD", "x").unwrap_err();
        let logged = std::fs::read_to_string(&log).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(err, "nope");
        assert_eq!(
            logged.lines().take(2).collect::<Vec<_>>(),
            vec![
                "secret put BYNK_MIGRATE_KEY --name shop-staging stdin=s3cret",
                "secret delete BYNK_MIGRATE_KEY --name shop-staging stdin=",
            ]
        );
    }

    #[test]
    fn the_last_code_deployment_skips_secret_changes() {
        use std::time::{Duration, UNIX_EPOCH};
        let d = |at: &str, by: Option<&str>| match by {
            Some(by) => {
                serde_json::json!({ "created_on": at, "annotations": { "workers/triggered_by": by } })
            }
            None => serde_json::json!({ "created_on": at }),
        };
        let at = |s: u64| UNIX_EPOCH + Duration::from_secs(s);
        // As probed live: a redeploy, then the sweep's two secret changes.
        let list = vec![
            d("2026-10-21T16:54:47.109922Z", Some("secret")),
            d("2026-10-21T16:54:43.964347Z", Some("secret")),
            d("2026-10-21T16:54:38.858364Z", Some("upload")),
        ];
        assert_eq!(
            last_code_deployment_in(&list).unwrap(),
            at(1_792_601_678) + Duration::from_nanos(858_364_000)
        );
        // An unknown or absent trigger counts as code.
        let list = vec![
            d("2026-10-21T17:00:00Z", None),
            d("2026-10-21T16:00:00Z", Some("upload")),
        ];
        assert_eq!(last_code_deployment_in(&list).unwrap(), at(1_792_602_000));
        // Only secret changes in view: the oldest listed bounds the code's age.
        let list = vec![
            d("2026-10-21T17:00:00Z", Some("secret")),
            d("2026-10-21T16:00:00Z", Some("secret")),
        ];
        assert_eq!(last_code_deployment_in(&list).unwrap(), at(1_792_598_400));
        assert!(last_code_deployment_in(&[]).is_err());
        assert!(last_code_deployment_in(&[d("yesterday", Some("upload"))]).is_err());
    }

    #[test]
    fn the_listings_follow_every_page_and_keep_stored_objects_only() {
        let (base, seen) = stub(vec![
            (
                "namespaces?per_page=1000&page=1",
                r#"{"success":true,"result":[{"id":"n1","script":"shop","class":"Cart"}],"result_info":{"total_pages":2}}"#.into(),
            ),
            (
                "namespaces?per_page=1000&page=2",
                r#"{"success":true,"result":[{"id":"n2","script":"shop-staging","class":"Cart"}],"result_info":{"total_pages":2}}"#.into(),
            ),
            (
                "n1/objects?limit=10000",
                r#"{"success":true,"result":[{"id":"a","hasStoredData":true},{"id":"b","hasStoredData":false}],"result_info":{"cursor":"c1"}}"#.into(),
            ),
            (
                "n1/objects?limit=10000&cursor=c1",
                r#"{"success":true,"result":[{"id":"c","hasStoredData":true}],"result_info":{"cursor":""}}"#.into(),
            ),
            (
                "/workers/scripts/shop/subdomain",
                r#"{"success":true,"result":{"enabled":true}}"#.into(),
            ),
            (
                "/workers/scripts/shop-staging/subdomain",
                r#"{"success":true,"result":{"enabled":false}}"#.into(),
            ),
            (
                // Page 1 holds only a sweep's secret changes; the code
                // deployment is on page 2, as on the scratch account after
                // seven sweeps (#2279 review).
                "/workers/scripts/shop/deployments?per_page=50&page=1",
                r#"{"success":true,"result":{"deployments":[{"created_on":"2026-10-21T17:00:00Z","annotations":{"workers/triggered_by":"secret"}}]},"result_info":{"total_pages":2}}"#.into(),
            ),
            (
                "/workers/scripts/shop/deployments?per_page=50&page=2",
                r#"{"success":true,"result":{"deployments":[{"created_on":"2026-10-21T16:00:00Z","annotations":{"workers/triggered_by":"deployment"}}]},"result_info":{"total_pages":2}}"#.into(),
            ),
            (
                "/workers/subdomain",
                r#"{"success":true,"result":{"subdomain":"team"}}"#.into(),
            ),
        ]);
        let api = rest(base);
        let namespaces = api.namespaces().unwrap();
        assert_eq!(
            namespaces.iter().map(|n| n.id.as_str()).collect::<Vec<_>>(),
            vec!["n1", "n2"]
        );
        assert_eq!(api.stored_objects("n1").unwrap(), vec!["a", "c"]);
        assert_eq!(
            api.workers_dev_url("shop").unwrap().as_deref(),
            Some("https://shop.team.workers.dev")
        );
        assert_eq!(api.workers_dev_url("shop-staging").unwrap(), None);
        assert_eq!(
            api.last_code_deployment("shop").unwrap(),
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_792_598_400)
        );
        let err = api.stored_objects("missing").unwrap_err();
        assert!(err.contains("7003"), "{err}");
        let seen = seen.lock().unwrap();
        assert!(
            seen[0].starts_with(
                "/accounts/acct/workers/durable_objects/namespaces?per_page=1000&page=1"
            )
        );
        assert!(seen.iter().all(|s| s.ends_with("Bearer tok")), "{seen:?}");
    }
}

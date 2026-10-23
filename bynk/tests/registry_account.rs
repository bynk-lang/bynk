//! The account client (first-party registry R3.1, #2378), over the real
//! binary: `bynk login`, `bynk logout` and `bynk org` against a fake of the
//! hosted registry's account API on a loopback port. No test touches the
//! network. Each test has its own `BYNK_HOME` and `XDG_CONFIG_HOME`.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_bynk"))
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("registry-account")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("project")).unwrap();
    dir
}

/// `bynk <args>` in `root/project`, with `root/home` and `root/config`.
fn bynk(root: &Path, args: &[&str], env: &[(&str, &str)]) -> (i32, String, String) {
    let mut cmd = Command::new(bin());
    cmd.args(args)
        .current_dir(root.join("project"))
        .env("BYNK_HOME", root.join("home"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env_remove("BYNK_ACCOUNT_ACME_INTERNAL_TOKEN")
        .env_remove("BYNK_REGISTRY_ACME_INTERNAL_TOKEN")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("spawn");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn credentials(root: &Path) -> String {
    std::fs::read_to_string(root.join("config/bynk/credentials.toml")).unwrap_or_default()
}

/// One request the fake saw: method, path, bearer token, JSON body.
type Seen = Arc<Mutex<Vec<(String, String, Option<String>, Value)>>>;

/// The fake's organisation `acme`.
#[derive(Default)]
struct Org {
    claimed: bool,
    blocked: bool,
    members: BTreeMap<i64, String>,
    tokens: Vec<Value>,
}

const ACCOUNT: &str = "bynka_test";
const TOKEN_ID: &str = "7f3a0c1d9e2b4a5f6c7d8e9f0a1b2c3d4e5f60718293a4b5c6d7e8f9a0b1c2d3";

/// A registry at `root/registry` with an `api`, served with a fake account
/// API on a loopback port; answers the port and what the fake saw.
fn serve(root: &Path) -> (u16, Seen) {
    let dir = root.join("registry");
    let (code, _, err) = bynk(
        root,
        &[
            "registry",
            "init",
            dir.to_str().unwrap(),
            "--id",
            "acme",
            "--key-out",
            root.join("acme.key").to_str().unwrap(),
        ],
        &[],
    );
    assert_eq!(code, 0, "{err}");
    let config = std::fs::read_to_string(dir.join("config.json")).unwrap();
    std::fs::write(
        dir.join("config.json"),
        config.replacen('{', "{\"api\":\"api\",", 1),
    )
    .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen: Seen = Arc::default();
    let log = seen.clone();
    std::thread::spawn(move || {
        let mut org = Org::default();
        let mut polls = 0;
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() {
                continue;
            }
            let mut parts = line.split_whitespace();
            let method = parts.next().unwrap_or_default().to_string();
            let path = parts.next().unwrap_or("/").to_string();
            let (mut length, mut bearer) = (0usize, None);
            loop {
                let mut h = String::new();
                if reader.read_line(&mut h).is_err() || h.trim().is_empty() {
                    break;
                }
                let lower = h.to_ascii_lowercase();
                if let Some(v) = lower.strip_prefix("content-length:") {
                    length = v.trim().parse().unwrap_or(0);
                }
                if lower.starts_with("authorization:") {
                    bearer = h.split_once("Bearer ").map(|(_, t)| t.trim().to_string());
                }
            }
            let mut body = vec![0; length];
            let _ = reader.read_exact(&mut body);
            let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            log.lock()
                .unwrap()
                .push((method.clone(), path.clone(), bearer.clone(), body.clone()));
            let (status, answer) = if !path.starts_with("/account/") {
                match std::fs::read(dir.join(path.trim_start_matches('/'))) {
                    Ok(b) => (200, String::from_utf8(b).unwrap()),
                    Err(_) => (404, String::new()),
                }
            } else {
                let (s, v) = account(&mut org, &mut polls, &method, &path, bearer, &body);
                (s, v.to_string())
            };
            let mut out = stream;
            let _ = write!(
                out,
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}",
                answer.len()
            );
        }
    });
    (port, seen)
}

fn problem(code: &str) -> Value {
    json!({ "error": code, "message": format!("the fake says {code}") })
}

/// The fake account API: the first poll is pending, the second signs ann in.
fn account(
    org: &mut Org,
    polls: &mut u32,
    method: &str,
    path: &str,
    bearer: Option<String>,
    body: &Value,
) -> (u16, Value) {
    let rest = path.trim_start_matches("/account/");
    match (method, rest) {
        ("POST", "login") => {
            return (
                200,
                json!({ "poll": "p1", "userCode": "WDJB-MJHT", "verificationUri": "http://localhost/device", "interval": 0, "expiresIn": 900 }),
            );
        }
        ("POST", "login/poll") => {
            *polls += 1;
            return if *polls == 1 {
                (400, problem("authorization_pending"))
            } else {
                (
                    200,
                    json!({ "token": ACCOUNT, "login": "ann", "expiresAt": 4_102_444_800_000_i64 }),
                )
            };
        }
        _ => {}
    }
    if bearer.as_deref() != Some(ACCOUNT) {
        return (401, problem("unauthorised"));
    }
    let view = |org: &Org| {
        json!({
            "name": "acme",
            "owner": { "id": 1, "login": "ann" },
            "members": org.members.iter().map(|(id, l)| json!({ "id": id, "login": l })).collect::<Vec<_>>(),
            "blocked": org.blocked,
            "tokens": org.tokens,
        })
    };
    let ok = json!({ "ok": true });
    match (method, rest) {
        ("DELETE", "session") => (204, Value::Null),
        ("POST", "orgs/acme") => {
            if org.claimed {
                return (
                    200,
                    json!({ "org": "acme", "owner": "ann", "blocked": org.blocked }),
                );
            }
            org.claimed = true;
            org.blocked = body["block"] == json!(true);
            org.members.insert(1, "ann".into());
            (
                201,
                json!({ "org": "acme", "owner": "ann", "blocked": org.blocked }),
            )
        }
        ("POST", "orgs/npm") => (403, problem("reserved")),
        ("GET", "orgs/acme") => (200, view(org)),
        ("PUT", "orgs/acme/blocked") => {
            org.blocked = body["blocked"] == json!(true);
            (200, ok)
        }
        ("PUT", "orgs/acme/members/bob") => {
            org.members.insert(2, "bob".into());
            (200, ok)
        }
        ("DELETE", "orgs/acme/members/2") => {
            org.members.remove(&2);
            (200, ok)
        }
        ("POST", "orgs/acme/tokens") => {
            let info = json!({ "id": TOKEN_ID, "can": body["can"], "expiresAt": 4_102_444_800_000_i64, "note": body["note"], "mintedBy": 1 });
            org.tokens.push(info.clone());
            (201, json!({ "token": "bynk_minted", "info": info }))
        }
        ("DELETE", p) if p == format!("orgs/acme/tokens/{TOKEN_ID}") => {
            org.tokens.clear();
            (200, ok)
        }
        _ => (404, problem("not_found")),
    }
}

/// A project routing `acme` to the fake (and, with `mirror`, reading it
/// through a mirror).
fn project(root: &Path, port: u16, mirror: bool) {
    let index = format!("http://localhost:{port}");
    let mut registries =
        format!("[registries.acme-internal]\nindex = \"{index}\"\norganisations = [\"acme\"]\n");
    if mirror {
        registries.push_str(&format!(
            "\n[registries.near]\nindex = \"{index}\"\nmirror-of = \"acme-internal\"\n"
        ));
    }
    let dir = root.join("project");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("bynk.toml"),
        format!("[project]\nname = \"app\"\n\n{registries}"),
    )
    .unwrap();
}

fn login(root: &Path) {
    let (code, _, err) = bynk(root, &["login", "--registry", "acme-internal"], &[]);
    assert_eq!(code, 0, "{err}");
    assert!(err.contains("enter the code WDJB-MJHT"), "{err}");
    assert!(
        err.contains("Never enter a code you didn't request"),
        "{err}"
    );
    assert!(err.contains("as ann"), "{err}");
}

#[test]
fn login_keeps_the_session_apart_and_logout_forgets_it() {
    let root = scratch("login");
    let (port, seen) = serve(&root);
    project(&root, port, false);
    login(&root);
    let saved = credentials(&root);
    assert!(
        saved.contains("[accounts.acme-internal]\ntoken = \"bynka_test\"\nlogin = \"ann\""),
        "{saved}"
    );
    assert!(!saved.contains("[registries"), "{saved}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(root.join("config/bynk/credentials.toml"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    // It polled through `authorization_pending`, sending no token.
    let polls: Vec<_> = seen
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, p, _, _)| p == "/account/login/poll")
        .map(|(_, _, bearer, body)| (bearer.clone(), body.clone()))
        .collect();
    assert_eq!(polls, vec![(None, json!({ "poll": "p1" })); 2]);

    let (code, _, err) = bynk(&root, &["logout", "--registry", "acme-internal"], &[]);
    assert_eq!(code, 0, "{err}");
    assert!(
        !credentials(&root).contains("accounts"),
        "{}",
        credentials(&root)
    );
    let last = seen.lock().unwrap().last().cloned().unwrap();
    assert_eq!(
        (last.0.as_str(), last.1.as_str(), last.2.as_deref()),
        ("DELETE", "/account/session", Some(ACCOUNT))
    );
    // Signed out, an `org` command says how to sign in.
    let (code, _, err) = bynk(&root, &["org", "show", "acme"], &[]);
    assert_eq!(code, 1);
    assert!(
        err.contains("bynk.registry.account_refused")
            && err.contains("bynk login --registry acme-internal"),
        "{err}"
    );
}

#[test]
fn org_commands_route_by_organisation_and_drive_the_api() {
    let root = scratch("org");
    let (port, seen) = serve(&root);
    project(&root, port, false);
    login(&root);

    let (code, _, err) = bynk(&root, &["org", "claim", "acme"], &[]);
    assert_eq!(code, 0, "{err}");
    assert!(err.contains("claimed `acme` on `acme-internal`"), "{err}");
    let (code, _, err) = bynk(&root, &["org", "claim", "acme"], &[]);
    assert_eq!(code, 0, "{err}");
    assert!(err.contains("already yours"), "{err}");
    // A refusal carries the API's code.
    let (code, _, err) = bynk(
        &root,
        &["org", "claim", "npm", "--registry", "acme-internal"],
        &[],
    );
    assert_eq!(code, 1);
    assert!(
        err.contains("bynk.registry.account_refused") && err.contains("`reserved`"),
        "{err}"
    );

    let (code, _, err) = bynk(&root, &["org", "member", "add", "acme", "bob"], &[]);
    assert_eq!(code, 0, "{err}");
    let (code, _, err) = bynk(&root, &["org", "block", "acme"], &[]);
    assert_eq!(code, 0, "{err}");
    let (code, out, err) = bynk(&root, &["org", "show", "acme"], &[]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("members: ann (#1), bob (#2)"), "{out}");
    assert!(out.contains("blocked: yes"), "{out}");
    let (code, out, err) = bynk(&root, &["org", "show", "acme", "--format", "json"], &[]);
    assert_eq!(code, 0, "{err}");
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["blocked"], json!(true));
    let (code, _, err) = bynk(&root, &["org", "unblock", "acme"], &[]);
    assert_eq!(code, 0, "{err}");

    // Removing by login removes by id.
    let (code, _, err) = bynk(&root, &["org", "member", "remove", "acme", "bob"], &[]);
    assert_eq!(code, 0, "{err}");
    assert!(
        seen.lock()
            .unwrap()
            .iter()
            .any(|(m, p, _, _)| m == "DELETE" && p == "/account/orgs/acme/members/2")
    );

    // Minted without `--save`: alone on stdout, and nothing saved.
    let (code, out, err) = bynk(
        &root,
        &[
            "org", "token", "mint", "acme", "--can", "publish", "--days", "7", "--note", "ci",
        ],
        &[],
    );
    assert_eq!(code, 0, "{err}");
    assert_eq!(out, "bynk_minted\n");
    assert!(err.contains("minted token 7f3a0c1d9e2b"), "{err}");
    assert!(!credentials(&root).contains("[registries"));
    let mint = seen
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find(|(m, p, _, _)| m == "POST" && p == "/account/orgs/acme/tokens")
        .map(|(_, _, _, b)| b.clone())
        .unwrap();
    assert_eq!(mint, json!({ "can": ["publish"], "days": 7, "note": "ci" }));

    // Revoked by the prefix `org show` prints.
    let (code, _, err) = bynk(
        &root,
        &["org", "token", "revoke", "acme", "7f3a0c1d9e2b"],
        &[],
    );
    assert_eq!(code, 0, "{err}");
    assert!(
        seen.lock()
            .unwrap()
            .iter()
            .any(|(m, p, _, _)| m == "DELETE"
                && p == &format!("/account/orgs/acme/tokens/{TOKEN_ID}"))
    );

    // The account token went only to the account API, never the index.
    for (_, path, bearer, _) in seen.lock().unwrap().iter() {
        if bearer.is_some() {
            assert!(path.starts_with("/account/"), "{path}");
        }
    }
}

#[test]
fn a_minted_token_is_saved_where_publish_reads_it_but_never_over_another() {
    let root = scratch("save");
    let (port, _) = serve(&root);
    project(&root, port, false);
    login(&root);
    let mint = ["org", "token", "mint", "acme", "--save"];
    let (code, out, err) = bynk(&root, &mint, &[]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(out, "", "the token isn't printed when saved");
    let saved = credentials(&root);
    assert!(
        saved.contains("[registries.acme-internal]\ntoken = \"bynk_minted\""),
        "{saved}"
    );
    assert!(saved.contains("[accounts.acme-internal]"), "{saved}");

    // A different token in the slot is kept, and nothing is minted.
    std::fs::write(
        root.join("config/bynk/credentials.toml"),
        saved.replace("bynk_minted", "bynk_other"),
    )
    .unwrap();
    let (code, _, err) = bynk(&root, &mint, &[]);
    assert_eq!(code, 1);
    assert!(err.contains("--force"), "{err}");
    assert!(credentials(&root).contains("bynk_other"));
    let (code, _, err) = bynk(
        &root,
        &["org", "token", "mint", "acme", "--save", "--force"],
        &[("BYNK_REGISTRY_ACME_INTERNAL_TOKEN", "bynk_env")],
    );
    assert_eq!(code, 0, "{err}");
    assert!(credentials(&root).contains("token = \"bynk_minted\""));
    assert!(
        err.contains("`BYNK_REGISTRY_ACME_INTERNAL_TOKEN` is set"),
        "{err}"
    );
}

#[test]
fn the_environment_supplies_an_account_token() {
    let root = scratch("env");
    let (port, seen) = serve(&root);
    project(&root, port, false);
    let (code, _, err) = bynk(
        &root,
        &["org", "claim", "acme"],
        &[("BYNK_ACCOUNT_ACME_INTERNAL_TOKEN", ACCOUNT)],
    );
    assert_eq!(code, 0, "{err}");
    assert!(
        seen.lock()
            .unwrap()
            .iter()
            .any(|(_, p, b, _)| p == "/account/orgs/acme" && b.as_deref() == Some(ACCOUNT))
    );
    // A wrong one is refused with the sign-in hint.
    let (code, _, err) = bynk(
        &root,
        &["org", "show", "acme"],
        &[("BYNK_ACCOUNT_ACME_INTERNAL_TOKEN", "bynka_wrong")],
    );
    assert_eq!(code, 1);
    assert!(
        err.contains("HTTP 401") && err.contains("bynk login"),
        "{err}"
    );
}

#[test]
fn an_account_token_never_goes_through_a_mirror() {
    let root = scratch("mirror");
    let (port, seen) = serve(&root);
    project(&root, port, true);
    let (code, _, err) = bynk(
        &root,
        &["org", "show", "acme"],
        &[("BYNK_ACCOUNT_ACME_INTERNAL_TOKEN", ACCOUNT)],
    );
    assert_eq!(code, 1);
    assert!(err.contains("through the mirror `near`"), "{err}");
    assert!(
        seen.lock()
            .unwrap()
            .iter()
            .all(|(_, p, b, _)| !p.starts_with("/account/") && b.is_none())
    );
}

#[test]
fn a_registry_without_an_api_has_no_accounts() {
    let root = scratch("no-api");
    let (port, _) = serve(&root);
    let config = root.join("registry/config.json");
    let text = std::fs::read_to_string(&config).unwrap();
    std::fs::write(&config, text.replacen("\"api\":\"api\",", "", 1)).unwrap();
    project(&root, port, false);
    let (code, _, err) = bynk(&root, &["login", "--registry", "acme-internal"], &[]);
    assert_eq!(code, 1);
    assert!(err.contains("has no account API"), "{err}");
}

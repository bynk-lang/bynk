//! `bynk dev inspect` (#3581, the dev agent-inspection track): list the agent
//! instances a running `bynk dev` session has seen, and read one's stored state.
//!
//! A running session writes `.bynk/dev/session.json` (see [`SessionFile`]) naming
//! each served context and the URL it answers on. This command reads it, signs a
//! request under the local call key as `bynk.dev`, and prints the Worker's answer
//! as JSON on stdout. The request is the published call signature, as `bynk
//! migrate sweep`'s wake is, so a Worker that was not built by `bynk dev` has no
//! such route and answers `404`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Where a running session records itself, under the managed build directory.
pub fn session_path(project_root: &Path) -> PathBuf {
    project_root.join(".bynk").join("dev").join("session.json")
}

/// One served Worker, as the session file records it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionWorker {
    /// The qualified context it serves: the callee its signatures are bound to.
    pub context: String,
    /// `http://localhost:<port>`.
    pub url: String,
    /// A Worker group's members that host an agent (#3562, G6), in the order the
    /// group's entry tries them. The group is one Worker with one registry, but
    /// each member decides before it verifies whether a request is its own, so a
    /// request is signed for the member that answers it. Empty for a Worker that
    /// is one context, whose `context` is the audience of every request.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<InspectMember>,
}

/// One member of a group that hosts agents, as `bynk-inspect.json` records it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InspectMember {
    pub context: String,
    pub classes: Vec<String>,
}

impl SessionWorker {
    /// The audience of the listing: the first member that hosts an agent answers it
    /// (the registry is the Worker's, so it lists every member's instances).
    fn lister(&self) -> &str {
        self.members.first().map_or(&self.context, |m| &m.context)
    }

    /// The audience of a read of `class`: the member that owns the class.
    fn owner(&self, class: &str) -> &str {
        self.members
            .iter()
            .find(|m| m.classes.iter().any(|c| c == class))
            .map_or(self.lister(), |m| &m.context)
    }

    /// Whether `context` is one this Worker serves.
    fn serves(&self, context: &str) -> bool {
        self.context == context || self.members.iter().any(|m| m.context == context)
    }
}

/// The session file: each served Worker by its worker-dir name.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Session {
    pub workers: BTreeMap<String, SessionWorker>,
}

/// A written session file, removed when dropped (when the session stops).
pub struct SessionFile(PathBuf);

impl SessionFile {
    /// Record `session` for the project at `project_root`. A failure is reported
    /// and the session serves without it: `bynk dev inspect` then says there is
    /// no running session.
    pub fn write(project_root: &Path, session: &Session) -> SessionFile {
        let path = session_path(project_root);
        let text = serde_json::to_string_pretty(session).expect("a session serialises");
        if let Err(e) = std::fs::write(&path, text) {
            eprintln!(
                "bynk: could not write {}: {e}; `bynk dev inspect` will not find this session",
                path.display()
            );
        }
        SessionFile(path)
    }
}

impl Drop for SessionFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// The agent-hosting members a group's build records beside its entry
/// (`bynk-inspect.json`); none for a Worker that is one context, or an unreadable file.
pub fn inspect_members(worker_dir: &Path) -> Vec<InspectMember> {
    #[derive(Deserialize)]
    struct File {
        members: Vec<InspectMember>,
    }
    std::fs::read_to_string(worker_dir.join(bynk_emit::INSPECT_MEMBERS_FILE))
        .ok()
        .and_then(|text| serde_json::from_str::<File>(&text).ok())
        .map_or_else(Vec::new, |f| f.members)
}

/// The qualified context a worker-dir name serves.
pub fn context_of_worker(worker: &str) -> String {
    worker.replace('-', ".")
}

/// What `bynk dev inspect` was asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inspect {
    /// `inspect agents [--context C] [--class A]`.
    Agents {
        context: Option<String>,
        class: Option<String>,
    },
    /// `inspect agent <Class> <key>`: its state, paged by `page`.
    Agent {
        context: Option<String>,
        class: String,
        key: String,
        page: Page,
    },
}

/// Which page of a collection to read (`--field`, `--after`, `--limit`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Page {
    pub field: Option<String>,
    pub after: Option<String>,
    pub limit: Option<u32>,
}

impl Page {
    fn query(&self) -> Vec<(&'static str, String)> {
        let mut q = Vec::new();
        if let Some(f) = &self.field {
            q.push(("field", f.clone()));
        }
        if let Some(a) = &self.after {
            q.push(("after", a.clone()));
        }
        if let Some(l) = self.limit {
            q.push(("limit", l.to_string()));
        }
        q
    }
}

/// The signed `POST` of one inspect request: its URL and headers.
pub fn signed_request(
    base: &str,
    context: &str,
    path: &str,
    query: &[(&str, String)],
    now: u64,
) -> (String, Vec<(String, String)>) {
    let headers = crate::migrate::sweep::signed_headers_as(
        bynk_emit::INSPECT_CALLER,
        bynk_emit::LOCAL_CALL_KEY,
        context,
        path,
        "",
        now,
    );
    let mut url = format!("{}{path}", base.trim_end_matches('/'));
    for (i, (k, v)) in query.iter().enumerate() {
        url.push(if i == 0 { '?' } else { '&' });
        url.push_str(k);
        url.push('=');
        url.push_str(&percent_encode(v));
    }
    (url, headers)
}

/// Percent-encode everything but unreserved characters, for a query value or a path segment.
fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Whether a registry key (its wire form) is the one the user typed: a string key
/// by its text, any other by its JSON.
pub fn key_matches(key: &Value, typed: &str) -> bool {
    match key {
        Value::String(s) => s == typed,
        other => serde_json::from_str::<Value>(typed).is_ok_and(|v| &v == other),
    }
}

/// One HTTP exchange with a Worker: the status and the body.
pub trait Post {
    fn post(&self, url: &str, headers: &[(String, String)]) -> Result<(u16, String), String>;
}

/// The real transport: plain HTTP to a local Worker.
pub struct Local(ureq::Agent);

impl Local {
    pub fn new() -> Self {
        Local(
            ureq::Agent::config_builder()
                .http_status_as_error(false)
                .timeout_global(Some(std::time::Duration::from_secs(15)))
                .build()
                .into(),
        )
    }
}

impl Default for Local {
    fn default() -> Self {
        Self::new()
    }
}

impl Post for Local {
    fn post(&self, url: &str, headers: &[(String, String)]) -> Result<(u16, String), String> {
        let mut request = self.0.post(url);
        for (k, v) in headers {
            request = request.header(k.as_str(), v.as_str());
        }
        let mut response = request.send("").map_err(|e| e.to_string())?;
        let status = response.status().as_u16();
        let text = response
            .body_mut()
            .read_to_string()
            .map_err(|e| e.to_string())?;
        Ok((status, text))
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

fn selected<'a>(
    session: &'a Session,
    context: &Option<String>,
) -> Result<Vec<(&'a String, &'a SessionWorker)>, String> {
    let chosen: Vec<_> = session
        .workers
        .iter()
        .filter(|(dir, w)| context.as_ref().is_none_or(|c| w.serves(c) || c == *dir))
        .collect();
    if chosen.is_empty() {
        return Err(match context {
            Some(c) => format!("the session serves no context `{c}`"),
            None => "the session serves no context".to_string(),
        });
    }
    Ok(chosen)
}

fn call(
    http: &dyn Post,
    worker: &SessionWorker,
    audience: &str,
    path: &str,
    query: &[(&str, String)],
) -> Result<Value, String> {
    let (url, headers) = signed_request(&worker.url, audience, path, query, unix_now());
    let (status, body) = http.post(&url, &headers)?;
    match status {
        200 => serde_json::from_str(&body).map_err(|e| format!("unreadable answer: {e}")),
        401 => {
            Err("refused the signature: is this `bynk dev` session built by this `bynk`?".into())
        }
        404 => Err("has no inspect route: is this a `bynk dev` session?".into()),
        s => Err(format!("answered {s}")),
    }
}

/// The listing of one Worker. A Worker that hosts no agent has no inspect route (the
/// emitter adds it only where there is a class to read), so a `404` is "no agents
/// here", an empty listing, and not a failure.
fn listing(
    http: &dyn Post,
    worker: &SessionWorker,
    query: &[(&str, String)],
) -> Result<Value, String> {
    let (url, headers) = signed_request(
        &worker.url,
        worker.lister(),
        "/_bynk/inspect/agents",
        query,
        unix_now(),
    );
    match http.post(&url, &headers)? {
        (404, _) => Ok(json!({ "instances": [] })),
        (200, body) => serde_json::from_str(&body).map_err(|e| format!("unreadable answer: {e}")),
        (401, _) => Err(
            "refused the signature: is this `bynk dev` session built by this `bynk`?".to_string(),
        ),
        (s, _) => Err(format!("answered {s}")),
    }
}

/// Run `what` against the session, returning the JSON to print. A context that
/// cannot be reached is named in `errors` and the rest still answer.
pub fn run(session: &Session, what: &Inspect, http: &dyn Post) -> Result<Value, String> {
    match what {
        Inspect::Agents { context, class } => {
            let mut agents = Vec::new();
            let mut errors = Vec::new();
            let query: Vec<(&str, String)> = class.iter().map(|c| ("class", c.clone())).collect();
            for (_, w) in selected(session, context)? {
                match listing(http, w, &query) {
                    Ok(v) => {
                        for i in v["instances"].as_array().into_iter().flatten() {
                            agents.push(json!({
                                "context": i["class"].as_str().map_or(w.lister(), |c| w.owner(c)),
                                "class": i["class"],
                                "key": i["key"],
                                "id": i["id"],
                            }));
                        }
                    }
                    Err(e) => errors.push(json!({ "context": w.lister(), "error": e })),
                }
            }
            Ok(json!({ "agents": agents, "errors": errors }))
        }
        Inspect::Agent {
            context,
            class,
            key,
            page,
        } => read_agent(session, context, class, key, page, http),
    }
}

fn read_agent(
    session: &Session,
    context: &Option<String>,
    class: &str,
    key: &str,
    page: &Page,
    http: &dyn Post,
) -> Result<Value, String> {
    let class_query = [("class", class.to_string())];
    let mut failed = Vec::new();
    for (_, w) in selected(session, context)? {
        // A Worker that is down, or refuses, is skipped: the instance may be in another.
        let listed = match listing(http, w, &class_query) {
            Ok(v) => v,
            Err(e) => {
                failed.push(format!("{}: {e}", w.lister()));
                continue;
            }
        };
        let found = listed["instances"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|i| i["class"] == class && key_matches(&i["key"], key));
        let Some(instance) = found else { continue };
        let id = instance["id"].as_str().unwrap_or_default().to_string();
        let mut query = vec![("id", id.clone())];
        query.extend(page.query());
        let path = format!("/_bynk/inspect/agent/{}", percent_encode(class));
        let owner = w.owner(class);
        let mut state = call(http, w, owner, &path, &query)?;
        if let Some(obj) = state.as_object_mut() {
            obj.insert("context".into(), json!(owner));
            obj.insert("key".into(), instance["key"].clone());
            obj.insert("id".into(), json!(id));
        }
        return Ok(state);
    }
    let mut message = format!(
        "no instance of `{class}` with key `{key}` has been addressed in this session \
         (an instance is listed once a handler addresses it)"
    );
    if !failed.is_empty() {
        message.push_str(&format!(
            "; these contexts could not be asked: {} (a session killed without stopping \
             leaves a stale `.bynk/dev/session.json`)",
            failed.join("; ")
        ));
    }
    Err(message)
}

/// `bynk dev inspect`: find the session under `project_root`, run `what`, print
/// the JSON.
pub fn command(project_root: &Path, what: &Inspect) -> std::process::ExitCode {
    let text = match std::fs::read_to_string(session_path(project_root)) {
        Ok(t) => t,
        Err(_) => {
            eprintln!(
                "bynk: no running `bynk dev` session for this project (start one with `bynk dev`)"
            );
            return std::process::ExitCode::FAILURE;
        }
    };
    let session: Session = match serde_json::from_str(&text) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("bynk: unreadable session file: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    match run(&session, what, &Local::new()) {
        Ok(v) => {
            println!("{}", serde_json::to_string_pretty(&v).expect("json prints"));
            std::process::ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("bynk: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn session() -> Session {
        Session {
            workers: BTreeMap::from([(
                "shop-orders".to_string(),
                SessionWorker {
                    context: "shop.orders".to_string(),
                    url: "http://localhost:8787".to_string(),
                    members: vec![],
                },
            )]),
        }
    }

    /// Answers by path, recording each request.
    struct Fake(RefCell<Vec<String>>);

    impl Post for Fake {
        fn post(&self, url: &str, headers: &[(String, String)]) -> Result<(u16, String), String> {
            self.0.borrow_mut().push(url.to_string());
            assert!(
                headers
                    .iter()
                    .any(|(k, v)| k == "X-Bynk-Caller" && v == "bynk.dev"),
                "{headers:?}"
            );
            if url.contains("/_bynk/inspect/agents") {
                Ok((200, r#"{"context":"shop.orders","instances":[{"class":"Ledger","key":"u1","id":"abc"},{"class":"Ledger","key":7,"id":"def"}]}"#.into()))
            } else if url.contains("/_bynk/inspect/agent/Ledger") {
                Ok((200, r#"{"class":"Ledger","exists":true,"readable":true,"cells":{"n":1},"collections":{}}"#.into()))
            } else {
                Ok((404, String::new()))
            }
        }
    }

    #[test]
    fn the_context_of_a_worker_is_its_dotted_name() {
        assert_eq!(context_of_worker("shop-orders"), "shop.orders");
    }

    #[test]
    fn a_typed_key_matches_a_string_by_text_and_anything_else_by_json() {
        assert!(key_matches(&json!("u1"), "u1"));
        assert!(!key_matches(&json!("u1"), "\"u1\""));
        assert!(key_matches(&json!(7), "7"));
        assert!(key_matches(&json!({"a": 1}), r#"{"a":1}"#));
        assert!(!key_matches(&json!(7), "8"));
    }

    #[test]
    fn a_request_is_signed_as_bynk_dev_and_carries_its_query() {
        let (url, headers) = signed_request(
            "http://localhost:8787/",
            "shop.orders",
            "/_bynk/inspect/agent/Ledger",
            &[("id", "a b".to_string()), ("limit", "5".to_string())],
            1_700_000_000,
        );
        assert_eq!(
            url,
            "http://localhost:8787/_bynk/inspect/agent/Ledger?id=a%20b&limit=5"
        );
        assert!(
            headers
                .iter()
                .any(|(k, v)| k == "X-Bynk-Caller" && v == "bynk.dev")
        );
        assert!(headers.iter().any(|(k, _)| k == "X-Bynk-Signature"));
    }

    #[test]
    fn the_caller_key_var_is_the_edge_key_of_the_caller_name() {
        let suffix = bynk_emit::INSPECT_CALLER.to_uppercase().replace('.', "__");
        assert_eq!(
            bynk_emit::INSPECT_CALLER_KEY_VAR,
            format!("BYNK_CALL_FROM_{suffix}")
        );
    }

    #[test]
    fn agents_are_listed_with_their_context() {
        let http = Fake(RefCell::default());
        let out = run(
            &session(),
            &Inspect::Agents {
                context: None,
                class: None,
            },
            &http,
        )
        .unwrap();
        assert_eq!(out["agents"].as_array().unwrap().len(), 2);
        assert_eq!(out["agents"][0]["context"], "shop.orders");
        assert_eq!(out["agents"][0]["key"], "u1");
        assert_eq!(out["errors"], json!([]));
    }

    #[test]
    fn an_agent_is_found_by_key_and_read_by_id() {
        let http = Fake(RefCell::default());
        let what = Inspect::Agent {
            context: None,
            class: "Ledger".into(),
            key: "7".into(),
            page: Page {
                limit: Some(10),
                ..Page::default()
            },
        };
        let out = run(&session(), &what, &http).unwrap();
        assert_eq!(out["id"], "def");
        assert_eq!(out["key"], 7);
        assert_eq!(out["context"], "shop.orders");
        assert_eq!(out["cells"]["n"], 1);
        let seen = http.0.borrow();
        assert!(
            seen.last()
                .unwrap()
                .ends_with("/_bynk/inspect/agent/Ledger?id=def&limit=10"),
            "{seen:?}"
        );
    }

    #[test]
    fn an_unaddressed_instance_says_so() {
        let http = Fake(RefCell::default());
        let what = Inspect::Agent {
            context: Some("shop.orders".into()),
            class: "Ledger".into(),
            key: "nobody".into(),
            page: Page::default(),
        };
        let e = run(&session(), &what, &http).unwrap_err();
        assert!(e.contains("has been addressed"), "{e}");
    }

    /// Two Workers: `shop.api` hosts no agent, so it has no inspect route and answers
    /// `404`; `shop.orders` hosts `Ledger`. The one without agents must not hide the other.
    fn two_workers() -> Session {
        let mut session = session();
        session.workers.insert(
            "shop-api".to_string(),
            SessionWorker {
                context: "shop.api".to_string(),
                url: "http://localhost:8788".to_string(),
                members: vec![],
            },
        );
        // `shop-api` sorts before `shop-orders`.
        session.workers.insert(
            "shop-orders".to_string(),
            SessionWorker {
                context: "shop.orders".to_string(),
                url: "http://localhost:8787".to_string(),
                members: vec![],
            },
        );
        session
    }

    struct Routes;

    impl Post for Routes {
        fn post(&self, url: &str, _: &[(String, String)]) -> Result<(u16, String), String> {
            if url.starts_with("http://localhost:8788") {
                return Ok((404, "Not Found".into()));
            }
            Fake(RefCell::default()).post(url, &[("X-Bynk-Caller".into(), "bynk.dev".into())])
        }
    }

    #[test]
    fn a_worker_without_agents_does_not_hide_the_one_that_has_them() {
        let out = run(
            &two_workers(),
            &Inspect::Agent {
                context: None,
                class: "Ledger".into(),
                key: "u1".into(),
                page: Page::default(),
            },
            &Routes,
        )
        .unwrap();
        assert_eq!(out["context"], "shop.orders");
        assert_eq!(out["key"], "u1");
        // And listing says nothing is wrong with it.
        let listed = run(
            &two_workers(),
            &Inspect::Agents {
                context: None,
                class: None,
            },
            &Routes,
        )
        .unwrap();
        assert_eq!(listed["errors"], json!([]));
        assert_eq!(listed["agents"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn a_worker_that_is_down_is_skipped_and_named_when_nothing_matches() {
        struct OneDown;
        impl Post for OneDown {
            fn post(&self, url: &str, h: &[(String, String)]) -> Result<(u16, String), String> {
                if url.starts_with("http://localhost:8788") {
                    return Err("connection refused".into());
                }
                Routes.post(url, h)
            }
        }
        let found = run(
            &two_workers(),
            &Inspect::Agent {
                context: None,
                class: "Ledger".into(),
                key: "u1".into(),
                page: Page::default(),
            },
            &OneDown,
        );
        assert!(found.is_ok(), "{found:?}");
        let missing = run(
            &two_workers(),
            &Inspect::Agent {
                context: None,
                class: "Ledger".into(),
                key: "nobody".into(),
                page: Page::default(),
            },
            &OneDown,
        )
        .unwrap_err();
        assert!(
            missing.contains("shop.api: connection refused"),
            "{missing}"
        );
    }

    #[test]
    fn an_unknown_context_is_refused() {
        let http = Fake(RefCell::default());
        let e = run(
            &session(),
            &Inspect::Agents {
                context: Some("nope".into()),
                class: None,
            },
            &http,
        )
        .unwrap_err();
        assert!(e.contains("no context `nope`"), "{e}");
    }

    #[test]
    fn a_worker_that_is_down_is_reported_and_the_rest_answer() {
        struct Down;
        impl Post for Down {
            fn post(&self, _: &str, _: &[(String, String)]) -> Result<(u16, String), String> {
                Err("connection refused".into())
            }
        }
        let out = run(
            &session(),
            &Inspect::Agents {
                context: None,
                class: None,
            },
            &Down,
        )
        .unwrap();
        assert_eq!(out["agents"], json!([]));
        assert_eq!(out["errors"][0]["context"], "shop.orders");
    }

    #[test]
    fn the_session_file_is_written_and_removed() {
        let dir = std::env::temp_dir().join(format!("bynk-session-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".bynk").join("dev")).unwrap();
        {
            let _file = SessionFile::write(&dir, &session());
            let read: Session =
                serde_json::from_str(&std::fs::read_to_string(session_path(&dir)).unwrap())
                    .unwrap();
            assert_eq!(read, session());
        }
        assert!(!session_path(&dir).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// One Worker, a group `core` of `shop.api` (no agent), `shop.orders` (`Ledger`) and
    /// `shop.stock` (`Shelf`).
    fn group() -> Session {
        Session {
            workers: BTreeMap::from([(
                "core".to_string(),
                SessionWorker {
                    context: "core".to_string(),
                    url: "http://localhost:8787".to_string(),
                    members: vec![
                        InspectMember {
                            context: "shop.orders".to_string(),
                            classes: vec!["Ledger".to_string()],
                        },
                        InspectMember {
                            context: "shop.stock".to_string(),
                            classes: vec!["Shelf".to_string()],
                        },
                    ],
                },
            )]),
        }
    }

    /// Answers as the group's entry does: a request is answered only if it is signed for
    /// the member that owns it (the listing: the first agent-hosting member; a read: the
    /// class's owner), and refused with `401` otherwise.
    struct Entry;

    impl Entry {
        fn signed_for(url: &str, headers: &[(String, String)], audience: &str) -> bool {
            let path = url
                .trim_start_matches("http://localhost:8787")
                .split('?')
                .next()
                .unwrap();
            let stamp: u64 = headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(bynk_check::wire_format::TIMESTAMP_HEADER))
                .unwrap()
                .1
                .parse()
                .unwrap();
            crate::migrate::sweep::signed_headers_as(
                bynk_emit::INSPECT_CALLER,
                bynk_emit::LOCAL_CALL_KEY,
                audience,
                path,
                "",
                stamp,
            ) == headers
        }
    }

    impl Post for Entry {
        fn post(&self, url: &str, headers: &[(String, String)]) -> Result<(u16, String), String> {
            if url.contains("/_bynk/inspect/agents") {
                return if Self::signed_for(url, headers, "shop.orders") {
                    Ok((200, r#"{"context":"shop.orders","instances":[{"class":"Ledger","key":"u1","id":"a"},{"class":"Shelf","key":"s1","id":"b"}]}"#.into()))
                } else {
                    Ok((401, String::new()))
                };
            }
            let owner = if url.contains("/agent/Shelf") {
                "shop.stock"
            } else {
                "shop.orders"
            };
            if Self::signed_for(url, headers, owner) {
                Ok((
                    200,
                    r#"{"exists":true,"readable":true,"cells":{},"collections":{}}"#.into(),
                ))
            } else {
                Ok((401, String::new()))
            }
        }
    }

    #[test]
    fn a_group_lists_every_members_agents_under_the_member_that_owns_each() {
        let what = Inspect::Agents {
            context: None,
            class: None,
        };
        let out = run(&group(), &what, &Entry).unwrap();
        assert_eq!(out["errors"], json!([]), "{out}");
        let contexts: Vec<(&str, &str)> = out["agents"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| (a["class"].as_str().unwrap(), a["context"].as_str().unwrap()))
            .collect();
        assert_eq!(
            contexts,
            [("Ledger", "shop.orders"), ("Shelf", "shop.stock")]
        );
    }

    #[test]
    fn a_group_read_is_signed_for_the_member_that_owns_the_class() {
        for (class, key, owner) in [
            ("Ledger", "u1", "shop.orders"),
            ("Shelf", "s1", "shop.stock"),
        ] {
            let what = Inspect::Agent {
                context: None,
                class: class.to_string(),
                key: key.to_string(),
                page: Page::default(),
            };
            let out = run(&group(), &what, &Entry).unwrap();
            assert_eq!(out["context"], owner, "{out}");
        }
    }

    #[test]
    fn a_member_context_selects_the_group_that_serves_it() {
        let what = Inspect::Agents {
            context: Some("shop.stock".to_string()),
            class: None,
        };
        assert!(run(&group(), &what, &Entry).is_ok());
        let what = Inspect::Agents {
            context: Some("shop.nowhere".to_string()),
            class: None,
        };
        assert!(run(&group(), &what, &Entry).is_err());
    }
}

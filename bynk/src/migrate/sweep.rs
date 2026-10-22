//! `bynk migrate sweep` (state-migrations S4, #2259 Decisions C–F).
//!
//! The sweep lists every stored instance of the selected agent classes
//! through Cloudflare's API, wakes each one through its Worker's signed
//! `/_bynk/migrate/<Class>` route (which loads it, running any step, and
//! commits it when older), and then raises a class's floor when the run
//! covered every environment the ledger records its Worker in.
//!
//! Nothing about the result is recorded: how old the stored instances are is
//! Cloudflare's state, and a recorded copy would go stale (Decision F).
//!
//! The side effects sit behind three small traits, so the orchestration is
//! tested against fakes: [`Cloudflare`] (the REST listings), [`Secrets`]
//! (`wrangler secret put`/`delete`) and [`Wake`] (the signed HTTPS call).

use std::collections::BTreeMap;
use std::io::Write;

use bynk_emit::wire_format as wf;
use bynk_project::schema_registry::SchemaRegistry;
use sha2::{Digest, Sha256};

/// One Durable Object namespace on the account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Namespace {
    pub id: String,
    pub script: String,
    pub class: String,
}

/// Cloudflare's REST API, as far as the sweep uses it.
pub(crate) trait Cloudflare {
    /// Every Durable Object namespace on the account (all pages).
    fn namespaces(&self) -> Result<Vec<Namespace>, String>;
    /// The ids of the namespace's objects that hold stored data (all pages).
    fn stored_objects(&self, namespace: &str) -> Result<Vec<String>, String>;
    /// `https://<script>.<subdomain>.workers.dev` when the Worker serves on
    /// `workers.dev`, `None` when that is turned off for it.
    fn workers_dev_url(&self, script: &str) -> Result<Option<String>, String>;
    /// When the Worker's code was last deployed: its newest deployment not
    /// triggered by a secret change, or, when every listed one was, the
    /// oldest listed (the code is at least that old). #2276 Decision A.
    fn last_code_deployment(&self, script: &str) -> Result<std::time::SystemTime, String>;
}

/// Setting and deleting a deployed Worker's secret.
pub(crate) trait Secrets {
    fn put(&self, script: &str, name: &str, value: &str) -> Result<(), String>;
    fn delete(&self, script: &str, name: &str) -> Result<(), String>;
}

/// One signed `POST` to a deployed Worker: the status and the body.
pub(crate) trait Wake {
    fn post(
        &self,
        url: &str,
        headers: &[(String, String)],
        body: &str,
    ) -> Result<(u16, String), String>;
}

/// One agent class in one environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Target {
    /// `<context>.<Agent>`, the lock's key.
    pub class: String,
    pub agent: String,
    /// The Worker directory (`<context>` dasherised).
    pub worker: String,
    pub environment: String,
    /// The deployed Worker's name.
    pub script: String,
}

/// What a sweep run covers.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Selection {
    pub targets: Vec<Target>,
    /// Per class: whether the run covers every environment the ledger records
    /// its Worker in (and there is at least one). Only such a class's floor
    /// can rise.
    pub covers_all: BTreeMap<String, bool>,
    /// Classes whose Worker the ledger records in no environment.
    pub never_deployed: Vec<String>,
}

/// The classes `lock` records, narrowed to `context` (its qualified name or
/// its Worker directory) and to `environments` (all recorded ones when
/// empty).
pub(crate) fn select(
    lock: &SchemaRegistry,
    deployed: &BTreeMap<String, Vec<String>>,
    context: Option<&str>,
    environments: &[String],
) -> Result<Selection, String> {
    let mut targets = Vec::new();
    let mut covers_all = BTreeMap::new();
    let mut never_deployed = Vec::new();
    let mut matched = false;
    for (class, _) in lock.agents() {
        let (unit, agent) = class.rsplit_once('.').unwrap_or(("", class.as_str()));
        let worker = super::worker_of(class);
        if let Some(c) = context
            && c != unit
            && c != worker
        {
            continue;
        }
        matched = true;
        let recorded = deployed.get(&worker).cloned().unwrap_or_default();
        if recorded.is_empty() {
            never_deployed.push(class.clone());
            covers_all.insert(class.clone(), false);
            continue;
        }
        let chosen: Vec<&String> = recorded
            .iter()
            .filter(|e| environments.is_empty() || environments.contains(e))
            .collect();
        covers_all.insert(class.clone(), chosen.len() == recorded.len());
        for env in chosen {
            targets.push(Target {
                class: class.clone(),
                agent: agent.to_string(),
                worker: worker.clone(),
                environment: env.clone(),
                script: crate::deploy::deployed_worker_name(env, &worker),
            });
        }
    }
    if let Some(c) = context
        && !matched
    {
        return Err(format!(
            "`bynk.schema.lock` records no agent class in context `{c}`"
        ));
    }
    for env in environments {
        if !targets.iter().any(|t| &t.environment == env) {
            return Err(format!(
                "`bynk.deploy.lock` records none of the selected Workers in environment `{env}`"
            ));
        }
    }
    Ok(Selection {
        targets,
        covers_all,
        never_deployed,
    })
}

/// One Worker's share of a run: where it serves and what it wakes.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ScriptPlan {
    pub url: String,
    /// Each target on this Worker, with the ids holding stored data.
    pub classes: Vec<(Target, Vec<String>)>,
    /// When its code was last deployed, read before the run's key is set,
    /// or why that could not be read (which holds the floor).
    pub deployed_at: Result<std::time::SystemTime, String>,
}

/// List what each target holds and find where its Worker serves. `urls` are
/// `--url <script>=<https://…>` overrides, for a Worker not on `workers.dev`.
pub(crate) fn prepare(
    selection: &Selection,
    cloudflare: &dyn Cloudflare,
    urls: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, ScriptPlan>, String> {
    let namespaces = cloudflare.namespaces()?;
    let mut plans: BTreeMap<String, ScriptPlan> = BTreeMap::new();
    for target in &selection.targets {
        let ids = match namespaces
            .iter()
            .find(|n| n.script == target.script && n.class == target.agent)
        {
            // No namespace: the class has never stored anything there.
            None => Vec::new(),
            Some(ns) => cloudflare.stored_objects(&ns.id)?,
        };
        if !plans.contains_key(&target.script) {
            let url = match urls.get(&target.script) {
                Some(u) => u.trim_end_matches('/').to_string(),
                None => cloudflare.workers_dev_url(&target.script)?.ok_or_else(|| {
                    format!(
                        "Worker `{}` does not serve on workers.dev; pass `--url {}=https://<its host>`",
                        target.script, target.script
                    )
                })?,
            };
            plans.insert(
                target.script.clone(),
                ScriptPlan {
                    url,
                    classes: Vec::new(),
                    deployed_at: cloudflare.last_code_deployment(&target.script),
                },
            );
        }
        plans
            .get_mut(&target.script)
            .expect("inserted above")
            .classes
            .push((target.clone(), ids));
    }
    Ok(plans)
}

/// What waking one target found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClassResult {
    pub target: Target,
    /// The ids listed as holding stored data.
    pub listed: usize,
    /// The ids woken without a fault.
    pub woke: usize,
    /// Each id that faulted, with its fault.
    pub failed: Vec<(String, String)>,
    /// The version the deployed Worker writes, from its answer.
    pub current: Option<i64>,
    /// The oldest version a woken instance now holds.
    pub oldest_after: Option<i64>,
    /// Why the target could not be swept at all.
    pub error: Option<String>,
}

impl ClassResult {
    /// Every listed id woke, and the Worker said which version it writes.
    pub fn complete(&self) -> bool {
        self.error.is_none() && self.failed.is_empty() && self.current.is_some()
    }
}

/// How long the sweep waits for a freshly set key to reach the Worker.
pub(crate) struct Timing {
    pub ready_attempts: u32,
    pub ready_pause: std::time::Duration,
}

impl Timing {
    pub(crate) const LIVE: Timing = Timing {
        ready_attempts: 15,
        ready_pause: std::time::Duration::from_secs(2),
    };
}

/// Wake every planned instance. Each Worker gets a fresh key for the run,
/// set before its first wake and deleted after its last, whatever happens in
/// between; a failure to delete it is reported, never hidden. `stopped` is
/// polled between batches, so a Ctrl-C stops the waking, still deletes the
/// key, and leaves the run incomplete.
#[allow(clippy::too_many_arguments)]
pub(crate) fn execute(
    plans: &BTreeMap<String, ScriptPlan>,
    secrets: &dyn Secrets,
    wake: &dyn Wake,
    batch: usize,
    timing: &Timing,
    mint: &dyn Fn() -> Result<String, String>,
    stopped: &dyn Fn() -> bool,
    out: &mut dyn Write,
) -> Vec<ClassResult> {
    let mut results = Vec::new();
    for (script, plan) in plans {
        let blank = |target: &Target, listed: usize, error: String| ClassResult {
            target: target.clone(),
            listed,
            woke: 0,
            failed: Vec::new(),
            current: None,
            oldest_after: None,
            error: Some(error),
        };
        let key = match mint() {
            Ok(k) => k,
            Err(e) => {
                results.extend(
                    plan.classes
                        .iter()
                        .map(|(t, ids)| blank(t, ids.len(), e.clone())),
                );
                continue;
            }
        };
        if let Err(e) = secrets.put(script, wf::MIGRATE_KEY, &key) {
            // Best effort: a `put` interrupted after Cloudflare accepted it
            // fails here with the key set.
            let _ = secrets.delete(script, wf::MIGRATE_KEY);
            let e = format!("could not set {} on `{script}`: {e}", wf::MIGRATE_KEY);
            results.extend(
                plan.classes
                    .iter()
                    .map(|(t, ids)| blank(t, ids.len(), e.clone())),
            );
            continue;
        }
        for (target, ids) in &plan.classes {
            let r = if stopped() {
                blank(target, ids.len(), "interrupted".to_string())
            } else {
                wake_class(&plan.url, target, ids, &key, wake, batch, timing, stopped)
            };
            let _ = writeln!(out, "{}", summary_line(&r));
            results.push(r);
        }
        if let Err(e) = secrets.delete(script, wf::MIGRATE_KEY) {
            let _ = writeln!(
                out,
                "bynk: could not delete {key_name} from `{script}`: {e}\n  delete it now: \
                 wrangler secret delete {key_name} --name {script}",
                key_name = wf::MIGRATE_KEY
            );
        }
    }
    results
}

/// One target's line in the run's output.
fn summary_line(r: &ClassResult) -> String {
    let head = format!(
        "{} ({}): {}",
        r.target.script, r.target.environment, r.target.agent
    );
    if let Some(e) = &r.error {
        return format!("{head}  not swept: {e}");
    }
    let mut line = format!(
        "{head}  {} with stored data, woke {} ({} failed)",
        r.listed,
        r.woke,
        r.failed.len()
    );
    if let Some(c) = r.current {
        line.push_str(&format!("; deployed at {c}"));
    }
    if let Some(a) = r.oldest_after {
        line.push_str(&format!(", oldest held now {a}"));
    }
    for (id, fault) in &r.failed {
        line.push_str(&format!("\n    {id}: {fault}"));
    }
    line
}

/// Wake one target's ids in batches. The first call carries no ids: it waits
/// for the key to reach the Worker and learns the version it writes.
#[allow(clippy::too_many_arguments)]
fn wake_class(
    url: &str,
    target: &Target,
    ids: &[String],
    key: &str,
    wake: &dyn Wake,
    batch: usize,
    timing: &Timing,
    stopped: &dyn Fn() -> bool,
) -> ClassResult {
    let path = format!("{}{}", wf::MIGRATE_PATH_PREFIX, target.agent);
    let context = target.class.rsplit_once('.').map_or("", |(u, _)| u);
    let call = |body: &str| -> Result<Answer, String> {
        let headers = signed_headers(key, context, &path, body, unix_now());
        match wake.post(&format!("{url}{path}"), &headers, body)? {
            (200, text) => parse_answer(&text),
            (status, _) => Err(format!("the Worker answered {status}")),
        }
    };
    let mut result = ClassResult {
        target: target.clone(),
        listed: ids.len(),
        woke: 0,
        failed: Vec::new(),
        current: None,
        oldest_after: None,
        error: None,
    };
    let mut ready = Err("not tried".to_string());
    for attempt in 0..timing.ready_attempts.max(1) {
        if attempt > 0 {
            std::thread::sleep(timing.ready_pause);
        }
        ready = call(r#"{"ids":[]}"#);
        if ready.is_ok() || stopped() {
            break;
        }
    }
    match ready {
        Ok(a) => result.current = Some(a.current),
        Err(e) => {
            // A Worker built before this class existed (or before the lock was
            // in use) has no route for it.
            let hint = if e.ends_with("answered 404") {
                format!(
                    "; the deployed Worker has no wake route for `{}`: redeploy the `{}` \
                     environment first",
                    target.agent, target.environment
                )
            } else {
                String::new()
            };
            result.error = Some(format!("the wake route did not answer: {e}{hint}"));
            return result;
        }
    }
    let mut retry: Vec<String> = Vec::new();
    let absorb = |result: &mut ClassResult, answer: Answer, retry: Option<&mut Vec<String>>| {
        let mut retry = retry;
        for w in answer.results {
            match w.outcome {
                Outcome::Woke { after } => {
                    result.woke += 1;
                    if let Some(after) = after {
                        result.oldest_after =
                            Some(result.oldest_after.map_or(after, |o: i64| o.min(after)));
                    }
                }
                // A transport fault is worth one retry; a load fault is the
                // instance's own and would only repeat.
                Outcome::Fault(f) if is_transient(&f) && retry.is_some() => {
                    retry.as_mut().expect("checked").push(w.id)
                }
                Outcome::Fault(f) => result.failed.push((w.id, f)),
            }
        }
    };
    for chunk in ids.chunks(batch.max(1)) {
        if stopped() {
            result.error = Some("interrupted".to_string());
            return result;
        }
        let body = ids_body(chunk);
        match call(&body).or_else(|_| call(&body)) {
            Ok(a) => absorb(&mut result, a, Some(&mut retry)),
            Err(e) => result
                .failed
                .extend(chunk.iter().map(|id| (id.clone(), e.clone()))),
        }
    }
    for chunk in std::mem::take(&mut retry).chunks(batch.max(1)) {
        let body = ids_body(chunk);
        match call(&body) {
            Ok(a) => absorb(&mut result, a, None),
            Err(e) => result
                .failed
                .extend(chunk.iter().map(|id| (id.clone(), e.clone()))),
        }
    }
    result
}

fn is_transient(fault: &str) -> bool {
    fault == "Unreachable" || fault.starts_with("Status")
}

fn ids_body(ids: &[String]) -> String {
    serde_json::json!({ "ids": ids }).to_string()
}

/// The Worker's answer to one wake.
struct Answer {
    current: i64,
    results: Vec<Woken>,
}

struct Woken {
    id: String,
    outcome: Outcome,
}

enum Outcome {
    /// `after` is `None` for an object with no stored record.
    Woke {
        after: Option<i64>,
    },
    Fault(String),
}

fn parse_answer(text: &str) -> Result<Answer, String> {
    let v: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("unreadable answer: {e}"))?;
    let current = v
        .get("current")
        .and_then(serde_json::Value::as_i64)
        .ok_or("the answer names no current version")?;
    let results = v
        .get("results")
        .and_then(serde_json::Value::as_array)
        .ok_or("the answer has no results")?
        .iter()
        .map(|r| {
            let id = r
                .get("id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let outcome = match r.get("fault").and_then(serde_json::Value::as_str) {
                Some(f) => Outcome::Fault(f.to_string()),
                None => Outcome::Woke {
                    after: r.get("after").and_then(serde_json::Value::as_i64),
                },
            };
            Woken { id, outcome }
        })
        .collect();
    Ok(Answer { current, results })
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

/// The headers a sweep signs one wake with: the published call signature
/// (`bynk_check::wire_format::signing_input`), as `bynk.migrate`, under the
/// run's key.
pub(crate) fn signed_headers(
    key: &str,
    context: &str,
    path: &str,
    body: &str,
    now: u64,
) -> Vec<(String, String)> {
    let timestamp = now.to_string();
    let digest = hex(&Sha256::digest(body.as_bytes()));
    let input = wf::signing_input(
        wf::MIGRATE_CALLER,
        context,
        "POST",
        path,
        "",
        "1",
        &timestamp,
        &digest,
    );
    vec![
        (
            wf::CALLER_HEADER.to_string(),
            wf::MIGRATE_CALLER.to_string(),
        ),
        (wf::TIMESTAMP_HEADER.to_string(), timestamp),
        (
            wf::SIGNATURE_HEADER.to_string(),
            hex(&hmac_sha256(key.as_bytes(), input.as_bytes())),
        ),
    ]
}

/// HMAC-SHA256 (RFC 2104), as `crypto.subtle` computes it for the runtime's
/// `verifyCall`.
fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let pad = |byte: u8| k.map(|b| b ^ byte);
    let inner = Sha256::new()
        .chain_update(pad(0x36))
        .chain_update(message)
        .finalize();
    Sha256::new()
        .chain_update(pad(0x5c))
        .chain_update(inner)
        .finalize()
        .into()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// How long after a code deployment the floor waits (#2276 Decision B).
/// Cloudflare's objects listing was seen to show a newly stored instance 5 to
/// 11 minutes after its first write, so an instance the previous build first
/// stored just before a deploy can be missing from a sweep run straight after
/// it. 30 minutes is about three times the longest lag observed.
pub(crate) const SETTLING_MARGIN: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// The Workers whose listing may not have settled yet, each with why: one
/// deployed less than `margin` before `now`, or one whose last deployment
/// could not be read. A floor never rises on such a Worker's evidence (#2276
/// Decision C). A deployment time ahead of `now` (clock skew) counts as just
/// deployed.
pub(crate) fn unsettled(
    plans: &BTreeMap<String, ScriptPlan>,
    now: std::time::SystemTime,
    margin: std::time::Duration,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (script, plan) in plans {
        let env = plan
            .classes
            .first()
            .map_or("", |(t, _)| t.environment.as_str());
        match &plan.deployed_at {
            Err(e) => {
                out.insert(
                    script.clone(),
                    format!("could not read when `{script}` ({env}) was last deployed: {e}"),
                );
            }
            Ok(at) => {
                let age = now.duration_since(*at).unwrap_or_default();
                if age < margin {
                    let minutes = |d: std::time::Duration| d.as_secs().div_ceil(60);
                    out.insert(
                        script.clone(),
                        format!(
                            "`{script}` ({env}) was deployed {} minute(s) ago; Cloudflare's \
                             listing can lag a new instance by about 10 minutes, so the floor \
                             waits until {} minutes after a deploy; sweep again in {} minute(s)",
                            age.as_secs() / 60,
                            margin.as_secs() / 60,
                            minutes(margin - age)
                        ),
                    );
                }
            }
        }
    }
    out
}

/// The floors this run can raise: for each class it covered in every
/// recorded environment, completely, the minimum of each environment's
/// deployed version and every woken instance's version, kept at or below the
/// lock's own version. Only a rise is returned: `(class, from, to)`.
pub(crate) fn floor_raises(
    lock: &SchemaRegistry,
    selection: &Selection,
    results: &[ClassResult],
    unsettled: &BTreeMap<String, String>,
) -> Vec<(String, i64, i64)> {
    let mut raises = Vec::new();
    for (class, entry) in lock.agents() {
        if !selection.covers_all.get(class).copied().unwrap_or(false) {
            continue;
        }
        let mine: Vec<&ClassResult> = results
            .iter()
            .filter(|r| &r.target.class == class)
            .collect();
        let expected = selection
            .targets
            .iter()
            .filter(|t| &t.class == class)
            .count();
        if mine.len() != expected || mine.is_empty() || !mine.iter().all(|r| r.complete()) {
            continue;
        }
        if mine
            .iter()
            .any(|r| unsettled.contains_key(&r.target.script))
        {
            continue;
        }
        let oldest = mine
            .iter()
            .flat_map(|r| r.current.into_iter().chain(r.oldest_after))
            .min()
            .expect("every complete result names its current version");
        let to = oldest.min(entry.version);
        if to > entry.floor {
            raises.push((class.clone(), entry.floor, to));
        }
    }
    raises
}

/// Why a class's floor stays where it is, for the run's closing lines.
pub(crate) fn floor_held_reason(
    class: &str,
    selection: &Selection,
    results: &[ClassResult],
    unsettled: &BTreeMap<String, String>,
    // Whether the floor would have risen had every Worker settled: only then
    // is the margin the reason (#2279 review).
    would_rise: bool,
) -> Option<String> {
    if selection.never_deployed.iter().any(|c| c == class) {
        return Some("its Worker is recorded in no environment".to_string());
    }
    if !selection.covers_all.get(class).copied().unwrap_or(false) {
        return Some(
            "this run did not cover every environment `bynk.deploy.lock` records".to_string(),
        );
    }
    if results
        .iter()
        .any(|r| r.target.class == class && !r.complete())
    {
        return Some("the sweep was incomplete".to_string());
    }
    if !would_rise {
        return None;
    }
    selection
        .targets
        .iter()
        .filter(|t| t.class == class)
        .find_map(|t| unsettled.get(&t.script).cloned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bynk_project::schema_registry::{AgentEntry, AgentShape};
    use std::cell::RefCell;

    fn lock_with(classes: &[(&str, i64, i64)]) -> SchemaRegistry {
        let mut lock = SchemaRegistry::new();
        for (class, version, floor) in classes {
            lock.insert_agent(
                class.to_string(),
                AgentEntry {
                    version: *version,
                    floor: *floor,
                    shapes: (*floor..=*version)
                        .map(|v| AgentShape {
                            version: v,
                            additive: false,
                            fields: Vec::new(),
                        })
                        .collect(),
                    renamed_from: Vec::new(),
                },
            );
        }
        lock
    }

    fn deployed(entries: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
        entries
            .iter()
            .map(|(w, envs)| (w.to_string(), envs.iter().map(|e| e.to_string()).collect()))
            .collect()
    }

    #[test]
    fn hmac_matches_rfc_4231_case_2() {
        let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(
            hex(&mac),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn the_signature_covers_the_published_signing_input() {
        let headers = signed_headers("k", "shop", "/_bynk/migrate/Cart", "{}", 1_700_000_000);
        let digest = hex(&Sha256::digest(b"{}"));
        let input = wf::signing_input(
            "bynk.migrate",
            "shop",
            "POST",
            "/_bynk/migrate/Cart",
            "",
            "1",
            "1700000000",
            &digest,
        );
        assert_eq!(
            headers,
            vec![
                ("X-Bynk-Caller".to_string(), "bynk.migrate".to_string()),
                ("X-Bynk-Timestamp".to_string(), "1700000000".to_string()),
                (
                    "X-Bynk-Signature".to_string(),
                    hex(&hmac_sha256(b"k", input.as_bytes()))
                ),
            ]
        );
    }

    #[test]
    fn selection_covers_every_recorded_environment_unless_narrowed() {
        let lock = lock_with(&[("shop.Cart", 2, 1), ("billing.Ledger", 1, 1)]);
        let deployed = deployed(&[("shop", &["default", "staging"])]);
        let all = select(&lock, &deployed, None, &[]).unwrap();
        assert_eq!(
            all.targets
                .iter()
                .map(|t| (t.class.as_str(), t.script.as_str()))
                .collect::<Vec<_>>(),
            vec![("shop.Cart", "shop"), ("shop.Cart", "shop-staging")]
        );
        assert!(all.covers_all["shop.Cart"]);
        assert_eq!(all.never_deployed, vec!["billing.Ledger".to_string()]);

        let narrowed = select(&lock, &deployed, Some("shop"), &["staging".to_string()]).unwrap();
        assert_eq!(narrowed.targets.len(), 1);
        assert!(!narrowed.covers_all["shop.Cart"]);

        assert!(select(&lock, &deployed, Some("nope"), &[]).is_err());
        assert!(select(&lock, &deployed, None, &["prod".to_string()]).is_err());
    }

    struct FakeCloudflare;

    impl Cloudflare for FakeCloudflare {
        fn namespaces(&self) -> Result<Vec<Namespace>, String> {
            Ok(vec![
                Namespace {
                    id: "ns-default".into(),
                    script: "shop".into(),
                    class: "Cart".into(),
                },
                Namespace {
                    id: "ns-other".into(),
                    script: "elsewhere".into(),
                    class: "Cart".into(),
                },
            ])
        }
        fn stored_objects(&self, namespace: &str) -> Result<Vec<String>, String> {
            assert_eq!(namespace, "ns-default");
            Ok((0..5).map(|i| format!("id{i}")).collect())
        }
        fn workers_dev_url(&self, script: &str) -> Result<Option<String>, String> {
            Ok((script == "shop").then(|| "https://shop.acct.workers.dev".to_string()))
        }
        fn last_code_deployment(&self, script: &str) -> Result<std::time::SystemTime, String> {
            if script == "shop" {
                Ok(std::time::UNIX_EPOCH)
            } else {
                Err("no permission".into())
            }
        }
    }

    #[test]
    fn prepare_lists_by_worker_and_class_and_finds_each_url() {
        let lock = lock_with(&[("shop.Cart", 2, 1)]);
        let sel = select(
            &lock,
            &deployed(&[("shop", &["default", "staging"])]),
            None,
            &[],
        )
        .unwrap();
        // `shop-staging` is not on workers.dev and has no override.
        let err = prepare(&sel, &FakeCloudflare, &BTreeMap::new()).unwrap_err();
        assert!(err.contains("--url shop-staging=https://"), "{err}");
        let urls = BTreeMap::from([(
            "shop-staging".to_string(),
            "https://staging.example.com/".to_string(),
        )]);
        let plans = prepare(&sel, &FakeCloudflare, &urls).unwrap();
        assert_eq!(plans["shop"].url, "https://shop.acct.workers.dev");
        assert_eq!(plans["shop"].classes[0].1.len(), 5);
        // No namespace in staging: nothing stored there, but still woken for
        // its `current`.
        assert_eq!(plans["shop-staging"].url, "https://staging.example.com");
        assert!(plans["shop-staging"].classes[0].1.is_empty());
    }

    #[derive(Default)]
    struct FakeSecrets {
        log: RefCell<Vec<String>>,
        fail_put: bool,
    }

    impl Secrets for FakeSecrets {
        fn put(&self, script: &str, name: &str, _value: &str) -> Result<(), String> {
            self.log.borrow_mut().push(format!("put {script} {name}"));
            if self.fail_put {
                Err("no permission".into())
            } else {
                Ok(())
            }
        }
        fn delete(&self, script: &str, name: &str) -> Result<(), String> {
            self.log
                .borrow_mut()
                .push(format!("delete {script} {name}"));
            Ok(())
        }
    }

    /// A Worker at `current` whose instances start at `before`; ids in
    /// `flaky` fail transiently once, ids in `broken` fault on load.
    struct FakeWorker {
        current: i64,
        before: i64,
        flaky: RefCell<Vec<String>>,
        broken: Vec<String>,
        calls: RefCell<Vec<usize>>,
        not_ready: RefCell<u32>,
    }

    impl FakeWorker {
        fn new(current: i64, before: i64) -> Self {
            FakeWorker {
                current,
                before,
                flaky: RefCell::new(Vec::new()),
                broken: Vec::new(),
                calls: RefCell::new(Vec::new()),
                not_ready: RefCell::new(0),
            }
        }
    }

    impl Wake for FakeWorker {
        fn post(
            &self,
            url: &str,
            headers: &[(String, String)],
            body: &str,
        ) -> Result<(u16, String), String> {
            assert!(url.ends_with("/_bynk/migrate/Cart"), "{url}");
            assert_eq!(headers[0].1, "bynk.migrate");
            if *self.not_ready.borrow() > 0 {
                *self.not_ready.borrow_mut() -= 1;
                return Ok((404, "Not found".into()));
            }
            let v: serde_json::Value = serde_json::from_str(body).unwrap();
            let ids: Vec<String> = v["ids"]
                .as_array()
                .unwrap()
                .iter()
                .map(|i| i.as_str().unwrap().to_string())
                .collect();
            self.calls.borrow_mut().push(ids.len());
            let results: Vec<serde_json::Value> = ids
                .iter()
                .map(|id| {
                    let mut flaky = self.flaky.borrow_mut();
                    if let Some(at) = flaky.iter().position(|f| f == id) {
                        flaky.remove(at);
                        serde_json::json!({ "id": id, "fault": "Unreachable" })
                    } else if self.broken.contains(id) {
                        serde_json::json!({ "id": id, "before": 1, "fault": "NoMigrationPath" })
                    } else {
                        serde_json::json!({ "id": id, "before": self.before, "after": self.current })
                    }
                })
                .collect();
            Ok((
                200,
                serde_json::json!({ "current": self.current, "results": results }).to_string(),
            ))
        }
    }

    const NOW: Timing = Timing {
        ready_attempts: 3,
        ready_pause: std::time::Duration::from_millis(0),
    };

    fn one_plan(ids: usize) -> BTreeMap<String, ScriptPlan> {
        let target = Target {
            class: "shop.Cart".into(),
            agent: "Cart".into(),
            worker: "shop".into(),
            environment: "default".into(),
            script: "shop".into(),
        };
        BTreeMap::from([(
            "shop".to_string(),
            ScriptPlan {
                url: "https://shop.acct.workers.dev".into(),
                classes: vec![(target, (0..ids).map(|i| format!("id{i}")).collect())],
                deployed_at: Ok(std::time::UNIX_EPOCH),
            },
        )])
    }

    fn run(
        plans: &BTreeMap<String, ScriptPlan>,
        secrets: &FakeSecrets,
        worker: &FakeWorker,
        stopped: bool,
    ) -> (Vec<ClassResult>, String) {
        let mut out = Vec::new();
        let results = execute(
            plans,
            secrets,
            worker,
            2,
            &NOW,
            &|| Ok("key".to_string()),
            &|| stopped,
            &mut out,
        );
        (results, String::from_utf8(out).unwrap())
    }

    #[test]
    fn a_sweep_batches_retries_once_and_deletes_its_key() {
        let secrets = FakeSecrets::default();
        let worker = FakeWorker::new(3, 2);
        worker.flaky.borrow_mut().push("id1".into());
        *worker.not_ready.borrow_mut() = 1;
        let (results, out) = run(&one_plan(5), &secrets, &worker, false);
        let r = &results[0];
        assert!(r.complete(), "{r:?}");
        assert_eq!((r.woke, r.current, r.oldest_after), (5, Some(3), Some(3)));
        // The readiness probe (empty), three batches of at most 2, then the
        // one retried id.
        assert_eq!(*worker.calls.borrow(), vec![0, 2, 2, 1, 1]);
        assert_eq!(
            *secrets.log.borrow(),
            vec![
                "put shop BYNK_MIGRATE_KEY".to_string(),
                "delete shop BYNK_MIGRATE_KEY".to_string()
            ]
        );
        assert!(
            out.contains(
                "shop (default): Cart  5 with stored data, woke 5 (0 failed); deployed at 3"
            ),
            "{out}"
        );
    }

    #[test]
    fn a_load_fault_leaves_the_sweep_incomplete() {
        let secrets = FakeSecrets::default();
        let mut worker = FakeWorker::new(3, 3);
        worker.broken.push("id0".into());
        let (results, out) = run(&one_plan(2), &secrets, &worker, false);
        assert!(!results[0].complete());
        assert_eq!(
            results[0].failed,
            vec![("id0".to_string(), "NoMigrationPath".to_string())]
        );
        assert!(out.contains("id0: NoMigrationPath"), "{out}");
        assert_eq!(secrets.log.borrow().len(), 2, "the key is still deleted");
    }

    #[test]
    fn a_failed_put_wakes_nothing_and_an_interrupt_still_deletes_the_key() {
        let secrets = FakeSecrets {
            fail_put: true,
            ..Default::default()
        };
        let worker = FakeWorker::new(3, 2);
        let (results, _) = run(&one_plan(2), &secrets, &worker, false);
        assert!(
            results[0]
                .error
                .as_deref()
                .unwrap()
                .contains("no permission")
        );
        assert!(worker.calls.borrow().is_empty());
        // A put that failed may still have landed, so the key is deleted too.
        assert_eq!(
            secrets.log.borrow().last().map(String::as_str),
            Some("delete shop BYNK_MIGRATE_KEY")
        );

        let secrets = FakeSecrets::default();
        let (results, _) = run(&one_plan(2), &secrets, &worker, true);
        assert_eq!(results[0].error.as_deref(), Some("interrupted"));
        assert_eq!(
            *secrets.log.borrow(),
            vec![
                "put shop BYNK_MIGRATE_KEY".to_string(),
                "delete shop BYNK_MIGRATE_KEY".to_string()
            ]
        );
    }

    #[test]
    fn a_worker_without_the_route_is_reported_as_needing_a_redeploy() {
        let secrets = FakeSecrets::default();
        let worker = FakeWorker::new(3, 2);
        *worker.not_ready.borrow_mut() = 10;
        let (results, _) = run(&one_plan(1), &secrets, &worker, false);
        let error = results[0].error.as_deref().unwrap();
        assert!(
            error.contains("no wake route for `Cart`: redeploy the `default` environment first"),
            "{error}"
        );
        assert_eq!(secrets.log.borrow().len(), 2, "the key is still deleted");
    }

    fn result(env: &str, current: i64, oldest: Option<i64>, complete: bool) -> ClassResult {
        ClassResult {
            target: Target {
                class: "shop.Cart".into(),
                agent: "Cart".into(),
                worker: "shop".into(),
                environment: env.into(),
                script: crate::deploy::deployed_worker_name(env, "shop"),
            },
            listed: 1,
            woke: 1,
            failed: if complete {
                Vec::new()
            } else {
                vec![("x".into(), "Unreachable".into())]
            },
            current: Some(current),
            oldest_after: oldest,
            error: None,
        }
    }

    #[test]
    fn the_floor_rises_only_on_a_complete_run_over_every_environment() {
        let lock = lock_with(&[("shop.Cart", 4, 1)]);
        let both = select(
            &lock,
            &deployed(&[("shop", &["default", "staging"])]),
            None,
            &[],
        )
        .unwrap();
        // Staging holds no instances but runs version 3: the floor stops at 3.
        let results = vec![
            result("default", 4, Some(4), true),
            result("staging", 3, None, true),
        ];
        assert_eq!(
            floor_raises(&lock, &both, &results, &BTreeMap::new()),
            vec![("shop.Cart".to_string(), 1, 3)]
        );
        // One failed wake anywhere holds it.
        let results = vec![
            result("default", 4, Some(4), false),
            result("staging", 4, None, true),
        ];
        assert!(floor_raises(&lock, &both, &results, &BTreeMap::new()).is_empty());
        assert_eq!(
            floor_held_reason("shop.Cart", &both, &results, &BTreeMap::new(), true).as_deref(),
            Some("the sweep was incomplete")
        );
        // A run narrowed to one environment never raises it.
        let one = select(
            &lock,
            &deployed(&[("shop", &["default", "staging"])]),
            None,
            &["default".to_string()],
        )
        .unwrap();
        let results = vec![result("default", 4, Some(4), true)];
        assert!(floor_raises(&lock, &one, &results, &BTreeMap::new()).is_empty());
        // A Worker deployed ahead of the local lock never pushes the floor
        // past the lock's own version.
        let results = vec![
            result("default", 5, Some(5), true),
            result("staging", 5, Some(5), true),
        ];
        assert_eq!(
            floor_raises(&lock, &both, &results, &BTreeMap::new()),
            vec![("shop.Cart".to_string(), 1, 4)]
        );
    }

    #[test]
    fn the_floor_waits_until_every_swept_worker_has_settled() {
        use std::time::{Duration, UNIX_EPOCH};
        let lock = lock_with(&[("shop.Cart", 4, 1)]);
        let both = select(
            &lock,
            &deployed(&[("shop", &["default", "staging"])]),
            None,
            &[],
        )
        .unwrap();
        let results = vec![
            result("default", 4, Some(4), true),
            result("staging", 4, None, true),
        ];
        let plan = |at: Result<std::time::SystemTime, String>, env: &str| ScriptPlan {
            url: String::new(),
            classes: vec![(result(env, 4, None, true).target, Vec::new())],
            deployed_at: at,
        };
        let now = UNIX_EPOCH + Duration::from_secs(10_000);
        let plans = BTreeMap::from([
            (
                "shop".to_string(),
                plan(Ok(now - Duration::from_secs(31 * 60)), "default"),
            ),
            (
                "shop-staging".to_string(),
                plan(Ok(now - Duration::from_secs(4 * 60)), "staging"),
            ),
        ]);
        let waiting = unsettled(&plans, now, SETTLING_MARGIN);
        assert_eq!(waiting.keys().collect::<Vec<_>>(), vec!["shop-staging"]);
        assert!(floor_raises(&lock, &both, &results, &waiting).is_empty());
        let why = floor_held_reason("shop.Cart", &both, &results, &waiting, true).unwrap();
        // A floor that would not have risen anyway is not blamed on the margin.
        assert_eq!(
            floor_held_reason("shop.Cart", &both, &results, &waiting, false),
            None
        );
        assert!(
            why.contains("`shop-staging` (staging) was deployed 4 minute(s) ago")
                && why.contains("sweep again in 26 minute(s)"),
            "{why}"
        );
        // Settled everywhere: the floor rises.
        let settled = unsettled(
            &BTreeMap::from([(
                "shop".to_string(),
                plan(Ok(now - Duration::from_secs(3600)), "default"),
            )]),
            now,
            SETTLING_MARGIN,
        );
        assert!(settled.is_empty());
        assert_eq!(
            floor_raises(&lock, &both, &results, &settled),
            vec![("shop.Cart".to_string(), 1, 4)]
        );
        // An unreadable deployment, or one ahead of the local clock, holds it.
        let held = unsettled(
            &BTreeMap::from([
                ("shop".to_string(), plan(Err("403".into()), "default")),
                (
                    "shop-staging".to_string(),
                    plan(Ok(now + Duration::from_secs(60)), "staging"),
                ),
            ]),
            now,
            SETTLING_MARGIN,
        );
        assert!(
            held["shop"].contains("could not read when `shop` (default) was last deployed: 403")
        );
        assert!(held["shop-staging"].contains("deployed 0 minute(s) ago"));
    }

    #[test]
    fn prepare_reads_each_workers_last_deployment_without_failing_on_it() {
        let lock = lock_with(&[("shop.Cart", 2, 1)]);
        let sel = select(
            &lock,
            &deployed(&[("shop", &["default", "staging"])]),
            None,
            &[],
        )
        .unwrap();
        let urls = BTreeMap::from([(
            "shop-staging".to_string(),
            "https://s.example.com".to_string(),
        )]);
        let plans = prepare(&sel, &FakeCloudflare, &urls).unwrap();
        assert_eq!(plans["shop"].deployed_at, Ok(std::time::UNIX_EPOCH));
        assert_eq!(
            plans["shop-staging"].deployed_at,
            Err("no permission".to_string())
        );
    }
}

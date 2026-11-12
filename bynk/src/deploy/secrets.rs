use super::*;

// ---------------------------------------------------------------------------
// Slice 3: secrets at deploy time (ADR 0195)
// ---------------------------------------------------------------------------

/// Where a secret's name came from — the plan's `declared`/`supplied` mark.
///
/// The distinction is the **floor, not a census** contract made visible
/// (ADR 0195 D2): `declared` is a name the compiler proved this Worker reads,
/// `supplied` is one only the user knows about. A reader must be able to tell
/// which of the two they are looking at, because the compiler's silence about a
/// `bynk.Secrets` name is not evidence that no such name exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Origin {
    /// The compiler proved a handler reads it *and* that its absence is
    /// fail-closed — an `actor`'s auth secret. Required: no value is an error.
    Declared,
    /// The compiler proved a handler reads it, but `Secrets.get` returns
    /// `Option`, so absence is a legitimate handled outcome (ADR 0196 D3).
    /// Advisory: no value warns.
    Read,
    /// The user named it. The compiler knows nothing about it either way.
    Supplied,
    /// I2 (#1971): an edge key, `BYNK_CALL_FROM_<caller>` on a callee and
    /// `BYNK_CALL_TO_<callee>` on a caller ([`Edge`]). `deploy` mints its value
    /// and sets it on both ends ([`decide_edge_key`]); it is never the user's.
    Managed,
}

impl Origin {
    /// Is a missing value fatal?
    ///
    /// Only for `Declared`. This is the whole of ADR 0196 D3, and the reason the
    /// increment does **not** promote a read into the required class: an unset
    /// auth secret 401s every request, while an unset `Secrets.get` name is a
    /// `None` the program may be entirely happy about — erroring on it would
    /// break a legal program.
    pub(crate) fn required(self) -> bool {
        matches!(self, Origin::Declared | Origin::Managed)
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Origin::Declared => "declared",
            Origin::Read => "read",
            Origin::Supplied => "supplied",
            Origin::Managed => "managed",
        }
    }
}

/// One secret this run intends to set on one context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WantedSecret {
    pub(crate) name: String,
    pub(crate) origin: Origin,
}

/// The user's secret input.
///
/// Names and values are separate questions (ADR 0195 D3). The file supplies
/// both; `--secret` supplies a name alone; the environment supplies values for
/// names **already known** and is never scanned for names — sweeping `env` into
/// Cloudflare would exfiltrate the user's whole shell.
#[derive(Debug, Default, Clone)]
pub(crate) struct SecretSource {
    /// `--secrets-file` — names and values.
    file: BTreeMap<String, String>,
    /// `--secret NAME` — names only.
    named: BTreeSet<String>,
}

impl SecretSource {
    /// A source holding `file`'s values, for a test.
    #[cfg(test)]
    pub(crate) fn of_values(file: &[(&str, &str)]) -> Self {
        Self {
            file: file
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            named: BTreeSet::new(),
        }
    }

    /// Read the source the options describe. Failing to read a named file is an
    /// error rather than an empty source: the user pointed at it, so silently
    /// proceeding with no values would surface later as a missing-secret error
    /// naming the wrong cause.
    pub(crate) fn read(opts: &DeployOptions) -> Result<Self, String> {
        let file = match &opts.secrets_file {
            Some(path) => {
                let text = std::fs::read_to_string(path)
                    .map_err(|e| format!("could not read {}: {e}", path.display()))?;
                parse_secrets_file(&text)
                    .map_err(|e| format!("could not read {}: {e}", path.display()))?
            }
            None => BTreeMap::new(),
        };
        Ok(Self {
            file,
            named: opts.secrets.iter().cloned().collect(),
        })
    }
}

/// Parse a dotenv-style `NAME=value` file.
///
/// Deliberately thin — `deploy` moves values, it does not store them, and the
/// source is an input rather than a vault this track owns. `#` comments, blank
/// lines, an optional `export ` prefix, and one layer of matching quotes around
/// a value that would otherwise lose its spacing.
///
/// A malformed line is an **error naming its number**, never a skip: a silently
/// dropped line is a secret that does not get set, which surfaces in production
/// as a 401 rather than here as a typo.
fn parse_secrets_file(text: &str) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
        let Some((name, value)) = line.split_once('=') else {
            return Err(format!("line {}: expected `NAME=value`", i + 1));
        };
        let name = name.trim();
        // Not a full Cloudflare naming rule — that is Cloudflare's to enforce,
        // and inventing one here would reject a name the platform accepts. This
        // catches the shapes that are unambiguously a typo in *this* file.
        if name.is_empty() || name.split_whitespace().count() > 1 {
            return Err(format!(
                "line {}: `{name}` is not a usable secret name",
                i + 1
            ));
        }
        if out
            .insert(name.to_string(), unquote(value.trim()))
            .is_some()
        {
            return Err(format!("line {}: `{name}` is set twice", i + 1));
        }
    }
    Ok(out)
}

/// Strip one layer of matching quotes, so a value with meaningful spacing can
/// survive the line trim.
fn unquote(value: &str) -> String {
    for quote in ['"', '\''] {
        if value.len() >= 2 && value.starts_with(quote) && value.ends_with(quote) {
            return value[1..value.len() - 1].to_string();
        }
    }
    value.to_string()
}

/// Which secrets this context wants set, and where each name came from.
///
/// Declared names are this context's own — the compiler proved *this* Worker
/// reads them. Supplied names go to **every** context in the run: nothing tells
/// `deploy` which contexts read a `bynk.Secrets` name, so the only available
/// answers are "all of them" or "none", and none would make `--secrets-file`
/// useless. The plan lists them per context so that spread is visible rather
/// than implied (ADR 0195 D2).
///
/// A name that is both declared and supplied is marked `declared`: the
/// compiler's knowledge is the more informative label, and it is the reason a
/// missing value is an error rather than a shrug.
pub(crate) fn wanted_secrets(
    declared: &[String],
    read: &[String],
    source: &SecretSource,
) -> Vec<WantedSecret> {
    let mut marks: BTreeMap<String, Origin> = BTreeMap::new();
    for name in source.file.keys().chain(source.named.iter()) {
        marks.insert(name.clone(), Origin::Supplied);
    }
    // Read beats supplied, and declared beats both: the marks are ordered by how
    // much the compiler knows, and the strongest thing known about a name is the
    // most useful label for it. `declared` last because it is the only one that
    // makes a missing value fatal.
    for name in read {
        marks.insert(name.clone(), Origin::Read);
    }
    for name in declared {
        marks.insert(name.clone(), Origin::Declared);
    }
    // I2 (#1971): the edge keys are handled apart (`decide_edge_key`), a value
    // supplied for one included: spread to every context, it would overwrite
    // each end's key with one the other end does not hold.
    marks.retain(|name, _| !is_edge_key(name));
    marks
        .into_iter()
        .map(|(name, origin)| WantedSecret { name, origin })
        .collect()
}

/// What the run will do with one secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SecretAction {
    Set,
    Overwrite,
    SkipPresent,
}

/// May a secret we cannot find a value for be left alone rather than failing the
/// deploy? Pure, so the rule is tested without an account.
///
/// Only where **both** hold: presence is unknown (the account could not be
/// asked), and the Worker has been live before. That combination is far more
/// likely to mean "already set, and the user had no reason to supply it again"
/// — the common CI redeploy, no `--secrets-file`, no TTY — than "genuinely
/// missing". Failing there would block a deploy that works, on the strength of a
/// *read* failure in a check that is advisory by design (ADR 0195 D4).
///
/// A first deploy gets no benefit of the doubt: its Worker is new, so an
/// unresolvable declared secret really is missing. And where presence *is*
/// known, the answer is authoritative — a secret we know to be absent, with no
/// value to set, is a real failure that must be named.
fn tolerate_unresolvable(present: Option<&BTreeSet<String>>, first_deploy: bool) -> bool {
    present.is_none() && !first_deploy
}

/// The set-if-absent rule (ADR 0195 D4). Pure, so the rule is tested without an
/// account.
///
/// `present` is `None` when the account could not be asked — a Worker that does
/// not exist yet, or an auth/network failure. Both are read as "assume nothing
/// is set and try": a first deploy genuinely has no secrets, and a real auth
/// failure then surfaces as the `secret put`'s own complaint rather than as a
/// diagnosis this function invented (the `queue_exists` posture).
fn secret_action(name: &str, present: Option<&BTreeSet<String>>, force: bool) -> SecretAction {
    match (present.is_some_and(|p| p.contains(name)), force) {
        (false, _) => SecretAction::Set,
        (true, true) => SecretAction::Overwrite,
        (true, false) => SecretAction::SkipPresent,
    }
}

/// The non-interactive half of D3's precedence: the file, else the environment.
/// `None` means only a prompt is left. Pure (the environment is read by the
/// caller), so the precedence is tested without touching the process env.
fn value_from(name: &str, source: &SecretSource, from_env: Option<String>) -> Option<String> {
    source.file.get(name).cloned().or(from_env)
}

/// I3 (#1971): a secret's value as the user supplied it, by the same
/// precedence as any other: the file, then the environment.
pub(crate) fn value_from_source(name: &str, source: &SecretSource) -> Option<String> {
    value_from(name, source, std::env::var(name).ok())
}

/// Whether `deploy` could see an edge key on one Worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Presence {
    Present,
    Absent,
    /// The account could not be asked, about a Worker that has been live.
    Unknown,
}

/// Presence of `name` on one Worker, from [`list_secrets`]' answer. A Worker
/// the ledger has never pushed has no secrets, whatever the answer was.
pub(crate) fn presence_of(
    name: &str,
    listed: Option<&BTreeSet<String>>,
    first_deploy: bool,
) -> Presence {
    match listed {
        _ if first_deploy => Presence::Absent,
        Some(names) if names.contains(name) => Presence::Present,
        Some(_) => Presence::Absent,
        None => Presence::Unknown,
    }
}

/// I2 (#1971): whether `name` is an edge key's, which `deploy` alone sets.
pub(crate) fn is_edge_key(name: &str) -> bool {
    name.starts_with(bynk_emit::EDGE_FROM_PREFIX) || name.starts_with(bynk_emit::EDGE_TO_PREFIX)
}

/// I2 (#1971): one edge of the deployment, by Worker: `caller` signs the
/// requests it sends `callee` (a call, or an event delivery from a publisher
/// to a subscriber) under a key the two share, and no one else holds.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Edge {
    pub(crate) caller: String,
    pub(crate) callee: String,
}

/// The qualified context a Worker serves. A Worker is named after its context
/// with each dot dashed (`bynk_project::paths::worker_dir_name`), and a context
/// name is identifiers, which never hold a dash, so this is the inverse.
pub(crate) fn context_of(worker: &str) -> String {
    worker.replace('-', ".")
}

impl Edge {
    /// Each end's Worker and the name it holds the edge's key under: the
    /// callee's `BYNK_CALL_FROM_<caller>`, then the caller's
    /// `BYNK_CALL_TO_<callee>`. An error names a context whose name no key
    /// can be derived from.
    pub(crate) fn ends(&self) -> Result<[(&str, String); 2], String> {
        let unnamed = |worker: &str| {
            format!(
                "context `{}` cannot sign or verify cross-context calls: an edge key is named from the context's name, which must be lowercase letters and digits in dot-separated segments, each starting with a letter, with single underscores between runs (no `__`, no trailing `_`)",
                context_of(worker)
            )
        };
        let from = bynk_emit::edge_key_from(&context_of(&self.caller))
            .ok_or_else(|| unnamed(&self.caller))?;
        let to = bynk_emit::edge_key_to(&context_of(&self.callee))
            .ok_or_else(|| unnamed(&self.callee))?;
        Ok([(&self.callee, from), (&self.caller, to)])
    }

    /// `caller → callee`, for a message: each end by its
    /// [`deployed_worker_name`] in `environment` (#2047).
    pub(crate) fn describe(&self, environment: &str) -> String {
        format!(
            "`{}` → `{}`",
            deployed_worker_name(environment, &self.caller),
            deployed_worker_name(environment, &self.callee)
        )
    }
}

/// Every edge of the project: a Worker's Service Binding to another of the
/// project's Workers is one (a caller to the context it calls, or a publisher
/// to a subscriber). A binding to anything else is not a Bynk edge.
pub(crate) fn project_edges(resources: &BTreeMap<String, Resources>) -> Vec<Edge> {
    let mut edges: Vec<Edge> = resources
        .iter()
        .flat_map(|(caller, r)| {
            r.binds_to
                .iter()
                .filter(|callee| resources.contains_key(*callee))
                .map(|callee| Edge {
                    caller: caller.clone(),
                    callee: callee.clone(),
                })
        })
        .collect();
    edges.sort();
    edges.dedup();
    edges
}

/// The names of the edge keys `worker` holds, in name order.
pub(crate) fn edge_keys_of(worker: &str, edges: &[Edge]) -> Vec<String> {
    let mut keys: Vec<String> = edges
        .iter()
        .filter_map(|e| e.ends().ok())
        .flat_map(|ends| ends.into_iter())
        .filter(|(w, _)| *w == worker)
        .map(|(_, key)| key)
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

/// What this run does with one edge's key (I2, #1971).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EdgeKey {
    /// Both ends already hold one: leave it.
    Keep,
    /// Put this value on both ends. `rotating` when an end already held a
    /// key, so a run stopped between the two ends leaves them disagreeing,
    /// and a plain rerun, seeing a key on each, would keep both.
    Set { value: String, rotating: bool },
}

/// Decide one edge's key for one run. Pure, so the rule is tested without an
/// account; `mint` is only called when a fresh value is wanted.
///
/// Both ends of an edge must hold the **same** value, and Cloudflare never
/// returns one, so a key one end holds cannot be copied to the other. The
/// ledger is committed, so it cannot hold the value either (ADR 0195 D1). So:
///
/// - Both ends hold a key, and `--force` is off: keep it. An unknown answer
///   about a Worker that has been live counts as holding it, as for any other
///   secret (`tolerate_unresolvable`).
/// - Otherwise mint a fresh key and put it on both ends: a new edge, a
///   project upgraded from before edge keys, a context redeployed after its
///   Worker was deleted, or `--force` (a rotation). That needs both ends in
///   the run. A `--context` run that would have to re-key an edge to a Worker
///   outside it is refused before anything is pushed; under `--force` such an
///   edge is kept, since only the edges the run deploys both ends of rotate.
pub(crate) fn decide_edge_key(
    edge: &Edge,
    caller: Presence,
    callee: Presence,
    both_in_run: bool,
    force: bool,
    mint: &mut dyn FnMut() -> Result<String, String>,
    environment: &str,
) -> Result<EdgeKey, String> {
    let held = caller != Presence::Absent && callee != Presence::Absent;
    if held && (!force || !both_in_run) {
        return Ok(EdgeKey::Keep);
    }
    if !both_in_run {
        return Err(format!(
            "the edge {} needs a new key on both Workers, but Cloudflare never returns a key's value, so a `--context` run cannot copy it to the Worker it does not deploy.\n  \
             Deploy the whole project (`{}`) to mint a key for every edge.",
            edge.describe(environment),
            deploy_command(environment)
        ));
    }
    Ok(EdgeKey::Set {
        value: mint()?,
        rotating: caller != Presence::Absent || callee != Presence::Absent,
    })
}

/// A fresh edge key: 32 random bytes, hex-encoded.
pub(crate) fn mint_edge_key() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| format!("could not generate an edge key: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

const BASE64URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Unpadded base64url, the form a session key takes (the runtime reads it strictly).
fn base64url_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
        let chars = chunk.len() + 1;
        for i in 0..chars {
            out.push(BASE64URL[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    out
}

/// The bytes of an unpadded base64url string, or `None` for anything else.
fn base64url_decode(s: &str) -> Option<Vec<u8>> {
    if s.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = BASE64URL.iter().position(|b| *b == c)? as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// The least a session key may be, in bytes (the runtime refuses a shorter one).
const SESSION_KEY_MIN_BYTES: usize = 32;

/// #3386: a fresh session key: 32 random bytes, unpadded base64url.
pub(crate) fn mint_session_key() -> Result<String, String> {
    let mut bytes = [0u8; SESSION_KEY_MIN_BYTES];
    getrandom::fill(&mut bytes).map_err(|e| format!("could not generate a session key: {e}"))?;
    Ok(base64url_encode(&bytes))
}

/// #3386: the signing key `bynk dev` uses for one session-key binding: derived from the binding's
/// name, so it differs per actor and is the same in every `bynk dev --context` session (two side by
/// side admit each other's cookies, as they admit each other's calls under the local edge key). It
/// is a local-development key only: it is public in this source and `deploy` never uses it.
pub(crate) fn local_session_key(binding: &str) -> String {
    use sha2::{Digest, Sha256};
    base64url_encode(&Sha256::digest(format!(
        "bynk dev session key v1:{binding}"
    )))
}

/// #3386: whether a value an operator supplies for a session key is one the runtime will accept:
/// canonical unpadded base64url of at least 32 bytes. Caught here, at deploy, rather than as a
/// faulted request on the first login.
pub(crate) fn check_session_key(value: &str) -> Result<(), String> {
    let bytes = base64url_decode(value).filter(|b| base64url_encode(b) == value);
    match bytes {
        None => Err("a session key is unpadded base64url (letters, digits, `-` and `_`)".into()),
        Some(b) if b.len() < SESSION_KEY_MIN_BYTES => Err(format!(
            "a session key must be at least {SESSION_KEY_MIN_BYTES} bytes; this one is {}",
            b.len()
        )),
        Some(_) => Ok(()),
    }
}

/// #3386: what this run does with a `SignedCookie` actor's signing key, on the context that
/// declares it: as for a foreign caller's secret ([`decide_foreign_key`]), except that a value the
/// operator supplies must be a key the runtime accepts, and nothing is handed off (only this
/// context holds it). Unset, `deploy` mints one; set, it is kept unless `--force`, which replaces
/// it, **ending every session signed under the old key** (Cloudflare never returns a secret's
/// value, so a minted key cannot be kept as the `previous` one: supply `secret` and `previous`
/// yourself to rotate without a global logout).
pub(crate) fn decide_session_key(
    supplied: Option<String>,
    presence: Presence,
    force: bool,
    mint: &mut dyn FnMut() -> Result<String, String>,
) -> Result<ForeignKey, String> {
    if let Some(value) = &supplied {
        check_session_key(value)?;
    }
    decide_foreign_key(supplied, presence, force, mint)
}

/// I3 (#1971): what this run does with one foreign caller's secret, on the
/// context that admits it. Only that end is ours: the foreign Worker's
/// operator sets the other from the hand-off [`foreign_hand_off`] prints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ForeignKey {
    /// The context already holds one, and `--force` is off: leave it.
    Keep,
    /// Put this value on the context. `minted` when `deploy` made it, so the
    /// foreign Worker cannot know it yet and the hand-off must be printed.
    Set { value: String, minted: bool },
}

/// Decide one foreign caller's secret for one run. Pure; `mint` is only called
/// when a fresh value is wanted.
///
/// - A value the user supplies (`--secrets-file` or the environment) is the one
///   they already share with the foreign Worker: set it where absent, and under
///   `--force`. Nothing is printed.
/// - Otherwise, if the context holds one (an unknown answer about a live Worker
///   counts as holding it), and `--force` is off, keep it.
/// - Otherwise mint one, set it, and print the hand-off. A `--context` run may
///   do this: the foreign end is never ours to set.
pub(crate) fn decide_foreign_key(
    supplied: Option<String>,
    presence: Presence,
    force: bool,
    mint: &mut dyn FnMut() -> Result<String, String>,
) -> Result<ForeignKey, String> {
    if let Some(value) = supplied {
        if value.is_empty() {
            return Err("the supplied value is empty — every call from it would be refused".into());
        }
        if presence == Presence::Present && !force {
            return Ok(ForeignKey::Keep);
        }
        return Ok(ForeignKey::Set {
            value,
            minted: false,
        });
    }
    if presence != Presence::Absent && !force {
        return Ok(ForeignKey::Keep);
    }
    Ok(ForeignKey::Set {
        value: mint()?,
        minted: true,
    })
}

/// I3 (#1971): what `deploy` prints once a minted foreign caller's secret is
/// set on `worker`: a ready-to-run `wrangler secret put` for the foreign
/// Worker's operator (§11 Q4: `deploy` never writes to a Worker it does not
/// own). The one place a minted value is ever shown. The foreign Worker holds
/// it under the name a Bynk caller would (`BYNK_CALL_TO_<CALLEE>`), and the
/// command names that Worker when the declaration did. Its remedy repeats the
/// run's `--env` ([`deploy_command`], #2047).
pub(crate) fn foreign_hand_off(
    environment: &str,
    worker: &str,
    caller: &ForeignCaller,
    value: &str,
    runner: &str,
) -> String {
    let context = context_of(worker);
    let key = bynk_emit::edge_key_to(&context).unwrap_or_else(|| "BYNK_CALL_KEY".to_string());
    let name = caller
        .worker
        .clone()
        .unwrap_or_else(|| "<foreign-worker>".to_string());
    format!(
        "bynk: minted {binding} for `{actor}`, a foreign caller of `{context}`. Its operator sets it on their Worker:\n\n  \
         printf '%s' '{value}' | {runner} wrangler secret put {key} --name {name}\n\n  \
         It signs its calls to `{context}` as `{actor}` (X-Bynk-Caller), under that key; see the wire format reference.\n  \
         This is the only time the value is shown: `{deploy} --force` mints a new one.\n",
        binding = caller.binding,
        actor = caller.actor,
        deploy = deploy_command(environment),
    )
}

/// #3438: how a user runs `wrangler` by hand in this project, for the commands
/// `deploy` prints: the detected package manager's own runner (`npx`,
/// `pnpm dlx`, `bunx`), and `npx` for Yarn classic, which has none (#3426).
pub(crate) fn wrangler_runner(manager: bynk_driver::npm_install::PackageManager) -> &'static str {
    manager.runner_label().unwrap_or("npx")
}

/// [`wrangler_runner`] for the manager detected in `project_root`.
pub(crate) fn wrangler_runner_in(project_root: &std::path::Path) -> &'static str {
    wrangler_runner(bynk_driver::npm_install::system_tool_manager(project_root))
}

/// Everything the secret step needs, threaded as one value rather than as four
/// more parameters on an already-long list.
pub(crate) struct Secrets<'a> {
    pub(crate) source: &'a SecretSource,
    pub(crate) force: bool,
    /// I2 (#1971): what [`decide_edge_key`] decided for each edge this run
    /// sets a key on.
    pub(crate) edge_keys: &'a BTreeMap<Edge, EdgeKey>,
    /// I3 (#1971): what [`decide_foreign_key`] decided, by (worker, secret).
    pub(crate) foreign_keys: &'a BTreeMap<(String, String), ForeignKey>,
    /// I3 (#1971): every foreign caller's secret in the project, which a
    /// supplied value is never spread as.
    pub(crate) foreign_secrets: &'a BTreeSet<String>,
    /// #2004: each Worker's `secret list` answer, asked once before the loop
    /// (edge keys are decided over both ends at once) and reused here.
    pub(crate) listed: &'a BTreeMap<String, Option<BTreeSet<String>>>,
    /// Values resolved earlier in this run, so two contexts wanting the same
    /// name prompt once rather than once each.
    ///
    /// In memory only, and dropped with the run: holding a value long enough to
    /// hand it to wrangler is unavoidable, but nothing here is ever written —
    /// the ledger records no secret, not even its presence (ADR 0195 D1).
    pub(crate) resolved: &'a mut BTreeMap<String, String>,
    /// #2055: whether the wrangler running this deploy takes
    /// `wrangler deploy --secrets-file`, so a first deploy can upload its
    /// secrets with the Worker's first version. `false` only for an installed
    /// wrangler known to be older ([`secrets_file_supported`]).
    pub(crate) with_upload: bool,
}

/// Decide what to set and resolve every value — the non-mutating half.
///
/// Returns the `(name, value)` pairs [`apply_secrets`] will put. Split from the
/// put so that **every failure a user can act on happens before anything is
/// pushed**, on both sides of D6's straddle: a first deploy uploads its secrets
/// with the push (#2055), so resolving lazily there would surface a missing
/// value only once the push was under way. Nothing here touches Cloudflare
/// except the advisory presence read.
#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_bindings(
    provenance: &Provenance,
    worker_dir: &Path,
    worker: &str,
    declared: &Resources,
    secrets: &mut Secrets<'_>,
    first_deploy: bool,
    environment: &str,
) -> Result<Vec<(String, String)>, DeployFailure> {
    // I3 (#1971): a foreign caller's secret, supplied or not, is decided by
    // `decide_foreign_key` for the context that admits it, never spread.
    let wanted: Vec<WantedSecret> = wanted_secrets(
        &declared.declared_secrets,
        &declared.read_secrets,
        secrets.source,
    )
    .into_iter()
    .filter(|w| !secrets.foreign_secrets.contains(&w.name))
    .collect();
    // Said once per context, before any of it: a reader who takes the lines
    // below for the whole story would be wrong, and this is the only place that
    // can tell them (ADR 0196 D2).
    if !declared.reads_complete {
        eprint!("{}", unseen_reads_notice(environment, worker));
    }
    // Asked live, never recorded: presence is the only observable Cloudflare
    // offers (it does not return values), and a recorded answer could only ever
    // be a stale one (ADR 0195 D1/D4). Asked *before* the push on both paths —
    // our own `wrangler deploy` does not change which secrets are set, so one
    // answer serves the whole step, and `secret list` has no draft-Worker path
    // to trip over on a first deploy. #2004: the run asks once per Worker
    // before the loop, so this reuses that answer.
    let present = match secrets.listed.get(worker) {
        Some(listed) => listed.clone(),
        None => list_secrets(provenance, worker_dir, environment),
    };
    let mut prepared = Vec::new();
    // I2 (#1971): this Worker's end of each edge being keyed, first, so a
    // Worker never runs a moment without a key it could have had.
    for (edge, decision) in secrets.edge_keys {
        let EdgeKey::Set { value, .. } = decision else {
            continue;
        };
        let ends = edge.ends().map_err(DeployFailure::driver)?;
        for (end, key) in ends {
            if end == worker {
                prepared.push((key, value.clone()));
            }
        }
    }
    // I3 (#1971): and its foreign callers' secrets.
    for ((w, secret), decision) in secrets.foreign_keys {
        if let (true, ForeignKey::Set { value, .. }) = (w == worker, decision) {
            prepared.push((secret.clone(), value.clone()));
        }
    }
    for want in &wanted {
        match secret_action(&want.name, present.as_ref(), secrets.force) {
            SecretAction::SkipPresent => {
                eprint!("{}", kept_binding_notice(environment, worker, &want.name));
                continue;
            }
            SecretAction::Set | SecretAction::Overwrite => {}
        }
        match resolve_secret_value(&want.name, want.origin, worker, secrets, environment) {
            Ok(value) => prepared.push((want.name.clone(), value)),
            // Advisory by type (ADR 0196 D3): `Secrets.get` returns `Option`, so
            // a read with no value is a `None` the program may be entirely happy
            // about. Say what will happen and carry on — failing here would
            // refuse to deploy a legal program over a secret it never needed.
            Err(_) if !want.origin.required() => {
                eprint!(
                    "{}",
                    unsupplied_read_notice(environment, worker, &want.name)
                );
            }
            Err(_) if tolerate_unresolvable(present.as_ref(), first_deploy) => {
                eprint!(
                    "{}",
                    unknown_presence_notice(environment, worker, &want.name)
                );
            }
            Err(e) => return Err(e),
        }
    }
    Ok(prepared)
}

// #2047: the notices `prepare_bindings` and `apply_secrets` print, each naming
// the Worker by its `deployed_worker_name` in the run's environment, the
// Worker the secret is (or is not) set on. Pure, so they are goldened.

/// The Worker reads secrets the compiler cannot name (ADR 0196 D2).
pub(crate) fn unseen_reads_notice(environment: &str, worker: &str) -> String {
    format!(
        "bynk: `{}` reads at least one secret whose name the compiler cannot see (a computed `Secrets.get` name, or an adapter binding given `Secrets`), so the list below is not everything it reads.\n",
        deployed_worker_name(environment, worker)
    )
}

/// A secret already set, kept because `--force` is off.
pub(crate) fn kept_binding_notice(environment: &str, worker: &str, binding_name: &str) -> String {
    format!(
        "bynk: secret `{binding_name}` is already set on `{}`, skipping — use --force to overwrite\n",
        deployed_worker_name(environment, worker)
    )
}

/// An optional read with no value supplied (ADR 0196 D3).
pub(crate) fn unsupplied_read_notice(
    environment: &str,
    worker: &str,
    binding_name: &str,
) -> String {
    format!(
        "bynk: `{}` reads the secret `{binding_name}`, but no value was supplied — it will see None.\n  \
         Supply it with --secrets-file or --secret {binding_name}, if it is meant to be set.\n",
        deployed_worker_name(environment, worker)
    )
}

/// A required secret with no value, on a Worker whose secrets could not be
/// listed: left as it is. The check it suggests names the deployed Worker,
/// so it asks about the right one whatever directory it is run from.
pub(crate) fn unknown_presence_notice(
    environment: &str,
    worker: &str,
    binding_name: &str,
) -> String {
    let worker = deployed_worker_name(environment, worker);
    format!(
        "bynk: could not ask Cloudflare which secrets `{worker}` has, and no value for `{binding_name}` was supplied — leaving it as it is.\n  \
         If it is not set, `{worker}` will answer 401. Check with `wrangler secret list --name {worker}`, or supply a value to set it.\n"
    )
}

/// `wrangler secret put` failed.
pub(crate) fn binding_put_failure(
    environment: &str,
    worker: &str,
    binding_name: &str,
    e: &str,
) -> String {
    format!(
        "could not set the secret `{binding_name}` on `{}`: {e}",
        deployed_worker_name(environment, worker)
    )
}

/// Put each prepared secret. The mutating half, and deliberately the whole of
/// it: everything that can fail for a reason the user can fix — a missing value,
/// a malformed file — has already happened by the time this runs.
pub(crate) fn apply_secrets(
    provenance: &Provenance,
    worker_dir: &Path,
    worker: &str,
    prepared: &[(String, String)],
    environment: &str,
) -> Result<(), DeployFailure> {
    for (binding_name, value) in prepared {
        set_secret(provenance, worker_dir, binding_name, value, environment).map_err(|e| {
            DeployFailure::driver(binding_put_failure(environment, worker, binding_name, &e))
        })?;
        eprintln!(
            "{}",
            binding_set_line(
                binding_name,
                &deployed_worker_name(environment, worker),
                false
            )
        );
    }
    Ok(())
}

/// #2055: say, by name only, each binding a new Worker's first version was
/// uploaded with. The values beside the names are never read here.
pub(crate) fn report_first_version_bindings(prepared: &[(String, String)], deployed: &str) {
    for (binding_name, _) in prepared {
        eprintln!("{}", binding_set_line(binding_name, deployed, true));
    }
}

/// #2055: the line a deploy prints once it has set a secret — the **name**
/// only, never the value — beside the "already set, skipping" line, so a run
/// says what it changed as well as what it left alone. `deployed` is the
/// Worker's account name ([`deployed_worker_name`]), `-staging` under
/// `--env staging`, since that is the Worker the secret went to.
/// `with_first_version` is a new Worker's case: the secret went up with its
/// first version rather than as a version of its own.
pub(crate) fn binding_set_line(
    binding_name: &str,
    deployed: &str,
    with_first_version: bool,
) -> String {
    if with_first_version {
        format!("bynk: set secret `{binding_name}` on `{deployed}`, with its first version")
    } else {
        format!("bynk: set secret `{binding_name}` on `{deployed}`")
    }
}

/// The value for one secret: this run's cache, then the file, then the
/// environment, then a prompt (ADR 0195 D3).
///
/// A name with no value anywhere is a **hard error naming it** when there is no
/// terminal to ask — never a blank. A blank would be worse than the failure it
/// replaces: the deploy would report success and the Worker would 401 every
/// request, with nothing to read that says why.
fn resolve_secret_value(
    name: &str,
    origin: Origin,
    worker: &str,
    secrets: &mut Secrets<'_>,
    environment: &str,
) -> Result<String, DeployFailure> {
    if let Some(cached) = secrets.resolved.get(name) {
        return Ok(cached.clone());
    }
    let from_env = std::env::var(name).ok();
    let value = match value_from(name, secrets.source, from_env) {
        Some(value) => value,
        None => prompt_for_secret(name).ok_or_else(|| {
            DeployFailure::driver(missing_binding_message(environment, name, origin, worker))
        })?,
    };
    secrets.resolved.insert(name.to_string(), value.clone());
    Ok(value)
}

/// Ask for a value, when there is a terminal to ask.
///
/// `None` when there is no TTY (CI, a piped session) — the caller turns that
/// into the named error. Read from the terminal without echoing would be better
/// still; v1 does not, so the prompt says so rather than implying secrecy it
/// does not provide.
fn prompt_for_secret(name: &str) -> Option<String> {
    if !io::stdin().is_terminal() {
        return None;
    }
    eprint!("Secret `{name}` (input is visible): ");
    let _ = io::stderr().flush();
    let mut answer = String::new();
    if io::stdin().read_line(&mut answer).is_err() {
        return None;
    }
    let answer = answer.trim_end_matches(['\n', '\r']).to_string();
    (!answer.is_empty()).then_some(answer)
}

/// Why a secret could not be resolved, in the vocabulary of where its name came
/// from — the two origins are missing for different reasons and want different
/// remedies. The Worker by its [`deployed_worker_name`] (#2047).
pub(crate) fn missing_binding_message(
    environment: &str,
    binding_name: &str,
    origin: Origin,
    worker: &str,
) -> String {
    let worker = deployed_worker_name(environment, worker);
    match origin {
        Origin::Declared => format!(
            "`{worker}` declares the secret `{binding_name}` (an actor's `auth` secret) but no value was supplied — \
             pass it in --secrets-file, or set {binding_name} in the environment. \
             Deploying without it would answer every request with 401."
        ),
        // Reached only when a read is *required*, which it never is — kept
        // total rather than unreachable!(), since the compiler cannot know that
        // and a panic here would be a poor way to learn otherwise.
        Origin::Read | Origin::Supplied | Origin::Managed => format!(
            "the secret `{binding_name}` was named but no value was supplied — \
             set {binding_name} in the environment, or give it a value in --secrets-file."
        ),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::deploy::config::tests::project;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// A user secret input literal: `source(&[("A", "v")], &["B"])` is a
    /// `--secrets-file` carrying `A=v` plus a `--secret B`.
    pub(crate) fn source(file: &[(&str, &str)], named: &[&str]) -> SecretSource {
        SecretSource {
            file: file
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            named: names(named).into_iter().collect(),
        }
    }

    // ---- #602 slice 3: secrets at deploy (ADR 0195) --------------------

    #[test]
    fn a_secrets_file_is_read_as_names_and_values() {
        let parsed = parse_secrets_file(
            r#"
# a comment
API_KEY=sk_live_abc

export EXPORTED=fine
QUOTED="  spaced  "
SINGLE='single'
EMPTY=
WITH_EQUALS=a=b
"#,
        )
        .expect("a well-formed file parses");
        assert_eq!(
            parsed.get("API_KEY").map(String::as_str),
            Some("sk_live_abc")
        );
        assert_eq!(
            parsed.get("EXPORTED").map(String::as_str),
            Some("fine"),
            "an `export ` prefix is the shape a shell-sourced file already has"
        );
        assert_eq!(
            parsed.get("QUOTED").map(String::as_str),
            Some("  spaced  "),
            "quotes are what let a value keep spacing the line trim would eat"
        );
        assert_eq!(parsed.get("SINGLE").map(String::as_str), Some("single"));
        assert_eq!(
            parsed.get("EMPTY").map(String::as_str),
            Some(""),
            "an empty value is the user's to mean; only a missing *name* is an error"
        );
        assert_eq!(
            parsed.get("WITH_EQUALS").map(String::as_str),
            Some("a=b"),
            "only the first `=` separates — a value may contain more"
        );
        assert!(!parsed.contains_key("# a comment"));
    }

    #[test]
    fn a_malformed_secrets_line_is_an_error_naming_its_number() {
        // The consequence that makes this worth failing on: a silently skipped
        // line is a secret that never gets set, which surfaces in production as
        // a 401 rather than here as a typo.
        let err = parse_secrets_file("GOOD=1\nnonsense\n").expect_err("a bare word is not a pair");
        assert!(err.contains("line 2"), "{err}");
        let err = parse_secrets_file("A B=1\n").expect_err("a spaced name is a typo");
        assert!(err.contains("line 1"), "{err}");
        let err = parse_secrets_file("=1\n").expect_err("no name at all");
        assert!(err.contains("line 1"), "{err}");
        let err = parse_secrets_file("A=1\nA=2\n").expect_err("set twice");
        assert!(err.contains("line 2") && err.contains('A'), "{err}");
    }

    #[test]
    fn the_wanted_set_is_declared_union_supplied_and_declared_wins_the_mark() {
        let src = source(&[("STRIPE_KEY", "v")], &["PROBE_TOKEN"]);
        let wanted = wanted_secrets(&names(&["AUTH_JWT_SECRET"]), &[], &src);
        assert_eq!(
            wanted,
            vec![
                WantedSecret {
                    name: "AUTH_JWT_SECRET".into(),
                    origin: Origin::Declared
                },
                WantedSecret {
                    name: "PROBE_TOKEN".into(),
                    origin: Origin::Supplied
                },
                WantedSecret {
                    name: "STRIPE_KEY".into(),
                    origin: Origin::Supplied
                },
            ]
        );

        // A name that is both is `declared`: the compiler's knowledge is the
        // more informative label, and it is why a missing value is an error.
        let both = wanted_secrets(&names(&["SHARED"]), &[], &source(&[("SHARED", "v")], &[]));
        assert_eq!(both[0].origin, Origin::Declared);

        // No input at all: nothing to set, and in particular no invented name.
        assert!(wanted_secrets(&[], &[], &SecretSource::default()).is_empty());
    }

    #[test]
    fn set_if_absent_skips_a_present_secret_unless_forced() {
        let present: BTreeSet<String> = ["THERE".to_string()].into_iter().collect();
        assert_eq!(
            secret_action("THERE", Some(&present), false),
            SecretAction::SkipPresent,
            "the default must not cut a fresh Cloudflare version every deploy"
        );
        assert_eq!(
            secret_action("THERE", Some(&present), true),
            SecretAction::Overwrite
        );
        assert_eq!(
            secret_action("ABSENT", Some(&present), false),
            SecretAction::Set
        );

        // `None` is "could not ask" — a Worker that does not exist yet, or an
        // auth failure. Both mean try: a first deploy genuinely has none, and a
        // real auth failure surfaces as the put's own complaint rather than as
        // a diagnosis invented here.
        assert_eq!(secret_action("THERE", None, false), SecretAction::Set);
    }

    #[test]
    fn an_unaskable_account_does_not_fail_a_redeploy_that_supplies_nothing() {
        // The regression this encodes: `secret_action` reads "could not ask" as
        // `Set`, so a redeploy whose secrets are all already on the account —
        // the common CI shape, no --secrets-file, no TTY — would try to resolve
        // a value it has no reason to have, find none, and fail. A *read*
        // failure in an advisory check would block a deploy that works, and both
        // the code's own comment and ADR 0195 D4 claimed it would not.
        //
        // `tolerate_unresolvable` is the rule: it holds only where presence is
        // unknown *and* the Worker has been live before.
        assert!(
            tolerate_unresolvable(None, false),
            "a redeploy with no presence answer leaves the secret alone"
        );
        // A first deploy gets no benefit of the doubt — its Worker is new, so an
        // unresolvable declared secret really is missing.
        assert!(!tolerate_unresolvable(None, true));
        // Presence known: the answer is authoritative either way, so an
        // unresolvable secret we know to be absent is a real failure.
        let none_present = BTreeSet::new();
        assert!(!tolerate_unresolvable(Some(&none_present), false));
        assert!(!tolerate_unresolvable(Some(&none_present), true));
    }

    #[test]
    fn a_value_comes_from_the_file_before_the_environment() {
        let src = source(&[("A", "from-file")], &[]);
        assert_eq!(
            value_from("A", &src, Some("from-env".into())).as_deref(),
            Some("from-file"),
            "the file is the more specific instruction, so it wins"
        );
        assert_eq!(
            value_from("B", &src, Some("from-env".into())).as_deref(),
            Some("from-env")
        );
        assert_eq!(
            value_from("C", &src, None),
            None,
            "nothing left but a prompt — and, with no terminal, a named error"
        );
    }

    #[test]
    fn wranglers_secret_list_is_read_as_names() {
        // Wrangler's shape (`secret list --format json`, wrangler 4.103).
        assert_eq!(
            parse_secret_list(
                r#"[{"name":"A","type":"secret_text"},{"name":"B","type":"secret_text"}]"#
            ),
            Some(["A".to_string(), "B".to_string()].into_iter().collect()),
        );
        assert_eq!(parse_secret_list("[]"), Some(BTreeSet::new()));
        // Anything unreadable is "could not tell" rather than "none present" —
        // and `secret_action` reads that as "try", which is idempotent.
        assert_eq!(parse_secret_list("not json"), None);
        assert_eq!(parse_secret_list(r#"{"unexpected":"shape"}"#), None);
    }

    #[test]
    fn a_missing_secret_names_itself_and_says_what_it_costs() {
        // The error is the whole mitigation for the silent-blank risk, so it
        // must name the secret and the remedy rather than saying "failed".
        let declared =
            missing_binding_message("default", "AUTH_JWT_SECRET", Origin::Declared, "api");
        assert!(declared.contains("AUTH_JWT_SECRET"), "{declared}");
        assert!(declared.contains("api"), "{declared}");
        assert!(
            declared.contains("401"),
            "a declared secret's absence is fail-closed — say so: {declared}"
        );
        let supplied = missing_binding_message("default", "STRIPE_KEY", Origin::Supplied, "api");
        assert!(supplied.contains("STRIPE_KEY"), "{supplied}");
        assert!(
            !supplied.contains("401"),
            "a supplied name is not known to gate auth, so do not claim it does: {supplied}"
        );
    }

    #[test]
    fn a_read_is_advisory_and_a_declared_secret_is_not() {
        // ADR 0196 D3, the increment's load-bearing distinction. `Secrets.get`
        // returns `Option`, so an unsupplied read is a `None` the program may be
        // happy about; an unset auth secret 401s every request. Erroring on the
        // first would refuse to deploy a legal program.
        assert!(Origin::Declared.required());
        assert!(!Origin::Read.required());
        assert!(!Origin::Supplied.required());
        assert_eq!(Origin::Read.label(), "read");
    }

    #[test]
    fn the_marks_are_ordered_by_how_much_the_compiler_knows() {
        // A name can be in more than one class. The strongest thing known about
        // it is the most useful label — and `declared` is the only one that
        // makes a missing value fatal, so it must win.
        let src = source(&[("BOTH", "v"), ("SUPPLIED_ONLY", "v")], &[]);
        let wanted = wanted_secrets(&names(&["BOTH"]), &names(&["BOTH", "READ_ONLY"]), &src);
        let mark = |n: &str| {
            wanted
                .iter()
                .find(|w| w.name == n)
                .unwrap_or_else(|| panic!("{n} is wanted"))
                .origin
        };
        assert_eq!(
            mark("BOTH"),
            Origin::Declared,
            "declared beats read and supplied"
        );
        assert_eq!(mark("READ_ONLY"), Origin::Read);
        assert_eq!(mark("SUPPLIED_ONLY"), Origin::Supplied);
    }

    #[test]
    fn the_plan_never_carries_a_secret_value() {
        // ADR 0195 D1's headline guarantee, asserted rather than described: the
        // plan is printed and piped to CI logs, so a value reaching it would be
        // a leak in the most-copied surface `deploy` has.
        const SENTINEL: &str = "sk_live_do_not_leak_me";
        let order = names(&["api"]);
        let declared = project(vec![(
            "api",
            Resources::default().declares(&["AUTH_JWT_SECRET"]),
        )]);
        let plan = derive_plan(
            &order,
            &declared,
            &DeployLock::default(),
            &source(&[("STRIPE_KEY", SENTINEL)], &[]),
            false,
            "default",
        );
        for format in [DeployFormat::Short, DeployFormat::Json] {
            let rendered = plan_report(&plan, format);
            assert!(
                !rendered.contains(SENTINEL),
                "a secret value reached the plan ({format:?}): {rendered}"
            );
            assert!(
                rendered.contains("STRIPE_KEY"),
                "the name is the whole point of the line: {rendered}"
            );
        }
    }

    #[test]
    fn the_ledger_never_carries_a_secret() {
        // The other half of D1: the ledger is a *committed* file, so a value —
        // or even a name — reaching it would be published, not merely logged.
        // The type has no field for one; this pins that as a property rather
        // than an observation about today's struct.
        const SENTINEL: &str = "sk_live_do_not_commit_me";
        let mut lock = DeployLock::default();
        lock.record_deployed("default", "api", Some(Default::default()));
        lock.record_queue("default", "intake");
        let text = toml::to_string_pretty(&lock).expect("the ledger serialises");
        assert!(!text.contains(SENTINEL));
        assert!(
            !text.to_ascii_lowercase().contains("secret"),
            "the ledger records no secret at all — not even its presence: {text}"
        );
    }

    // ---- I2 (#1971): per-edge keys -------------------------------------------

    use Presence::{Absent, Present, Unknown};

    fn edge(caller: &str, callee: &str) -> Edge {
        Edge {
            caller: caller.to_string(),
            callee: callee.to_string(),
        }
    }

    fn minted() -> Result<String, String> {
        Ok("minted".to_string())
    }

    fn never() -> Result<String, String> {
        panic!("a run that keeps a key must not mint one")
    }

    fn decide(
        caller: Presence,
        callee: Presence,
        both_in_run: bool,
        force: bool,
        mut mint: impl FnMut() -> Result<String, String>,
    ) -> Result<EdgeKey, String> {
        decide_edge_key(
            &edge("shop-orders", "shop-payment"),
            caller,
            callee,
            both_in_run,
            force,
            &mut mint,
            "default",
        )
    }

    fn set(rotating: bool) -> Result<EdgeKey, String> {
        Ok(EdgeKey::Set {
            value: "minted".to_string(),
            rotating,
        })
    }

    #[test]
    fn an_edge_names_its_key_on_each_end() {
        let e = edge("shop-orders", "shop-payment");
        let ends = e.ends().unwrap();
        assert_eq!(
            ends,
            [
                ("shop-payment", "BYNK_CALL_FROM_SHOP__ORDERS".to_string()),
                ("shop-orders", "BYNK_CALL_TO_SHOP__PAYMENT".to_string()),
            ]
        );
        let refused = edge("a__b", "c").ends().expect_err("not an edge name");
        assert!(refused.contains("`a__b`"), "{refused}");
    }

    #[test]
    fn the_project_edges_are_its_bindings_between_its_own_workers() {
        let mut resources = BTreeMap::new();
        resources.insert(
            "shop-orders".to_string(),
            Resources {
                binds_to: vec!["shop-payment".to_string(), "not-a-worker".to_string()],
                ..Resources::default()
            },
        );
        resources.insert(
            "shop-payment".to_string(),
            Resources {
                binds_to: vec!["shop-payment".to_string()],
                ..Resources::default()
            },
        );
        let edges = project_edges(&resources);
        assert_eq!(
            edges,
            vec![
                edge("shop-orders", "shop-payment"),
                edge("shop-payment", "shop-payment"),
            ]
        );
        assert_eq!(
            edge_keys_of("shop-payment", &edges),
            vec![
                "BYNK_CALL_FROM_SHOP__ORDERS".to_string(),
                "BYNK_CALL_FROM_SHOP__PAYMENT".to_string(),
                "BYNK_CALL_TO_SHOP__PAYMENT".to_string(),
            ],
            "a context delivering its own events holds both ends of its own edge"
        );
        assert_eq!(
            edge_keys_of("shop-orders", &edges),
            vec!["BYNK_CALL_TO_SHOP__PAYMENT".to_string()]
        );
    }

    #[test]
    fn a_new_edge_is_keyed_on_both_ends() {
        assert_eq!(decide(Absent, Absent, true, false, minted), set(false));
    }

    #[test]
    fn an_edge_both_ends_hold_keeps_its_key() {
        assert_eq!(
            decide(Present, Present, true, false, never),
            Ok(EdgeKey::Keep)
        );
        // An unknown answer about a live Worker is read as holding it, as for
        // any other secret (`tolerate_unresolvable`).
        assert_eq!(
            decide(Present, Unknown, true, false, never),
            Ok(EdgeKey::Keep)
        );
        // `--context`: nothing needs copying.
        assert_eq!(
            decide(Present, Present, false, false, never),
            Ok(EdgeKey::Keep)
        );
    }

    #[test]
    fn an_end_without_the_key_rekeys_the_edge() {
        // The value the other end holds cannot be read back, so both get a
        // fresh one; one end held a key, so an interruption would split them.
        assert_eq!(decide(Present, Absent, true, false, minted), set(true));
        // `--force` rotates even when both ends hold one.
        assert_eq!(decide(Present, Present, true, true, minted), set(true));
    }

    #[test]
    fn a_context_run_cannot_rekey_an_edge_to_a_worker_it_does_not_deploy() {
        let refused = decide(Present, Absent, false, false, never)
            .expect_err("a --context run that needs a key on a Worker outside it is refused");
        assert!(
            refused.contains("`shop-orders` → `shop-payment`"),
            "{refused}"
        );
        assert!(
            refused.contains("bynk deploy"),
            "names the remedy: {refused}"
        );
        // Under `--force`, such an edge keeps its key: only edges the run
        // deploys both ends of rotate.
        assert_eq!(
            decide(Present, Present, false, true, never),
            Ok(EdgeKey::Keep)
        );
    }

    #[test]
    fn edge_key_presence_reads_a_never_deployed_worker_as_absent() {
        let name = "BYNK_CALL_FROM_SHOP__ORDERS";
        let listed: BTreeSet<String> = [name.to_string()].into_iter().collect();
        assert_eq!(presence_of(name, Some(&listed), false), Present);
        assert_eq!(presence_of(name, Some(&BTreeSet::new()), false), Absent);
        assert_eq!(presence_of(name, None, false), Unknown);
        assert_eq!(presence_of(name, None, true), Absent);
        assert_eq!(presence_of(name, Some(&listed), true), Absent);
    }

    #[test]
    fn a_minted_edge_key_is_64_hex_digits_and_fresh() {
        let a = mint_edge_key().expect("the OS has randomness");
        let b = mint_edge_key().expect("the OS has randomness");
        // Checked without ever printing a value, even on failure: a minted
        // key is never logged, test output included.
        assert!(a.len() == 64, "a minted edge key is 64 characters");
        assert!(
            a.chars().all(|c| c.is_ascii_hexdigit()),
            "a minted edge key is hex"
        );
        assert!(a != b, "two mints differ");
    }

    #[test]
    fn a_supplied_edge_key_is_not_spread_as_a_user_secret() {
        let src = source(
            &[
                ("BYNK_CALL_FROM_SHOP__ORDERS", "v"),
                ("BYNK_CALL_TO_SHOP__PAYMENT", "v"),
                ("OTHER", "w"),
            ],
            &[],
        );
        let wanted = wanted_secrets(&[], &[], &src);
        assert_eq!(
            wanted.iter().map(|w| w.name.as_str()).collect::<Vec<_>>(),
            vec!["OTHER"],
            "edge keys are decided apart, by `decide_edge_key`"
        );
        assert!(Origin::Managed.required());
        assert_eq!(Origin::Managed.label(), "managed");
    }

    // ---- #3386: a `SignedCookie` actor's signing key --------------------------

    /// 32 bytes of 0xAB, as unpadded base64url (43 characters).
    const GOOD_KEY: &str = "q6urq6urq6urq6urq6urq6urq6urq6urq6urq6urq6s";

    fn session(
        supplied: Option<&str>,
        presence: Presence,
        force: bool,
        mut mint: impl FnMut() -> Result<String, String>,
    ) -> Result<ForeignKey, String> {
        decide_session_key(supplied.map(str::to_string), presence, force, &mut mint)
    }

    #[test]
    fn base64url_round_trips_and_matches_the_alphabet() {
        for len in 0..40usize {
            let bytes: Vec<u8> = (0..len).map(|i| (i * 37 + 11) as u8).collect();
            let text = base64url_encode(&bytes);
            assert!(
                text.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            );
            assert!(!text.contains('='));
            assert_eq!(base64url_decode(&text), Some(bytes), "len {len}");
        }
        // The standard test vector, and the URL alphabet.
        assert_eq!(
            base64url_encode(b"Many hands make light work."),
            "TWFueSBoYW5kcyBtYWtlIGxpZ2h0IHdvcmsu"
        );
        assert_eq!(base64url_encode(&[0xfb, 0xff, 0xfe]), "-__-");
        assert_eq!(base64url_decode("a"), None);
        assert_eq!(base64url_decode("ab+/"), None);
        assert_eq!(base64url_decode("ab=="), None);
    }

    #[test]
    fn a_minted_session_key_is_32_random_bytes_the_runtime_accepts() {
        let a = mint_session_key().expect("the OS has randomness");
        let b = mint_session_key().expect("the OS has randomness");
        assert_eq!(a.len(), 43);
        assert_ne!(a, b);
        assert!(check_session_key(&a).is_ok());
        assert_eq!(base64url_decode(&a).map(|v| v.len()), Some(32));
    }

    #[test]
    fn a_supplied_session_key_must_be_one_the_runtime_accepts() {
        assert!(check_session_key(GOOD_KEY).is_ok());
        assert!(check_session_key("").is_err());
        assert!(check_session_key("short").is_err());
        // 31 bytes: one short.
        assert!(
            check_session_key(&base64url_encode(&[7u8; 31]))
                .unwrap_err()
                .contains("at least 32")
        );
        assert!(check_session_key(&base64url_encode(&[7u8; 33])).is_ok());
        for bad in [
            "not base64url!",
            "AAAA=",
            &format!("{GOOD_KEY}=")[..],
            "+/+/+/+/+/+/+/+/+/+/+/+/+/+/+/+/+/+/+/+/+/+/",
        ] {
            assert!(check_session_key(bad).is_err(), "{bad}");
        }
        // A non-canonical encoding (a stray trailing bit) is refused, as the runtime refuses it.
        let mut twin = GOOD_KEY.to_string();
        twin.pop();
        twin.push('t');
        assert!(check_session_key(&twin).is_err());
    }

    #[test]
    fn a_new_session_key_is_minted_and_a_held_one_kept() {
        let set = Ok(ForeignKey::Set {
            value: "minted".into(),
            minted: true,
        });
        assert_eq!(session(None, Absent, false, minted), set);
        assert_eq!(session(None, Present, true, minted), set);
        assert_eq!(session(None, Present, false, never), Ok(ForeignKey::Keep));
        assert_eq!(session(None, Unknown, false, never), Ok(ForeignKey::Keep));
    }

    #[test]
    fn a_supplied_session_key_is_set_where_absent_or_forced_and_never_minted_over() {
        let set = Ok(ForeignKey::Set {
            value: GOOD_KEY.into(),
            minted: false,
        });
        assert_eq!(session(Some(GOOD_KEY), Absent, false, never), set);
        assert_eq!(session(Some(GOOD_KEY), Present, true, never), set);
        assert_eq!(
            session(Some(GOOD_KEY), Present, false, never),
            Ok(ForeignKey::Keep)
        );
    }

    #[test]
    fn a_weak_supplied_session_key_is_refused_before_anything_is_decided() {
        for presence in [Absent, Present, Unknown] {
            assert!(session(Some("hunter2"), presence, true, never).is_err());
            assert!(session(Some(""), presence, false, never).is_err());
        }
    }

    // ---- I3 (#1971): foreign callers' secrets ---------------------------------

    fn foreign(
        supplied: Option<&str>,
        presence: Presence,
        force: bool,
        mut mint: impl FnMut() -> Result<String, String>,
    ) -> Result<ForeignKey, String> {
        decide_foreign_key(supplied.map(str::to_string), presence, force, &mut mint)
    }

    #[test]
    fn a_new_foreign_caller_gets_a_minted_secret() {
        assert_eq!(
            foreign(None, Absent, false, minted),
            Ok(ForeignKey::Set {
                value: "minted".into(),
                minted: true
            })
        );
        // `--force` rotates it.
        assert_eq!(
            foreign(None, Present, true, minted),
            Ok(ForeignKey::Set {
                value: "minted".into(),
                minted: true
            })
        );
    }

    #[test]
    fn a_held_foreign_secret_is_kept() {
        assert_eq!(foreign(None, Present, false, never), Ok(ForeignKey::Keep));
        assert_eq!(foreign(None, Unknown, false, never), Ok(ForeignKey::Keep));
    }

    #[test]
    fn a_supplied_foreign_secret_is_set_and_never_handed_off() {
        let set = Ok(ForeignKey::Set {
            value: "theirs".into(),
            minted: false,
        });
        assert_eq!(foreign(Some("theirs"), Absent, false, never), set);
        assert_eq!(foreign(Some("theirs"), Present, true, never), set);
        assert_eq!(
            foreign(Some("theirs"), Present, false, never),
            Ok(ForeignKey::Keep)
        );
        assert!(foreign(Some(""), Absent, false, never).is_err());
    }

    #[test]
    fn the_hand_off_is_a_ready_to_run_command_for_the_foreign_worker() {
        let caller = ForeignCaller {
            actor: "Ledger".into(),
            binding: "LEDGER_CALL_KEY".into(),
            worker: Some("ledger-svc".into()),
        };
        let text = foreign_hand_off("default", "shop-orders", &caller, "abc123", "npx");
        assert!(
            text.contains(
                "printf '%s' 'abc123' | npx wrangler secret put BYNK_CALL_TO_SHOP__ORDERS --name ledger-svc"
            ),
            "{text}"
        );
        assert!(
            text.contains("`Ledger`") && text.contains("`shop.orders`"),
            "{text}"
        );
        let unnamed = ForeignCaller {
            worker: None,
            ..caller
        };
        assert!(
            foreign_hand_off("default", "shop-orders", &unnamed, "abc123", "npx")
                .contains("--name <foreign-worker>"),
            "an undeclared worker is a placeholder to fill in"
        );
    }

    #[test]
    fn the_hand_off_names_the_detected_managers_runner() {
        use bynk_driver::npm_install::PackageManager::{Bun, Npm, Pnpm, Yarn};
        let caller = ForeignCaller {
            actor: "Ledger".into(),
            binding: "LEDGER_CALL_KEY".into(),
            worker: Some("ledger-svc".into()),
        };
        for (manager, runner) in [
            (Npm, "npx"),
            (Pnpm, "pnpm dlx"),
            (Bun, "bunx"),
            // Yarn classic has no runner: it keeps `npx` (#3426).
            (Yarn, "npx"),
        ] {
            let text = foreign_hand_off(
                "default",
                "shop-orders",
                &caller,
                "abc123",
                wrangler_runner(manager),
            );
            assert!(
                text.contains(&format!(
                    "| {runner} wrangler secret put BYNK_CALL_TO_SHOP__ORDERS --name ledger-svc"
                )),
                "{text}"
            );
        }
    }
}

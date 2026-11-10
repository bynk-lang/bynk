//! Compile-time and editor-latency benchmarks (#3168).
//!
//! `cargo xtask bench` generates a synthetic project of N contexts
//! ([`generate_project`]) and measures, against the release `bynk` and
//! `bynk-lsp` binaries:
//!
//! - `cli_check` — `bynk check` on the project, a fresh process each run, so
//!   the compiler's in-process caches are always cold;
//! - `cli_recheck` — the same after a one-line edit (a fresh process again: it
//!   shows whether anything survives between runs; today nothing does);
//! - `lsp_first_round` — open one file and wait for its diagnostics: the
//!   server's first, cold project-wide round;
//! - `lsp_edit` — a one-line edit in an open buffer to the diagnostics for that
//!   version arriving: the **edit-to-diagnostics** time, warm (the content-keyed
//!   parse cache is populated by the first round).
//!
//! The LSP leg runs with `[lsp] diagnostics_debounce_ms = 0`, so the figure is
//! the compute time; the shipped default debounce (300 ms) is added on top for
//! a user, and is not part of the budget question this measures.
//!
//! Results are written per release to `design/benchmarks/v<version>.json`
//! (`--record`), not gated per PR: shared CI runners are too noisy for a
//! threshold. [`latest_record`] reads the newest one back for the
//! `keystroke_latency` probe in `design/greenfield-status.md`.

use crate::stamp::Version;
use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Where the per-release results live, relative to the repo root.
pub const RECORD_DIR: &str = "design/benchmarks";

/// How many agent/service/type groups each generated context holds.
pub const GROUPS_PER_CONTEXT: usize = 3;

/// The file the LSP benchmark edits, relative to the project root.
pub const EDITED_FILE: &str = "src/ctx0.bynk";

/// The generated project: `(relative path, contents)`. `contexts` independent
/// contexts, each with `groups` groups of a refined type, a record, an agent
/// with two handlers and an http service. Every file checks clean.
pub fn generate_project(contexts: usize, groups: usize) -> Vec<(String, String)> {
    generate_project_shaped(Shape::Independent, contexts, groups)
}

/// The two project shapes the benchmarks measure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// `contexts` independent contexts with small handler bodies: the shape the
    /// budget (#3168) is stated on.
    Independent,
    /// Contexts that consume each other and larger handler bodies, closer to a
    /// real project (#3243): context `i` consumes context `(i-1)/4`, a 4-ary
    /// tree, so a project of N contexts is about `log4(N)` consumes-levels deep
    /// and the contexts near the root are consumed by many. Every context
    /// forwards a `price` call through its parent.
    Linked,
}

impl Shape {
    /// The shape's name, for progress and the CLI.
    pub fn name(self) -> &'static str {
        match self {
            Shape::Independent => "independent",
            Shape::Linked => "linked",
        }
    }

    /// The key a record stores this shape's sizes under.
    pub fn record_key(self) -> &'static str {
        match self {
            Shape::Independent => "sizes",
            Shape::Linked => "linked_sizes",
        }
    }
}

/// [`generate_project`] for a given [`Shape`].
pub fn generate_project_shaped(
    shape: Shape,
    contexts: usize,
    groups: usize,
) -> Vec<(String, String)> {
    let mut files = vec![(
        "bynk.toml".to_string(),
        "[project]\nname = \"bench\"\nversion = \"0.1.0\"\n\n[lsp]\ndiagnostics_debounce_ms = 0\n"
            .to_string(),
    )];
    for i in 0..contexts {
        let source = match shape {
            Shape::Independent => generate_context(i, groups),
            Shape::Linked => generate_linked_context(i, groups),
        };
        files.push((format!("src/ctx{i}.bynk"), source));
    }
    files
}

/// A context of the [`Shape::Linked`] project: it consumes its parent, forwards
/// `price` through it, and each group has a record, an agent with three handlers
/// whose bodies do real work (a cross-context call, branching, a filter, a map
/// and a sum), and an http service.
fn generate_linked_context(index: usize, groups: usize) -> String {
    let mut s = format!("context ctx{index}\n\n");
    let parent = index.checked_sub(1).map(|i| i / 4);
    if let Some(p) = parent {
        s.push_str(&format!("consumes ctx{p}\n\n"));
    }
    let forward = match parent {
        Some(p) => format!("    let p <- ctx{p}.price(n)\n    Effect.pure(p)\n"),
        None => "    Effect.pure(n)\n".to_string(),
    };
    s.push_str(&format!(
        "service price {{\n  on call(n: Int) -> Effect[Int] {{\n{forward}  }}\n}}\n\n"
    ));
    for j in 0..groups {
        // The cross-context call in the first handler needs a parent to call.
        let priced = match parent {
            Some(p) => format!("    let p <- ctx{p}.price(row.qty)\n"),
            None => "    let p = row.qty\n".to_string(),
        };
        s.push_str(&format!(
            "type Id{j} = String where NonEmpty\n\n\
             type Row{j} = {{\n  id:    Id{j},\n  label: String,\n  qty:   Int,\n  note:  String,\n}}\n\n\
             agent Store{j} {{\n  key name: String\n\n  store rows: Map[String, Row{j}]\n\n  \
             on call put(row: Row{j}) -> Effect[Row{j}] {{\n{priced}    \
             let size = row.label.length() + row.note.length()\n    \
             let tag = if (size > 10) {{ \"long\" }} else {{ \"short\" }}\n    \
             let kept = if (p > 0) {{ row }} else {{ Row{j} {{ id: row.id, label: tag, qty: 1, note: row.note }} }}\n    \
             let _ <- rows.put(kept.id, kept)\n    kept\n  }}\n\n  \
             on call total(min: Int) -> Effect[Int] {{\n    rows.filter((r) => r.qty > min).sum((r) => r.qty)\n  }}\n\n  \
             on call labels() -> Effect[List[String]] {{\n    rows.filter((r) => r.qty > 0).map((r) => r.label).collect()\n  }}\n}}\n\n\
             service api{j} from http {{\n  \
             on POST(\"/rows{j}\") (body: Row{j}) -> Effect[HttpResult[Row{j}]] by Visitor {{\n    \
             let row <- Store{j}(\"default\").put(body)\n    Created(row)\n  }}\n}}\n\n"
        ));
    }
    s
}

fn generate_context(index: usize, groups: usize) -> String {
    let mut s = format!("context ctx{index}\n\n");
    for j in 0..groups {
        s.push_str(&format!(
            "type Id{j} = String where NonEmpty\n\n\
             type Row{j} = {{\n  id:    Id{j},\n  label: String,\n  count: Int,\n}}\n\n\
             agent Store{j} {{\n  key name: String\n\n  store rows: Map[String, Row{j}]\n\n  \
             on call put(row: Row{j}) -> Effect[Row{j}] {{\n    let _ <- rows.put(row.label, row)\n    row\n  }}\n\n  \
             on call total() -> Effect[Int] {{\n    rows.filter((r) => r.count > 0).sum((r) => r.count)\n  }}\n}}\n\n\
             service api{j} from http {{\n  \
             on POST(\"/rows{j}\") (body: Row{j}) -> Effect[HttpResult[Row{j}]] by Visitor {{\n    \
             let row <- Store{j}(\"default\").put(body)\n    Created(row)\n  }}\n}}\n\n"
        ));
    }
    s
}

/// The one-line edit the benchmarks make: `valid` toggles a field's type
/// between a real one and an unknown name, so each edit changes the diagnostics
/// and the client can tell its own round from a stale one.
pub fn edit_source(source: &str, valid: bool) -> String {
    const GOOD: &str = "label: String,";
    const BAD: &str = "label: Strng,";
    if valid {
        source.replacen(BAD, GOOD, 1)
    } else {
        source.replacen(GOOD, BAD, 1)
    }
}

/// A **body-only** edit: a comparison inside a handler body becomes ill-typed
/// and back (`> 0)` to `> "x")`). Nothing outside that body changes, so it is
/// the edit an incremental analysis is meant to make cheap, and, like
/// [`edit_source`], it moves the diagnostics so the client can see its own round.
/// The signature edit [`edit_source`] makes is the pessimistic one for a cache.
pub fn edit_body(source: &str, valid: bool) -> String {
    const GOOD: &str = "> 0)";
    const BAD: &str = "> \"x\")";
    if valid {
        source.replacen(BAD, GOOD, 1)
    } else {
        source.replacen(GOOD, BAD, 1)
    }
}

/// Nearest-rank percentile of `samples` (milliseconds), `p` in 0..=100.
pub fn percentile(samples: &[f64], p: f64) -> f64 {
    assert!(!samples.is_empty(), "percentile of no samples");
    let mut v = samples.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).expect("no NaN timings"));
    let rank = ((p / 100.0) * v.len() as f64).ceil() as usize;
    v[rank.clamp(1, v.len()) - 1]
}

fn summary(samples: &[f64]) -> Value {
    let round = |x: f64| (x * 10.0).round() / 10.0;
    json!({
        "runs": samples.len(),
        "median_ms": round(percentile(samples, 50.0)),
        "p95_ms": round(percentile(samples, 95.0)),
        "max_ms": round(percentile(samples, 100.0)),
    })
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Options for [`run`].
pub struct Options {
    pub sizes: Vec<usize>,
    pub runs: usize,
    pub edits: usize,
    /// The shapes to measure; both by default.
    pub shapes: Vec<Shape>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            sizes: vec![10, 50, 200],
            runs: 7,
            edits: 20,
            shapes: vec![Shape::Independent, Shape::Linked],
        }
    }
}

/// Run every size and return the result document (not yet written anywhere).
pub fn run(root: &Path, opts: &Options, progress: &dyn Fn(&str)) -> Result<Value, String> {
    let bynk = binary(root, "bynk")?;
    let lsp = binary(root, "bynk-lsp")?;
    let mut by_shape: Vec<(Shape, Vec<Value>)> = Vec::new();
    for &shape in &opts.shapes {
        let mut sizes = Vec::new();
        for &n in &opts.sizes {
            let label = format!("{n} {} contexts", shape.name());
            progress(&format!("{label}: generating"));
            let dir = root.join(match shape {
                Shape::Independent => format!("target/bench-project-{n}"),
                Shape::Linked => format!("target/bench-linked-project-{n}"),
            });
            let _ = fs::remove_dir_all(&dir);
            let files = generate_project_shaped(shape, n, GROUPS_PER_CONTEXT);
            let mut lines = 0usize;
            for (rel, body) in &files {
                let p = dir.join(rel);
                fs::create_dir_all(p.parent().expect("has parent")).map_err(|e| e.to_string())?;
                fs::write(&p, body).map_err(|e| e.to_string())?;
                lines += body.lines().count();
            }

            progress(&format!("{label}: bynk check"));
            let cli_check = time_cli(&bynk, &dir, opts.runs, false)?;
            progress(&format!("{label}: bynk check after an edit"));
            let cli_recheck = time_cli(&bynk, &dir, opts.runs, true)?;
            progress(&format!("{label}: language server"));
            let lsp_run = time_lsp(&lsp, &dir, opts.edits)?;

            sizes.push(json!({
                "contexts": n,
                "groups_per_context": GROUPS_PER_CONTEXT,
                "source_lines": lines,
                "cli_check": summary(&cli_check),
                "cli_recheck": summary(&cli_recheck),
                "lsp_first_round": summary(&[lsp_run.first]),
                "lsp_edit": summary(&lsp_run.edits),
                "lsp_first_round_levels": level_medians(&lsp_run.first_levels.into_iter().collect::<Vec<_>>()),
                "lsp_edit_levels": level_medians(&lsp_run.edit_levels),
                "lsp_body_edit": summary(&lsp_run.body_edits),
                "lsp_body_edit_levels": level_medians(&lsp_run.body_edit_levels),
                "lsp_edit_outside_analysis_ms": outside_analysis(&lsp_run.edits, &lsp_run.edit_levels),
            }));
        }
        by_shape.push((shape, sizes));
    }
    let mut record = json!({
        "version": workspace_version(root)?,
        "commit": git(root, &["rev-parse", "--short", "HEAD"]).unwrap_or_default(),
        "host": host(),
        "debounce_ms_in_lsp_runs": 0,
    });
    for (shape, sizes) in by_shape {
        record[shape.record_key()] = Value::Array(sizes);
    }
    Ok(record)
}

fn workspace_version(root: &Path) -> Result<String, String> {
    let src = fs::read_to_string(root.join("Cargo.toml")).map_err(|e| e.to_string())?;
    crate::stamp::parse_workspace_version(&src)
        .map(|v| v.to_string())
        .ok_or_else(|| "no workspace version in Cargo.toml".to_string())
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn host() -> Value {
    let cpu = fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("model name"))
                .and_then(|l| l.split_once(':'))
                .map(|(_, v)| v.trim().to_string())
        })
        .unwrap_or_else(|| "unknown".to_string());
    json!({
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "cpu": cpu,
        "logical_cpus": std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0),
    })
}

/// Build (release) and locate a workspace binary.
fn binary(root: &Path, name: &str) -> Result<PathBuf, String> {
    let status = Command::new("cargo")
        .args(["build", "--release", "--quiet", "--package", name])
        .current_dir(root)
        .status()
        .map_err(|e| format!("cannot run cargo: {e}"))?;
    if !status.success() {
        return Err(format!("cargo build --release -p {name} failed"));
    }
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target"));
    let p = target.join("release").join(name);
    p.exists()
        .then_some(p.clone())
        .ok_or_else(|| format!("{} not found after build", p.display()))
}

fn time_cli(bynk: &Path, dir: &Path, runs: usize, edit_first: bool) -> Result<Vec<f64>, String> {
    let path = dir.join(EDITED_FILE);
    let original = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut samples = Vec::new();
    for i in 0..runs {
        if edit_first {
            // Keep the project clean-checking: the edit is a value-level one-line
            // change (a comment line carrying the run number).
            fs::write(&path, format!("{original}// edit {i}\n")).map_err(|e| e.to_string())?;
        }
        let t = Instant::now();
        let out = Command::new(bynk)
            .arg("check")
            .current_dir(dir)
            .output()
            .map_err(|e| e.to_string())?;
        let elapsed = t.elapsed();
        if !out.status.success() {
            return Err(format!(
                "bynk check failed on the generated project:\n{}",
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        samples.push(ms(elapsed));
    }
    if edit_first {
        fs::write(&path, original).map_err(|e| e.to_string())?;
    }
    Ok(samples)
}

// --- a minimal LSP client -------------------------------------------------

struct Client {
    child: Child,
    rx: mpsc::Receiver<Value>,
    /// The per-level timing line the server writes to stderr after each
    /// analysis round (`BYNK_PHASE_TIMING`), as `[file, unit, definition, project]` ms.
    timings: mpsc::Receiver<[f64; 4]>,
}

/// Parse `bynk-phase-timing file=1.0 unit=2.0 definition=3.0 project=4.0`.
pub fn parse_timing_line(line: &str) -> Option<[f64; 4]> {
    let rest = line.trim().strip_prefix("bynk-phase-timing")?;
    let mut out = [f64::NAN; 4];
    for part in rest.split_whitespace() {
        let (k, v) = part.split_once('=')?;
        let i = LEVELS.iter().position(|l| *l == k)?;
        out[i] = v.parse().ok()?;
    }
    out.iter().all(|x| x.is_finite()).then_some(out)
}

/// R3.13's four query levels, in the order the timing line reports them.
pub const LEVELS: [&str; 4] = ["file", "unit", "definition", "project"];

fn send(child: &mut Child, msg: &Value) -> Result<(), String> {
    let body = msg.to_string();
    let stdin = child.stdin.as_mut().ok_or("no stdin")?;
    write!(stdin, "Content-Length: {}\r\n\r\n{body}", body.len()).map_err(|e| e.to_string())?;
    stdin.flush().map_err(|e| e.to_string())
}

impl Client {
    fn spawn(lsp: &Path) -> Result<Client, String> {
        let mut child = Command::new(lsp)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("BYNK_PHASE_TIMING", "1")
            .spawn()
            .map_err(|e| format!("cannot start bynk-lsp: {e}"))?;
        let err = BufReader::new(child.stderr.take().ok_or("no stderr")?);
        let (ttx, timings) = mpsc::channel();
        std::thread::spawn(move || {
            for line in err.lines().map_while(Result::ok) {
                if let Some(t) = parse_timing_line(&line)
                    && ttx.send(t).is_err()
                {
                    return;
                }
            }
        });
        let mut out = BufReader::new(child.stdout.take().ok_or("no stdout")?);
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            loop {
                let mut len = None;
                loop {
                    let mut line = String::new();
                    match out.read_line(&mut line) {
                        Ok(0) | Err(_) => return,
                        Ok(_) => {}
                    }
                    let line = line.trim();
                    if line.is_empty() {
                        break;
                    }
                    if let Some(v) = line.strip_prefix("Content-Length:") {
                        len = v.trim().parse::<usize>().ok();
                    }
                }
                let Some(len) = len else { return };
                let mut buf = vec![0u8; len];
                if out.read_exact(&mut buf).is_err() {
                    return;
                }
                if let Ok(v) = serde_json::from_slice::<Value>(&buf)
                    && tx.send(v).is_err()
                {
                    return;
                }
            }
        });
        Ok(Client { child, rx, timings })
    }

    /// Drop any timing lines already received (a late line from a previous round).
    fn discard_timings(&self) {
        while self.timings.try_recv().is_ok() {}
    }

    /// The timing line of the round whose diagnostics just arrived. The server
    /// writes it to stderr before it publishes, but stdout and stderr are read
    /// by separate threads, so the diagnostics can be seen first: wait briefly
    /// for the line rather than take whatever has arrived. `None` when the
    /// server writes none (an older `bynk-lsp`), after a few seconds.
    fn round_timing(&self) -> Option<[f64; 4]> {
        self.timings.recv_timeout(Duration::from_secs(5)).ok()
    }

    /// Wait for a message satisfying `want`, answering any server-to-client
    /// request on the way (an empty `null` result is accepted for all of them).
    fn wait(&mut self, mut want: impl FnMut(&Value) -> bool) -> Result<Duration, String> {
        let t = Instant::now();
        loop {
            let left = Duration::from_secs(120).saturating_sub(t.elapsed());
            let msg = self
                .rx
                .recv_timeout(left)
                .map_err(|_| "timed out after 120 s waiting for the language server (is the project path file-URI safe: no spaces, `#` or `%`?)".to_string())?;
            let at = t.elapsed();
            if want(&msg) {
                return Ok(at);
            }
            if msg.get("method").is_some()
                && let Some(id) = msg.get("id")
            {
                send(
                    &mut self.child,
                    &json!({"jsonrpc": "2.0", "id": id, "result": null}),
                )?;
            }
        }
    }
}

fn is_diagnostics_for(msg: &Value, uri: &str, version: i64) -> bool {
    msg.get("method").and_then(Value::as_str) == Some("textDocument/publishDiagnostics")
        && msg["params"]["uri"].as_str() == Some(uri)
        && msg["params"]["version"].as_i64() == Some(version)
}

/// What one language-server run measured, in milliseconds.
struct LspTimings {
    first: f64,
    edits: Vec<f64>,
    /// The first round's `[file, unit, definition, project]` attribution.
    first_levels: Option<[f64; 4]>,
    /// One `[file, unit, definition, project]` per timed edit.
    edit_levels: Vec<[f64; 4]>,
    /// Edit-to-diagnostics for a **body-only** edit (see [`edit_body`]).
    body_edits: Vec<f64>,
    body_edit_levels: Vec<[f64; 4]>,
}

fn time_lsp(lsp: &Path, dir: &Path, edits: usize) -> Result<LspTimings, String> {
    let dir = dir.canonicalize().map_err(|e| e.to_string())?;
    let root_uri = format!("file://{}", dir.display());
    let file = dir.join(EDITED_FILE);
    let uri = format!("file://{}", file.display());
    let clean = fs::read_to_string(&file).map_err(|e| e.to_string())?;
    // The server publishes only when a file's diagnostics change, so a clean
    // file never answers. Open it broken (a diagnostic to publish), then toggle:
    // every edit flips the set, so every round is observable.
    let text = edit_source(&clean, false);

    let mut c = Client::spawn(lsp)?;
    send(
        &mut c.child,
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "processId": std::process::id(),
            "rootUri": root_uri,
            "capabilities": {},
            "workspaceFolders": [{"uri": root_uri, "name": "bench"}],
        }}),
    )?;
    c.wait(|m| m.get("id") == Some(&json!(1)) && m.get("method").is_none())?;
    send(
        &mut c.child,
        &json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
    )?;

    let did_open = |text: &str| {
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{
            "uri": uri, "languageId": "bynk", "version": 1, "text": text}}})
    };
    send(&mut c.child, &did_open(&text))?;
    let first = c.wait(|m| is_diagnostics_for(m, &uri, 1))?;
    let first_levels = c.round_timing();

    let mut samples = Vec::new();
    let mut edit_levels = Vec::new();
    let warmup = 2;
    for i in 0..(edits + warmup) {
        let version = i as i64 + 2;
        // Opened broken, so the first edit restores it.
        let valid = i % 2 == 0;
        let edited = edit_source(&clean, valid);
        // Nothing from an earlier round may be mistaken for this one's.
        c.discard_timings();
        send(
            &mut c.child,
            &json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{
                "textDocument": {"uri": uri, "version": version},
                "contentChanges": [{"text": edited}],
            }}),
        )?;
        let took = c.wait(|m| is_diagnostics_for(m, &uri, version))?;
        let levels = c.round_timing();
        if i >= warmup {
            samples.push(ms(took));
            edit_levels.extend(levels);
        }
    }

    // The signature phase opened broken and toggles each edit, so it ends broken
    // after an even number of edits and clean after an odd number. If broken,
    // restore the clean text first (waiting for the round, which clears the
    // diagnostic), so the body phase starts from a state where its first edit
    // adds one; if already clean, sending the clean text again would change
    // nothing, publish nothing, and wait forever.
    let mut version = (edits + warmup) as i64 + 1;
    if (edits + warmup).is_multiple_of(2) {
        version += 1;
        c.discard_timings();
        send(
            &mut c.child,
            &json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{
                "textDocument": {"uri": uri, "version": version},
                "contentChanges": [{"text": clean}],
            }}),
        )?;
        c.wait(|m| is_diagnostics_for(m, &uri, version))?;
        let _ = c.round_timing();
    }
    let mut body_samples = Vec::new();
    let mut body_levels = Vec::new();
    for j in 0..(edits + warmup) {
        version += 1;
        // The first edit breaks the body, the next restores it, and so on.
        let valid = j % 2 == 1;
        let edited = edit_body(&clean, valid);
        c.discard_timings();
        send(
            &mut c.child,
            &json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{
                "textDocument": {"uri": uri, "version": version},
                "contentChanges": [{"text": edited}],
            }}),
        )?;
        let took = c.wait(|m| is_diagnostics_for(m, &uri, version))?;
        let levels = c.round_timing();
        if j >= warmup {
            body_samples.push(ms(took));
            body_levels.extend(levels);
        }
    }

    let _ = send(
        &mut c.child,
        &json!({"jsonrpc":"2.0","id":2,"method":"shutdown","params":null}),
    );
    let _ = send(&mut c.child, &json!({"jsonrpc":"2.0","method":"exit"}));
    std::thread::sleep(Duration::from_millis(100));
    let _ = c.child.kill();
    let _ = c.child.wait();
    Ok(LspTimings {
        first: ms(first),
        edits: samples,
        first_levels,
        edit_levels,
        body_edits: body_samples,
        body_edit_levels: body_levels,
    })
}

/// Median time per edit spent outside the analysis round (the server's content
/// sweep, converting and publishing diagnostics): the edit-to-diagnostics time
/// less the four levels. `null` unless every timed edit has its timing line.
fn outside_analysis(edits: &[f64], levels: &[[f64; 4]]) -> Value {
    if edits.is_empty() || edits.len() != levels.len() {
        return Value::Null;
    }
    let outside: Vec<f64> = edits
        .iter()
        .zip(levels)
        .map(|(e, l)| (e - l.iter().sum::<f64>()).max(0.0))
        .collect();
    json!((percentile(&outside, 50.0) * 10.0).round() / 10.0)
}

/// Median of each level across `rows`, as a JSON object (`null` when there are none).
fn level_medians(rows: &[[f64; 4]]) -> Value {
    if rows.is_empty() {
        return Value::Null;
    }
    let round = |x: f64| (x * 10.0).round() / 10.0;
    let mut o = serde_json::Map::new();
    for (i, name) in LEVELS.iter().enumerate() {
        let col: Vec<f64> = rows.iter().map(|r| r[i]).collect();
        o.insert((*name).to_string(), json!(round(percentile(&col, 50.0))));
    }
    Value::Object(o)
}

// --- recorded results -------------------------------------------------------

/// The path of the record for `version`.
pub fn record_path(root: &Path, version: &str) -> PathBuf {
    root.join(RECORD_DIR).join(format!("v{version}.json"))
}

/// The newest recorded result (highest version) under `root`, with its version.
pub fn latest_record(root: &Path) -> Option<(Version, Value)> {
    let mut best: Option<(Version, PathBuf)> = None;
    for entry in fs::read_dir(root.join(RECORD_DIR)).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(v) = name
            .strip_prefix('v')
            .and_then(|n| n.strip_suffix(".json"))
            .and_then(Version::parse)
        else {
            continue;
        };
        let newer = best
            .as_ref()
            .is_none_or(|(b, _)| (v.major, v.minor, v.patch) > (b.major, b.minor, b.patch));
        if newer {
            best = Some((v, entry.path()));
        }
    }
    let (v, path) = best?;
    Some((
        v,
        serde_json::from_str(&fs::read_to_string(path).ok()?).ok()?,
    ))
}

/// The accepted editor-latency budget (#3168, accepted by the maintainer):
/// the language server's edit-to-diagnostics **p95** stays under this many
/// milliseconds on the [`BUDGET_CONTEXTS`]-context synthetic project. It is
/// checked against each release record, not gated in CI. (The scheduler's own
/// trigger is separate: [`TRIGGER_P95_MS`].)
pub const BUDGET_P95_MS: f64 = 100.0;

/// The project size the budget is stated for.
pub const BUDGET_CONTEXTS: u64 = 50;

/// The incremental-scheduler trigger (#1523, #3243): the scheduler decision is
/// reopened when the language server's edit-to-diagnostics **p95** exceeds this
/// many milliseconds at [`TRIGGER_CONTEXTS`] contexts, on **either** project
/// shape, in a release record. It was the trigger the scheduler (S2 of #3243)
/// was first deferred on; the maintainer has since decided to build it, and this
/// stays as the yardstick the slices are measured against.
pub const TRIGGER_P95_MS: f64 = 250.0;

/// The project size the scheduler trigger is stated for.
pub const TRIGGER_CONTEXTS: u64 = 200;

/// The p95 at [`TRIGGER_CONTEXTS`] contexts for `shape`'s sizes, if measured.
fn trigger_p95(record: &Value, shape: Shape) -> Option<f64> {
    record[shape.record_key()]
        .as_array()?
        .iter()
        .find(|s| s["contexts"].as_u64() == Some(TRIGGER_CONTEXTS))?["lsp_edit"]["p95_ms"]
        .as_f64()
}

/// The scheduler trigger's reading on `record`, one clause: which shapes were
/// measured at [`TRIGGER_CONTEXTS`] contexts, their p95, and whether the trigger
/// is reached. `None` when neither shape measured that size.
fn trigger_reading(record: &Value) -> Option<String> {
    let cells: Vec<(Shape, f64)> = [Shape::Independent, Shape::Linked]
        .into_iter()
        .filter_map(|shape| Some((shape, trigger_p95(record, shape)?)))
        .collect();
    if cells.is_empty() {
        return None;
    }
    let reached = cells.iter().any(|(_, p)| *p > TRIGGER_P95_MS);
    let shown: Vec<String> = cells
        .iter()
        .map(|(shape, p)| format!("{} {p} ms", shape.name()))
        .collect();
    Some(format!(
        "scheduler yardstick p95 > {TRIGGER_P95_MS} ms at {TRIGGER_CONTEXTS} contexts: {} ({})",
        if reached { "REACHED" } else { "not reached" },
        shown.join(", ")
    ))
}

/// A warning for `cargo xtask bench` when `record` reaches the scheduler
/// trigger on either shape; `None` otherwise.
pub fn trigger_warning(record: &Value) -> Option<String> {
    let reached = [Shape::Independent, Shape::Linked]
        .into_iter()
        .any(|shape| trigger_p95(record, shape).is_some_and(|p| p > TRIGGER_P95_MS));
    reached.then(|| {
        format!(
            "{} — latency is at the incremental-scheduler yardstick (#3243, #1523)",
            trigger_reading(record).unwrap_or_default()
        )
    })
}

/// The budget's verdict on a record: `Some(p95)` of the [`BUDGET_CONTEXTS`]-context
/// size, or `None` when the record did not measure that size.
fn budget_p95(record: &Value) -> Option<f64> {
    record["sizes"]
        .as_array()?
        .iter()
        .find(|s| s["contexts"].as_u64() == Some(BUDGET_CONTEXTS))?["lsp_edit"]["p95_ms"]
        .as_f64()
}

/// A warning for `cargo xtask bench` to print when `record` breaches the
/// budget; `None` when it is met or the size was not measured.
pub fn budget_warning(record: &Value) -> Option<String> {
    let p95 = budget_p95(record).filter(|p| *p >= BUDGET_P95_MS)?;
    Some(format!(
        "edit-to-diagnostics p95 is {p95} ms at {BUDGET_CONTEXTS} contexts, over the {BUDGET_P95_MS} ms budget (the accepted latency budget, #3168)"
    ))
}

/// One-line reading of a record, for the `keystroke_latency` probe: the
/// edit-to-diagnostics median at each measured size, smallest first.
pub fn reading(version: Version, record: &Value) -> Option<String> {
    let mut sizes: Vec<&Value> = record["sizes"].as_array()?.iter().collect();
    sizes.sort_by_key(|s| s["contexts"].as_u64().unwrap_or(0));
    let cells: Option<Vec<String>> = sizes
        .iter()
        .map(|s| {
            Some(format!(
                "{} contexts {} ms",
                s["contexts"].as_u64()?,
                s["lsp_edit"]["median_ms"].as_f64()?
            ))
        })
        .collect();
    let cells = cells.filter(|c| !c.is_empty())?;
    let budget = match budget_p95(record) {
        Some(p) if p < BUDGET_P95_MS => {
            format!("; budget p95 < {BUDGET_P95_MS} ms at {BUDGET_CONTEXTS} contexts: met ({p} ms)")
        }
        Some(p) => format!(
            "; budget p95 < {BUDGET_P95_MS} ms at {BUDGET_CONTEXTS} contexts: BREACHED ({p} ms)"
        ),
        None => {
            format!("; budget p95 < {BUDGET_P95_MS} ms at {BUDGET_CONTEXTS} contexts: not measured")
        }
    };
    let trigger = trigger_reading(record)
        .map(|t| format!("; {t}"))
        .unwrap_or_default();
    Some(format!(
        "edit-to-diagnostics median: {}; debounce excluded{budget}{trigger} (v{version}, {RECORD_DIR}/v{version}.json)",
        cells.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_generator_is_deterministic_and_sized() {
        let a = generate_project(4, 3);
        assert_eq!(a, generate_project(4, 3));
        // The manifest plus one file per context.
        assert_eq!(a.len(), 5);
        assert!(a[0].1.contains("diagnostics_debounce_ms = 0"));
        let ctx = &a.iter().find(|(p, _)| p == EDITED_FILE).unwrap().1;
        assert!(ctx.starts_with("context ctx0\n"));
        assert_eq!(ctx.matches("agent Store").count(), 3);
        assert_eq!(ctx.matches("service api").count(), 3);
    }

    #[test]
    fn the_edit_toggles_and_round_trips() {
        let src = generate_context(0, 1);
        let broken = edit_source(&src, false);
        assert_ne!(broken, src);
        assert_eq!(edit_source(&broken, true), src);
    }

    #[test]
    fn the_timing_line_parses_in_level_order() {
        let t =
            parse_timing_line("bynk-phase-timing file=1.5 unit=2.0 definition=30.25 project=4.0");
        assert_eq!(t, Some([1.5, 2.0, 30.25, 4.0]));
        // A missing level, a bad number or another line is not a timing line.
        assert_eq!(parse_timing_line("bynk-phase-timing file=1 unit=2"), None);
        assert_eq!(
            parse_timing_line("bynk-phase-timing file=x unit=2 definition=3 project=4"),
            None
        );
        assert_eq!(parse_timing_line("some other log line"), None);
    }

    #[test]
    fn level_medians_are_per_level() {
        let rows = [
            [1.0, 10.0, 100.0, 5.0],
            [3.0, 30.0, 300.0, 7.0],
            [2.0, 20.0, 200.0, 6.0],
        ];
        let m = level_medians(&rows);
        assert_eq!(m["file"], json!(2.0));
        assert_eq!(m["definition"], json!(200.0));
        assert_eq!(level_medians(&[]), Value::Null);
    }

    #[test]
    fn the_budget_is_read_from_the_50_context_p95() {
        let rec = |p95: f64| {
            json!({"sizes": [
                {"contexts": 10, "lsp_edit": {"median_ms": 1.0, "p95_ms": 500.0}},
                {"contexts": 50, "lsp_edit": {"median_ms": 40.0, "p95_ms": p95}},
            ]})
        };
        // Only the 50-context size counts; a slow 10-context p95 does not breach.
        assert_eq!(budget_warning(&rec(54.5)), None);
        assert!(
            budget_warning(&rec(100.0))
                .unwrap()
                .contains("over the 100 ms budget")
        );
        assert!(budget_warning(&rec(250.0)).unwrap().contains("#3168"));
        // A record that did not measure 50 contexts says nothing, and reads "not measured".
        let none = json!({"sizes": [{"contexts": 10, "source_lines": 1, "lsp_edit": {"median_ms": 1.0, "p95_ms": 1.0}}]});
        assert_eq!(budget_warning(&none), None);
        let v = Version::parse("0.1.0").unwrap();
        assert!(reading(v, &none).unwrap().contains("not measured"));
        assert!(reading(v, &rec(54.5)).unwrap().contains("met (54.5 ms)"));
        assert!(
            reading(v, &rec(120.0))
                .unwrap()
                .contains("BREACHED (120 ms)")
        );
    }

    /// The trigger is p95 > 250 ms at 200 contexts on either shape; a shape that
    /// did not measure 200 contexts says nothing.
    #[test]
    fn the_scheduler_trigger_is_checked_on_both_shapes() {
        let at = |key: &str, p95: f64| json!({ key: [{"contexts": 200, "lsp_edit": {"median_ms": 1.0, "p95_ms": p95}}] });
        let merged = |a: Value, b: Value| {
            let mut m = a;
            m["linked_sizes"] = b["linked_sizes"].clone();
            m
        };
        let quiet = merged(at("sizes", 118.5), at("linked_sizes", 190.0));
        assert_eq!(trigger_warning(&quiet), None);
        assert!(
            trigger_reading(&quiet)
                .unwrap()
                .contains("not reached (independent 118.5 ms, linked 190 ms)")
        );
        // Either shape reaching it is enough; exactly 250 is not over it.
        let linked_over = merged(at("sizes", 118.5), at("linked_sizes", 250.1));
        assert!(trigger_warning(&linked_over).unwrap().contains("REACHED"));
        let indep_over = merged(at("sizes", 300.0), at("linked_sizes", 10.0));
        assert!(trigger_warning(&indep_over).is_some());
        assert_eq!(
            trigger_warning(&merged(at("sizes", 250.0), at("linked_sizes", 250.0))),
            None
        );
        // A record measuring neither shape at 200 contexts has no reading.
        assert_eq!(trigger_reading(&json!({"sizes": [{"contexts": 50}]})), None);
        // The probe line carries it when present, and still reads without it.
        let v = Version::parse("0.1.0").unwrap();
        let rec = json!({
            "sizes": [
                {"contexts": 50, "lsp_edit": {"median_ms": 1.0, "p95_ms": 2.0}},
                {"contexts": 200, "lsp_edit": {"median_ms": 1.0, "p95_ms": 118.5}},
            ],
            "linked_sizes": [{"contexts": 200, "lsp_edit": {"median_ms": 1.0, "p95_ms": 190.0}}],
        });
        assert!(
            reading(v, &rec)
                .unwrap()
                .contains("scheduler yardstick p95 > 250 ms at 200 contexts: not reached")
        );
    }

    #[test]
    fn the_linked_shape_consumes_a_parent_tree_and_keeps_the_edit_target() {
        let files = generate_project_shaped(Shape::Linked, 10, 2);
        let src = |i: usize| {
            &files
                .iter()
                .find(|(p, _)| p == &format!("src/ctx{i}.bynk"))
                .unwrap()
                .1
        };
        // The root consumes nothing; the others consume `(i-1)/4`.
        assert!(!src(0).contains("consumes"));
        assert!(src(1).contains("consumes ctx0\n"));
        assert!(src(4).contains("consumes ctx0\n"));
        assert!(src(5).contains("consumes ctx1\n"));
        assert!(src(9).contains("consumes ctx2\n"));
        // Each group's handlers are bigger than the independent shape's.
        assert_eq!(src(3).matches("agent Store").count(), 2);
        assert!(src(3).lines().count() > generate_context(3, 2).lines().count());
        // The edit still toggles a field in the edited file, and round-trips.
        let edited = src(0);
        assert_ne!(&edit_source(edited, false), edited);
        assert_eq!(&edit_source(&edit_source(edited, false), true), edited);
        // The independent generator is unchanged by shapes.
        assert_eq!(
            generate_project(4, 3),
            generate_project_shaped(Shape::Independent, 4, 3)
        );
    }

    #[test]
    fn the_body_edit_changes_only_a_body_and_round_trips() {
        for shape in [Shape::Independent, Shape::Linked] {
            let files = generate_project_shaped(shape, 4, 2);
            let src = &files.iter().find(|(p, _)| p == EDITED_FILE).unwrap().1;
            let broken = edit_body(src, false);
            assert_ne!(&broken, src, "{} has a body to edit", shape.name());
            // Only the one comparison changed: same length difference as the swap.
            assert_eq!(broken.len() + "> 0)".len(), src.len() + "> \"x\")".len());
            assert_eq!(&edit_body(&broken, true), src);
            // The signature edit and the body edit touch different things.
            assert!(!broken.contains("Strng"));
        }
    }

    #[test]
    fn percentile_is_nearest_rank() {
        let s = [5.0, 1.0, 3.0, 2.0, 4.0];
        assert_eq!(percentile(&s, 50.0), 3.0);
        assert_eq!(percentile(&s, 95.0), 5.0);
        assert_eq!(percentile(&s, 100.0), 5.0);
        assert_eq!(percentile(&[7.0], 50.0), 7.0);
    }

    #[test]
    fn the_latest_record_is_the_highest_version_and_reads_back() {
        let root = std::env::temp_dir().join(format!("xtask-bench-latest-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join(RECORD_DIR)).unwrap();
        assert!(latest_record(&root).is_none());
        let rec = |ms: f64| {
            json!({"sizes": [
                {"contexts": 10, "source_lines": 1, "lsp_edit": {"median_ms": 1.0, "p95_ms": 1.0}},
                {"contexts": 200, "source_lines": 9, "lsp_edit": {"median_ms": ms, "p95_ms": ms + 1.0}},
            ]})
        };
        fs::write(record_path(&root, "0.9.0"), rec(5.0).to_string()).unwrap();
        fs::write(record_path(&root, "0.10.0"), rec(8.5).to_string()).unwrap();
        fs::write(root.join(RECORD_DIR).join("notes.md"), "ignored").unwrap();
        let (v, r) = latest_record(&root).unwrap();
        assert_eq!(v.to_string(), "0.10.0");
        let line = reading(v, &r).unwrap();
        assert!(
            line.contains("10 contexts 1 ms, 200 contexts 8.5 ms;"),
            "{line}"
        );
    }
}

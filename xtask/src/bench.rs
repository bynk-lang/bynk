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
    let mut files = vec![(
        "bynk.toml".to_string(),
        "[project]\nname = \"bench\"\nversion = \"0.1.0\"\n\n[lsp]\ndiagnostics_debounce_ms = 0\n"
            .to_string(),
    )];
    for i in 0..contexts {
        files.push((format!("src/ctx{i}.bynk"), generate_context(i, groups)));
    }
    files
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
}

impl Default for Options {
    fn default() -> Self {
        Options {
            sizes: vec![10, 50, 200],
            runs: 7,
            edits: 20,
        }
    }
}

/// Run every size and return the result document (not yet written anywhere).
pub fn run(root: &Path, opts: &Options, progress: &dyn Fn(&str)) -> Result<Value, String> {
    let bynk = binary(root, "bynk")?;
    let lsp = binary(root, "bynk-lsp")?;
    let mut sizes = Vec::new();
    for &n in &opts.sizes {
        progress(&format!("{n} contexts: generating"));
        let dir = root.join(format!("target/bench-project-{n}"));
        let _ = fs::remove_dir_all(&dir);
        let files = generate_project(n, GROUPS_PER_CONTEXT);
        let mut lines = 0usize;
        for (rel, body) in &files {
            let p = dir.join(rel);
            fs::create_dir_all(p.parent().expect("has parent")).map_err(|e| e.to_string())?;
            fs::write(&p, body).map_err(|e| e.to_string())?;
            lines += body.lines().count();
        }

        progress(&format!("{n} contexts: bynk check"));
        let cli_check = time_cli(&bynk, &dir, opts.runs, false)?;
        progress(&format!("{n} contexts: bynk check after an edit"));
        let cli_recheck = time_cli(&bynk, &dir, opts.runs, true)?;
        progress(&format!("{n} contexts: language server"));
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
            "lsp_edit_outside_analysis_ms": outside_analysis(&lsp_run.edits, &lsp_run.edit_levels),
        }));
    }
    Ok(json!({
        "version": workspace_version(root)?,
        "commit": git(root, &["rev-parse", "--short", "HEAD"]).unwrap_or_default(),
        "host": host(),
        "debounce_ms_in_lsp_runs": 0,
        "sizes": sizes,
    }))
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
    Some(format!(
        "edit-to-diagnostics median: {}; debounce excluded (v{version}, {RECORD_DIR}/v{version}.json)",
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

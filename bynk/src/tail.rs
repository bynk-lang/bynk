//! `bynk tail` (#2546): a deployed Worker's logs, with the stack frames that
//! name generated TypeScript rewritten to the `.bynk` line they came from.
//!
//! The second slice of ADR 0622. `wrangler tail` does the streaming; the
//! driver compiles the project (nothing is deployed or provisioned) so the
//! emitter's source maps are on disk, and pipes the tail through the same
//! `Rewriter` `bynk dev` uses. Frames that name the
//! deployed bundle rather than a module, or a module the build has no map for,
//! pass through unchanged.

use std::path::Path;
use std::process::{ExitCode, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::compiler::Compiler;
use crate::dev::forward_lines;
use crate::logs::{Classifier, Rewriter};
use crate::probe::{self, DetectOpts, Provenance, Toolbox};
use crate::workers;

/// Parsed `bynk tail` flags.
#[derive(Debug, Clone, Default)]
pub struct TailOptions {
    /// `--context NAME`; `None` when the project has a single context.
    pub context: Option<String>,
    /// `--env NAME` (default `"default"`): the `bynk deploy --env` environment.
    pub environment: String,
    /// Arguments after `--`, forwarded to `wrangler tail`.
    pub wrangler_args: Vec<String>,
}

/// The one worker to tail: the requested context, or the only one built.
fn pick_worker(available: &[String], requested: Option<&str>) -> Result<String, String> {
    let requested: Vec<String> = requested.map(str::to_string).into_iter().collect();
    let chosen = workers::select_contexts(available, &requested).map_err(|e| e.to_string())?;
    match chosen.as_slice() {
        [one] => Ok(one.clone()),
        _ => Err(format!(
            "this project has several contexts — pick one with `--context NAME` (available: {})",
            available.join(", ")
        )),
    }
}

/// Compile, resolve the Worker's deployed name, and stream `wrangler tail`.
/// Returns wrangler's exit code.
pub fn run(
    tb: &dyn Toolbox,
    compiler: &Compiler,
    project_root: &Path,
    opts: &TailOptions,
) -> ExitCode {
    let probe = probe::detect(
        tb,
        "wrangler",
        DetectOpts {
            project_root: Some(project_root),
            allow_npx: true,
        },
    );
    if matches!(probe.provenance, Provenance::Missing) {
        eprintln!("bynk: wrangler not found (run `bynk doctor --only deploy`)");
        return ExitCode::FAILURE;
    }

    let build_dir = project_root.join(".bynk").join("tail");
    if let Err(e) = workers::prepare_build_dir(project_root, &build_dir) {
        eprintln!("bynk: could not prepare build directory: {e}");
        return ExitCode::FAILURE;
    }
    // `schema_registry` off: tailing must not touch `bynk.schema.lock`.
    if !workers::compile_once(compiler, project_root, &build_dir, false) {
        return ExitCode::FAILURE;
    }
    let workers_dir = build_dir.join("workers");
    let available = workers::discover_workers(&workers_dir);
    let worker = match pick_worker(&available, opts.context.as_deref()) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("bynk: {e}");
            return ExitCode::FAILURE;
        }
    };
    let deployed = crate::deploy::deployed_worker_name(&opts.environment, &worker);

    let Some(mut cmd) = workers::wrangler_command(&probe.provenance, "tail") else {
        eprintln!("bynk: wrangler not found (run `bynk doctor --only deploy`)");
        return ExitCode::FAILURE;
    };
    cmd.current_dir(workers_dir.join(&worker))
        .arg(&deployed)
        .args(&opts.wrangler_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if std::io::IsTerminal::is_terminal(&std::io::stderr())
        && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty())
    {
        cmd.env("FORCE_COLOR", "1");
    }
    crate::stop::install();
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("bynk: could not run wrangler tail: {e}");
            return ExitCode::FAILURE;
        }
    };
    eprintln!(
        "bynk tail: tailing `{deployed}` (frames are mapped to .bynk where the build has a map)."
    );

    let rewriter = Arc::new(Rewriter::new(&build_dir));
    let classifier = Arc::new(Mutex::new(Classifier::default()));
    let mut pumps = Vec::new();
    if let Some(out) = child.stdout.take() {
        let (rw, cl, w) = (rewriter.clone(), classifier.clone(), worker.clone());
        pumps.push(std::thread::spawn(move || {
            use std::io::Write;
            forward_lines(out, &w, &rw, &cl, |line, _| {
                let _ = writeln!(std::io::stdout().lock(), "{line}");
            });
        }));
    }
    if let Some(err) = child.stderr.take() {
        let (rw, cl, w) = (rewriter.clone(), classifier.clone(), worker.clone());
        pumps.push(std::thread::spawn(move || {
            use std::io::Write;
            forward_lines(err, &w, &rw, &cl, |line, _| {
                let _ = writeln!(std::io::stderr().lock(), "{line}");
            });
        }));
    }

    let code = loop {
        if let Some(sig) = crate::stop::received() {
            let _ = child.kill();
            let _ = child.wait();
            break crate::stop::exit_byte(sig);
        }
        match child.try_wait() {
            Ok(Some(status)) => break crate::shell::exit_status_byte(&status),
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => {
                eprintln!("bynk: could not poll wrangler: {e}");
                break 1;
            }
        }
    };
    // Bounded drain, as `dev`: the last lines usually say why it ended.
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while std::time::Instant::now() < deadline && !pumps.iter().all(|p| p.is_finished()) {
        std::thread::sleep(Duration::from_millis(10));
    }
    ExitCode::from(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(n: &[&str]) -> Vec<String> {
        n.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_lone_context_needs_no_flag() {
        assert_eq!(
            pick_worker(&names(&["shop-cart"]), None),
            Ok("shop-cart".into())
        );
    }

    #[test]
    fn several_contexts_need_a_choice() {
        let err = pick_worker(&names(&["a", "b"]), None).unwrap_err();
        assert!(err.contains("--context") && err.contains("a, b"), "{err}");
    }

    #[test]
    fn a_dotted_name_resolves_to_its_worker_dir() {
        assert_eq!(
            pick_worker(&names(&["shop-cart", "shop-pay"]), Some("shop.cart")),
            Ok("shop-cart".into())
        );
    }

    #[test]
    fn an_unknown_context_is_named() {
        let err = pick_worker(&names(&["a"]), Some("zzz")).unwrap_err();
        assert!(err.contains("zzz"), "{err}");
    }

    #[test]
    fn tailed_lines_get_bynk_frames() {
        let dir = std::env::temp_dir().join(format!("bynk-tail-{}", std::process::id()));
        let worker = dir.join("workers").join("shop");
        std::fs::create_dir_all(&worker).unwrap();
        std::fs::write(
            worker.join("handlers.ts.map"),
            r#"{"version":3,"file":"h.ts","sources":["/p/src/a.bynk"],"mappings":";AASA"}"#,
        )
        .unwrap();
        let rewriter = Rewriter::new(&dir);
        let classifier = Mutex::new(Classifier::default());
        let input = "Error: boom\n    at http_GET (handlers.ts:2:1)\nplain\n";
        let mut got = Vec::new();
        forward_lines(input.as_bytes(), "shop", &rewriter, &classifier, |l, _| {
            got.push(l.to_string())
        });
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            got,
            [
                "Error: boom",
                "    at http_GET (/p/src/a.bynk:10:1)",
                "plain"
            ]
        );
    }
}

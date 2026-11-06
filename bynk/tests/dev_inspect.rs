//! Slice 3 (ADR 0104) end-to-end proof: a breakpoint set in a `.bynk` handler
//! statement binds and pauses on a worker running under `wrangler dev`'s inspector.
//!
//! This is the productised slice-3 spike. It builds a one-context HTTP worker
//! (Workers target, with source maps), starts `wrangler dev --inspector-port`, and
//! drives a headless CDP session — exactly what a JavaScript debugger does — to
//! confirm the map round-trip *through the wrangler/esbuild bundle* on a real
//! worker. The arg wiring (`--inspect` → `--inspector-port`) is unit-tested in
//! `dev.rs`; this exercises the attach + composed-map breakpoint.
//!
//! Skipped when `wrangler` or `node` is unavailable, so it never blocks CI on a
//! toolchain that lacks the Cloudflare runtime — the always-on guarantee that the
//! emitted handler maps are correct is `bynkc`'s in-process decode goldens.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

fn have(tool: &str) -> bool {
    Command::new(tool)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn tmp() -> PathBuf {
    static C: AtomicU32 = AtomicU32::new(0);
    std::env::temp_dir().join(format!(
        "bynk_devdbg_{}_{}",
        std::process::id(),
        C.fetch_add(1, Ordering::Relaxed)
    ))
}

/// Two distinct localhost TCP ports that are free when asked. Both are bound to
/// `:0` at once, so the OS can't hand out the same port twice, then read back
/// and released.
fn free_ports() -> (u16, u16) {
    let bind = || std::net::TcpListener::bind("127.0.0.1:0").expect("bind an ephemeral port");
    let (a, b) = (bind(), bind());
    let port = |l: &std::net::TcpListener| l.local_addr().expect("local addr").port();
    (port(&a), port(&b))
}

/// #2069: stops `wrangler` and everything it started, and nothing else.
///
/// On Unix the child leads its own process group (`process_group(0)` at
/// spawn), and workerd, a grandchild, inherits it. Signalling the group
/// (`kill(-pgid)`) reaches exactly that tree: `SIGTERM` first so wrangler can
/// shut workerd down, then `SIGKILL` for anything still there. This replaces a
/// `pkill -f workerd`, which also killed every other test's workerd and any
/// `bynk dev` on the machine. Elsewhere it kills the direct child only.
fn stop_tree(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let Ok(pid) = libc::pid_t::try_from(child.id()) else {
            let _ = child.kill();
            let _ = child.wait();
            return;
        };
        // SAFETY: `kill` takes plain integers and has no memory effects; a
        // negative pid names the process group this test created for `child`.
        unsafe { libc::kill(-pid, libc::SIGTERM) };
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut gone = false;
        while Instant::now() < deadline {
            // Reap wrangler if it has exited, so its zombie doesn't keep the
            // group alive; `kill(-pgid, 0)` fails once no member is left.
            let _ = child.try_wait();
            // SAFETY: as above; signal 0 only checks for existence.
            if unsafe { libc::kill(-pid, 0) } != 0 {
                gone = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if !gone {
            // SAFETY: as above.
            unsafe { libc::kill(-pid, libc::SIGKILL) };
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// #2069 review: owns the spawned `wrangler` and stops its whole process group
/// on drop, so a panic between `spawn()` and the end of the test (an `expect`
/// in `drive`, say) can't leak the tree. That leak is the one #2069 fixes.
///
/// One case it does not cover: because wrangler leads its own process group,
/// a Ctrl-C at the terminal no longer reaches it, and an interrupted
/// `cargo test` dies without unwinding, so no `Drop` runs. Stopping the group
/// then would need a `SIGINT`/`SIGTERM` handler in the test.
struct Group(std::process::Child);

impl Drop for Group {
    fn drop(&mut self) {
        stop_tree(&mut self.0);
    }
}

#[test]
fn workerd_breakpoint_in_bynk_handler_binds_and_pauses() {
    if !have("wrangler") || !have("node") {
        eprintln!("skipping: `wrangler` and/or `node` not available");
        return;
    }

    // A one-context HTTP worker. The handler's effect-let is the breakpoint target.
    let dir = tmp();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("bynk.toml"), "[project]\nname = \"svc\"\n").unwrap();
    let svc = "context svc\n\nconsumes bynk { Logger }\n\nservice api from http {\n\ton GET(\"/\") () -> Effect[HttpResult[String]] by v: Visitor given Logger {\n\t\tlet _ <- Logger.info(\"hit\")\n\t\tOk(\"ok\")\n\t}\n}\n";
    std::fs::write(dir.join("src").join("svc.bynk"), svc).unwrap();
    let bynk_line = svc.lines().position(|l| l.contains("Logger.info")).unwrap() + 1;

    // Build the Workers output with maps (what `bynk dev` writes to `.bynk/dev/`).
    let build = dir.join("build");
    let opts = bynk_testkit::compile_options_split(
        dir.clone(),
        bynk_driver::project::try_read_project_paths(&dir).expect("well-formed fixture manifest"),
    )
    .target(bynk_driver::project::BuildTarget::Workers);
    let out = bynk_driver::project::compile_project(&opts)
        .map_err(bynk_driver::project::ProjectFailure::flatten)
        .unwrap_or_else(|e| panic!("compile failed: {e:?}"));
    bynk_driver::write_output(&out, &build).unwrap();
    let worker_dir = build.join("workers").join("svc");
    assert!(
        worker_dir.join("handlers.ts.map").exists(),
        "worker output must carry handlers.ts.map"
    );

    // #2069: ports the OS reports free right now, not pid-derived ones. A
    // derived port could already be held by another process, such as a workerd
    // a past run leaked. Then wrangler exits at once, and the inspector probe
    // reaches *that* process's inspector instead.
    let (inspector_port, app_port) = free_ports();

    let mut cmd = Command::new("wrangler");
    // #2069: its own process group, so cleanup can stop wrangler *and* the
    // workerd it spawns (a grandchild of this test) without touching any other
    // workerd on the machine.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    // Held in a `Group`, so a panic anywhere below (`drive`'s `.expect`s among
    // them) still stops the tree as the stack unwinds.
    let wrangler = Group(
        cmd.args([
            "dev",
            "--inspector-port",
            &inspector_port.to_string(),
            "--port",
            &app_port.to_string(),
        ])
        .current_dir(&worker_dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn wrangler dev"),
    );

    let result = drive(&worker_dir, inspector_port, app_port, bynk_line);

    drop(wrangler);
    let _ = std::fs::remove_dir_all(&dir);

    let (status, stdout, stderr) = result;
    assert!(
        status,
        "breakpoint did not bind/pause on the worker\n--- harness stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    assert!(
        stdout.contains("BIND OK"),
        "harness did not confirm bind\n{stdout}"
    );
}

/// Wait for the inspector, then run the CDP harness; returns `(ok, stdout, stderr)`.
fn drive(
    _worker_dir: &Path,
    inspector_port: u16,
    app_port: u16,
    bynk_line: usize,
) -> (bool, String, String) {
    // Poll the CDP discovery endpoint until the worker's inspector is up.
    let deadline = Instant::now() + Duration::from_secs(30);
    let ready = loop {
        if Instant::now() > deadline {
            break false;
        }
        let probe = Command::new("node")
            .arg("-e")
            .arg(format!(
                "fetch('http://127.0.0.1:{inspector_port}/json').then(r=>r.json()).then(j=>process.exit(j[0]?0:1)).catch(()=>process.exit(1))"
            ))
            .output();
        if probe.map(|o| o.status.success()).unwrap_or(false) {
            break true;
        }
        std::thread::sleep(Duration::from_millis(500));
    };
    if !ready {
        return (
            false,
            String::new(),
            "inspector never became reachable".to_string(),
        );
    }

    let harness = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("support")
        .join("wrangler_attach.mjs");
    let out = Command::new("node")
        .arg(harness)
        .arg(inspector_port.to_string())
        .arg(app_port.to_string())
        .arg(bynk_line.to_string())
        .output()
        .expect("run the CDP harness");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

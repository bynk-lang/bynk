use super::run::DevOptions;
use std::collections::BTreeMap;
use std::path::Path;

/// Wrangler's own default dev port — the base of the per-context allocation
/// when `--base-port` is not given, so a multi-context project's first worker
/// lands where a single-context one always has.
pub(super) const DEFAULT_BASE_PORT: u16 = 8787;

/// wrangler's own default inspector port, the base of the per-context inspector
/// allocation when `--inspect-port` is not given.
const DEFAULT_INSPECT_PORT: u16 = 9229;

/// One worker to serve: its dasherised context dir, its HTTP port (`None` = let
/// wrangler choose, the lone-worker default), its inspector port, and whether
/// the inspector was asked for (`--inspect`) rather than only allocated so the
/// contexts' default inspectors do not collide (#1954).
#[derive(Debug, PartialEq, Eq)]
pub struct Serving {
    pub worker: String,
    pub port: Option<u16>,
    pub inspector_port: Option<u16>,
    pub inspect: bool,
}

/// Allocate a port per worker (#552): `wrangler dev` binds one port per process,
/// so serving N contexts means N distinct ports, assigned `base + i` over the
/// deterministic worker order.
///
/// The one exception preserves the pre-#552 contract: a **lone** worker with no
/// explicit `--base-port` gets `None` — no injected `--port` at all — so it
/// lands on wrangler's own default and `-- --port N` still works. Injecting
/// unconditionally would break that, because a repeated `--port` is a hard
/// wrangler error, not last-wins.
///
/// Inspector ports follow the same shape, from `--inspect-port`. They are
/// allocated under `--inspect`, whenever `--inspect-port` is given (#3237: an
/// explicit port is always honoured, for one worker too), and whenever more than
/// one worker is served (#1954): `wrangler dev` always starts its inspector, on
/// 9229 unless told otherwise, so N wranglers started together all bind 9229 and
/// every one but the first dies with `Address already in use`. A lone worker
/// with none of those gets none here; [`assign_free_inspector`] gives it a free
/// port of its own.
pub fn allocate(workers: &[String], base_port: Option<u16>, opts: &DevOptions) -> Vec<Serving> {
    let lone = workers.len() == 1 && base_port.is_none();
    let base = base_port.unwrap_or(DEFAULT_BASE_PORT);
    let inspectors = opts.inspect || workers.len() > 1 || opts.inspect_port.is_some();
    let inspect_base = opts.inspect_port.unwrap_or(DEFAULT_INSPECT_PORT);
    workers
        .iter()
        .enumerate()
        .map(|(i, worker)| Serving {
            worker: worker.clone(),
            port: (!lone).then(|| base.saturating_add(i as u16)),
            inspector_port: inspectors.then(|| inspect_base.saturating_add(i as u16)),
            inspect: opts.inspect,
        })
        .collect()
}

/// A port the allocation hands out twice: one context's HTTP port that is also
/// some context's inspector port.
pub(super) fn port_clash(serving: &[Serving]) -> Option<u16> {
    serving
        .iter()
        .filter_map(|s| s.port)
        .find(|p| serving.iter().any(|s| s.inspector_port == Some(*p)))
}

/// #3237: a lone worker with no `--inspect` and no `--inspect-port` has no
/// inspector port of its own, so `wrangler dev` takes 9229 — and two sessions
/// started together both see it free and both bind it, one dying with `Address
/// already in use`. Give it the free port the OS hands out instead. `free` is
/// `None` when no port could be found, or when the `--` passthrough already
/// carries `--inspector-port` (wrangler rejects it twice, and the caller chose
/// it); the worker then keeps wrangler's own default, as before.
pub fn assign_free_inspector(serving: &mut [Serving], free: Option<u16>) {
    if let [only] = serving
        && only.inspector_port.is_none()
    {
        only.inspector_port = free;
    }
}

/// A free localhost port, as the OS reports it right now. The listener is
/// dropped before wrangler binds the port, so another process could still take
/// it in between; that window is milliseconds, against the seconds in which two
/// sessions starting together both saw 9229 free.
pub(super) fn free_local_port() -> Option<u16> {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .ok()
}

/// The first inspector port in the allocation that something is already
/// listening on, so `bynk dev` can say so instead of leaving it to wrangler's
/// `Address already in use`.
pub(super) fn inspector_port_in_use(serving: &[Serving]) -> Option<u16> {
    first_inspector_port_where(serving, port_is_in_use)
}

/// The first inspector port in the allocation for which `in_use` holds. The
/// probe is a parameter so a test can say "this port is free" without relying
/// on the OS leaving a port it just released alone (#3526).
fn first_inspector_port_where(serving: &[Serving], in_use: impl Fn(u16) -> bool) -> Option<u16> {
    serving
        .iter()
        .filter_map(|s| s.inspector_port)
        .find(|p| in_use(*p))
}

/// Whether binding `port` on localhost fails because something holds it.
fn port_is_in_use(port: u16) -> bool {
    matches!(
        std::net::TcpListener::bind(("127.0.0.1", port)),
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse
    )
}

/// The `wrangler dev` flags the driver injects for one worker: the ports it
/// allocated (#552) — `--port` when serving several contexts, `--inspector-port`
/// under `--inspect` (slice 3, ADR 0104), so a JavaScript debugger can attach and
/// `.bynk` breakpoints resolve through the emitted source maps.
///
/// A lone worker without `--base-port` or `--inspect` gets no port flag — the
/// pre-#552 invocation, so that path keeps its `-- --port N` passthrough.
///
/// I2 (#1971): and the local key for each edge the Worker is on (`edge_keys`,
/// its [`crate::deploy::local_edge_keys`]), as `--var`s rather than a file:
/// every internal request (`/_bynk/call/`, `/_bynk/event/`) is signed under
/// its edge's key, and a lone context still delivers its own events to
/// itself. The fixed [`bynk_emit::LOCAL_CALL_KEY`], so two `bynk dev
/// --context` sessions run side by side admit each other's calls.
/// The keys `bynk dev` gives each Worker through `--var`: the local edge keys (I2, #1971), and
/// (#3386) a local signing key for each `SignedCookie` binding: derived from the binding's name,
/// so each actor has its own and two `bynk dev --context` sessions side by side admit each other's
/// cookies. It is for local development only; `deploy` mints its own.
pub(super) struct LocalKeys {
    pub(super) edge: BTreeMap<String, Vec<String>>,
    session: BTreeMap<String, Vec<String>>,
}

impl LocalKeys {
    pub(super) fn read(
        workers_dir: &Path,
        available: &[String],
        model: &bynk_emit::emit_model::WorkersBuild,
    ) -> Result<Self, String> {
        Ok(Self {
            edge: crate::deploy::local_edge_keys(workers_dir, available, model)?,
            session: crate::deploy::local_session_keys(workers_dir, available, model)?,
        })
    }

    /// `--var NAME:<key>` for each session key a Worker holds, except one the operator already
    /// passes after `--` (that is theirs).
    pub(super) fn session_args(&self, worker: &str, passthrough: &[String]) -> Vec<String> {
        let mut args = Vec::new();
        for name in self.session.get(worker).into_iter().flatten() {
            if passthrough.iter().any(|a| {
                let a = a.strip_prefix("--var=").unwrap_or(a);
                a.starts_with(&format!("{name}:"))
            }) {
                continue;
            }
            args.push("--var".to_string());
            args.push(format!("{name}:{}", crate::deploy::local_session_key(name)));
        }
        args
    }
}

pub(super) fn serve_args(s: &Serving, edge_keys: &[String]) -> Vec<String> {
    let mut args = Vec::new();
    for key in edge_keys {
        args.push("--var".to_string());
        args.push(format!("{key}:{}", bynk_emit::LOCAL_CALL_KEY));
    }
    if let Some(port) = s.port {
        args.push("--port".to_string());
        args.push(port.to_string());
    }
    if let Some(port) = s.inspector_port {
        args.push("--inspector-port".to_string());
        args.push(port.to_string());
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dev::run::DevOptions;
    use crate::dev::test_support::names;

    use std::collections::BTreeMap;

    /// I2 (#1971): a Worker's edge keys come first, each the local key.
    #[test]
    fn every_edge_key_is_passed_as_the_local_key() {
        let serving = allocate(&names(&["api"]), None, &DevOptions::default());
        assert_eq!(
            serve_args(
                &serving[0],
                &names(&["BYNK_CALL_FROM_WORKER", "BYNK_CALL_TO_WORKER"])
            ),
            names(&[
                "--var",
                "BYNK_CALL_FROM_WORKER:bynk-local-call-key",
                "--var",
                "BYNK_CALL_TO_WORKER:bynk-local-call-key",
            ])
        );
    }

    #[test]
    fn inspect_injects_the_inspector_port() {
        let off = DevOptions::default();
        let lone = allocate(&names(&["links"]), None, &off);
        assert_eq!(
            serve_args(&lone[0], &[]),
            Vec::<String>::new(),
            "a lone worker without --inspect keeps the pre-#552 invocation"
        );

        let on = DevOptions {
            inspect: true,
            inspect_port: Some(9229),
            ..Default::default()
        };
        let lone = allocate(&names(&["links"]), None, &on);
        assert_eq!(
            serve_args(&lone[0], &[]),
            names(&["--inspector-port", "9229"])
        );
    }

    // ---- #3237: inspector ports for a lone worker -------------------------

    #[test]
    fn an_explicit_inspect_port_is_honoured_for_a_lone_worker() {
        let given = DevOptions {
            inspect_port: Some(9300),
            ..Default::default()
        };
        let lone = allocate(&names(&["links"]), None, &given);
        assert_eq!(lone[0].inspector_port, Some(9300));
        assert!(!lone[0].inspect, "an explicit port is not a debug session");
        assert_eq!(
            serve_args(&lone[0], &[]),
            names(&["--inspector-port", "9300"])
        );
        // Several contexts still count up from it.
        let two = allocate(&names(&["a", "b"]), None, &given);
        assert_eq!(
            two.iter().map(|s| s.inspector_port).collect::<Vec<_>>(),
            vec![Some(9300), Some(9301)]
        );
    }

    #[test]
    fn a_session_key_is_a_per_run_var_unless_the_operator_passes_their_own() {
        let local = LocalKeys {
            edge: BTreeMap::new(),
            session: BTreeMap::from([("shop".to_string(), vec!["SESSION_KEY".to_string()])]),
        };
        let key = crate::deploy::local_session_key("SESSION_KEY");
        assert_eq!(
            local.session_args("shop", &[]),
            ["--var".to_string(), format!("SESSION_KEY:{key}")]
        );
        // Each binding has its own key, and it is the same in every session.
        assert_ne!(key, crate::deploy::local_session_key("ADMIN_KEY"));
        assert_eq!(key, crate::deploy::local_session_key("SESSION_KEY"));
        assert!(local.session_args("other", &[]).is_empty());
        let own = ["--var".to_string(), "SESSION_KEY:mine".to_string()];
        assert!(local.session_args("shop", &own).is_empty());
        let eq_form = ["--var=SESSION_KEY:mine".to_string()];
        assert!(local.session_args("shop", &eq_form).is_empty());
    }

    #[test]
    fn a_lone_worker_with_no_inspector_flags_is_given_a_free_port() {
        let mut lone = allocate(&names(&["links"]), None, &DevOptions::default());
        assert_eq!(lone[0].inspector_port, None);
        assign_free_inspector(&mut lone, Some(40123));
        assert_eq!(lone[0].inspector_port, Some(40123));
        assert_eq!(
            serve_args(&lone[0], &[]),
            names(&["--inspector-port", "40123"])
        );
        // No free port found (or `--inspector-port` passed through): unchanged.
        let mut none = allocate(&names(&["links"]), None, &DevOptions::default());
        assign_free_inspector(&mut none, None);
        assert_eq!(none[0].inspector_port, None);
    }

    #[test]
    fn a_free_inspector_never_overrides_an_allocated_one() {
        let given = DevOptions {
            inspect_port: Some(9300),
            ..Default::default()
        };
        let mut lone = allocate(&names(&["links"]), None, &given);
        assign_free_inspector(&mut lone, Some(40123));
        assert_eq!(lone[0].inspector_port, Some(9300));
        let mut two = allocate(&names(&["a", "b"]), None, &DevOptions::default());
        assign_free_inspector(&mut two, Some(40123));
        assert_eq!(two[0].inspector_port, Some(9229));
    }

    #[test]
    fn a_taken_inspector_port_is_found() {
        // The listener is held for the whole test, so the port stays taken.
        let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let taken = held.local_addr().unwrap().port();
        let given = DevOptions {
            inspect_port: Some(taken),
            ..Default::default()
        };
        let lone = allocate(&names(&["links"]), None, &given);
        assert_eq!(inspector_port_in_use(&lone), Some(taken));
        // A worker with no inspector port has nothing to find.
        let plain = allocate(&names(&["links"]), None, &DevOptions::default());
        assert_eq!(inspector_port_in_use(&plain), None);
        assert!(free_local_port().is_some());
    }

    #[test]
    fn a_free_inspector_port_is_not_reported() {
        // #3526: this used to drop the listener and assert the port was then
        // free, which fails whenever a parallel test's `bind(":0")` is handed
        // the port just released. The probe says "free" without the OS.
        let given = DevOptions {
            inspect_port: Some(9300),
            ..Default::default()
        };
        let lone = allocate(&names(&["links"]), None, &given);
        assert_eq!(first_inspector_port_where(&lone, |_| false), None);
        assert_eq!(first_inspector_port_where(&lone, |p| p == 9300), Some(9300));
    }

    // ---- #552: port allocation --------------------------------------------

    #[test]
    fn ports_are_allocated_per_context_from_the_base() {
        let opts = DevOptions {
            inspect: true,
            inspect_port: Some(9229),
            ..Default::default()
        };
        let serving = allocate(&names(&["api", "worker"]), None, &opts);
        assert_eq!(
            serving.iter().map(|s| s.port).collect::<Vec<_>>(),
            vec![Some(8787), Some(8788)],
            "each wrangler dev binds its own port"
        );
        assert_eq!(
            serving.iter().map(|s| s.inspector_port).collect::<Vec<_>>(),
            vec![Some(9229), Some(9230)],
            "inspector ports must not collide either"
        );
        assert_eq!(
            serve_args(&serving[1], &[]),
            names(&["--port", "8788", "--inspector-port", "9230"])
        );
    }

    #[test]
    fn base_port_moves_the_whole_allocation() {
        let serving = allocate(&names(&["a", "b"]), Some(9000), &DevOptions::default());
        assert_eq!(
            serving.iter().map(|s| s.port).collect::<Vec<_>>(),
            vec![Some(9000), Some(9001)]
        );
    }

    #[test]
    fn a_lone_worker_keeps_wranglers_own_port() {
        // The back-compat contract: no injected --port, so `-- --port N` still
        // reaches wrangler (a repeated --port is a hard error, not last-wins).
        let serving = allocate(&names(&["links"]), None, &DevOptions::default());
        assert_eq!(serving[0].port, None);
        assert_eq!(serve_args(&serving[0], &[]), Vec::<String>::new());
        // …but an explicit --base-port is honoured even for one context.
        let pinned = allocate(&names(&["links"]), Some(8900), &DevOptions::default());
        assert_eq!(pinned[0].port, Some(8900));
    }

    #[test]
    fn several_contexts_get_distinct_inspector_ports_without_inspect() {
        // #1954: wrangler's inspector is always on, so two wranglers left on
        // its default both bind 9229 and the second dies at boot.
        let plain = DevOptions {
            inspect_port: Some(9229), // the CLI default
            ..Default::default()
        };
        let serving = allocate(&names(&["api", "worker"]), None, &plain);
        assert_eq!(
            serving.iter().map(|s| s.inspector_port).collect::<Vec<_>>(),
            vec![Some(9229), Some(9230)]
        );
        assert_eq!(
            serve_args(&serving[1], &[]),
            names(&["--port", "8788", "--inspector-port", "9230"])
        );
        // `--inspect-port` moves that allocation even without `--inspect`…
        let moved = DevOptions {
            inspect_port: Some(9300),
            ..Default::default()
        };
        let serving = allocate(&names(&["api", "worker"]), None, &moved);
        assert_eq!(
            serving.iter().map(|s| s.inspector_port).collect::<Vec<_>>(),
            vec![Some(9300), Some(9301)]
        );
        // …and a lone worker without `--inspect` still gets none.
        let lone = allocate(&names(&["links"]), Some(8900), &DevOptions::default());
        assert_eq!(lone[0].inspector_port, None);
    }

    #[test]
    fn overlapping_http_and_inspector_ranges_are_caught() {
        let opts = DevOptions {
            inspect_port: Some(9229),
            ..Default::default()
        };
        let two = names(&["api", "worker"]);
        // HTTP 9228/9229 against inspectors 9229/9230.
        assert_eq!(port_clash(&allocate(&two, Some(9228), &opts)), Some(9229));
        assert_eq!(port_clash(&allocate(&two, None, &opts)), None);
        // A lone worker has neither, so nothing can clash.
        assert_eq!(port_clash(&allocate(&names(&["links"]), None, &opts)), None);
    }
}

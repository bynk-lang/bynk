use super::serving::DEFAULT_BASE_PORT;
use super::serving::Serving;
use std::path::Path;

/// `bynk dev --format` (#2094).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DevFormat {
    /// The readable stream on stderr, unchanged.
    #[default]
    Human,
    /// The readable stream on stderr, plus one JSON [`DevEvent`] per line on
    /// stdout. `wrangler dev`'s own stdout moves to stderr, so stdout carries
    /// nothing but events.
    Json,
}

/// One lifecycle event of a `bynk dev --format json` session (#2094): a stable,
/// line-delimited interface for the editor and CI scripts, so neither has to
/// parse the human report, whose wording is UX and changes. Serialised with an
/// `event` tag in snake_case. A consumer should ignore fields and events it
/// does not know: new ones may be added, existing ones keep their meaning.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum DevEvent {
    /// Every `wrangler dev` has been started. Ports are allocated, but a
    /// worker may still be booting; poll its URL for readiness.
    Serving { contexts: Vec<ServedContext> },
    /// A source change was seen and a rebuild has begun.
    Rebuilding,
    /// The rebuild succeeded; the workers hot-reload from it.
    RebuildOk,
    /// The rebuild failed. Its diagnostics are on stderr, and the last good
    /// build keeps serving.
    RebuildFailed,
    /// The session ended, with the exit code `bynk dev` is about to return.
    Stopped { code: u8 },
    /// One request a worker served (`--trace-events`, #3580): its route, status
    /// and duration, and the steps it took in call order. Steps carry no argument
    /// values. A cross-context call is its own step here; the callee's own
    /// request is a separate event.
    Request {
        /// The worker dir that served it, as in [`ServedContext::name`].
        context: String,
        /// This request's id.
        id: String,
        /// The id of the request that made this one, when it is a cross-context
        /// call from another worker (that request's `call` step carries this one's
        /// id as its `request`); `null` for a request from outside.
        parent: Option<String>,
        /// `<METHOD> <path>`.
        route: String,
        status: u16,
        duration_ms: f64,
        steps: Vec<crate::trace::TraceStep>,
        /// More steps happened than are listed (the list is bounded).
        truncated: bool,
        /// The fault that answered the request with a 500, with its stack frames
        /// mapped to `.bynk` locations; `null` when it did not fault.
        fault: Option<crate::trace::TraceFault>,
    },
    /// One line a worker printed (`--log-events`, #2474), with any stack frame
    /// into generated code rewritten to the `.bynk` line it came from. A
    /// thrown error is a headline line followed by one event per frame, all
    /// at the same level.
    Log {
        /// The worker dir that printed it, as in [`ServedContext::name`].
        context: String,
        level: crate::logs::LogLevel,
        text: String,
    },
}

/// One served context in a [`DevEvent::Serving`] event.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ServedContext {
    /// The context's worker dir (dasherised), as `--context` accepts it.
    pub name: String,
    /// The HTTP port it answers on.
    pub port: u16,
    /// `http://localhost:<port>`.
    pub url: String,
    /// Its V8 inspector port, when one was allocated (`--inspect`, or several
    /// contexts served); `null` otherwise.
    pub inspector_port: Option<u16>,
}

/// The [`DevEvent::Serving`] event for an allocation. A lone worker left on
/// wrangler's default (`port: None`) reports the port it actually gets: a
/// `-- --port N` passthrough, else wrangler's 8787.
pub fn serving_event(serving: &[Serving], wrangler_args: &[String]) -> DevEvent {
    DevEvent::Serving {
        contexts: serving
            .iter()
            .map(|s| {
                let port = s
                    .port
                    .or_else(|| passthrough_port(wrangler_args))
                    .unwrap_or(DEFAULT_BASE_PORT);
                ServedContext {
                    name: s.worker.clone(),
                    port,
                    url: format!("http://localhost:{port}"),
                    inspector_port: s.inspector_port,
                }
            })
            .collect(),
    }
}

/// One event as its JSON line (no trailing newline).
pub fn event_line(event: &DevEvent) -> String {
    serde_json::to_string(event).expect("a dev event serialises")
}

/// Write `event` to stdout under `--format json`; nothing under `human`.
pub(super) fn emit(format: DevFormat, event: &DevEvent) {
    if format == DevFormat::Json {
        use std::io::Write;
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{}", event_line(event));
        let _ = out.flush();
    }
}

/// The `--port` value of a `-- --port N` / `-- --port=N` passthrough.
fn passthrough_port(args: &[String]) -> Option<u16> {
    args.iter().enumerate().find_map(|(i, a)| {
        if a == "--port" {
            args.get(i + 1).and_then(|v| v.parse().ok())
        } else {
            a.strip_prefix("--port=").and_then(|v| v.parse().ok())
        }
    })
}

/// The session file's record (#3581) of the contexts a `serving` event names.
pub(super) fn session_of(serving: &DevEvent, workers_dir: &Path) -> crate::dev_inspect::Session {
    let DevEvent::Serving { contexts } = serving else {
        unreachable!("serving_event builds a Serving event")
    };
    crate::dev_inspect::Session {
        workers: contexts
            .iter()
            .map(|c| {
                (
                    c.name.clone(),
                    crate::dev_inspect::SessionWorker {
                        context: crate::dev_inspect::context_of_worker(&c.name),
                        url: c.url.clone(),
                        members: crate::dev_inspect::inspect_members(&workers_dir.join(&c.name)),
                    },
                )
            })
            .collect(),
    }
}

/// The start-up report: which context answers on which URL, plus the inspector
/// notice under `--inspect`. Pure and deterministic, so it is golden-pinned in
/// the style of ADR 0096 §Exit — unlike the `wrangler dev` streams it precedes.
///
/// A lone worker on wrangler's own default port prints no table: there is no
/// allocation to disclose and wrangler announces its own `Ready on` line, so
/// that session reads exactly as it did before #552.
pub fn serving_report(serving: &[Serving]) -> String {
    let mut out = String::new();
    if serving.iter().any(|s| s.port.is_some()) {
        // Only claim the wiring when there is something to wire: a subset of
        // one has no sibling to bind to, and saying otherwise would explain a
        // cross-context call's failure as a bug rather than as the missing
        // context it is.
        out.push_str(&match serving.len() {
            1 => "bynk dev: serving 1 context.\n".to_string(),
            n => format!(
                "bynk dev: serving {n} contexts — service bindings between them are wired.\n"
            ),
        });
        let width = serving.iter().map(|s| s.worker.len()).max().unwrap_or(0);
        for s in serving {
            let Some(port) = s.port else { continue };
            out.push_str(&format!(
                "  {:width$}  http://localhost:{port}\n",
                s.worker,
                width = width
            ));
        }
    }
    // Only `--inspect` announces the inspector. Without it a multi-context
    // session still allocates inspector ports (#1954), but only so they do not
    // collide; that is wrangler's default inspector, moved, not a debug session.
    let inspected = serving
        .iter()
        .filter(|s| s.inspect && s.inspector_port.is_some())
        .count();
    if inspected > 0 {
        out.push_str(&match inspected {
            1 => "bynk dev --inspect: the worker runs with the V8 inspector enabled.\n".to_string(),
            _ => "bynk dev --inspect: each worker runs with the V8 inspector enabled, on its own port.\n"
                .to_string(),
        });
        for s in serving {
            let (true, Some(port)) = (s.inspect, s.inspector_port) else {
                continue;
            };
            out.push_str(&format!(
                "  {} — inspector on port {port} (CDP discovery: http://127.0.0.1:{port}/json)\n",
                s.worker
            ));
        }
        out.push_str(
            "  Breakpoints set in `.bynk` sources resolve through the emitted source maps.\n\
             \x20 A hand-rolled CDP client must send an `Origin` header — VS Code's\n\
             \x20 JavaScript debugger does this for you.\n",
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dev::run::DevOptions;
    use crate::dev::run::passthrough_has;

    use crate::dev::serving::allocate;
    use crate::dev::test_support::names;

    #[test]
    fn an_allocated_inspector_is_announced_only_under_inspect() {
        let report = serving_report(&allocate(
            &names(&["api", "worker"]),
            None,
            &DevOptions::default(),
        ));
        assert!(!report.contains("inspector"), "{report}");
    }

    #[test]
    fn passthrough_port_is_detected_in_both_spellings() {
        assert!(passthrough_has(&names(&["--port", "8788"]), "--port"));
        assert!(passthrough_has(&names(&["--port=8788"]), "--port"));
        assert!(!passthrough_has(&names(&["--remote"]), "--port"));
        // `--inspector-port` must not be mistaken for `--port`.
        assert!(!passthrough_has(
            &names(&["--inspector-port", "9229"]),
            "--port"
        ));
    }

    #[test]
    fn the_serving_report_lists_context_urls() {
        let serving = allocate(
            &names(&["commerce-orders", "commerce-payment"]),
            None,
            &DevOptions::default(),
        );
        let report = serving_report(&serving);
        assert!(report.contains("serving 2 contexts"), "{report}");
        // Names are padded to the widest, so the URLs line up in a column.
        assert!(
            report.contains("commerce-orders   http://localhost:8787"),
            "{report}"
        );
        assert!(
            report.contains("commerce-payment  http://localhost:8788"),
            "{report}"
        );
        // A lone default-port worker discloses no allocation — wrangler's own
        // `Ready on` line is the announcement, exactly as before #552.
        assert_eq!(
            serving_report(&allocate(&names(&["links"]), None, &DevOptions::default())),
            ""
        );
    }

    #[test]
    fn the_serving_event_reports_every_port() {
        let serving = allocate(
            &names(&["commerce-orders", "commerce-payment"]),
            None,
            &DevOptions {
                inspect_port: Some(9229),
                ..Default::default()
            },
        );
        assert_eq!(
            event_line(&serving_event(&serving, &[])),
            r#"{"event":"serving","contexts":[{"name":"commerce-orders","port":8787,"url":"http://localhost:8787","inspector_port":9229},{"name":"commerce-payment","port":8788,"url":"http://localhost:8788","inspector_port":9230}]}"#
        );
        // A lone worker on wrangler's default still reports a port: 8787, or a
        // `-- --port N` passthrough in either spelling.
        let lone = allocate(&names(&["links"]), None, &DevOptions::default());
        let port = |args: &[&str]| match serving_event(&lone, &names(args)) {
            DevEvent::Serving { contexts } => (contexts[0].port, contexts[0].inspector_port),
            other => panic!("{other:?}"),
        };
        assert_eq!(port(&[]), (8787, None));
        assert_eq!(port(&["--port", "9000"]), (9000, None));
        assert_eq!(port(&["--remote", "--port=9001"]), (9001, None));
    }

    #[test]
    fn lifecycle_events_are_tagged_lines() {
        assert_eq!(
            event_line(&DevEvent::Rebuilding),
            r#"{"event":"rebuilding"}"#
        );
        assert_eq!(
            event_line(&DevEvent::RebuildOk),
            r#"{"event":"rebuild_ok"}"#
        );
        assert_eq!(
            event_line(&DevEvent::RebuildFailed),
            r#"{"event":"rebuild_failed"}"#
        );
        assert_eq!(
            event_line(&DevEvent::Stopped { code: 130 }),
            r#"{"event":"stopped","code":130}"#
        );
    }

    #[test]
    fn a_log_event_is_a_tagged_line() {
        assert_eq!(
            event_line(&DevEvent::Log {
                context: "shop".into(),
                level: crate::logs::LogLevel::Error,
                text: "    at f (/p/src/a.bynk:3:1)".into(),
            }),
            r#"{"event":"log","context":"shop","level":"error","text":"    at f (/p/src/a.bynk:3:1)"}"#
        );
    }

    /// #3580: the `request` event's documented shape.
    #[test]
    fn a_request_event_is_a_tagged_line() {
        let event = DevEvent::Request {
            context: "shop-orders".into(),
            id: "r1".into(),
            parent: Some("p0".into()),
            route: "POST /orders".into(),
            status: 201,
            duration_ms: 12.5,
            steps: vec![crate::trace::TraceStep {
                kind: "call".into(),
                target: "shop.payment.authorise".into(),
                span: Some(crate::trace::TraceSpan {
                    file: "src/shop/orders.bynk".into(),
                    line: 23,
                    col: 14,
                }),
                start_ms: Some(1.5),
                ms: 3.5,
                outcome: "ok".into(),
                request: Some("c1".into()),
            }],
            truncated: false,
            fault: None,
        };
        assert_eq!(
            event_line(&event),
            r#"{"event":"request","context":"shop-orders","id":"r1","parent":"p0","route":"POST /orders","status":201,"duration_ms":12.5,"steps":[{"kind":"call","target":"shop.payment.authorise","span":{"file":"src/shop/orders.bynk","line":23,"col":14},"start_ms":1.5,"ms":3.5,"outcome":"ok","request":"c1"}],"truncated":false,"fault":null}"#
        );
    }

    #[test]
    fn a_subset_of_one_claims_no_wiring() {
        // "bindings between them are wired" would be a lie for a single
        // context, and a misleading one: it invites reading a cross-context
        // call's failure as a bug rather than as the context left unserved.
        let report = serving_report(&allocate(
            &names(&["commerce-payment"]),
            Some(8890),
            &DevOptions::default(),
        ));
        assert!(report.contains("serving 1 context."), "{report}");
        assert!(!report.contains("bindings"), "{report}");
    }
}

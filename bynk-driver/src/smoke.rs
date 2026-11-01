//! `bynk test --smoke` (#2658): a `system` suite per context, derived from its
//! `from http` services, so every declared route is exercised on a real
//! runtime without a test written for it.
//!
//! Each route gets up to three cases, all in existing test surface:
//!
//! 1. **answers** — a typed call with every argument fabricated by `Val`, under
//!    a fabricated principal; it passes unless the answer is a 5xx. A smoke
//!    input is valid by construction, so a server error is the defect a smoke
//!    exists to find, even one the handler constructs itself.
//! 2. **rejects a malformed body** (a route with a JSON `body`) — the same call
//!    with `Wire("{not json")` as the body, which the boundary must refuse. A
//!    service with `on rejected` renders that refusal itself, as a `400` the
//!    harness reads as `Handled(BadRequest(_))`.
//! 3. **refuses no credential** (a route whose principal is not public) — the
//!    same call `by Nobody`, which the auth seam must refuse.
//!
//! The synthesised suites are added to the build's source overlay, beside the
//! context file they test, as the doc-example suites are (#2461), and the
//! project's own suites are left out: a smoke run runs only the smoke. Whether a
//! case can be built is decided by checking it: a case whose check reports one
//! of the codes in `SKIPPABLE` (a `Matches` type `Val` cannot fabricate, a
//! principal it cannot construct) is dropped and reported as skipped, with the
//! checker's reason. Any other diagnostic in a synthesised suite is a bug in
//! this module, never a skip, so a run cannot pass green by skipping everything.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bynk_emit::project::AttributedError;
use bynk_syntax::ast::{
    ActorDecl, CommonsItem, Context, Handler, HandlerKind, HttpMethod, ServiceDecl,
    ServiceProtocol, SourceUnit,
};
use bynk_syntax::lexer::tokenize;
use bynk_syntax::parser::parse_unit_with_recovery;

/// The tail of a synthesised suite's file name: `<stem>.smoke.bynk` beside the
/// context's `<stem>.bynk`.
pub const SYNTHETIC_SUFFIX: &str = ".smoke.bynk";

/// Every case a smoke suite holds is named with this prefix, so it cannot
/// collide with a case the author wrote.
pub const CASE_PREFIX: &str = "smoke: ";

/// The diagnostics that make a case un-buildable rather than wrong: the case is
/// skipped with the checker's message as its reason. A code ending in `*`
/// matches by prefix.
const SKIPPABLE: &[&str] = &[
    // A `String where Matches(…)` has no generator.
    "bynk.val.needs_pin",
    "bynk.val.agent_not_generable",
    // A principal the synthesiser could not construct.
    "bynk.test.actor_identity_required",
    "bynk.test.actor_no_identity",
    "bynk.test.principal_identity_mismatch",
    "bynk.test.principal_required",
    // A case a run against a real runtime cannot make.
    "bynk.test.remote_unsupported_*",
];

/// Which routes a smoke run may call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Methods {
    /// In process, or against a local `bynk dev`: state is disposable.
    All,
    /// Against a named target without `--smoke-unsafe`: `GET` only, since a
    /// fabricated `POST` would write real records to a shared deployment.
    SafeOnly,
}

/// A route the smoke run does not call, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skip {
    /// The suite it would have been in: the context's dotted name.
    pub suite: String,
    pub case: String,
    /// A diagnostic code, or `smoke` for a route left out by policy.
    pub code: String,
    pub reason: String,
}

/// One synthesised case: its name and its byte range in the suite text.
#[derive(Debug, Clone)]
struct Case {
    name: String,
    range: std::ops::Range<usize>,
}

/// One context's synthesised suite.
#[derive(Debug, Clone)]
struct Suite {
    key: PathBuf,
    context: String,
    header: String,
    /// The text of each case, in order.
    cases: Vec<(Case, String)>,
}

impl Suite {
    /// The suite's source, recording each case's byte range.
    fn render(&mut self) -> String {
        let mut text = self.header.clone();
        for (case, body) in &mut self.cases {
            let start = text.len();
            text.push_str(body);
            case.range = start..text.len();
        }
        text.push_str("}\n");
        text
    }
}

/// The smoke suites for a project, and what they leave out.
#[derive(Debug, Clone, Default)]
pub struct Smoke {
    suites: Vec<Suite>,
    skipped: Vec<Skip>,
    routes: usize,
}

/// Why a smoke run cannot go ahead.
pub enum SmokeError {
    /// A synthesised suite has a diagnostic that is not a skip: a bug here.
    Synthesis(Vec<AttributedError>),
    /// The project itself does not check.
    Project(Vec<AttributedError>),
}

impl Smoke {
    /// Synthesise the smoke suites for every `from http` service in `sources`.
    pub fn collect(sources: &HashMap<PathBuf, String>, methods: Methods) -> Self {
        let mut keys: Vec<&PathBuf> = sources.keys().collect();
        keys.sort();
        let mut contexts: Vec<(&PathBuf, Context)> = Vec::new();
        let mut actors: HashMap<String, (ActorDecl, String)> = HashMap::new();
        for key in keys {
            let source = &sources[key];
            let Ok(tokens) = tokenize(source) else {
                continue;
            };
            let (unit, errors) = parse_unit_with_recovery(&tokens, source);
            if !errors.is_empty() {
                continue;
            }
            let items = match &unit {
                Some(SourceUnit::Context(c)) => &c.items,
                Some(SourceUnit::Commons(c)) => &c.items,
                _ => continue,
            };
            for item in items {
                if let CommonsItem::Actor(a) = item {
                    actors
                        .entry(a.name.name.clone())
                        .or_insert_with(|| (a.clone(), source.clone()));
                }
            }
            if let Some(SourceUnit::Context(c)) = unit {
                contexts.push((key, c));
            }
        }
        let mut smoke = Smoke::default();
        for (key, context) in contexts {
            smoke.add_context(sources, key, &context, &actors, methods);
        }
        smoke
    }

    fn add_context(
        &mut self,
        sources: &HashMap<PathBuf, String>,
        key: &Path,
        context: &Context,
        actors: &HashMap<String, (ActorDecl, String)>,
        methods: Methods,
    ) {
        let source = &sources[key];
        let name = context.name.joined();
        let services: Vec<&ServiceDecl> = context
            .items
            .iter()
            .filter_map(|item| match item {
                CommonsItem::Service(s) if matches!(s.protocol, ServiceProtocol::Http) => Some(s),
                _ => None,
            })
            .collect();
        if services.is_empty() {
            return;
        }
        let stem = key.file_stem().unwrap_or_default().to_string_lossy();
        let synth = key.with_file_name(format!("{stem}{SYNTHETIC_SUFFIX}"));
        // A file the project really has is never overwritten.
        if sources.contains_key(&synth) {
            return;
        }
        let mut suite = Suite {
            key: synth,
            context: name.clone(),
            header: format!(
                "// Generated by `bynk test --smoke` from the routes of `{name}`.\n\
                 suite {name} as system {{\n"
            ),
            cases: Vec::new(),
        };
        let qualify = services.len() > 1;
        for service in services {
            let renders_rejections = service
                .handlers
                .iter()
                .any(|h| matches!(h.kind, HandlerKind::Rejected));
            for handler in &service.handlers {
                let HandlerKind::Http { method, path } = &handler.kind else {
                    continue;
                };
                self.routes += 1;
                let route = if qualify {
                    format!("{} {} {path}", service.name.name, method.as_str())
                } else {
                    format!("{} {path}", method.as_str())
                };
                let route = Route {
                    label: route,
                    service: &service.name.name,
                    method: *method,
                    path,
                    handler,
                    source,
                    renders_rejections,
                };
                if methods == Methods::SafeOnly && *method != HttpMethod::Get {
                    self.skipped.push(Skip {
                        suite: name.clone(),
                        case: route.case_name("answers"),
                        code: "smoke".to_string(),
                        reason: format!(
                            "a `{}` route writes, so a run against a named target calls only `GET` routes; pass `--smoke-unsafe` to call it",
                            method.as_str()
                        ),
                    });
                    continue;
                }
                let principal = principal(handler, service, actors);
                suite.cases.extend(route.cases(&principal));
            }
        }
        if !suite.cases.is_empty() {
            self.suites.push(suite);
        }
    }

    /// The overlay a smoke run builds: `sources` without the project's own
    /// suites, plus the synthesised ones.
    pub fn overlay(&mut self, sources: &HashMap<PathBuf, String>) -> HashMap<PathBuf, String> {
        let mut out: HashMap<PathBuf, String> = sources
            .iter()
            .filter(|(_, text)| !is_suite(text))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for suite in &mut self.suites {
            out.insert(suite.key.clone(), suite.render());
        }
        out
    }

    /// Check the synthesised suites with `check`, dropping every case that
    /// reports a code in `SKIPPABLE`, until they check clean. Returns the
    /// overlay to build.
    pub fn settle(
        &mut self,
        sources: &HashMap<PathBuf, String>,
        check: impl Fn(HashMap<PathBuf, String>) -> Vec<AttributedError>,
    ) -> Result<HashMap<PathBuf, String>, SmokeError> {
        loop {
            let overlay = self.overlay(sources);
            let errors = check(overlay.clone());
            if errors.is_empty() {
                return Ok(overlay);
            }
            let (ours, theirs): (Vec<_>, Vec<_>) =
                errors.into_iter().partition(|e| self.suite_of(e).is_some());
            if !theirs.is_empty() {
                return Err(SmokeError::Project(theirs));
            }
            let mut unexplained = Vec::new();
            let mut dropped = false;
            for error in ours {
                let suite = self.suite_of(&error).expect("partitioned on it");
                let start = error.error.span.start;
                let Some(i) = self.suites[suite]
                    .cases
                    .iter()
                    .position(|(c, _)| c.range.contains(&start))
                else {
                    unexplained.push(error);
                    continue;
                };
                if !skippable(error.error.category) {
                    unexplained.push(error);
                    continue;
                }
                let (case, _) = self.suites[suite].cases.remove(i);
                self.skipped.push(Skip {
                    suite: self.suites[suite].context.clone(),
                    case: case.name,
                    code: error.error.category.to_string(),
                    reason: error.error.message.clone(),
                });
                dropped = true;
                // The other errors' ranges are stale until the next render.
                break;
            }
            if !unexplained.is_empty() {
                return Err(SmokeError::Synthesis(unexplained));
            }
            if !dropped {
                return Ok(overlay);
            }
            self.suites.retain(|s| !s.cases.is_empty());
        }
    }

    fn suite_of(&self, error: &AttributedError) -> Option<usize> {
        let path = error.source_path.as_ref()?;
        self.suites
            .iter()
            .position(|s| path == &s.key || s.key.ends_with(path))
    }

    /// The routes considered, the cases that will run, and the ones skipped.
    pub fn summary(&self) -> (usize, usize, &[Skip]) {
        let cases = self.suites.iter().map(|s| s.cases.len()).sum();
        (self.routes, cases, &self.skipped)
    }

    /// The synthesised suites as source, for `--print`: each headed by the
    /// path it would live at, with the skipped cases listed as comments.
    pub fn print(&mut self, root: &Path) -> String {
        let mut out = String::new();
        for suite in &mut self.suites {
            let path = suite.key.strip_prefix(root).unwrap_or(&suite.key);
            out.push_str(&format!("// {}\n", path.display()));
            out.push_str(&suite.render());
            out.push('\n');
        }
        for skip in &self.skipped {
            out.push_str(&format!(
                "// skipped `{}` case \"{}\" [{}]: {}\n",
                skip.suite, skip.case, skip.code, skip.reason
            ));
        }
        out
    }
}

/// The environment variable that turns on the test runtime's strict decoding
/// of an answer (`responseToHttpResult`): a smoke case fails on a status the
/// runtime does not know, or a JSON body that does not decode as the route's
/// declared type, where an author's own case gets the raw value to judge.
pub const STRICT_WIRE_ENV: &str = "BYNK_TEST_STRICT_WIRE";

/// A smoke run ready to build: the options with the synthesised suites in place
/// of the project's own, and the skips to report.
pub struct Prepared {
    pub options: bynk_emit::project::CompileOptions,
    pub smoke: Smoke,
}

/// Synthesise and settle the smoke suites over `options`' sources. `report`
/// prints what was skipped and the counts; a nested run (one `--against dev`
/// group) leaves that to the run that started it. On a failure the reason has
/// been printed and the exit code is returned.
pub fn prepare(
    program: &str,
    options: bynk_emit::project::CompileOptions,
    methods: Methods,
    json: bool,
    stream: bool,
    report: bool,
) -> Result<Prepared, std::process::ExitCode> {
    use crate::test_json::TestRun;
    let fail = |message: String| {
        if json {
            crate::test_stream::print_run(stream, &TestRun::runtime_error(message, None));
        } else {
            eprintln!("{program} test: {message}");
        }
        std::process::ExitCode::FAILURE
    };
    let Some(sources) = options.sources.clone() else {
        return Err(fail(
            "`--smoke` needs a project directory, not a single file".to_string(),
        ));
    };
    let mut smoke = Smoke::collect(&sources, methods);
    let check = |overlay: HashMap<PathBuf, String>| {
        bynk_emit::project::check_project(&options.clone().sources(overlay)).errors
    };
    let overlay = match smoke.settle(&sources, check) {
        Ok(overlay) => overlay,
        Err(e) => {
            let (errors, note) = match e {
                SmokeError::Project(errors) => (errors, None),
                SmokeError::Synthesis(errors) => (
                    errors,
                    Some(
                        "a synthesised smoke suite does not check: this is a bug in `--smoke`, please report it at https://github.com/bynk-lang/bynk/issues",
                    ),
                ),
            };
            let check = bynk_emit::project::check_project(
                &options.clone().sources(smoke.overlay(&sources)),
            );
            let failure = bynk_emit::project::ProjectFailure {
                errors,
                snapshots: check.snapshots,
                display_root: check.display_root,
            };
            if json {
                crate::test_stream::print_run(
                    stream,
                    &TestRun::compile_error(crate::project_failure_short_lines(&failure)),
                );
            } else {
                crate::print_project_failure(&failure);
            }
            if let Some(note) = note {
                eprintln!("{program} test: {note}");
            }
            return Err(std::process::ExitCode::FAILURE);
        }
    };
    let (routes, cases, skipped) = smoke.summary();
    if report {
        for skip in skipped {
            eprintln!(
                "{program} test: skipped `{}` case \"{}\" [{}]: {}",
                skip.suite, skip.case, skip.code, skip.reason
            );
        }
        eprintln!(
            "{program} test: smoke: {cases} case{} from {routes} route{}, {} skipped",
            if cases == 1 { "" } else { "s" },
            if routes == 1 { "" } else { "s" },
            skipped.len()
        );
    }
    if cases == 0 {
        return Err(fail(if routes == 0 {
            "`--smoke` found no `from http` route to call".to_string()
        } else {
            format!("`--smoke` could build no case: all {routes} routes were skipped")
        }));
    }
    Ok(Prepared {
        options: options.sources(overlay),
        smoke,
    })
}

impl Smoke {
    /// The skips as `(suite, skip)` pairs, the shape a remote run's take.
    pub fn remote_skips(&self) -> Vec<(String, bynk_emit::project::RemoteSkip)> {
        self.skipped
            .iter()
            .map(|s| {
                (
                    s.suite.clone(),
                    bynk_emit::project::RemoteSkip {
                        case: s.case.clone(),
                        code: s.code.clone(),
                        reason: s.reason.clone(),
                    },
                )
            })
            .collect()
    }
}

fn skippable(code: &str) -> bool {
    SKIPPABLE.iter().any(|s| match s.strip_suffix('*') {
        Some(prefix) => code.starts_with(prefix),
        None => code == *s,
    })
}

/// Whether `text` is a test file: a `suite` unit.
fn is_suite(text: &str) -> bool {
    let Ok(tokens) = tokenize(text) else {
        return false;
    };
    matches!(
        parse_unit_with_recovery(&tokens, text).0,
        Some(SourceUnit::Suite(_))
    )
}

/// How a case acts.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Principal {
    /// The `by` clause's text, such as `Visitor` or `User(Val[UserId])`.
    by: String,
    /// The route admits an unauthenticated caller, so there is nothing to
    /// refuse.
    public: bool,
}

/// The principal to call `handler` as: a public member of its actor sum if it
/// has one, else its first actor, with an identity fabricated by `Val`.
fn principal(
    handler: &Handler,
    service: &ServiceDecl,
    actors: &HashMap<String, (ActorDecl, String)>,
) -> Principal {
    let by = handler.by_clause.as_ref().or(service.default_by.as_ref());
    let names: Vec<&str> = by
        .map(|b| b.actors.iter().map(|a| a.name.as_str()).collect())
        .unwrap_or_default();
    let is_public = |name: &str| {
        name == "Visitor"
            || actors
                .get(name)
                .is_some_and(|(a, _)| a.auth.as_ref().is_some_and(|s| s.name == "None"))
    };
    if let Some(public) = names.iter().find(|n| is_public(n)) {
        return Principal {
            by: (*public).to_string(),
            public: true,
        };
    }
    let Some(first) = names.first() else {
        return Principal {
            by: "Visitor".to_string(),
            public: true,
        };
    };
    // A refinement actor carries its base actor's identity.
    let base = actors
        .get(*first)
        .and_then(|(a, _)| a.refinement.as_ref())
        .map_or(*first, |r| r.base.name.as_str());
    let identity = actors.get(base).and_then(|(a, source)| {
        let ty = a.identity.as_ref()?;
        let span = ty.span();
        source.get(span.start..span.end).map(str::to_string)
    });
    Principal {
        by: match identity {
            Some(ty) => format!("{first}(Val[{ty}])"),
            None => (*first).to_string(),
        },
        public: false,
    }
}

/// One route, and the source of the context declaring it.
struct Route<'a> {
    label: String,
    service: &'a str,
    method: HttpMethod,
    path: &'a str,
    handler: &'a Handler,
    source: &'a str,
    renders_rejections: bool,
}

impl Route<'_> {
    fn case_name(&self, what: &str) -> String {
        format!("{CASE_PREFIX}{} {what}", self.label)
    }

    fn is_raw(&self) -> bool {
        self.handler
            .annotations
            .iter()
            .any(|a| a.name.name == "raw")
    }

    /// Each argument's text, in the handler's parameter order; `body` is
    /// replaced by `body` when given.
    fn args(&self, body: Option<&str>) -> Vec<String> {
        self.handler
            .params
            .iter()
            .map(|p| {
                if p.name.name == "headers" {
                    return "RequestHeaders.of([])".to_string();
                }
                if p.name.name == "body"
                    && let Some(body) = body
                {
                    return body.to_string();
                }
                let span = p.type_ref.span();
                let ty = self.source.get(span.start..span.end).unwrap_or("()");
                format!("Val[{ty}]")
            })
            .collect()
    }

    fn call(&self, args: &[String], by: &str) -> String {
        let mut all = vec![format!("{:?}", self.path)];
        all.extend(args.iter().cloned());
        format!(
            "{}.{}({}) by {by}",
            self.service,
            self.method.as_str(),
            all.join(", ")
        )
    }

    fn cases(&self, principal: &Principal) -> Vec<(Case, String)> {
        let case = |name: String, call: String, expect: &str| {
            (
                Case {
                    name: name.clone(),
                    range: 0..0,
                },
                format!("  case {name:?} {{\n    let r <- {call}\n    expect {expect}\n  }}\n"),
            )
        };
        let args = self.args(None);
        let mut out = vec![case(
            self.case_name("answers"),
            self.call(&args, &principal.by),
            "!(r is ServerError(_) || r is NotImplemented(_) || r is BadGateway(_) || r is ServiceUnavailable(_) || r is GatewayTimeout(_))",
        )];
        let has_body = self.handler.params.iter().any(|p| p.name.name == "body");
        if has_body && !self.is_raw() {
            let expect = if self.renders_rejections {
                "r is Rejected(_) || r is Handled(BadRequest(_))"
            } else {
                "r is Rejected(_)"
            };
            out.push(case(
                self.case_name("rejects a malformed body"),
                self.call(&self.args(Some("Wire(\"{not json\")")), &principal.by),
                expect,
            ));
        }
        if !principal.public {
            out.push(case(
                self.case_name("refuses no credential"),
                self.call(&args, "Nobody"),
                "r is Rejected(Unauthorized)",
            ));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sources(files: &[(&str, &str)]) -> HashMap<PathBuf, String> {
        files
            .iter()
            .map(|(k, v)| (PathBuf::from(k), v.to_string()))
            .collect()
    }

    const SHOP: &str = r#"context shop.api

type UserId = String where NonEmpty
actor User { auth = Bearer(secret = "AUTH_SECRET"), identity = UserId }
type Item = { sku: String where NonEmpty }

service api from http {
  on GET("/ping") () -> Effect[HttpResult[String]] by v: Visitor {
    Ok("pong")
  }
  on PUT("/cart/:sku") (sku: String, body: Item) -> Effect[HttpResult[Item]] by u: User {
    Created(body)
  }
}
"#;

    #[test]
    fn a_route_gets_answers_rejects_and_refuses_cases() {
        let src = sources(&[("/p/src/shop/api.bynk", SHOP)]);
        let mut smoke = Smoke::collect(&src, Methods::All);
        let overlay = smoke.overlay(&src);
        let text = &overlay[&PathBuf::from("/p/src/shop/api.smoke.bynk")];
        assert!(text.contains("suite shop.api as system {"), "{text}");
        assert!(
            text.contains(r#"let r <- api.GET("/ping") by Visitor"#),
            "{text}"
        );
        assert!(
            text.contains(r#"api.PUT("/cart/:sku", Val[String], Val[Item]) by User(Val[UserId])"#),
            "{text}"
        );
        assert!(
            text.contains(
                r#"api.PUT("/cart/:sku", Val[String], Wire("{not json")) by User(Val[UserId])"#
            ),
            "{text}"
        );
        assert!(
            text.contains(r#"api.PUT("/cart/:sku", Val[String], Val[Item]) by Nobody"#),
            "{text}"
        );
        // A public route has no credential to refuse, and a GET no body.
        assert!(!text.contains("smoke: GET /ping refuses"), "{text}");
        assert!(!text.contains("smoke: GET /ping rejects"), "{text}");
        assert_eq!(smoke.summary().0, 2);
        assert_eq!(smoke.summary().1, 4);
    }

    #[test]
    fn a_named_target_calls_only_get_routes() {
        let src = sources(&[("/p/src/shop/api.bynk", SHOP)]);
        let mut smoke = Smoke::collect(&src, Methods::SafeOnly);
        let overlay = smoke.overlay(&src);
        let text = &overlay[&PathBuf::from("/p/src/shop/api.smoke.bynk")];
        assert!(!text.contains("PUT"), "{text}");
        let (_, cases, skipped) = smoke.summary();
        assert_eq!(cases, 1);
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].case, "smoke: PUT /cart/:sku answers");
        assert!(skipped[0].reason.contains("--smoke-unsafe"));
    }

    #[test]
    fn the_projects_own_suites_are_left_out() {
        let src = sources(&[
            ("/p/src/shop/api.bynk", SHOP),
            (
                "/p/tests/api.test.bynk",
                "suite shop.api {\n  case \"x\" {\n    expect true\n  }\n}\n",
            ),
        ]);
        let mut smoke = Smoke::collect(&src, Methods::All);
        let overlay = smoke.overlay(&src);
        assert!(!overlay.contains_key(&PathBuf::from("/p/tests/api.test.bynk")));
        assert!(overlay.contains_key(&PathBuf::from("/p/src/shop/api.bynk")));
    }

    #[test]
    fn a_service_that_renders_rejections_accepts_its_own_400() {
        let src = sources(&[(
            "/p/src/a.bynk",
            r#"context a

type Note = { text: String }

service web from http {
  on POST("/n") (body: Note) -> Effect[HttpResult[Note]] by v: Visitor {
    Created(body)
  }
  on rejected (r: BoundaryRejection) -> Effect[HttpResult[()]] {
    BadRequest("no")
  }
}
"#,
        )]);
        let mut smoke = Smoke::collect(&src, Methods::All);
        let text = smoke.overlay(&src)[&PathBuf::from("/p/src/a.smoke.bynk")].clone();
        assert!(
            text.contains("expect r is Rejected(_) || r is Handled(BadRequest(_))"),
            "{text}"
        );
    }

    #[test]
    fn skippable_codes_match_by_prefix() {
        assert!(skippable("bynk.val.needs_pin"));
        assert!(skippable("bynk.test.remote_unsupported_call"));
        assert!(!skippable("bynk.types.type_mismatch"));
    }
}

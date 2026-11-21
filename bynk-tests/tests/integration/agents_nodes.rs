//! 1A.27 (#3728): agent construction and agent calls (`Agent(key).m(args)`, a let-bound
//! `x.m(args)`, a bare `Agent(key)`, and the self-agent WebSocket transfer) lower to nodes. Each
//! project is emitted in memory and the exact line is pinned. The construction builder's own
//! forms are unit tests in `lower_call/agents.rs`.
//!

fn fixture_dir(fixture: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("positive")
        .join(fixture)
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        if entry.file_name() == "expected" {
            continue;
        }
        if src.is_dir() {
            copy_dir(&src, &dst);
        } else {
            std::fs::copy(&src, &dst).unwrap();
        }
    }
}

/// `every emitted document joined` for a fixture project, built as the fixture suite
/// builds it. `rewrite` replaces (or adds) files of a temporary copy, so a suite's own unrelated
/// constructs can be taken out of the count.
fn emit(fixture: &str, rewrite: &[(&str, &str)]) -> String {
    let original = fixture_dir(fixture);
    let (dir, copy) = if rewrite.is_empty() {
        (original, None)
    } else {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp =
            std::env::temp_dir().join(format!("bynk_agents_{}_{n}_{fixture}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        copy_dir(&original, &tmp);
        for (path, text) in rewrite {
            std::fs::write(tmp.join(path), text).unwrap();
        }
        (tmp.clone(), Some(tmp))
    };
    let result = super::e2e::compile_fixture(&dir, super::e2e::fixture_target(&dir));
    if let Some(tmp) = copy {
        let _ = std::fs::remove_dir_all(tmp);
    }
    let out = match result {
        Ok(out) => out,
        Err(failure) => panic!(
            "{fixture} should compile, got:\n{}",
            bynk_driver::render::render_project_errors(&failure.flatten())
        ),
    };
    let text = out
        .artefacts
        .docs
        .values()
        .map(|d| d.text())
        .collect::<Vec<_>>()
        .join("\n");
    text
}

/// A project of one context `r` (`src`) and one suite `r` (`suite`), built on the smallest fixture;
/// `workers` selects the Workers target a `system` suite needs.
fn project(src: &str, suite: &str, workers: bool) -> String {
    let mut files = vec![("src/r.bynk", src), ("tests/r.bynk", suite)];
    if workers {
        files.push(("target.txt", "workers"));
    }
    emit("2020_test_actor_sum_principal", &files)
}

/// The lines of `text` equal to `line` once indentation is trimmed.
fn assert_line(text: &str, line: &str) {
    assert!(
        text.lines().any(|l| l.trim_start() == line),
        "missing line {line:?} in\n{text}"
    );
}

fn assert_contains(text: &str, part: &str) {
    assert!(text.contains(part), "missing {part:?} in\n{text}");
}

const SRC: &str = r#"context r

capability Meter { fn read() -> Effect[Int] }

provides Meter = FixedMeter {
	fn read() -> Effect[Int] { Effect.pure(7) }
}

agent Room {
	key name: String
	store hits: Cell[Int] = 0

	on call bump(v: Int) -> Effect[Int] {
		let n = hits + v
		hits := n
		Effect.pure(n)
	}

	on call metered() -> Effect[Int] given Meter {
		Meter.read()
	}

	on call chain(v: Int) -> Effect[Int] {
		let other = Room("other")
		other.bump(v)
	}
}

fn pick(s: String) -> Result[String, String] { Ok(s) }

capability Via { fn go(v: Int) -> Effect[Int] }

provides Via = RoomVia {
	fn go(v: Int) -> Effect[Int] { Room("p").bump(v) }
}

service api {
	on call(v: Int) -> Effect[Result[Int, String]] given Via {
		let a <- Room("a").bump(v)
		let k = "lit"
		let b <- Room(k).bump(a)
		let x = Room(k)
		let c <- x.bump(b)
		let x = Room("again")
		let d <- x.bump(c)
		let e <- Room(pick("z")?).bump(d)
		Effect.pure(Ok(e))
	}

}

service meter {
	on call() -> Effect[Int] given Meter {
		Room("m").metered()
	}
}

service meter_bound {
	on call() -> Effect[Int] given Meter {
		let r = Room("m")
		r.metered()
	}
}

service bare {
	on call() -> Effect[Result[Int, String]] {
		let r = Room(pick("y")?)
		let n <- r.bump(1)
		Effect.pure(Ok(n))
	}
}

type Slug = String where Matches("[a-z]{3}")

agent Tag {
	key slug: Slug
	store n: Cell[Int] = 0

	on call touch() -> Effect[Int] { Effect.pure(n) }
}

service tagged {
	on call() -> Effect[Int] {
		Tag("abc").touch()
	}
}
"#;

const SUITE: &str = r#"suite r

case "agent calls" {
	let n <- Room("a").bump(1)
	expect n == 1
	let r = Room("b")
	let m <- r.bump(2)
	expect m == 2
}
"#;

/// Every line of `text` that mentions `needle`, trimmed.
fn lines_with<'a>(text: &'a str, needle: &str) -> Vec<&'a str> {
    text.lines()
        .map(str::trim)
        .filter(|l| l.contains(needle))
        .collect()
}

/// The inline call, the let-bound call (and a re-`let` that renames the binding, #908) and a
/// bare construction are nodes in a service handler, an agent handler and a test case, on the
/// bundle target.
#[test]
fn agent_calls_are_nodes_on_the_bundle_target() {
    let text = project(SRC, SUITE, false);
    for line in [
        // A service handler.
        r#"const a = await __makeRoom("a").bump(v, deps);"#,
        "const b = await __makeRoom(k).bump(a, deps);",
        "const x = __makeRoom(k);",
        "const c = await x.bump(b, deps);",
        // #908: the re-`let` renames the JS binding, and the call follows it.
        r#"const __r0 = __makeRoom("again");"#,
        "const d = await __r0.bump(c, deps);",
        // An agent handler.
        r#"const other = __makeRoom("other");"#,
        "return other.bump(v, deps);",
        // A provider body: `this.deps` (#2355).
        r#"return __makeRoom("p").bump(v, this.deps);"#,
        // A test case, over the locally built `deps`.
        r#"const n = await __makeRoom("a").bump(1, deps);"#,
        r#"const r = __makeRoom("b");"#,
        "const m = await r.bump(2, deps);",
    ] {
        assert_line(&text, line);
    }
}

#[test]
fn agent_calls_thread_deps_env_on_the_workers_target() {
    let text = project(SRC, SUITE, true);
    for line in [
        r#"const a = await __makeRoom("a", deps.env).bump(v, deps);"#,
        "const b = await __makeRoom(k, deps.env).bump(a, deps);",
        "const x = __makeRoom(k, deps.env);",
        "const c = await x.bump(b, deps);",
        r#"const __r0 = __makeRoom("again", deps.env);"#,
        r#"const other = __makeRoom("other", deps.env);"#,
        // #2355: the provider body's `env` is `this.deps.env`, and its `deps` argument `this.deps`.
        r#"return __makeRoom("p", this.deps.env).bump(v, this.deps);"#,
    ] {
        assert_line(&text, line);
    }
}

/// A key that hoists (`Room(pick("z")?)`) is lowered first: its hoisted statements precede the
/// call, and the call reads the hoisted value.
#[test]
fn a_hoisting_key_is_lowered_before_the_call() {
    for (workers, call) in [
        (
            false,
            "const e = await __makeRoom(__r1.value).bump(d, deps);",
        ),
        (
            true,
            "const e = await __makeRoom(__r1.value, deps.env).bump(d, deps);",
        ),
    ] {
        let text = project(SRC, SUITE, workers);
        let hoist = text
            .find(r#"const __r1 = pick("z");"#)
            .expect("the key is hoisted");
        let early_return = text[hoist..]
            .find("if (__r1.tag === \"Err\") return __r1;")
            .expect("the `?` returns early");
        let at = text.find(call).expect("the call reads the hoisted key");
        assert!(hoist + early_return < at, "the key hoists first:\n{text}");
    }
}

/// #527: the callee's `given` capabilities widen the caller's deps shape, on the inline path and
/// the let-bound path.
#[test]
fn the_callees_given_capabilities_fold_into_the_caller_on_both_paths() {
    let text = project(SRC, SUITE, false);
    for service in ["meter", "meter_bound"] {
        let header = format!("export const {service} = {{");
        let at = text.find(&header).expect("the service is emitted");
        let sig = text[at..].lines().nth(1).expect("a handler follows");
        assert!(sig.contains("deps: { Meter: Meter }"), "{service}: {sig}");
    }
    assert_line(&text, r#"return __makeRoom("m").metered(deps);"#);
    assert_line(&text, "return r.metered(deps);");
}

/// Without the refined key, a bare construction takes a hoisted call key too.
#[test]
fn the_agent_constructs_print_as_nodes() {
    let src = SRC.split("type Slug").next().unwrap();
    let text = project(src, SUITE, false);
    assert_line(&text, "const __r0 = pick(\"y\");");
    assert_line(&text, "const r = __makeRoom(__r0.value);");
}

/// A refined literal's brand cast is a node (`unchecked_construct`), so the call keyed by it is
/// a node; the line prints byte for byte as the text call did.
#[test]
fn a_refined_key_is_a_node_and_prints_byte_for_byte() {
    let text = project(SRC, SUITE, false);
    let found = lines_with(&text, "__makeTag(");
    assert!(
        found
            .iter()
            .any(|l| l.starts_with("return __makeTag(") && l.ends_with(".touch(deps);")),
        "{found:?}"
    );
}

/// A traced build wraps the agent call in the trace step; a deploy build does not. The call
/// inside is the same text either way.
#[test]
fn a_traced_agent_call_wraps_the_node_in_the_trace_step() {
    let dir = std::env::temp_dir().join(format!("bynk_agents_trace_{}", std::process::id()));
    let src = dir.join("src");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("r.bynk"), SRC).unwrap();
    let build = |traced: bool| {
        let options = bynk_testkit::compile_options_single(src.clone())
            .trace_requests(traced)
            .target(bynk_driver::project::BuildTarget::Workers)
            .tests(false);
        let out = bynk_driver::project::compile_project(&options)
            .map_err(bynk_driver::project::ProjectFailure::flatten)
            .unwrap_or_else(|e| panic!("compile failed: {e:?}"));
        out.artefacts
            .docs
            .values()
            .map(|d| d.text())
            .collect::<Vec<_>>()
            .join("\n")
    };
    let plain = build(false);
    let traced = build(true);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!plain.contains("__traceStep(\"agent\""), "{plain}");
    assert_contains(
        &traced,
        r#"const a = await __traceStep("agent", "Room.bump", { file: "r.bynk", line: 39, col: 12 })(__makeRoom("a", deps.env).bump(v, deps));"#,
    );
    assert_contains(
        &traced,
        r#"const c = await __traceStep("agent", "Room.bump", { file: "r.bynk", line: 43, col: 12 })(x.bump(b, deps));"#,
    );
    // A text key keeps the string wrapper, with the same call inside.
    assert_contains(
        &traced,
        r#"__traceStep("agent", "Tag.touch", { file: "r.bynk", line: 84, col: 3 })(__makeTag(("abc" as Slug), deps.env).touch(deps));"#,
    );
}

/// The self-agent WebSocket transfer is `this.<method>(args, deps)`: the key is not lowered, so
/// `__makeRoom` does not appear in the open handler and no key hoist does either.
#[test]
fn the_self_agent_transfer_does_not_lower_its_key() {
    let text = emit("237_websocket_chatroom_workers", &[]);
    assert_line(
        &text,
        "const __r1 = await this.join(deps.identity, connection, deps);",
    );
    for l in lines_with(&text, "this.join(") {
        assert!(!l.contains("__makeRoom"), "{l}");
    }
}

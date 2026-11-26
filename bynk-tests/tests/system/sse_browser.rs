//! #3918: a browser's own `EventSource` against a compiled `Streaming` handler.
//!
//! Compiles a small service, serves its Workers entry from Node, and drives Playwright's
//! Chromium at it. Proves what only a real browser can: that it dispatches an event under the
//! `event:` name the handler wrote, that a dropped connection reconnects with the last `id` as
//! `Last-Event-ID` (which the handler reads through a typed `headers` record), that a
//! multi-line `data` arrives as one event, that an `id` with a line break faults the body
//! in-band (the browser reports an error and never delivers the event), and that a plain
//! `Stream[String]` is unchanged.
//!
//! Skips loudly when there is no `tsc`, `node` or Playwright, and `BYNK_REQUIRE_BROWSER=1`
//! (set in CI) turns the skip into a failure. A heartbeat's comment line is invisible to the
//! `EventSource` API, so it is proved at the wire in the runtime's own tests.

use std::fs;
use std::path::Path;
use std::process::Command;

const REQUIRE_ENV: &str = "BYNK_REQUIRE_BROWSER";

const FEED: &str = r#"context feed

consumes bynk { SseEvent }

type FeedHeaders = { @name("last-event-id") lastId: Option[String] }

service Feed from http {
	on GET("/feed") (headers: FeedHeaders) -> Effect[HttpResult[()]] by v: Visitor {
		match headers.lastId {
			None => Streaming(Stream.of([
				SseEvent { data: "one", event: Some("tick"), id: Some("1"), retry: Some(50.milliseconds) },
				SseEvent { data: "two\nlines", event: Some("tick"), id: Some("2") },
			]))
			Some(last) => Streaming(Stream.of([
				SseEvent { data: "resumed after \(last)", event: Some("tick"), id: Some("3") },
			]))
		}
	}

	on GET("/plain") () -> Effect[HttpResult[()]] by v: Visitor {
		Streaming(Stream.of(["a", "b"]))
	}

	on GET("/bad") () -> Effect[HttpResult[()]] by v: Visitor {
		Streaming(Stream.of([
			SseEvent { data: "ok", id: Some("9"), retry: Some(60000.milliseconds) },
			SseEvent { data: "never", id: Some("x\ny") },
		]))
	}
}
"#;

const TSCONFIG_JSON: &str = r#"{
  "compilerOptions": {
    "module": "Node16",
    "moduleResolution": "node16",
    "target": "ES2022",
    "strict": true,
    "skipLibCheck": true,
    "outDir": "js",
    "rootDir": ".",
    "lib": ["ES2022", "DOM"]
  },
  "include": ["**/*.ts"],
  "exclude": ["js"]
}
"#;

/// Serves the built worker over HTTP, then reads it with Chromium's `EventSource` and prints
/// one JSON line of what the page saw.
const DRIVER_MJS: &str = r#"
import http from "node:http";
import { createRequire } from "node:module";
import worker from "./js/workers/feed/index.js";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");

const page = `<!doctype html><meta charset="utf-8"><title>sse</title>`;
// The longest any one wait below may take. Generous on purpose (#4479): the browser, the
// page and each stream share a machine that may be loaded far past its core count, and a
// wait that expires says which one it was, so a real hang still fails by name.
const WAIT_MS = 90000;

// The page tells the server (`/ack`) once it has been handed the event that comes before a
// fault. The server cuts the connection only then: a connection cut straight after the event
// was flushed can be reported to the page as an error before the event itself is delivered,
// which is the browser's ordering and not what the smoke is about (#4479).
let acknowledge;
const acknowledged = new Promise((resolve) => { acknowledge = resolve; });

const server = http.createServer(async (req, res) => {
  if (req.url === "/") {
    res.writeHead(200, { "content-type": "text/html" }).end(page);
    return;
  }
  if (req.url === "/ack") {
    acknowledge();
    res.writeHead(204).end();
    return;
  }
  const origin = `http://${req.headers.host}`;
  const response = await worker.fetch(new Request(origin + req.url, { method: req.method, headers: req.headers }), {});
  res.writeHead(response.status, Object.fromEntries(response.headers));
  const reader = response.body?.getReader();
  req.on("close", () => reader?.cancel().catch(() => {}));
  try {
    for (;;) {
      const { value, done } = reader ? await reader.read() : { done: true };
      if (done) break;
      // Flushed before the next read, so a fault in the next element cannot cut off this one.
      await new Promise((flushed) => res.write(value, flushed));
      // The encoder writes one line per chunk, so an event is complete on its blank line.
      if (req.url === "/bad" && new TextDecoder().decode(value) === "\n") {
        let timer;
        await Promise.race([acknowledged, new Promise((late) => { timer = setTimeout(late, WAIT_MS); })]);
        clearTimeout(timer);
      }
    }
    res.end();
  } catch {
    // The body failed after its status was sent: the connection is cut, as a real server's would be.
    res.destroy();
  }
});
await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const base = `http://127.0.0.1:${server.address().port}`;

const browser = await chromium.launch({ timeout: WAIT_MS });
try {
  const context = await browser.newContext();
  const tab = await context.newPage();
  await tab.goto(base + "/", { timeout: WAIT_MS });
  const seen = await tab.evaluate(async (waitMs) => {
    const within = (what, p) => Promise.race([p, new Promise((_, no) => setTimeout(() => no(new Error(`timed out after ${waitMs} ms waiting for ${what}`)), waitMs))]);
    const out = {};

    // Named events, a multi-line `data`, and a reconnect that carries `Last-Event-ID`.
    out.feed = await within("the three named `tick` events of /feed", new Promise((resolve) => {
      const got = [];
      const es = new EventSource("/feed");
      es.addEventListener("tick", (e) => {
        got.push({ type: e.type, data: e.data, id: e.lastEventId });
        if (got.length === 3) { es.close(); resolve(got); }
      });
    }));

    // A plain `Stream[String]` is unnamed events with no id.
    out.plain = await within("the two unnamed events of /plain", new Promise((resolve) => {
      const got = [];
      const es = new EventSource("/plain");
      es.onmessage = (e) => {
        got.push({ type: e.type, data: e.data, id: e.lastEventId });
        if (got.length === 2) { es.close(); resolve(got); }
      };
    }));

    // An `id` with a line break faults the body in-band: the event before it arrives, the
    // faulted one never does, and the browser reports an error.
    out.bad = await within("the error after the faulted body of /bad", new Promise((resolve) => {
      const got = [];
      const es = new EventSource("/bad");
      es.onmessage = (e) => { got.push({ data: e.data, id: e.lastEventId }); fetch("/ack"); };
      es.onerror = () => { es.close(); resolve(got); };
    }));
    return out;
  }, WAIT_MS);
  console.log("RESULT " + JSON.stringify(seen));
} finally {
  await browser.close();
  server.close();
}
process.exit(0);
"#;

fn tool_exists(name: &str) -> bool {
    let finder = if cfg!(windows) { "where" } else { "which" };
    Command::new(finder)
        .arg(name)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn skip(why: &str) {
    eprintln!("\n!!! SSE BROWSER VERIFICATION SKIPPED !!!\n{why}\n");
    if std::env::var(REQUIRE_ENV).is_ok() {
        panic!("{REQUIRE_ENV} is set but {why}");
    }
}

fn run(program: &str, args: &[&str], cwd: &Path) -> (bool, String) {
    let output = match Command::new(program).args(args).current_dir(cwd).output() {
        Ok(o) => o,
        Err(e) => return (false, format!("could not launch {program}: {e}")),
    };
    let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(&output.stderr));
    (output.status.success(), combined)
}

#[test]
fn a_browser_event_source_reads_a_streaming_handler() {
    if !tool_exists("node") {
        return skip("`node` is not on PATH");
    }
    let tsc: (&str, Vec<&str>) = if tool_exists("tsc") {
        ("tsc", vec![])
    } else {
        return skip("no `tsc` on PATH");
    };
    // Playwright resolves from NODE_PATH or a local install; if it does not, there is no browser.
    let (has_playwright, _) = run(
        "node",
        &["-e", "require('playwright').chromium"],
        Path::new("."),
    );
    if !has_playwright {
        return skip("Playwright is not installed (set NODE_PATH to its node_modules)");
    }

    let tmp = std::env::temp_dir().join(format!("bynk-sse-browser-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    let src = tmp.join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("feed.bynk"), FEED).unwrap();
    let out = bynk_driver::project::compile_project(
        &bynk_testkit::compile_options_single(&src)
            .target(bynk_driver::project::BuildTarget::Workers),
    )
    .map_err(bynk_driver::project::ProjectFailure::flatten)
    .expect("the SSE browser fixture must compile");
    let built = tmp.join("built");
    for (path, doc) in &out.artefacts.docs {
        if path.to_string_lossy() == "tsconfig.json" {
            continue;
        }
        let target = built.join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, doc.text()).unwrap();
    }
    fs::write(built.join("tsconfig.json"), TSCONFIG_JSON).unwrap();
    fs::write(built.join("driver.mjs"), DRIVER_MJS).unwrap();
    fs::write(built.join("package.json"), r#"{"type":"module"}"#).unwrap();

    let mut args = tsc.1.clone();
    args.extend(["--project", "tsconfig.json"]);
    let (ok, log) = run(tsc.0, &args, &built);
    assert!(ok, "the built worker must compile:\n{log}");
    let (ran, log) = run("node", &["driver.mjs"], &built);
    assert!(ran, "the browser driver must run:\n{log}");
    let line = log
        .lines()
        .find_map(|l| l.strip_prefix("RESULT "))
        .unwrap_or_else(|| panic!("the driver printed no result:\n{log}"));
    let seen: serde_json::Value = serde_json::from_str(line).unwrap();

    // Named events, a multi-line `data`, then the reconnect: the handler read `Last-Event-ID: 2`.
    assert_eq!(
        seen["feed"],
        serde_json::json!([
            {"type": "tick", "data": "one", "id": "1"},
            {"type": "tick", "data": "two\nlines", "id": "2"},
            {"type": "tick", "data": "resumed after 2", "id": "3"},
        ]),
        "{log}"
    );
    // A plain stream is unchanged: unnamed events with no id.
    assert_eq!(
        seen["plain"],
        serde_json::json!([
            {"type": "message", "data": "a", "id": ""},
            {"type": "message", "data": "b", "id": ""},
        ]),
        "{log}"
    );
    // An `id` with a line break faults in-band: the event before it arrived, the bad one did not.
    assert_eq!(
        seen["bad"],
        serde_json::json!([{"data": "ok", "id": "9"}]),
        "{log}"
    );

    let _ = fs::remove_dir_all(&tmp);
}

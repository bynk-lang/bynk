//! #1825: a Workers entry point logs a fault before it answers `500`.
//!
//! Compiles a context whose HTTP route calls an adapter that throws for one
//! input, strips it to JavaScript, and drives the Worker's `fetch` under Node.
//! The goldens pin the emitted text; this pins what it does:
//! - the faulting request answers the same bare `500` body;
//! - one line is logged, naming the context and the route's *pattern*, with
//!   the error;
//! - the request's path value (`boom`) never reaches the log;
//! - a request that doesn't fault logs nothing.
//!
//! Skips loudly without `node`; `BYNK_REQUIRE_TSC=1` turns the skip into a
//! failure, as the other Node-driven suites do.

use std::fs;
use std::process::{Command, Stdio};

const REQUIRE_ENV: &str = "BYNK_REQUIRE_TSC";

fn tool_exists(name: &str) -> bool {
    let finder = if cfg!(windows) { "where" } else { "which" };
    Command::new(finder)
        .arg(name)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

const ADAPTER: &str = "adapter text.flaky

binding \"./flaky.binding.ts\"
exports capability { Flaky }

capability Flaky {
\tfn run(input: String) -> Effect[String]
}

provides Flaky = ThrowingFlaky
";

const BINDING: &str = "import type { Flaky } from \"./flaky.js\";

export class ThrowingFlaky implements Flaky {
  async run(input: string): Promise<string> {
    if (input === \"boom\") throw new Error(\"provider exploded\");
    return input.toUpperCase();
  }
}
";

const CONTEXT: &str = "context shop.api

consumes text.flaky { Flaky }

service api from http {
\ton GET(\"/shout/:word\") (
\t\tword: String
\t) -> Effect[HttpResult[String]] by Visitor given Flaky {
\t\tlet loud <- Flaky.run(word)
\t\tOk(loud)
\t}
}
";

/// Drives `/shout/hello` then `/shout/boom`, printing each status and body
/// between markers so the test can tell them from the logged lines.
const DRIVER: &str = "import worker from \"./out/workers/shop-api/index.js\";
for (const p of [\"/shout/hello\", \"/shout/boom\"]) {
  const res = await worker.fetch(new Request(\"http://shop.example\" + p), {});
  console.log(`RESULT ${p} ${res.status} ${await res.text()}`);
}
";

#[test]
fn a_faulting_route_is_logged_by_pattern_and_answers_a_bare_500() {
    if !tool_exists("node") {
        eprintln!("\n!!! WORKERS FAULT LOG SKIPPED !!!\n`node` is not on PATH.\n");
        if std::env::var(REQUIRE_ENV).is_ok_and(|v| !v.is_empty()) {
            panic!("{REQUIRE_ENV} is set but `node` was not found");
        }
        return;
    }
    let tmp = std::env::temp_dir().join(format!("bynk-fault-log-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(tmp.join("src/text")).unwrap();
    fs::create_dir_all(tmp.join("src/shop")).unwrap();
    fs::write(
        tmp.join("bynk.toml"),
        "[project]\nname = \"fault-log\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(tmp.join("src/text/flaky.bynk"), ADAPTER).unwrap();
    fs::write(tmp.join("src/text/flaky.binding.ts"), BINDING).unwrap();
    fs::write(tmp.join("src/shop/api.bynk"), CONTEXT).unwrap();
    fs::write(tmp.join("driver.mjs"), DRIVER).unwrap();

    let compiled = Command::new(env!("CARGO_BIN_EXE_bynkc-under-test"))
        .args(["compile", "--target", "workers", "--emit", "js", "-o"])
        .arg(tmp.join("out"))
        .arg(&tmp)
        .output()
        .expect("run bynkc");
    assert!(
        compiled.status.success(),
        "bynkc compile failed:\n{}",
        String::from_utf8_lossy(&compiled.stderr)
    );

    let run = Command::new("node")
        .arg("driver.mjs")
        .current_dir(&tmp)
        .output()
        .expect("run node");
    let stdout = String::from_utf8_lossy(&run.stdout);
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success(), "driver failed:\n{stdout}\n{stderr}");

    assert!(
        stdout.contains("RESULT /shout/hello 200"),
        "the healthy route must still answer:\n{stdout}"
    );
    assert!(
        stdout.contains("RESULT /shout/boom 500 Internal Server Error"),
        "a fault answers the same bare 500:\n{stdout}"
    );
    let logged: Vec<&str> = stderr.lines().filter(|l| l.contains("faulted")).collect();
    assert_eq!(
        logged.len(),
        1,
        "exactly the faulting request is logged:\n{stderr}"
    );
    assert!(
        logged[0].starts_with("shop.api GET /shout/:word faulted"),
        "the line names the context and the route pattern:\n{stderr}"
    );
    assert!(
        stderr.contains("provider exploded"),
        "the error itself is logged:\n{stderr}"
    );
    assert!(
        !stderr.contains("boom"),
        "the request's path value must not reach the log:\n{stderr}"
    );
    let _ = fs::remove_dir_all(&tmp);
}

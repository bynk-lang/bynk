//! Crate-hygiene B2 (#2999): a byte-for-byte snapshot of the `system` suites
//! `bynk test --smoke` synthesises, and of what it skips, taken *before* the
//! smoke runner moves onto the system model (H-1: output stays
//! byte-identical).
//!
//! One project covers every HTTP method, a public route and an actor's, a
//! route with a JSON body and one without, `on rejected`, two `from http`
//! services in one context (whose cases name their service), a non-HTTP
//! service beside them (left out), and a second context. It is rendered under
//! both method policies: every route, and the `GET`-only one a run against a
//! named target uses.
//!
//! The expected text lives in `tests/snapshots/smoke_suites.txt`. Set
//! `BYNK_BLESS=1` to rewrite it after an *intended* change.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use bynk_driver::smoke::{Methods, Smoke};

const SHOP: &str = r#"context shop.api

type UserId = String where NonEmpty
actor User { auth = Bearer(secret = "AUTH_SECRET"), identity = UserId }
type Item = { sku: String where NonEmpty }

service api from http {
  on GET("/ping") () -> Effect[HttpResult[String]] by v: Visitor {
    Ok("pong")
  }

  on POST("/items") (body: Item) -> Effect[HttpResult[Item]] by u: User {
    Created(body)
  }

  on PUT("/cart/:sku") (sku: String, body: Item) -> Effect[HttpResult[Item]] by u: User {
    Created(body)
  }

  on PATCH("/cart/:sku") (sku: String) -> Effect[HttpResult[String]] by u: User {
    Ok(sku)
  }

  on DELETE("/cart/:sku") (sku: String) -> Effect[HttpResult[()]] by u: User {
    NoContent
  }

  on rejected(why: BoundaryRejection) -> Effect[HttpResult[()]] {
    RawError(BadRequest, Bytes.fromUtf8("bad"), "text/plain")
  }
}

service admin from http {
  on GET("/stats") () -> Effect[HttpResult[String]] by u: User {
    Ok("s")
  }
}

service ticker from cron {
  on schedule("*/5 * * * *") () -> Effect[Result[(), String]] {
    Ok(())
  }
}

service calc {
  on call(x: Int) -> Effect[Int] {
    x
  }
}
"#;

const STATUS: &str = r#"context status.web

service web from http {
  on GET("/health") () -> Effect[HttpResult[String]] by v: Visitor {
    Ok("ok")
  }
}
"#;

fn sources() -> HashMap<PathBuf, String> {
    [
        ("/p/src/shop/api.bynk", SHOP),
        ("/p/src/status/web.bynk", STATUS),
    ]
    .iter()
    .map(|(k, v)| (PathBuf::from(k), v.to_string()))
    .collect()
}

fn render() -> String {
    let src = sources();
    let mut out = String::new();
    for (label, methods) in [("all", Methods::All), ("safe only", Methods::SafeOnly)] {
        let mut smoke = Smoke::collect(&src, methods);
        let (routes, cases, skipped) = smoke.summary();
        let skipped = skipped.len();
        writeln!(
            out,
            "=== {label}: {routes} routes, {cases} cases, {skipped} skipped"
        )
        .unwrap();
        out.push_str(&smoke.print(Path::new("/p")));
    }
    out
}

/// The first line where `actual` and `expected` differ, with a little context,
/// so a drift is readable in a CI log without the whole snapshot.
fn first_difference(actual: &str, expected: &str) -> String {
    let (a, e): (Vec<&str>, Vec<&str>) = (actual.lines().collect(), expected.lines().collect());
    let at = (0..a.len().max(e.len()))
        .find(|&i| a.get(i) != e.get(i))
        .unwrap_or(0);
    let mut out = format!("first difference at line {}:\n", at + 1);
    for i in at.saturating_sub(3)..(at + 4) {
        let (x, y) = (a.get(i).copied(), e.get(i).copied());
        if x == y {
            if let Some(l) = x {
                out.push_str(&format!("    {l}\n"));
            }
        } else {
            if let Some(l) = y {
                out.push_str(&format!("  - {l}\n"));
            }
            if let Some(l) = x {
                out.push_str(&format!("  + {l}\n"));
            }
        }
    }
    out
}

#[test]
fn synthesised_smoke_suites_are_pinned() {
    let actual = render();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots/smoke_suites.txt");
    if std::env::var_os("BYNK_BLESS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {}: {e} (bless with BYNK_BLESS=1)", path.display()));
    assert!(
        actual == expected,
        "the synthesised smoke suites drifted from {} (H-1). Re-bless only for an intended \
         change.\n{}",
        path.display(),
        first_difference(&actual, &expected)
    );
}

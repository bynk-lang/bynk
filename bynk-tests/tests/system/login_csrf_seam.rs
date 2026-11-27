//! #3825: the edges of the login-CSRF check that the `system` tier's driver cannot send, which is where
//! the rule differs from the issue's wording. This compiles the `3825_issue_csrf_headers` Workers fixture and
//! drives the emitted Worker's `fetch` under Node with each combination of `Sec-Fetch-Site` and `Origin`:
//!
//! * a request with **neither** header is refused (`403`), where the issue's Decision A would have let it
//!   pass: the guarded-route rule refuses it, and a weaker rule on a login route is the wrong direction;
//! * a non-browser client that sends an `Origin` listed in `cors`, with no `Sec-Fetch-Site`, is admitted,
//!   which is the migration path for a native app or a server;
//! * the request's own origin, with no `Sec-Fetch-Site`, is admitted, and any other origin is refused;
//! * `Sec-Fetch-Site: same-origin` alone is admitted, `cross-site`, `same-site` and `none` are refused
//!   unless a listed `Origin` accompanies the first two, and a `null` or wildcard `Origin` never is.
//!
//! Like the other tsc-driven tests it skips loudly without a toolchain; `BYNK_REQUIRE_TSC=1` turns the
//! skip into a failure.

use super::bearer_claims_seam::drive;

const DRIVER_TS: &str = r#"
import worker from "./workers/shop-api/index.js";

function assert(cond: boolean, msg: string): void {
  if (!cond) {
    throw new Error(`assertion failed: ${msg}`);
  }
}

// 32 bytes, base64url: the signing key `Member` reads.
const env: Record<string, unknown> = { SESSION_KEY: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA" };

async function login(headers: Record<string, string>): Promise<{ status: number; cookie: string | null }> {
  const res = await worker.fetch(
    new Request("https://api.shop.example/login", {
      method: "POST",
      headers: { "content-type": "application/json", ...headers },
      body: JSON.stringify({ user: "ada" }),
    }),
    env,
  );
  return { status: res.status, cookie: res.headers.get("set-cookie") };
}

const LISTED = "https://app.example.com";
const cases: [Record<string, string>, number, string][] = [
  // Neither header: refused, however good the request otherwise is.
  [{}, 403, "no Origin and no Sec-Fetch-Site is refused"],
  // A non-browser client's migration path: a listed Origin and no Sec-Fetch-Site.
  [{ origin: LISTED }, 201, "a listed Origin with no Sec-Fetch-Site is admitted"],
  [{ origin: "https://evil.example" }, 403, "an unlisted Origin with no Sec-Fetch-Site is refused"],
  [{ origin: "null" }, 403, "a null Origin is refused"],
  [{ origin: "*" }, 403, "a wildcard Origin is refused"],
  [{ origin: "https://api.shop.example" }, 201, "the request's own origin with no Sec-Fetch-Site is admitted"],
  [{ origin: LISTED.toUpperCase() }, 403, "a differently cased Origin is not the listed one"],
  [{ origin: LISTED + "/" }, 403, "an Origin with a trailing slash is not the listed one"],
  [{ origin: "http://app.example.com" }, 403, "another scheme is not the listed Origin"],
  // Fetch metadata.
  [{ "sec-fetch-site": "same-origin" }, 201, "same-origin alone is admitted"],
  [{ "sec-fetch-site": "same-origin", origin: "https://evil.example" }, 201, "same-origin passes whatever the Origin"],
  [{ "sec-fetch-site": "cross-site" }, 403, "cross-site with no Origin is refused"],
  [{ "sec-fetch-site": "cross-site", origin: "https://evil.example" }, 403, "cross-site from an unlisted Origin is refused"],
  [{ "sec-fetch-site": "cross-site", origin: LISTED }, 201, "cross-site from a listed Origin is admitted"],
  [{ "sec-fetch-site": "same-site", origin: "https://evil.example" }, 403, "same-site from an unlisted Origin is refused"],
  [{ "sec-fetch-site": "same-site", origin: LISTED }, 201, "same-site from a listed Origin is admitted"],
  [{ "sec-fetch-site": "none", origin: LISTED }, 403, "a user-initiated navigation is not a sign-in, whatever its Origin"],
  [{ "sec-fetch-site": "unheard-of" }, 403, "an unknown Sec-Fetch-Site value with no Origin is refused"],
];
for (const [headers, want, why] of cases) {
  const got = await login(headers);
  assert(got.status === want, `${why} (got ${got.status}, want ${want})`);
  // Only an admitted sign-in sets the session cookie.
  assert((got.cookie !== null) === (want === 201), `${why}: the cookie is set exactly when the body ran`);
}

console.log("ALL OK");
"#;

#[test]
fn a_sign_in_is_admitted_or_refused_by_its_headers_including_none_at_all() {
    drive("3825_issue_csrf_headers", DRIVER_TS);
}

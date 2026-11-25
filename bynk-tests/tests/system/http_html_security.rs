//! #3923: behavioural proof that an HTML response gets framing and referrer protection
//! and that `security { }` can name the framing sources, the referrer policy and a whole
//! `csp`. The fixture is compiled for Workers and driven through the emitted `fetch`
//! in-process under Node, like the other `http_*_behaviour` tests; it skips loudly
//! without a TypeScript toolchain (`BYNK_REQUIRE_TSC=1` makes the skip a failure).

const DRIVER_TS: &str = r#"
import worker from "./workers/site/index.js";

function assert(cond: boolean, msg: string): void {
  if (!cond) {
    throw new Error(`assertion failed: ${msg}`);
  }
}

const env = {} as never;
const DEFAULT_REFERRER = "strict-origin-when-cross-origin";

async function get(path: string): Promise<Response> {
  return worker.fetch(new Request(`https://x.test${path}`), env);
}
const header = (r: Response, name: string): string | null => r.headers.get(name);

async function main(): Promise<void> {
  // The defaults: an HTML response is not framable and sends a referrer policy.
  for (const path of ["/", "/utf8"]) {
    const r = await get(path);
    assert(r.status === 200, `GET ${path} is 200 (got ${r.status})`);
    assert(header(r, "content-security-policy") === "frame-ancestors 'none'", `${path} CSP (got ${header(r, "content-security-policy")})`);
    assert(header(r, "referrer-policy") === DEFAULT_REFERRER, `${path} referrer policy (got ${header(r, "referrer-policy")})`);
    assert(header(r, "x-content-type-options") === "nosniff", `${path} keeps nosniff`);
  }

  // An HTML error page is protected too.
  let r = await get("/missing");
  assert(r.status === 404, `GET /missing is 404 (got ${r.status})`);
  assert(header(r, "content-security-policy") === "frame-ancestors 'none'", "an HTML error page is not framable");
  assert(header(r, "referrer-policy") === DEFAULT_REFERRER, "an HTML error page has a referrer policy");

  // A JSON response is unchanged.
  r = await get("/json");
  assert(r.status === 200, `GET /json is 200 (got ${r.status})`);
  assert(header(r, "content-security-policy") === null, `JSON has no CSP (got ${header(r, "content-security-policy")})`);
  assert(header(r, "referrer-policy") === null, "JSON has no referrer policy");

  // Named framing sources and referrer policy.
  r = await get("/embed");
  assert(
    header(r, "content-security-policy") === "frame-ancestors 'self' https://partner.example",
    `frameAncestors lists its sources (got ${header(r, "content-security-policy")})`,
  );
  assert(header(r, "referrer-policy") === "no-referrer", `referrerPolicy: noReferrer (got ${header(r, "referrer-policy")})`);

  // A csp without frame-ancestors gets the framing default appended.
  r = await get("/policy");
  assert(
    header(r, "content-security-policy") === "default-src 'self'; img-src 'self' https://cdn.example; frame-ancestors 'none'",
    `csp gets frame-ancestors appended (got ${header(r, "content-security-policy")})`,
  );
  assert(header(r, "referrer-policy") === DEFAULT_REFERRER, "a csp does not change the referrer policy");

  // A csp with its own frame-ancestors is sent verbatim.
  r = await get("/own");
  assert(
    header(r, "content-security-policy") === "default-src 'self'; frame-ancestors https://own.example",
    `csp with its own frame-ancestors is verbatim (got ${header(r, "content-security-policy")})`,
  );

  // nosniff: false does not turn the HTML headers off; ["*"] with a csp opens framing.
  r = await get("/open");
  assert(header(r, "x-content-type-options") === null, "nosniff: false drops nosniff");
  assert(
    header(r, "content-security-policy") === "default-src 'self'; frame-ancestors *",
    `frameAncestors ["*"] with a csp (got ${header(r, "content-security-policy")})`,
  );
  assert(header(r, "referrer-policy") === DEFAULT_REFERRER, "each field is independent");

  console.log("ALL OK");
}

main().catch((e: unknown) => {
  console.error(e);
  throw e;
});
"#;

#[test]
fn html_security_headers_run_on_workers_entry() {
    super::http_limits_behaviour::run_driver("3923_http_html_security", DRIVER_TS);
}

//! v0.151: behavioral auth-bypass tests for the emitted OIDC/JWKS verifier.
//!
//! `verifyOidcJwt` is an authentication boundary the compiler generates for an
//! `actor … { auth = Oidc(…) }` route. Its correctness was established by a
//! one-time `/security-review`; this is the *standing* regression guard — it
//! imports the emitted runtime, stands up an in-process JWKS (by overriding
//! `globalThis.fetch`), signs RS256/ES256 tokens with WebCrypto, and asserts
//! the verdict for every bypass class (a refactor that reopens one fails here).
//!
//! Like the sibling `bearer_auth` test it drives `tsc` + `node`; it skips
//! loudly without a toolchain, and `BYNK_REQUIRE_TSC=1` turns the skip into a
//! failure.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

const REQUIRE_ENV: &str = "BYNK_REQUIRE_TSC";

fn base_command(program: &str) -> Command {
    if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(program);
        c
    } else {
        Command::new(program)
    }
}

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

fn discover_tsc() -> Option<(String, Vec<String>)> {
    if tool_exists("tsc") {
        return Some(("tsc".to_string(), vec![]));
    }
    if tool_exists("npx") {
        return Some((
            "npx".to_string(),
            vec![
                "--yes".to_string(),
                "-p".to_string(),
                format!("typescript@{}", bynk_emit::TYPESCRIPT_MAJOR_TESTED),
                "tsc".to_string(),
            ],
        ));
    }
    None
}

fn run(program: &str, prefix: &[String], args: &[&str], cwd: &Path) -> (bool, String) {
    let mut cmd = base_command(program);
    for p in prefix {
        cmd.arg(p);
    }
    for a in args {
        cmd.arg(a);
    }
    cmd.current_dir(cwd);
    let output = match cmd.output() {
        Ok(o) => o,
        Err(e) => return (false, format!("could not launch {program}: {e}")),
    };
    let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(&output.stderr));
    (output.status.success(), combined)
}

const DRIVER_TS: &str = r#"import { verifyOidcJwt, oidcTrust, tokenClaims } from "./__bynk_runtime/auth.js";

function assert(cond: boolean, msg: string): void {
  if (!cond) throw new Error("FAIL: " + msg);
}

const ISS = "https://issuer.test";
const AUD = "my-api";
const JWKS = "https://issuer.test/jwks.json";
const enc = new TextEncoder();

function b64url(s: string): string {
  return btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}
function bytesB64url(bytes: Uint8Array): string {
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

async function rsaKeypair(kid: string): Promise<{ priv: CryptoKey; jwk: JsonWebKey }> {
  const kp = (await crypto.subtle.generateKey(
    { name: "RSASSA-PKCS1-v1_5", modulusLength: 2048, publicExponent: new Uint8Array([1, 0, 1]), hash: "SHA-256" },
    true,
    ["sign", "verify"],
  )) as CryptoKeyPair;
  const jwk = await crypto.subtle.exportKey("jwk", kp.publicKey);
  return { priv: kp.privateKey, jwk: { ...jwk, kid, alg: "RS256", use: "sig" } as JsonWebKey };
}

async function signRs256(payload: Record<string, unknown>, priv: CryptoKey, header: Record<string, unknown>): Promise<string> {
  const h = b64url(JSON.stringify(header));
  const p = b64url(JSON.stringify(payload));
  const sig = await crypto.subtle.sign("RSASSA-PKCS1-v1_5", priv, enc.encode(`${h}.${p}`) as BufferSource);
  return `${h}.${p}.${bytesB64url(new Uint8Array(sig))}`;
}

const now = Math.floor(Date.now() / 1000);
const main = async () => {
  const signer = await rsaKeypair("k1");
  const attacker = await rsaKeypair("k1"); // same kid, different key — not published

  // Stand up an in-process JWKS at JWKS; every other URL 404s.
  globalThis.fetch = (async (input: RequestInfo | URL): Promise<Response> => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.href : (input as Request).url;
    if (url === JWKS || url === "https://fake.test/jwks") {
      return new Response(JSON.stringify({ keys: [signer.jwk] }), { status: 200 });
    }
    return new Response("not found", { status: 404 });
  }) as typeof fetch;

  // --- accept path: a correctly signed, unexpired, in-audience token mints sub ---
  const good = await signRs256({ sub: "user-1", iss: ISS, aud: AUD, exp: now + 3600 }, signer.priv, { alg: "RS256", typ: "JWT", kid: "k1" });
  const r = await verifyOidcJwt(good, ISS, AUD, JWKS);
  assert(r.tag === "Ok", "valid token is accepted");
  assert(r.tag === "Ok" && r.value.sub === "user-1", "valid token yields the sub claim");

  // #2380 (L5): the verified claims pass through; a string claim reads as
  // `Some`, a non-string one as `None` but present.
  const withClaims = await signRs256(
    { sub: "repo:acme/money:ref:refs/heads/main", iss: ISS, aud: AUD, exp: now + 3600, repository: "acme/money", repository_id: "42", run_attempt: 1 },
    signer.priv,
    { alg: "RS256", typ: "JWT", kid: "k1" },
  );
  const rc = await verifyOidcJwt(withClaims, ISS, AUD, JWKS);
  assert(rc.tag === "Ok", "a token with claims is accepted");
  if (rc.tag === "Ok") {
    const claims = tokenClaims(rc.value.claims);
    const repo = claims.get("repository");
    assert(repo.tag === "Some" && repo.value === "acme/money", "a string claim reads through");
    assert(claims.get("run_attempt").tag === "None" && claims.has("run_attempt"), "a non-string claim is present but not a string");
    assert(claims.get("environment").tag === "None" && !claims.has("environment"), "an absent claim reads as none");
  }

  // #2380 (L5): `trustFrom` — unset keeps the declared roots, a well-formed
  // override replaces them, a malformed one fails closed.
  const declared = oidcTrust(undefined, ISS, AUD, JWKS);
  assert(declared !== null && declared.issuer === ISS && declared.jwks === JWKS, "an unset override keeps the declaration");
  const over = oidcTrust(JSON.stringify({ issuer: "https://fake.test", jwks: "https://fake.test/jwks" }), ISS, AUD, JWKS);
  assert(over !== null && over.issuer === "https://fake.test" && over.audience === AUD, "an override replaces the issuer and jwks, keeping the audience");
  const quiet = console.error;
  console.error = () => {};
  assert(oidcTrust("not json", ISS, AUD, JWKS) === null, "a malformed override fails closed");
  assert(oidcTrust(JSON.stringify({ issuer: 1, jwks: "x" }), ISS, AUD, JWKS) === null, "an override without a string issuer fails closed");
  assert(oidcTrust("null", ISS, AUD, JWKS) === null, "a JSON null override fails closed");
  assert(oidcTrust("[]", ISS, AUD, JWKS) === null, "a JSON array override fails closed");
  assert(oidcTrust(42, ISS, AUD, JWKS) === null, "a non-string override fails closed");
  console.error = quiet;
  // A token from the overriding issuer verifies only against the override.
  const fromFake = await signRs256({ sub: "u", iss: "https://fake.test", aud: AUD, exp: now + 3600 }, signer.priv, { alg: "RS256", typ: "JWT", kid: "k1" });
  assert((await verifyOidcJwt(fromFake, ISS, AUD, JWKS)).tag === "Err", "the declared issuer refuses another issuer's token");
  assert(over !== null && (await verifyOidcJwt(fromFake, over.issuer, over.audience, over.jwks)).tag === "Ok", "the override accepts its issuer's token");

  async function rejects(token: string, why: string): Promise<void> {
    const res = await verifyOidcJwt(token, ISS, AUD, JWKS);
    assert(res.tag === "Err", why);
  }

  // token signed by a key NOT in the JWKS (forgery)
  await rejects(await signRs256({ sub: "u", iss: ISS, aud: AUD, exp: now + 3600 }, attacker.priv, { alg: "RS256", typ: "JWT", kid: "k1" }), "unpublished-key token rejected");

  // tampered signature
  const parts = good.split(".");
  const flipped = (parts[2][0] === "A" ? "B" : "A") + parts[2].slice(1);
  await rejects(`${parts[0]}.${parts[1]}.${flipped}`, "tampered signature rejected");

  // alg:none
  const noneTok = `${b64url(JSON.stringify({ alg: "none", typ: "JWT", kid: "k1" }))}.${b64url(JSON.stringify({ sub: "u", iss: ISS, aud: AUD, exp: now + 3600 }))}.`;
  await rejects(noneTok, "alg:none rejected");

  // algorithm confusion: HS256 label (symmetric) against the public key
  const hsTok = `${b64url(JSON.stringify({ alg: "HS256", typ: "JWT", kid: "k1" }))}.${b64url(JSON.stringify({ sub: "u", iss: ISS, aud: AUD, exp: now + 3600 }))}.${b64url("sig")}`;
  await rejects(hsTok, "HS256 (symmetric) alg rejected");

  // wrong issuer
  await rejects(await signRs256({ sub: "u", iss: "https://evil.test", aud: AUD, exp: now + 3600 }, signer.priv, { alg: "RS256", typ: "JWT", kid: "k1" }), "wrong issuer rejected");

  // wrong audience
  await rejects(await signRs256({ sub: "u", iss: ISS, aud: "other", exp: now + 3600 }, signer.priv, { alg: "RS256", typ: "JWT", kid: "k1" }), "wrong audience rejected");

  // expired (well beyond the clock-skew leeway)
  await rejects(await signRs256({ sub: "u", iss: ISS, aud: AUD, exp: now - 300 }, signer.priv, { alg: "RS256", typ: "JWT", kid: "k1" }), "expired token rejected");

  // not yet valid (nbf in the future)
  await rejects(await signRs256({ sub: "u", iss: ISS, aud: AUD, exp: now + 3600, nbf: now + 1000 }, signer.priv, { alg: "RS256", typ: "JWT", kid: "k1" }), "nbf-future token rejected");

  // missing exp
  await rejects(await signRs256({ sub: "u", iss: ISS, aud: AUD }, signer.priv, { alg: "RS256", typ: "JWT", kid: "k1" }), "missing exp rejected");

  // missing / empty sub
  await rejects(await signRs256({ iss: ISS, aud: AUD, exp: now + 3600 }, signer.priv, { alg: "RS256", typ: "JWT", kid: "k1" }), "missing sub rejected");
  await rejects(await signRs256({ sub: "", iss: ISS, aud: AUD, exp: now + 3600 }, signer.priv, { alg: "RS256", typ: "JWT", kid: "k1" }), "empty sub rejected");

  // malformed token shapes
  await rejects("not.a.jwt.token", "4-segment token rejected");
  await rejects("garbage", "non-jwt rejected");
  await rejects("", "empty token rejected");

  console.log("ALL OK");
};
await main();
"#;

const TSCONFIG_JSON: &str = r#"{
  "compilerOptions": {
    "target": "ES2022",
    "module": "NodeNext",
    "moduleResolution": "NodeNext",
    "strict": true,
    "skipLibCheck": true,
    "outDir": "js",
    "rootDir": ".",
    "lib": ["ES2022", "DOM"]
  },
  "include": ["*.ts"]
}
"#;

#[test]
fn oidc_verifier_rejects_every_bypass_class() {
    let runner = match discover_tsc() {
        Some(r) => r,
        None => {
            eprintln!(
                "\n!!! OIDC AUTH VERIFICATION SKIPPED !!!\nneither `tsc` nor `npx` is on PATH.\n"
            );
            if std::env::var(REQUIRE_ENV).is_ok() {
                panic!("{REQUIRE_ENV} is set but no tsc runner was found");
            }
            return;
        }
    };
    if !tool_exists("node") {
        eprintln!("\n!!! OIDC AUTH VERIFICATION SKIPPED !!!\n`node` is not on PATH.\n");
        if std::env::var(REQUIRE_ENV).is_ok() {
            panic!("{REQUIRE_ENV} is set but `node` was not found");
        }
        return;
    }

    let tmp = std::env::temp_dir().join(format!("bynk-oidc-auth-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();
    bynk_testkit::write_runtime_modules(&tmp).unwrap();
    fs::write(tmp.join("driver.ts"), DRIVER_TS).unwrap();
    fs::write(tmp.join("tsconfig.json"), TSCONFIG_JSON).unwrap();
    fs::write(tmp.join("package.json"), "{ \"type\": \"module\" }").unwrap();

    let (program, prefix) = &runner;
    let (ok, out) = run(program, prefix, &["-p", "tsconfig.json"], &tmp);
    assert!(ok, "tsc failed on the oidc-auth driver:\n{out}");

    let (ok, out) = run("node", &[], &["js/driver.js"], &tmp);
    assert!(
        ok && out.contains("ALL OK"),
        "oidc-auth driver did not pass:\n{out}"
    );
    let _ = fs::remove_dir_all(&tmp);
}

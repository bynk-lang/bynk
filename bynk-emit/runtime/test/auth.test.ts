import { test } from "node:test";
import assert from "node:assert/strict";
import { verifyBearerJwtHs256, verifySignatureHmacSha256, verifyOidcJwt } from "../src/auth.ts";

const enc = new TextEncoder();

function b64url(bytes: Uint8Array | string): string {
  const u8 = typeof bytes === "string" ? enc.encode(bytes) : bytes;
  let bin = "";
  for (const b of u8) bin += String.fromCharCode(b);
  return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

async function hmacKey(secret: string, usage: KeyUsage): Promise<CryptoKey> {
  return crypto.subtle.importKey(
    "raw",
    enc.encode(secret) as BufferSource,
    { name: "HMAC", hash: "SHA-256" },
    false,
    [usage],
  );
}

async function signJwt(
  payload: Record<string, unknown>,
  secret: string,
  header: Record<string, unknown> = { alg: "HS256", typ: "JWT" },
): Promise<string> {
  const head = b64url(JSON.stringify(header));
  const body = b64url(JSON.stringify(payload));
  const key = await hmacKey(secret, "sign");
  const sig = await crypto.subtle.sign("HMAC", key, enc.encode(`${head}.${body}`) as BufferSource);
  return `${head}.${body}.${b64url(new Uint8Array(sig))}`;
}

async function hexHmac(body: string, secret: string): Promise<string> {
  const key = await hmacKey(secret, "sign");
  const sig = new Uint8Array(await crypto.subtle.sign("HMAC", key, enc.encode(body) as BufferSource));
  return [...sig].map((b) => b.toString(16).padStart(2, "0")).join("");
}

const SECRET = "top-secret";
const future = () => Math.floor(Date.now() / 1000) + 3600;
const past = () => Math.floor(Date.now() / 1000) - 3600;

test("JWT: valid token returns Ok with sub and full claims", async () => {
  const token = await signJwt({ sub: "user-1", exp: future(), role: "admin" }, SECRET);
  const r = await verifyBearerJwtHs256(token, SECRET);
  assert.equal(r.tag, "Ok");
  if (r.tag === "Ok") {
    assert.equal(r.value.sub, "user-1");
    assert.equal(r.value.claims.role, "admin");
  }
});

test("JWT: wrong secret is a bad signature", async () => {
  const token = await signJwt({ sub: "u", exp: future() }, SECRET);
  const r = await verifyBearerJwtHs256(token, "other-secret");
  assert.deepEqual(r, { tag: "Err", error: "bad signature" });
});

test("JWT: non-HS256 alg is rejected (alg confusion / none)", async () => {
  for (const alg of ["none", "RS256", "HS384"]) {
    const token = await signJwt({ sub: "u", exp: future() }, SECRET, { alg });
    const r = await verifyBearerJwtHs256(token, SECRET);
    assert.deepEqual(r, { tag: "Err", error: "unsupported alg" });
  }
});

test("JWT: expired token rejected", async () => {
  const token = await signJwt({ sub: "u", exp: past() }, SECRET);
  assert.deepEqual(await verifyBearerJwtHs256(token, SECRET), { tag: "Err", error: "token expired" });
});

test("JWT: nbf in the future rejected", async () => {
  const token = await signJwt({ sub: "u", exp: future(), nbf: future() }, SECRET);
  assert.deepEqual(await verifyBearerJwtHs256(token, SECRET), {
    tag: "Err",
    error: "token not yet valid",
  });
});

test("JWT: token with no exp rejected (must not never-expire)", async () => {
  const token = await signJwt({ sub: "u", role: "admin" }, SECRET);
  assert.deepEqual(await verifyBearerJwtHs256(token, SECRET), { tag: "Err", error: "missing exp" });
});

test("JWT: non-number exp is malformed (not silently skipped)", async () => {
  const token = await signJwt({ sub: "u", exp: "soon" }, SECRET);
  assert.deepEqual(await verifyBearerJwtHs256(token, SECRET), { tag: "Err", error: "malformed exp" });
});

test("JWT: missing/empty sub rejected", async () => {
  const token = await signJwt({ exp: future() }, SECRET);
  assert.deepEqual(await verifyBearerJwtHs256(token, SECRET), { tag: "Err", error: "missing sub" });
});

test("JWT: structurally malformed token rejected", async () => {
  assert.deepEqual(await verifyBearerJwtHs256("a.b", SECRET), { tag: "Err", error: "malformed token" });
});

test("webhook: correct bare-hex signature verifies", async () => {
  const body = '{"event":"ping"}';
  const sig = await hexHmac(body, SECRET);
  assert.equal(await verifySignatureHmacSha256(body, SECRET, sig, null, null), true);
});

test("webhook: sha256= prefix accepted", async () => {
  const body = "payload";
  const sig = await hexHmac(body, SECRET);
  assert.equal(await verifySignatureHmacSha256(body, SECRET, `sha256=${sig}`, null, null), true);
});

test("webhook: wrong signature and null header rejected", async () => {
  const body = "payload";
  assert.equal(await verifySignatureHmacSha256(body, SECRET, "00".repeat(32), null, null), false);
  assert.equal(await verifySignatureHmacSha256(body, SECRET, null, null, null), false);
});

test("webhook: a raw body is verified over its octets, with or without a timestamp", async () => {
  const octets = new Uint8Array([0x80, 0xff, 0x00, 0x01, 0xfe]);
  const mac = async (data: Uint8Array): Promise<string> => {
    const sig = new Uint8Array(await crypto.subtle.sign("HMAC", await hmacKey(SECRET, "sign"), data as BufferSource));
    return Array.from(sig, (b) => b.toString(16).padStart(2, "0")).join("");
  };
  assert.equal(await verifySignatureHmacSha256(octets, SECRET, await mac(octets), null, null), true);
  // Not valid UTF-8, so a text round trip would sign different bytes.
  const mangled = enc.encode(new TextDecoder().decode(octets));
  assert.equal(await verifySignatureHmacSha256(octets, SECRET, await mac(mangled), null, null), false);
  const ts = String(Math.floor(Date.now() / 1000));
  const prefixed = new Uint8Array([...enc.encode(`${ts}.`), ...octets]);
  assert.equal(await verifySignatureHmacSha256(octets, SECRET, await mac(prefixed), ts, 300), true);
  assert.equal(await verifySignatureHmacSha256(octets, SECRET, await mac(octets), ts, 300), false);
});

test("webhook: timestamp is part of the signed string and bounded by tolerance", async () => {
  const body = "payload";
  const ts = String(Math.floor(Date.now() / 1000));
  const signed = await hexHmac(`${ts}.${body}`, SECRET);
  // within tolerance
  assert.equal(await verifySignatureHmacSha256(body, SECRET, signed, ts, 300), true);
  // a signature over the bare body must NOT verify once a timestamp is bound
  const bareBodySig = await hexHmac(body, SECRET);
  assert.equal(await verifySignatureHmacSha256(body, SECRET, bareBodySig, ts, 300), false);
  // stale timestamp rejected
  const oldTs = String(Math.floor(Date.now() / 1000) - 10_000);
  const oldSigned = await hexHmac(`${oldTs}.${body}`, SECRET);
  assert.equal(await verifySignatureHmacSha256(body, SECRET, oldSigned, oldTs, 300), false);
});

test("webhook: non-finite timestamp rejected", async () => {
  const body = "payload";
  const sig = await hexHmac(`x.${body}`, SECRET);
  assert.equal(await verifySignatureHmacSha256(body, SECRET, sig, "not-a-number", 300), false);
});

// ---------------------------------------------------------------------------
// OIDC / JWKS (verifyOidcJwt)
// ---------------------------------------------------------------------------

const ISS = "https://issuer.test";
const AUD = "my-api";

// `kid`/`use` are JWK members not present on the WebCrypto `JsonWebKey` IDL.
type TestJwk = JsonWebKey & { kid?: string; use?: string };

// A registry-backed `fetch` mock: each test registers its JWKS at a unique URL,
// so the module's JWKS cache never crosses tests.
let __jwksCounter = 0;
const __jwksRegistry = new Map<string, unknown>();
const __fetchCounts = new Map<string, number>();
globalThis.fetch = (async (input: RequestInfo | URL): Promise<Response> => {
  const url =
    typeof input === "string" ? input : input instanceof URL ? input.href : (input as Request).url;
  __fetchCounts.set(url, (__fetchCounts.get(url) ?? 0) + 1);
  if (__jwksRegistry.has(url)) {
    return new Response(JSON.stringify(__jwksRegistry.get(url)), {
      status: 200,
      headers: { "content-type": "application/json" },
    });
  }
  return new Response("not found", { status: 404 });
}) as typeof fetch;

function registerJwks(keys: unknown[]): string {
  const uri = `${ISS}/jwks/${__jwksCounter++}`;
  __jwksRegistry.set(uri, { keys });
  return uri;
}

async function rsaKeypair(kid: string): Promise<{ priv: CryptoKey; jwk: TestJwk }> {
  const kp = (await crypto.subtle.generateKey(
    {
      name: "RSASSA-PKCS1-v1_5",
      modulusLength: 2048,
      publicExponent: new Uint8Array([1, 0, 1]),
      hash: "SHA-256",
    },
    true,
    ["sign", "verify"],
  )) as CryptoKeyPair;
  const jwk = await crypto.subtle.exportKey("jwk", kp.publicKey);
  return { priv: kp.privateKey, jwk: { ...jwk, kid, alg: "RS256", use: "sig" } };
}

async function ecKeypair(kid: string): Promise<{ priv: CryptoKey; jwk: TestJwk }> {
  const kp = (await crypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, true, [
    "sign",
    "verify",
  ])) as CryptoKeyPair;
  const jwk = await crypto.subtle.exportKey("jwk", kp.publicKey);
  return { priv: kp.privateKey, jwk: { ...jwk, kid, alg: "ES256", use: "sig" } };
}

async function signRs256(
  payload: Record<string, unknown>,
  priv: CryptoKey,
  kid: string,
): Promise<string> {
  const head = b64url(JSON.stringify({ alg: "RS256", typ: "JWT", kid }));
  const body = b64url(JSON.stringify(payload));
  const sig = await crypto.subtle.sign(
    "RSASSA-PKCS1-v1_5",
    priv,
    enc.encode(`${head}.${body}`) as BufferSource,
  );
  return `${head}.${body}.${b64url(new Uint8Array(sig))}`;
}

async function signEs256(
  payload: Record<string, unknown>,
  priv: CryptoKey,
  kid: string,
): Promise<string> {
  const head = b64url(JSON.stringify({ alg: "ES256", typ: "JWT", kid }));
  const body = b64url(JSON.stringify(payload));
  const sig = await crypto.subtle.sign(
    { name: "ECDSA", hash: "SHA-256" },
    priv,
    enc.encode(`${head}.${body}`) as BufferSource,
  );
  return `${head}.${body}.${b64url(new Uint8Array(sig))}`;
}

test("OIDC: valid RS256 token returns Ok with sub and claims", async () => {
  const { priv, jwk } = await rsaKeypair("k1");
  const jwks = registerJwks([jwk]);
  const token = await signRs256({ sub: "user-1", iss: ISS, aud: AUD, exp: future(), role: "x" }, priv, "k1");
  const r = await verifyOidcJwt(token, ISS, AUD, jwks);
  assert.equal(r.tag, "Ok");
  if (r.tag === "Ok") {
    assert.equal(r.value.sub, "user-1");
    assert.equal(r.value.claims.role, "x");
  }
});

test("OIDC: valid ES256 token verifies", async () => {
  const { priv, jwk } = await ecKeypair("e1");
  const jwks = registerJwks([jwk]);
  const token = await signEs256({ sub: "u", iss: ISS, aud: AUD, exp: future() }, priv, "e1");
  const r = await verifyOidcJwt(token, ISS, AUD, jwks);
  assert.equal(r.tag, "Ok");
});

test("OIDC: audience as an array containing the audience is accepted", async () => {
  const { priv, jwk } = await rsaKeypair("k1");
  const jwks = registerJwks([jwk]);
  const token = await signRs256({ sub: "u", iss: ISS, aud: ["other", AUD], exp: future() }, priv, "k1");
  assert.equal((await verifyOidcJwt(token, ISS, AUD, jwks)).tag, "Ok");
});

test("OIDC: wrong issuer rejected", async () => {
  const { priv, jwk } = await rsaKeypair("k1");
  const jwks = registerJwks([jwk]);
  const token = await signRs256({ sub: "u", iss: "https://evil.test", aud: AUD, exp: future() }, priv, "k1");
  assert.deepEqual(await verifyOidcJwt(token, ISS, AUD, jwks), { tag: "Err", error: "issuer mismatch" });
});

test("OIDC: wrong audience rejected", async () => {
  const { priv, jwk } = await rsaKeypair("k1");
  const jwks = registerJwks([jwk]);
  const token = await signRs256({ sub: "u", iss: ISS, aud: "someone-else", exp: future() }, priv, "k1");
  assert.deepEqual(await verifyOidcJwt(token, ISS, AUD, jwks), { tag: "Err", error: "audience mismatch" });
});

test("OIDC: expired token rejected", async () => {
  const { priv, jwk } = await rsaKeypair("k1");
  const jwks = registerJwks([jwk]);
  const token = await signRs256({ sub: "u", iss: ISS, aud: AUD, exp: past() }, priv, "k1");
  assert.deepEqual(await verifyOidcJwt(token, ISS, AUD, jwks), { tag: "Err", error: "token expired" });
});

test("OIDC: missing exp rejected", async () => {
  const { priv, jwk } = await rsaKeypair("k1");
  const jwks = registerJwks([jwk]);
  const token = await signRs256({ sub: "u", iss: ISS, aud: AUD }, priv, "k1");
  assert.deepEqual(await verifyOidcJwt(token, ISS, AUD, jwks), { tag: "Err", error: "missing exp" });
});

test("OIDC: a token signed by an unpublished key is a bad signature", async () => {
  const published = await rsaKeypair("k1");
  const attacker = await rsaKeypair("k1"); // same kid, different key
  const jwks = registerJwks([published.jwk]);
  const token = await signRs256({ sub: "u", iss: ISS, aud: AUD, exp: future() }, attacker.priv, "k1");
  assert.deepEqual(await verifyOidcJwt(token, ISS, AUD, jwks), { tag: "Err", error: "bad signature" });
});

test("OIDC: alg none / HS256 rejected (no algorithm confusion)", async () => {
  const { jwk } = await rsaKeypair("k1");
  const jwks = registerJwks([jwk]);
  for (const alg of ["none", "HS256"]) {
    const head = b64url(JSON.stringify({ alg, typ: "JWT", kid: "k1" }));
    const body = b64url(JSON.stringify({ sub: "u", iss: ISS, aud: AUD, exp: future() }));
    const token = `${head}.${body}.${b64url("x")}`;
    assert.deepEqual(await verifyOidcJwt(token, ISS, AUD, jwks), { tag: "Err", error: "unsupported alg" });
  }
});

test("OIDC: missing sub rejected", async () => {
  const { priv, jwk } = await rsaKeypair("k1");
  const jwks = registerJwks([jwk]);
  const token = await signRs256({ iss: ISS, aud: AUD, exp: future() }, priv, "k1");
  assert.deepEqual(await verifyOidcJwt(token, ISS, AUD, jwks), { tag: "Err", error: "missing sub" });
});

test("OIDC: structurally malformed token rejected", async () => {
  const jwks = registerJwks([]);
  assert.deepEqual(await verifyOidcJwt("a.b", ISS, AUD, jwks), { tag: "Err", error: "malformed token" });
});

test("OIDC: a flood of tokens with novel `kid`s triggers at most one JWKS fetch (no amplification)", async () => {
  const { priv, jwk } = await rsaKeypair("k1");
  const jwks = registerJwks([jwk]);
  // Prime the cache with one legitimate verification (the single expected fetch).
  const good = await signRs256({ sub: "u", iss: ISS, aud: AUD, exp: future() }, priv, "k1");
  assert.equal((await verifyOidcJwt(good, ISS, AUD, jwks)).tag, "Ok");
  const afterPrime = __fetchCounts.get(jwks) ?? 0;
  assert.equal(afterPrime, 1);
  // `kid` is attacker-controlled: several tokens whose `kid` matches no published
  // key are each a `kid` miss that would force a refetch. The cooldown bounds
  // them — no additional network fetch within the window.
  for (let i = 0; i < 5; i++) {
    const forged = await signRs256({ sub: "u", iss: ISS, aud: AUD, exp: future() }, priv, `unknown-${i}`);
    assert.deepEqual(await verifyOidcJwt(forged, ISS, AUD, jwks), { tag: "Err", error: "bad signature" });
  }
  assert.equal(__fetchCounts.get(jwks), afterPrime);
});

test("OIDC: a token within the clock-skew leeway of expiry is still accepted", async () => {
  const { priv, jwk } = await rsaKeypair("k1");
  const jwks = registerJwks([jwk]);
  const now = Math.floor(Date.now() / 1000);
  // Expired 5s ago — inside the 60s leeway, so accepted (small-clock-skew).
  const token = await signRs256({ sub: "u", iss: ISS, aud: AUD, exp: now - 5 }, priv, "k1");
  assert.equal((await verifyOidcJwt(token, ISS, AUD, jwks)).tag, "Ok");
});

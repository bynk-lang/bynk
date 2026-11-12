import { test } from "node:test";
import assert from "node:assert/strict";
import {
  SESSION_KEY_MIN_BYTES,
  SessionKeyFault,
  type SessionKeys,
  newSessionKey,
  signSessionCookie,
  verifySignedCookie,
} from "../src/session.ts";
import { verifyBearerJwtHs256 } from "../src/auth.ts";

const enc = new TextEncoder();
const NOW = 1_800_000_000_000; // ms
const NOW_S = NOW / 1000;
const TTL = 3600;
const KEY = newSessionKey();
const KEYS = { secret: KEY };

function b64url(bytes: Uint8Array | string): string {
  const u8 = typeof bytes === "string" ? enc.encode(bytes) : bytes;
  let bin = "";
  for (const b of u8) bin += String.fromCharCode(b);
  return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}
function unb64(s: string): Uint8Array {
  const bin = atob(s.replace(/-/g, "+").replace(/_/g, "/").padEnd(Math.ceil(s.length / 4) * 4, "="));
  return Uint8Array.from(bin, (c) => c.charCodeAt(0));
}
async function mac(key: string, signed: string): Promise<string> {
  const k = await crypto.subtle.importKey("raw", unb64(key) as BufferSource, { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  return b64url(new Uint8Array(await crypto.subtle.sign("HMAC", k, enc.encode(signed) as BufferSource)));
}
/** A cookie built by hand with the real key, so a test can vary one field of the header or payload. */
async function forge(
  key: string,
  header: Record<string, unknown>,
  payload: Record<string, unknown>,
): Promise<string> {
  const h = b64url(JSON.stringify(header));
  const p = b64url(JSON.stringify(payload));
  return `${h}.${p}.${await mac(key, `${h}.${p}`)}`;
}
async function kidOf(key: string): Promise<string> {
  const d = new Uint8Array(await crypto.subtle.digest("SHA-256", unb64(key) as BufferSource));
  return b64url(d.slice(0, 6));
}
const goodPayload = (over: Record<string, unknown> = {}) => ({ sub: "ada", aud: "User", iat: NOW_S, exp: NOW_S + TTL, ...over });
async function goodHeader(key = KEY, over: Record<string, unknown> = {}) {
  return { alg: "HS256", typ: "bynk+session", kid: await kidOf(key), ...over };
}
async function verify(value: string, keys: SessionKeys = KEYS, ttl = TTL, now = NOW) {
  return verifySignedCookie(value, keys, "User", ttl, now);
}
function assertErr(r: Awaited<ReturnType<typeof verify>>, why: string) {
  assert.equal(r.tag, "Err");
  if (r.tag === "Err") assert.equal(r.error, why);
}

test("a new key is 32 random bytes of unpadded base64url", () => {
  const k = newSessionKey();
  assert.match(k, /^[A-Za-z0-9_-]{43}$/);
  assert.equal(unb64(k).length, SESSION_KEY_MIN_BYTES);
  assert.notEqual(k, newSessionKey());
});

test("sign then verify round-trips the identity", async () => {
  const c = await signSessionCookie(KEYS, "User", "ada", TTL, NOW);
  assert.match(c, /^[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+$/);
  const r = await verify(c);
  assert.equal(r.tag, "Ok");
  if (r.tag === "Ok") assert.equal(r.value.sub, "ada");
});

test("the issued cookie has the documented header and payload", async () => {
  const c = await signSessionCookie(KEYS, "User", "ada", TTL, NOW);
  const [h, p] = c.split(".") as [string, string];
  assert.deepEqual(JSON.parse(new TextDecoder().decode(unb64(h))), { alg: "HS256", typ: "bynk+session", kid: await kidOf(KEY) });
  assert.deepEqual(JSON.parse(new TextDecoder().decode(unb64(p))), { sub: "ada", aud: "User", iat: NOW_S, exp: NOW_S + TTL });
});

test("an identity with non-ASCII characters round-trips", async () => {
  const c = await signSessionCookie(KEYS, "User", "zoë-日本", TTL, NOW);
  const r = await verify(c);
  assert.equal(r.tag === "Ok" && r.value.sub, "zoë-日本");
});

test("expiry: valid until exp, refused at exp", async () => {
  const c = await signSessionCookie(KEYS, "User", "ada", TTL, NOW);
  assert.equal((await verify(c, KEYS, TTL, NOW + (TTL - 1) * 1000)).tag, "Ok");
  assertErr(await verify(c, KEYS, TTL, NOW + TTL * 1000), "cookie expired");
  assertErr(await verify(c, KEYS, TTL, NOW + (TTL + 3600) * 1000), "cookie expired");
});

test("a cookie issued in the future is refused beyond one second of skew", async () => {
  const c = await signSessionCookie(KEYS, "User", "ada", TTL, NOW + 5000);
  assertErr(await verify(c), "issued in the future");
  const near = await signSessionCookie(KEYS, "User", "ada", TTL, NOW + 1000);
  assert.equal((await verify(near)).tag, "Ok");
});

test("shortening the ttl refuses cookies already issued with a longer one", async () => {
  const c = await signSessionCookie(KEYS, "User", "ada", TTL, NOW);
  assertErr(await verify(c, KEYS, TTL - 1), "lifetime exceeds ttl");
  assert.equal((await verify(c, KEYS, TTL)).tag, "Ok");
});

test("a cookie for another actor is refused (audience)", async () => {
  const c = await signSessionCookie(KEYS, "Admin", "ada", TTL, NOW);
  assertErr(await verify(c), "wrong audience");
});

test("a Bearer-style JWT signed with the same key is not a session cookie", async () => {
  const h = b64url(JSON.stringify({ alg: "HS256", typ: "JWT" }));
  const p = b64url(JSON.stringify({ sub: "ada", exp: NOW_S + TTL }));
  const jwt = `${h}.${p}.${await mac(KEY, `${h}.${p}`)}`;
  assertErr(await verify(jwt), "wrong type");
});

test("a session cookie is not a bearer token for the same key material", async () => {
  const c = await signSessionCookie(KEYS, "User", "ada", TTL, NOW);
  // Bearer keys are raw strings, so this models a deployment that (wrongly) shared the variable's text.
  const r = await verifyBearerJwtHs256(c, KEY);
  assert.equal(r.tag, "Err");
});

test("a tampered payload or signature is refused", async () => {
  const c = await signSessionCookie(KEYS, "User", "ada", TTL, NOW);
  const [h, p, m] = c.split(".") as [string, string, string];
  const evil = b64url(JSON.stringify(goodPayload({ sub: "root" })));
  assertErr(await verify(`${h}.${evil}.${m}`), "bad signature");
  const flipped = m.slice(0, -2) + (m.endsWith("AA") ? "BB" : "AA");
  assertErr(await verify(`${h}.${p}.${flipped}`), "bad signature");
});

test("a cookie signed with another key is refused", async () => {
  const other = newSessionKey();
  const c = await signSessionCookie({ secret: other }, "User", "ada", TTL, NOW);
  assertErr(await verify(c), "unknown key");
  // A forged kid naming the right key still fails the MAC.
  const forged = await forge(other, await goodHeader(KEY), goodPayload());
  assertErr(await verify(forged), "bad signature");
});

test("alg none, RS256 and a missing alg are refused", async () => {
  for (const alg of ["none", "RS256", "HS512", undefined]) {
    const hdr = await goodHeader(KEY, { alg });
    const c = await forge(KEY, hdr, goodPayload());
    assertErr(await verify(c), "unsupported alg");
  }
  const h = b64url(JSON.stringify({ alg: "none", typ: "bynk+session" }));
  const p = b64url(JSON.stringify(goodPayload()));
  assertErr(await verify(`${h}.${p}.`), "unsupported alg");
});

test("missing or wrongly typed header fields are refused", async () => {
  assertErr(await verify(await forge(KEY, await goodHeader(KEY, { typ: "JWT" }), goodPayload())), "wrong type");
  assertErr(await verify(await forge(KEY, await goodHeader(KEY, { typ: undefined }), goodPayload())), "wrong type");
  assertErr(await verify(await forge(KEY, await goodHeader(KEY, { kid: undefined }), goodPayload())), "missing key id");
  assertErr(await verify(await forge(KEY, await goodHeader(KEY, { kid: 7 }), goodPayload())), "missing key id");
});

test("bad payload fields are refused, each after the signature", async () => {
  const hdr = await goodHeader();
  const cases: [Record<string, unknown>, string][] = [
    [goodPayload({ sub: "" }), "missing sub"],
    [goodPayload({ sub: undefined }), "missing sub"],
    [goodPayload({ sub: 7 }), "missing sub"],
    [goodPayload({ iat: "1" }), "malformed iat"],
    [goodPayload({ iat: 1.5 }), "malformed iat"],
    [goodPayload({ iat: undefined }), "malformed iat"],
    [goodPayload({ exp: undefined }), "malformed exp"],
    [goodPayload({ exp: "x" }), "malformed exp"],
    [goodPayload({ exp: NOW_S + 0.5 }), "malformed exp"],
    [goodPayload({ exp: NOW_S }), "malformed lifetime"],
    [goodPayload({ exp: NOW_S - 10 }), "malformed lifetime"],
  ];
  for (const [payload, why] of cases) assertErr(await verify(await forge(KEY, hdr, payload)), why);
});

test("malformed cookies are refused without throwing", async () => {
  for (const v of ["", "a", "a.b", "a.b.c.d", "..", "a..c", "!!.!!.!!", "a.b.c", ".".repeat(3), "x".repeat(5000)]) {
    const r = await verify(v);
    assert.equal(r.tag, "Err", JSON.stringify(v.slice(0, 20)));
  }
});

test("non-canonical, padded and standard-alphabet encodings are refused", async () => {
  const c = await signSessionCookie(KEYS, "User", "ada", TTL, NOW);
  const [h, p, m] = c.split(".") as [string, string, string];
  assert.equal((await verify(`${h}=.${p}.${m}`)).tag, "Err");
  assert.equal((await verify(`${h}.${p}.${m}=`)).tag, "Err");
  assert.equal((await verify(`${h}.${p}.${m.replace(/-/g, "+").replace(/_/g, "/")}+`)).tag, "Err");
  // The same MAC bytes with a non-zero trailing bit re-encode differently: refused.
  const last = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
  const ix = last.indexOf(m[m.length - 1] as string);
  const twin = m.slice(0, -1) + (last[ix ^ 1] as string);
  assert.equal((await verify(`${h}.${p}.${twin}`)).tag, "Err");
});

test("a header that is not valid UTF-8 JSON is refused", async () => {
  const badUtf8 = b64url(new Uint8Array([0xff, 0xfe, 0xfd]));
  assertErr(await verify(`${badUtf8}.${b64url("{}")}.${b64url("x")}`), "malformed header");
  assertErr(await verify(`${b64url("[]")}.${b64url("{}")}.${b64url("x")}`), "malformed header");
  assertErr(await verify(`${b64url("null")}.${b64url("{}")}.${b64url("x")}`), "malformed header");
});

test("rotation: a previous key verifies, but only the current key signs", async () => {
  const oldKey = newSessionKey();
  const newKey = newSessionKey();
  const issuedBefore = await signSessionCookie({ secret: oldKey }, "User", "ada", TTL, NOW);
  // Before rotation completes: the old key is the secret. After: new secret, old as previous.
  const rotated = { secret: newKey, previous: oldKey };
  const r = await verify(issuedBefore, rotated);
  assert.equal(r.tag === "Ok" && r.value.sub, "ada");
  const issuedAfter = await signSessionCookie(rotated, "User", "bob", TTL, NOW);
  const [h] = issuedAfter.split(".") as [string];
  assert.equal(JSON.parse(new TextDecoder().decode(unb64(h))).kid, await kidOf(newKey));
  assert.equal((await verify(issuedAfter, rotated)).tag, "Ok");
  // Once `previous` is dropped, old cookies no longer verify.
  assertErr(await verify(issuedBefore, { secret: newKey }), "unknown key");
  // An empty previous is the same as none.
  assertErr(await verify(issuedBefore, { secret: newKey, previous: "" }), "unknown key");
  assertErr(await verify(issuedBefore, { secret: newKey, previous: undefined }), "unknown key");
});

test("a key that is too short, undecodable or duplicated is a fault, not a refusal", async () => {
  const short = b64url(crypto.getRandomValues(new Uint8Array(31)));
  await assert.rejects(signSessionCookie({ secret: short }, "User", "ada", TTL, NOW), SessionKeyFault);
  await assert.rejects(verify("a.b.c", { secret: short }), SessionKeyFault);
  for (const bad of ["", "not base64url!", "AAAA=", KEY + "=", KEY.replace(/[A-Za-z]/, "+")]) {
    await assert.rejects(verify("a.b.c", { secret: bad }), SessionKeyFault, JSON.stringify(bad));
  }
  await assert.rejects(verify("a.b.c", { secret: KEY, previous: short }), SessionKeyFault);
  await assert.rejects(verify("a.b.c", { secret: KEY, previous: KEY }), SessionKeyFault);
  // A fault names the variable and never carries the key.
  try {
    await verify("a.b.c", { secret: short });
  } catch (e) {
    assert.ok(e instanceof SessionKeyFault);
    assert.ok(!e.message.includes(short));
    assert.match(e.message, /`secret`/);
  }
});

test("a bad key faults even when the cookie is garbage", async () => {
  await assert.rejects(verify("", { secret: "short" }), SessionKeyFault);
});

test("signing refuses a bad ttl, an empty identity and an oversized value", async () => {
  for (const ttl of [0, -1, 1.5, NaN, Infinity]) {
    await assert.rejects(signSessionCookie(KEYS, "User", "ada", ttl, NOW), RangeError, String(ttl));
  }
  await assert.rejects(signSessionCookie(KEYS, "User", "", TTL, NOW), RangeError);
  await assert.rejects(signSessionCookie(KEYS, "User", "x".repeat(4000), TTL, NOW), RangeError);
});

test("the default clock is Date.now", async () => {
  const c = await signSessionCookie(KEYS, "User", "ada", 60);
  assert.equal((await verifySignedCookie(c, KEYS, "User", 60)).tag, "Ok");
});

// -- the router's cookie and CSRF helpers (http.ts) --------------------------------------------
import { cookieObject, csrfAllowed, requestCookies, sessionCookieOf } from "../src/http.ts";

const req = (headers: Record<string, string>, url = "https://api.example.com/x", method = "POST") =>
  new Request(url, { method, headers });

test("sessionCookieOf reads the named cookie, and refuses a missing or repeated one", () => {
  assert.equal(sessionCookieOf(req({ cookie: "a=1; __Host-session=tok; b=2" }), "__Host-session"), "tok");
  assert.equal(sessionCookieOf(req({}), "__Host-session"), null);
  assert.equal(sessionCookieOf(req({ cookie: "a=1" }), "__Host-session"), null);
  // A second cookie of the name is cookie tossing: refuse rather than pick one.
  assert.equal(sessionCookieOf(req({ cookie: "__Host-session=a; __Host-session=b" }), "__Host-session"), null);
  assert.equal(sessionCookieOf(req({ cookie: '__Host-session="quoted"' }), "__Host-session"), "quoted");
  assert.equal(sessionCookieOf(req({ cookie: "__host-session=x" }), "__Host-session"), null);
});

test("the credential is hidden from the handler's raw cookie view and typed record", () => {
  const r = req({ cookie: "theme=dark; __Host-session=tok" });
  const view = requestCookies(r, "__Host-session");
  assert.deepEqual(view.names(), ["theme"]);
  assert.deepEqual(view.get("__Host-session"), { tag: "None" });
  assert.deepEqual(view.all("__Host-session"), []);
  assert.deepEqual(requestCookies(r).names(), ["theme", "__Host-session"]);
  const fields = [{ name: "sid", wire: "__Host-session", kind: "string", list: false, optional: true }] as never;
  const shown = cookieObject(r, fields);
  assert.deepEqual(shown.ok && shown.value, { sid: "tok" });
  const hidden = cookieObject(r, fields, "__Host-session");
  assert.deepEqual(hidden.ok && hidden.value, {});
});

const CORS = ["https://app.example.com"];
test("csrf: a same-origin or user-initiated request passes", () => {
  assert.equal(csrfAllowed(req({ "sec-fetch-site": "same-origin" }), []), true);
  assert.equal(csrfAllowed(req({ "sec-fetch-site": "none" }), []), true);
  // `Origin` is irrelevant once the browser says same-origin.
  assert.equal(csrfAllowed(req({ "sec-fetch-site": "same-origin", origin: "https://evil.example" }), []), true);
});

test("csrf: a cross-site or same-site request is refused unless its Origin is a credentialed cors origin", () => {
  for (const site of ["cross-site", "same-site", "bogus"]) {
    assert.equal(csrfAllowed(req({ "sec-fetch-site": site }), CORS), false, site);
    assert.equal(csrfAllowed(req({ "sec-fetch-site": site, origin: "https://evil.example" }), CORS), false, site);
    assert.equal(csrfAllowed(req({ "sec-fetch-site": site, origin: "https://app.example.com" }), CORS), true, site);
    assert.equal(csrfAllowed(req({ "sec-fetch-site": site, origin: "https://app.example.com" }), []), false, site);
  }
  // The origin must match exactly: no prefix, scheme or port games.
  for (const o of ["https://app.example.com.evil.example", "http://app.example.com", "https://app.example.com:8443", "https://APP.example.com/"]) {
    assert.equal(csrfAllowed(req({ "sec-fetch-site": "cross-site", origin: o }), CORS), false, o);
  }
  // Same-origin by `Origin` alone does not rescue a request the browser called cross-site.
  assert.equal(csrfAllowed(req({ "sec-fetch-site": "cross-site", origin: "https://api.example.com" }), []), false);
});

test("csrf: with no Sec-Fetch-Site the Origin must be this origin or an allowed one", () => {
  assert.equal(csrfAllowed(req({ origin: "https://api.example.com" }), []), true);
  assert.equal(csrfAllowed(req({ origin: "https://app.example.com" }), CORS), true);
  assert.equal(csrfAllowed(req({ origin: "https://evil.example" }), CORS), false);
  assert.equal(csrfAllowed(req({ origin: "null" }), CORS), false);
});

test("csrf: a request revealing neither header is refused", () => {
  assert.equal(csrfAllowed(req({}), CORS), false);
  assert.equal(csrfAllowed(req({}), []), false);
});

test("a key that is not a string, or empty, is a fault that names the key", async () => {
  for (const bad of [undefined, null, 7, "", {}]) {
    await assert.rejects(verifySignedCookie("a.b.c", { secret: bad }, "User", TTL, NOW), (e: unknown) => {
      assert.ok(e instanceof SessionKeyFault);
      assert.match((e as Error).message, /`secret` is not set/);
      return true;
    });
  }
});

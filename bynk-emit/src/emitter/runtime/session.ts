import { Ok, Err, type Result } from "./result.ts";

// #3386: the `SignedCookie` actor's signer and verifier. A session cookie is a
// compact JWS (`header.payload.mac`, the layout the Bearer seam parses) signed
// with HMAC-SHA-256 through WebCrypto, and verified in constant time
// (`crypto.subtle.verify`). It is signed, never encrypted: it carries the
// identity (`sub`) and its expiry, and nothing the user may not read.
//
//   header   {"alg":"HS256","typ":"bynk+session","kid":"<key id>"}
//   payload  {"sub":"<identity>","aud":"<actor>","iat":<s>,"exp":<s>}
//
// `typ` and `aud` are domain separation: a Bearer JWT, or another actor's
// cookie, never verifies as this actor's, even were a key shared. The key is
// the base64url of at least 32 random bytes; a shorter or undecodable key is a
// configuration fault (`SessionKeyFault`), never a quiet "accept".

/** The `typ` header every session cookie carries. */
export const SESSION_TYP = "bynk+session";

/** Reject a presented cookie longer than this before parsing it (browsers cap a cookie at 4096). */
const MAX_COOKIE_CHARS = 4096;

/** The least key length, in bytes. */
export const SESSION_KEY_MIN_BYTES = 32;

/** The clock skew tolerated on `iat` (the issuer and verifier are one deployment). */
const IAT_SKEW_SECS = 1;

/** A session key that is missing, undecodable or too short: a configuration fault, not a bad cookie. */
export class SessionKeyFault extends Error {
  constructor(message: string) {
    super(message);
    this.name = "SessionKeyFault";
  }
}

/** The keys a `SignedCookie` actor holds: the signing `secret`, and the optional `previous` being rotated out. */
export interface SessionKeys {
  readonly secret: string;
  readonly previous?: string | undefined;
}

const enc = new TextEncoder();
const dec = new TextDecoder("utf-8", { fatal: true });
const B64URL = /^[A-Za-z0-9_-]+$/;

function toB64Url(bytes: Uint8Array): string {
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

/** Strict, canonical, unpadded base64url; `null` for anything else (padding, `+`, `/`, stray bits). */
function fromB64Url(s: string): Uint8Array | null {
  if (!B64URL.test(s) || s.length % 4 === 1) return null;
  let bytes: Uint8Array;
  try {
    const bin = atob(s.replace(/-/g, "+").replace(/_/g, "/").padEnd(Math.ceil(s.length / 4) * 4, "="));
    bytes = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  } catch {
    return null;
  }
  return toB64Url(bytes) === s ? bytes : null;
}

/** A fresh signing key: 32 bytes from the platform CSPRNG, base64url (what `bynk deploy` mints). */
export function newSessionKey(): string {
  return toB64Url(crypto.getRandomValues(new Uint8Array(SESSION_KEY_MIN_BYTES)));
}

function decodeKey(name: string, value: string): Uint8Array {
  const bytes = fromB64Url(value);
  if (bytes === null) {
    throw new SessionKeyFault(`the session key \`${name}\` is not base64url`);
  }
  if (bytes.length < SESSION_KEY_MIN_BYTES) {
    throw new SessionKeyFault(
      `the session key \`${name}\` is ${bytes.length} bytes; it must be at least ${SESSION_KEY_MIN_BYTES}`,
    );
  }
  return bytes;
}

function bytesEqual(a: Uint8Array, b: Uint8Array): boolean {
  return a.length === b.length && a.every((x, i) => x === b[i]);
}

interface ImportedKey {
  readonly kid: string;
  readonly bytes: Uint8Array;
}

/** The id a header names its key by: the first 6 bytes of SHA-256 of the key, base64url. */
async function keyId(bytes: Uint8Array): Promise<string> {
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", bytes as BufferSource));
  return toB64Url(digest.slice(0, 6));
}

async function loadKeys(keys: SessionKeys): Promise<{ current: ImportedKey; previous: ImportedKey | null }> {
  const secretBytes = decodeKey("secret", keys.secret);
  const current = { kid: await keyId(secretBytes), bytes: secretBytes };
  if (keys.previous === undefined || keys.previous === "") return { current, previous: null };
  const prevBytes = decodeKey("previous", keys.previous);
  const previous = { kid: await keyId(prevBytes), bytes: prevBytes };
  if (previous.kid === current.kid || bytesEqual(prevBytes, secretBytes)) {
    throw new SessionKeyFault("the session keys `secret` and `previous` are the same key");
  }
  return { current, previous };
}

function hmacKey(bytes: Uint8Array, usage: KeyUsage): Promise<CryptoKey> {
  return crypto.subtle.importKey("raw", bytes as BufferSource, { name: "HMAC", hash: "SHA-256" }, false, [usage]);
}

/**
 * Issue a session cookie value for `sub`, signed with `keys.secret`, valid for
 * `ttlSecs` from `nowMs`. `actor` is the declaring actor's name (the `aud`).
 * Throws `SessionKeyFault` for a bad key and `RangeError` for a bad `ttlSecs`
 * or an empty `sub`; the router turns either into a faulted request.
 */
export async function signSessionCookie(
  keys: SessionKeys,
  actor: string,
  sub: string,
  ttlSecs: number,
  nowMs: number = Date.now(),
): Promise<string> {
  if (!Number.isSafeInteger(ttlSecs) || ttlSecs <= 0) {
    throw new RangeError("a session ttl must be a positive whole number of seconds");
  }
  if (sub.length === 0) throw new RangeError("a session identity must not be empty");
  const { current } = await loadKeys(keys);
  const iat = Math.floor(nowMs / 1000);
  const head = toB64Url(enc.encode(JSON.stringify({ alg: "HS256", typ: SESSION_TYP, kid: current.kid })));
  const body = toB64Url(enc.encode(JSON.stringify({ sub, aud: actor, iat, exp: iat + ttlSecs })));
  const mac = new Uint8Array(
    await crypto.subtle.sign("HMAC", await hmacKey(current.bytes, "sign"), enc.encode(`${head}.${body}`) as BufferSource),
  );
  const value = `${head}.${body}.${toB64Url(mac)}`;
  if (value.length > MAX_COOKIE_CHARS) throw new RangeError("a session cookie value must fit in 4096 characters");
  return value;
}

function isRecord(x: unknown): x is Record<string, unknown> {
  return typeof x === "object" && x !== null && !Array.isArray(x);
}

function parseSegment(seg: string): Record<string, unknown> | null {
  const bytes = fromB64Url(seg);
  if (bytes === null) return null;
  try {
    const v: unknown = JSON.parse(dec.decode(bytes));
    return isRecord(v) ? v : null;
  } catch {
    return null;
  }
}

/**
 * Verify a presented session cookie for `actor`. `Ok({ sub })` only for a
 * well-formed cookie signed by `keys.secret` or `keys.previous` (selected by
 * its `kid`), made for this actor, issued no later than now (one second of
 * skew), unexpired, and whose lifetime (`exp - iat`) does not exceed
 * `ttlSecs`, so shortening a `ttl` shortens the cookies already out. Any
 * other cookie is an `Err` the router maps to `401`. A bad *key* throws
 * `SessionKeyFault`.
 */
export async function verifySignedCookie(
  value: string,
  keys: SessionKeys,
  actor: string,
  ttlSecs: number,
  nowMs: number = Date.now(),
): Promise<Result<{ readonly sub: string }, string>> {
  // The bound on a cookie's lifetime is the actor's `ttl`; one that is not a positive whole number
  // would turn it off (`NaN` compares false), so it is a fault, as it is for the signer.
  if (!Number.isSafeInteger(ttlSecs) || ttlSecs <= 0) {
    throw new RangeError("a session ttl must be a positive whole number of seconds");
  }
  const { current, previous } = await loadKeys(keys);
  if (value.length === 0 || value.length > MAX_COOKIE_CHARS) return Err("malformed cookie");
  const parts = value.split(".");
  if (parts.length !== 3) return Err("malformed cookie");
  const [headB64, bodyB64, macB64] = parts as [string, string, string];
  const header = parseSegment(headB64);
  if (header === null) return Err("malformed header");
  if (header.alg !== "HS256") return Err("unsupported alg");
  if (header.typ !== SESSION_TYP) return Err("wrong type");
  if (typeof header.kid !== "string") return Err("missing key id");
  const key = header.kid === current.kid ? current : previous !== null && header.kid === previous.kid ? previous : null;
  if (key === null) return Err("unknown key");
  const mac = fromB64Url(macB64);
  if (mac === null) return Err("malformed signature");
  let ok: boolean;
  try {
    ok = await crypto.subtle.verify(
      "HMAC",
      await hmacKey(key.bytes, "verify"),
      mac as BufferSource,
      enc.encode(`${headB64}.${bodyB64}`) as BufferSource,
    );
  } catch {
    return Err("verify failed");
  }
  if (!ok) return Err("bad signature");
  const body = parseSegment(bodyB64);
  if (body === null) return Err("malformed payload");
  const { sub, aud, iat, exp } = body;
  if (typeof sub !== "string" || sub.length === 0) return Err("missing sub");
  if (aud !== actor) return Err("wrong audience");
  if (typeof iat !== "number" || !Number.isInteger(iat)) return Err("malformed iat");
  if (typeof exp !== "number" || !Number.isInteger(exp)) return Err("malformed exp");
  const now = Math.floor(nowMs / 1000);
  if (iat > now + IAT_SKEW_SECS) return Err("issued in the future");
  if (exp <= iat) return Err("malformed lifetime");
  if (exp - iat > ttlSecs) return Err("lifetime exceeds ttl");
  if (exp <= now) return Err("cookie expired");
  return Ok({ sub });
}

// #1653: the `__` aliases emitted code imports.
export { newSessionKey as __newSessionKey, signSessionCookie as __signSessionCookie, verifySignedCookie as __verifySignedCookie };

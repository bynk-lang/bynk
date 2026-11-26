// #3939 (the API keys track, D1–D3): the `ApiKey` actor's key format, its pepper and its
// digests. Everything here runs at the seam or in a service handler; the generated agents
// never see a pepper or a secret, only the digests computed here.
//
//   <prefix>_<id>_<secret><checksum>
//
//   prefix    the actor's `prefix`, `[a-z][a-z0-9]{1,15}`
//   id        24 bytes, base62, 33 characters: a 16-byte random handle, then an 8-byte key number
//   secret    32 bytes from `crypto.getRandomValues`, base62, 43 characters
//   checksum  CRC32 of everything before it, base62, 6 characters
//
// Every field is big-endian base62, left-padded with `0` to its width, and a field whose value
// does not fit its byte count is malformed, so each key has exactly one encoding. The checksum
// is not a security control (anyone can compute it): it lets a scanner confirm a match offline
// and lets the seam refuse a mistyped key before any lookup.
//
// The stored value is `HMAC-SHA-256(pepper, secret)`. The pepper is base64url of at least 32
// bytes, read from the environment; a missing or bad one is `ApiKeyPepperFault`, a faulted
// request, and the fault names the variable, never its value.

/** The base62 alphabet: digit value `i` is `ALPHABET[i]`, so `0` is the pad character. */
const ALPHABET = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/** Field sizes, in bytes and in base62 characters. */
export const API_KEY_HANDLE_BYTES = 16;
export const API_KEY_NUMBER_BYTES = 8;
export const API_KEY_ID_BYTES = API_KEY_HANDLE_BYTES + API_KEY_NUMBER_BYTES;
export const API_KEY_SECRET_BYTES = 32;
export const API_KEY_ID_CHARS = 33;
export const API_KEY_SECRET_CHARS = 43;
export const API_KEY_CHECKSUM_CHARS = 6;

/** A presented key longer than this is refused before parsing (a well-formed key is at most 100). */
export const API_KEY_MAX_CHARS = 128;

/** The least pepper length, in bytes. */
export const API_KEY_PEPPER_MIN_BYTES = 32;

/** The digest a key number the store does not hold is compared against, so the work is the same. */
export const API_KEY_DUMMY_DIGEST: Uint8Array = new Uint8Array(32);

const enc = new TextEncoder();
const B64URL = /^[A-Za-z0-9_-]+$/;
const PREFIX = /^[a-z][a-z0-9]{1,15}$/;

/** A pepper that is missing, undecodable or too short: a configuration fault, not a bad key. */
export class ApiKeyPepperFault extends Error {
  constructor(message: string) {
    super(message);
    this.name = "ApiKeyPepperFault";
  }
}

/** The peppers an `ApiKey` actor holds, read from the environment: anything but a string is "not set". */
export interface ApiKeyPeppers {
  readonly secret: unknown;
  readonly previous?: unknown;
}

/** One pepper, decoded, with the `kid` a record names it by. */
export interface ApiKeyPepper {
  readonly kid: string;
  readonly bytes: Uint8Array;
}

/** A key's digest under one pepper, as the agent receives it. */
export interface ApiKeyDigest {
  readonly kid: string;
  readonly digest: Uint8Array;
}

/** A parsed key: its public id, the store it names, the key number within it and the secret. */
export interface ParsedApiKey {
  /** The public id, `<prefix>_<33 characters>`, safe to log and to pass to `revoke`. */
  readonly id: string;
  /** The handle, lower-case hex (32 characters): the key of `<Actor>Keys`. */
  readonly handle: string;
  /** The key number, lower-case hex (16 characters): the record's key within the store. */
  readonly keyNo: string;
  readonly secret: Uint8Array;
}

// ---------------------------------------------------------------------------------------------
// Base62, fixed width
// ---------------------------------------------------------------------------------------------

/** `bytes` as big-endian base62, left-padded with `0` to exactly `width` characters. */
export function base62Encode(bytes: Uint8Array, width: number): string {
  let n = 0n;
  for (const b of bytes) n = (n << 8n) | BigInt(b);
  let out = "";
  while (n > 0n) {
    out = ALPHABET[Number(n % 62n)] + out;
    n /= 62n;
  }
  if (out.length > width) throw new RangeError("bynk: a base62 field does not fit its width");
  return out.padStart(width, "0");
}

/**
 * The `byteCount` bytes `s` encodes, or `null` unless `s` is exactly `width` base62 characters
 * whose value fits in `byteCount` bytes. So every value has exactly one accepted encoding.
 */
export function base62Decode(s: string, width: number, byteCount: number): Uint8Array | null {
  if (s.length !== width) return null;
  let n = 0n;
  for (const ch of s) {
    const v = ALPHABET.indexOf(ch);
    if (v < 0) return null;
    n = n * 62n + BigInt(v);
  }
  if (n >> BigInt(byteCount * 8) !== 0n) return null;
  const out = new Uint8Array(byteCount);
  for (let i = byteCount - 1; i >= 0; i--) {
    out[i] = Number(n & 0xffn);
    n >>= 8n;
  }
  return out;
}

// ---------------------------------------------------------------------------------------------
// CRC32 (IEEE 802.3, reflected, polynomial 0xEDB88320)
// ---------------------------------------------------------------------------------------------

let crcTable: Uint32Array | null = null;

function table(): Uint32Array {
  if (crcTable !== null) return crcTable;
  const t = new Uint32Array(256);
  for (let i = 0; i < 256; i++) {
    let c = i;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[i] = c >>> 0;
  }
  crcTable = t;
  return t;
}

/** The CRC32 of `bytes`, as 4 big-endian bytes. */
export function crc32(bytes: Uint8Array): Uint8Array {
  const t = table();
  let c = 0xffffffff;
  for (const b of bytes) c = t[(c ^ b) & 0xff] ^ (c >>> 8);
  c = (c ^ 0xffffffff) >>> 0;
  return new Uint8Array([c >>> 24, (c >>> 16) & 0xff, (c >>> 8) & 0xff, c & 0xff]);
}

function checksumOf(body: string): string {
  return base62Encode(crc32(enc.encode(body)), API_KEY_CHECKSUM_CHARS);
}

// ---------------------------------------------------------------------------------------------
// Format and parse
// ---------------------------------------------------------------------------------------------

function hex(bytes: Uint8Array): string {
  let s = "";
  for (const b of bytes) s += b.toString(16).padStart(2, "0");
  return s;
}

function fromHex(s: string, byteCount: number): Uint8Array {
  if (!/^[0-9a-f]*$/.test(s) || s.length !== byteCount * 2) {
    throw new RangeError("bynk: an API key handle or key number is not hex of the right length");
  }
  const out = new Uint8Array(byteCount);
  for (let i = 0; i < byteCount; i++) out[i] = parseInt(s.slice(i * 2, i * 2 + 2), 16);
  return out;
}

/** The public id of the key numbered `keyNo` (hex) in the store `handle` (hex), under `prefix`. */
export function apiKeyIdOf(prefix: string, handle: string, keyNo: string): string {
  const id = new Uint8Array(API_KEY_ID_BYTES);
  id.set(fromHex(handle, API_KEY_HANDLE_BYTES), 0);
  id.set(fromHex(keyNo, API_KEY_NUMBER_BYTES), API_KEY_HANDLE_BYTES);
  return `${prefix}_${base62Encode(id, API_KEY_ID_CHARS)}`;
}

/** The whole key string for `secret` under the id `apiKeyIdOf` gives. */
export function formatApiKey(prefix: string, handle: string, keyNo: string, secret: Uint8Array): string {
  if (secret.length !== API_KEY_SECRET_BYTES) throw new RangeError("bynk: an API key secret is 32 bytes");
  const body = `${apiKeyIdOf(prefix, handle, keyNo)}_${base62Encode(secret, API_KEY_SECRET_CHARS)}`;
  return body + checksumOf(body);
}

/** The handle and key number a public id names, or `null` when it is not an id under `prefix`. */
export function parseApiKeyId(id: string, prefix: string): { handle: string; keyNo: string } | null {
  if (typeof id !== "string" || id.length !== prefix.length + 1 + API_KEY_ID_CHARS) return null;
  if (!id.startsWith(`${prefix}_`)) return null;
  const bytes = base62Decode(id.slice(prefix.length + 1), API_KEY_ID_CHARS, API_KEY_ID_BYTES);
  if (bytes === null) return null;
  return {
    handle: hex(bytes.slice(0, API_KEY_HANDLE_BYTES)),
    keyNo: hex(bytes.slice(API_KEY_HANDLE_BYTES)),
  };
}

/**
 * D3, step 2: parse a presented key for the actor whose prefix is `prefix`. `null` for anything
 * that is not exactly one well-formed key: too long, another prefix, a missing separator, a
 * character outside base62, a field of the wrong width or too large for its bytes, or a checksum
 * that does not match. A `null` is refused with no lookup.
 */
export function parseApiKey(raw: string, prefix: string): ParsedApiKey | null {
  if (typeof raw !== "string" || raw.length > API_KEY_MAX_CHARS) return null;
  const idLen = prefix.length + 1 + API_KEY_ID_CHARS;
  if (raw.length !== idLen + 1 + API_KEY_SECRET_CHARS + API_KEY_CHECKSUM_CHARS) return null;
  if (raw[idLen] !== "_") return null;
  const id = raw.slice(0, idLen);
  const parts = parseApiKeyId(id, prefix);
  if (parts === null) return null;
  const secretText = raw.slice(idLen + 1, idLen + 1 + API_KEY_SECRET_CHARS);
  const secret = base62Decode(secretText, API_KEY_SECRET_CHARS, API_KEY_SECRET_BYTES);
  if (secret === null) return null;
  const checksum = raw.slice(idLen + 1 + API_KEY_SECRET_CHARS);
  if (base62Decode(checksum, API_KEY_CHECKSUM_CHARS, 4) === null) return null;
  if (checksumOf(raw.slice(0, idLen + 1 + API_KEY_SECRET_CHARS)) !== checksum) return null;
  return { id, handle: parts.handle, keyNo: parts.keyNo, secret };
}

/** Whether `prefix` is a legal `ApiKey` prefix (the checker enforces this; the runtime re-checks). */
export function isApiKeyPrefix(prefix: string): boolean {
  return PREFIX.test(prefix);
}

// ---------------------------------------------------------------------------------------------
// The credential on the request
// ---------------------------------------------------------------------------------------------

/**
 * D3, step 1: the presented key, or `null` when it is absent or sent more than once. With
 * `header` the raw value of that header; without, the token of `Authorization: Bearer <key>`.
 * `request.headers.get` joins repeated fields with `, `, and a key never holds `,` or a space,
 * so a joined value is refused here or at parsing, never read as one key.
 */
export function apiKeyCredentialOf(request: Request, header: string | null): string | null {
  const raw = request.headers.get(header ?? "Authorization");
  if (raw === null) return null;
  if (raw.includes(",")) return null;
  if (header !== null) return raw === "" ? null : raw;
  if (!raw.startsWith("Bearer ")) return null;
  const token = raw.slice(7);
  return token === "" || token.includes(" ") ? null : token;
}

// ---------------------------------------------------------------------------------------------
// Peppers and digests
// ---------------------------------------------------------------------------------------------

function toB64Url(bytes: Uint8Array): string {
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

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

/** A fresh pepper: 32 bytes from the platform CSPRNG, base64url (what `bynk deploy` mints). */
export function newApiKeyPepper(): string {
  return toB64Url(crypto.getRandomValues(new Uint8Array(API_KEY_PEPPER_MIN_BYTES)));
}

/** Raise a pepper fault, logging it first (the router's fault log carries only an error's name).
 *  The message names the variable and never contains the pepper. */
function pepperFault(message: string): never {
  console.error(`bynk: ${message}`);
  throw new ApiKeyPepperFault(message);
}

function decodePepper(name: string, value: unknown): Uint8Array {
  if (typeof value !== "string" || value === "") pepperFault(`the API key pepper \`${name}\` is not set`);
  const bytes = fromB64Url(value);
  if (bytes === null) pepperFault(`the API key pepper \`${name}\` is not base64url`);
  if (bytes.length < API_KEY_PEPPER_MIN_BYTES) {
    pepperFault(
      `the API key pepper \`${name}\` is ${bytes.length} bytes; it must be at least ${API_KEY_PEPPER_MIN_BYTES}`,
    );
  }
  return bytes;
}

/** The id a record names its pepper by: the first 6 bytes of SHA-256 of the pepper, base64url. */
async function pepperKid(bytes: Uint8Array): Promise<string> {
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", bytes as BufferSource));
  return toB64Url(digest.slice(0, 6));
}

/**
 * The actor's peppers, decoded: `secret` first, then `previous` while it is set. `names` are the
 * environment variables, for the fault message. Throws `ApiKeyPepperFault` for a bad pepper, and
 * for a `previous` equal to `secret`.
 */
export async function loadApiKeyPeppers(
  peppers: ApiKeyPeppers,
  names: { readonly secret: string; readonly previous?: string },
): Promise<readonly ApiKeyPepper[]> {
  const current = decodePepper(names.secret, peppers.secret);
  const out: ApiKeyPepper[] = [{ kid: await pepperKid(current), bytes: current }];
  const prev = peppers.previous;
  if (prev === undefined || prev === null || prev === "") return out;
  const previous = decodePepper(names.previous ?? "previous", prev);
  const kid = await pepperKid(previous);
  if (kid === out[0].kid) pepperFault(`the API key peppers \`${names.secret}\` and \`${names.previous}\` are the same`);
  out.push({ kid, bytes: previous });
  return out;
}

/** `HMAC-SHA-256(pepper, secret)`. */
async function hmac(pepper: Uint8Array, secret: Uint8Array): Promise<Uint8Array> {
  const key = await crypto.subtle.importKey("raw", pepper as BufferSource, { name: "HMAC", hash: "SHA-256" }, false, [
    "sign",
  ]);
  return new Uint8Array(await crypto.subtle.sign("HMAC", key, secret as BufferSource));
}

/** D2: the secret's digest under each pepper, `secret`'s first. The agent compares the one its
 *  record's `kid` names, so the pepper never leaves the seam. */
export async function apiKeyDigests(peppers: readonly ApiKeyPepper[], secret: Uint8Array): Promise<ApiKeyDigest[]> {
  const out: ApiKeyDigest[] = [];
  for (const p of peppers) out.push({ kid: p.kid, digest: await hmac(p.bytes, secret) });
  return out;
}

/** A fresh key's random parts, before it has a store: a candidate handle (used only when the
 *  identity has none yet), a key number and the secret, with the secret's digest under `secret`. */
export interface FreshApiKey {
  readonly handle: string;
  readonly keyNo: string;
  readonly secret: Uint8Array;
  readonly kid: string;
  readonly digest: Uint8Array;
}

/** The random parts of a new key and its digest under the current pepper. */
export async function freshApiKey(peppers: readonly ApiKeyPepper[]): Promise<FreshApiKey> {
  const secret = crypto.getRandomValues(new Uint8Array(API_KEY_SECRET_BYTES));
  const [current] = peppers;
  return {
    handle: newApiKeyHandle(),
    keyNo: hex(crypto.getRandomValues(new Uint8Array(API_KEY_NUMBER_BYTES))),
    secret,
    kid: current.kid,
    digest: await hmac(current.bytes, secret),
  };
}

/** A fresh random handle, lower-case hex: a new `<Actor>Keys` instance. */
export function newApiKeyHandle(): string {
  return hex(crypto.getRandomValues(new Uint8Array(API_KEY_HANDLE_BYTES)));
}

// ---------------------------------------------------------------------------------------------
// The seam's arguments to `<Actor>Keys.verify`
// ---------------------------------------------------------------------------------------------

/** The presented digests by `kid`, as `verify` takes them: the store compares the one its record's
 *  `kid` names. */
export function apiKeyPresented(digests: readonly ApiKeyDigest[]): ReadonlyMap<string, Uint8Array> {
  return new Map(digests.map((d) => [d.kid, d.digest] as const));
}

/** The digest under the current pepper: what `verify` compares the dummy digest against when the
 *  store holds no record the presented key can match, so the work is the same (D3 B). */
export function apiKeyFirstDigest(digests: readonly ApiKeyDigest[]): Uint8Array {
  return digests[0].digest;
}

/** The fixed dummy digest (32 zero bytes). An HMAC-SHA-256 output equals it with probability
 *  2^-256, and `verify` also refuses whenever it compared the dummy, so it never admits. */
export function apiKeyDummyDigest(): Uint8Array {
  return API_KEY_DUMMY_DIGEST;
}

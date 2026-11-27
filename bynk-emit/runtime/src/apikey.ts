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

import { BynkSecret } from "./secret.ts";
import { constantTimeEqualBytes, platformTimingSafeEqualBytes } from "./bytes.ts";

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
// The key store's comparison
// ---------------------------------------------------------------------------------------------

/** `ApiKey.equal(a, b)` in a generated key store: the runtime's one constant-time comparison,
 *  the function `bynk.crypto`'s `Crypto.equal` calls, given this platform's primitive. */
export function apiKeyEqual(a: Uint8Array, b: Uint8Array): Promise<boolean> {
  return constantTimeEqualBytes(a, b, platformTimingSafeEqualBytes());
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

/** #3939 (slice 3): the `kid` of the pepper now live (`secret`'s). A record `verify` admits under
 *  any other is re-hashed under this one, in the same commit (§7). */
export function apiKeyCurrentKid(digests: readonly ApiKeyDigest[]): string {
  return digests[0].kid;
}

/** #3939 (slice 3): the `kid`s of the peppers now held, `secret`'s first, as the store records them. */
export function apiKeyLiveKids(digests: readonly ApiKeyDigest[]): string[] {
  return digests.map((d) => d.kid);
}

/** #3939 (slice 3): the time the seam passes the key store, for expiry and last use: the harness
 *  clock when the run sets one (`env.__bynkClock`), else the wall clock. */
export async function apiKeyNow(env: unknown): Promise<number> {
  const clock = (env as { __bynkClock?: { now(): Promise<number> | number } } | null | undefined)?.__bynkClock;
  return clock === undefined ? Date.now() : await clock.now();
}

/** The fixed dummy digest (32 zero bytes). An HMAC-SHA-256 output equals it with probability
 *  2^-256, and `verify` also refuses whenever it compared the dummy, so it never admits. */
export function apiKeyDummyDigest(): Uint8Array {
  return API_KEY_DUMMY_DIGEST;
}

// ---------------------------------------------------------------------------------------------
// `Actor.mint(identity, label)`
// ---------------------------------------------------------------------------------------------

/**
 * The peppers of the `ApiKey` actor `actor`, threaded in `deps.__apiKey` by the Worker's compose
 * from the environment. `callDurableObjectMethod` strips `__apiKey` from the `deps` an agent call
 * sends, so a pepper never reaches an agent (D2 B).
 */
export function apiKeyPeppersOf(deps: unknown, actor: string): ApiKeyPeppers {
  const all = (deps as { __apiKey?: Record<string, ApiKeyPeppers> } | null | undefined)?.__apiKey;
  const peppers = all?.[actor];
  if (peppers === undefined) pepperFault(`the API key pepper for \`${actor}\` is not available on this target`);
  return peppers;
}

/** What `<Actor>KeyOwner.mint` is given: a fresh key's parts, under a candidate handle. */
export interface MintRequest {
  readonly handle: string;
  readonly keyNo: string;
  readonly digest: Uint8Array;
  readonly kid: string;
  /** #3939 (slice 3): when the key expires (`now + ttl`), or `None` for an actor with no `ttl`. */
  readonly until: { readonly tag: "Some"; readonly value: number } | { readonly tag: "None" };
  /** #3939 (slice 3, §7): the `kid`s of the peppers now held, `secret`'s first. The store prunes
   *  each record under neither, with the expired ones, before it checks the cap. */
  readonly live: readonly string[];
}

/** What `mintApiKey` reads beyond the pepper: the actor's `ttl` and its `previous` variable. */
export interface MintOptions {
  /** The actor's `ttl` in milliseconds; absent or `null`, keys do not expire. */
  readonly ttlMs?: number | null;
  /** The name of the actor's `previous` variable, for the fault message. */
  readonly previous?: string;
}

/**
 * `Actor.mint(identity, label)`: make a key, have the identity's `KeyOwner` store its digest
 * (`store`, which answers the handle it used or the store's refusal), and answer the key once.
 * The secret is drawn and hashed here, in the service handler where the pepper is, and only its
 * digest goes to the store. The key comes back as a `BynkSecret`, which renders as `<secret>`
 * everywhere but the one wire encoding of the `ApiKeySecret` a handler returns.
 */
export async function mintApiKey<R>(
  peppers: ApiKeyPeppers,
  pepperName: string,
  prefix: string,
  label: string,
  nowMs: number,
  limit: number,
  store: (req: MintRequest) => Promise<{ readonly tag: "Ok"; readonly value: string } | { readonly tag: "Err"; readonly error: number }>,
  opts: MintOptions = {},
): Promise<R> {
  const held = await loadApiKeyPeppers(peppers, { secret: pepperName, previous: opts.previous });
  const fresh = await freshApiKey(held);
  const ttl = opts.ttlMs ?? null;
  const until = ttl === null ? ({ tag: "None" } as const) : ({ tag: "Some", value: nowMs + ttl } as const);
  const stored = await store({
    handle: fresh.handle,
    keyNo: fresh.keyNo,
    digest: fresh.digest,
    kid: fresh.kid,
    until,
    live: held.map((p) => p.kid),
  });
  // `R` is the call's checked `Result[ApiKeySecret, TooManyKeys]`, whose `ApiKeyId` and
  // `ApiKeyLabel` are brands: the id is well-formed by construction and the label was checked
  // as an `ApiKeyLabel` before the call, so the structural value is that type.
  if (stored.tag === "Err") return { tag: "Err", error: { limit } } as unknown as R;
  const handle = stored.value;
  const minted = {
    tag: "Ok",
    value: {
      id: apiKeyIdOf(prefix, handle, fresh.keyNo),
      key: new BynkSecret(formatApiKey(prefix, handle, fresh.keyNo, fresh.secret)),
      label,
      created: nowMs,
      expires: until,
    },
  };
  return minted as unknown as R;
}

// ---------------------------------------------------------------------------------------------
// `Actor.list(identity)`, `Actor.revoke(identity, id)` and `Actor.revokeAll(identity)` (slice 2)
// ---------------------------------------------------------------------------------------------

/** An `Option[String]` as an agent call answers it. */
type HandleOption = { readonly tag: "Some"; readonly value: string } | { readonly tag: "None" };

/** The generated `<Actor>Keys` handlers `list` and `sweep` call. */
export interface ApiKeyStore {
  expire(now: number, deps: unknown): Promise<number>;
  labelsOf(deps: unknown): Promise<ReadonlyMap<string, string>>;
  createdOf(deps: unknown): Promise<ReadonlyMap<string, number>>;
  expiresOf(deps: unknown): Promise<ReadonlyMap<string, number>>;
  usedOf(deps: unknown): Promise<ReadonlyMap<string, number>>;
  kidsOf(deps: unknown): Promise<ReadonlyMap<string, string>>;
  knownOf(deps: unknown): Promise<readonly string[]>;
  sweep(now: number, live: readonly string[], current: string, deps: unknown): Promise<number>;
}

/**
 * #3939 (slice 3): the `kid`s of the actor's peppers now held, `secret`'s first, when the pepper
 * reaches this handler (a service handler's `deps.__apiKey`), else `null` (an agent's handler).
 */
export async function apiKeyLiveKidsOf(
  deps: unknown,
  actor: string,
  names: { readonly secret: string; readonly previous?: string },
): Promise<string[] | null> {
  const peppers = (deps as { __apiKey?: Record<string, ApiKeyPeppers> } | null | undefined)?.__apiKey?.[actor];
  if (peppers === undefined) return null;
  return (await loadApiKeyPeppers(peppers, names)).map((p) => p.kid);
}

const PEPPER_CURRENT = { tag: "Current" } as const;
const PEPPER_PREVIOUS = { tag: "Previous" } as const;
const PEPPER_RETIRED = { tag: "Retired" } as const;

/** Which pepper a record's `kid` names, among the live ones (`secret`'s first). With none known
 *  (a store no key has reached since slice 3), every key reads as `Current`. */
function pepperOf(kid: string | undefined, live: readonly string[]) {
  if (live.length === 0) return PEPPER_CURRENT;
  if (kid === live[0]) return PEPPER_CURRENT;
  if (kid !== undefined && live.includes(kid)) return PEPPER_PREVIOUS;
  return PEPPER_RETIRED;
}

/**
 * `list`: the identity's keys, as `ApiKeyInfo` values. Expired keys are removed first (§9). Each
 * key's `pepper` is read against `live`, the peppers this handler holds; in an agent's handler,
 * which holds none, against the `kid`s the store last saw (its last mint, sweep or admitted key).
 * No handle, or a retired store, is no keys. Only a key in both the label and creation maps is
 * listed, so a key revoked between the reads is left out, never half-shown. Never a secret or a
 * digest.
 */
export async function apiKeyList<R>(
  prefix: string,
  handle: HandleOption,
  nowMs: number,
  live: readonly string[] | null,
  store: (handle: string) => ApiKeyStore,
  deps: unknown,
): Promise<R> {
  if (handle.tag !== "Some") return [] as unknown as R;
  const h = handle.value;
  const keys = store(h);
  await keys.expire(nowMs, deps);
  const labels = await keys.labelsOf(deps);
  const created = await keys.createdOf(deps);
  const expires = await keys.expiresOf(deps);
  const used = await keys.usedOf(deps);
  const kids = await keys.kidsOf(deps);
  const against = live ?? (await keys.knownOf(deps));
  const some = (v: number | undefined) => (v === undefined ? { tag: "None" } : { tag: "Some", value: v });
  const out: unknown[] = [];
  for (const [keyNo, label] of labels) {
    const at = created.get(keyNo);
    if (at === undefined) continue;
    out.push({
      id: apiKeyIdOf(prefix, h, keyNo),
      label,
      created: at,
      expires: some(expires.get(keyNo)),
      lastUsed: some(used.get(keyNo)),
      pepper: pepperOf(kids.get(keyNo), against),
    });
  }
  return out as unknown as R;
}

/**
 * #3939 (slice 3): `sweep`: remove the identity's expired keys and its keys under neither pepper,
 * and answer how many remain under `previous`, the keys dropping `previous` now would end. `live`
 * comes from the pepper, which a service handler holds; `sweep` is legal nowhere else.
 */
export async function apiKeySweep(
  handle: HandleOption,
  nowMs: number,
  live: readonly string[] | null,
  store: (handle: string) => ApiKeyStore,
  deps: unknown,
): Promise<number> {
  if (live === null) pepperFault("`sweep` needs the API key pepper, which reaches a service handler only");
  if (handle.tag !== "Some") return 0;
  return store(handle.value).sweep(nowMs, live, live[0], deps);
}

/**
 * `revoke`: `false` for an id that is not one of this actor's (a bad form or another prefix),
 * else what the identity's `<Actor>KeyOwner.revoke(handle, keyNo)` answers, which is `false` for
 * another identity's key or one already gone.
 */
export async function apiKeyRevoke(
  prefix: string,
  id: string,
  revoke: (handle: string, keyNo: string) => Promise<boolean>,
): Promise<boolean> {
  const parsed = parseApiKeyId(id, prefix);
  if (parsed === null) return false;
  return revoke(parsed.handle, parsed.keyNo);
}

// ---------------------------------------------------------------------------------------------
// The `system` test tier (track §10). These run only in a generated test module: they mint a real
// key into the in-process key store, build the keys the failure principals present, and count the
// key store's comparisons so a case can tell the seam did the same work (D3 B).
// ---------------------------------------------------------------------------------------------

/** The key `actor`'s principal `sub` presents in this test run: minted once per harness `env`,
 *  through `store` (the participant's in-process `<Actor>KeyOwner.mint`), under `peppers`, with
 *  the per-identity ceiling `limit` the generated store enforces. */
export async function apiKeyForTest(
  env: Record<string, unknown>,
  actor: string,
  sub: string,
  peppers: ApiKeyPeppers,
  pepperName: string,
  prefix: string,
  nowMs: number,
  limit: number,
  store: (req: MintRequest) => Promise<{ readonly tag: "Ok"; readonly value: string } | { readonly tag: "Err"; readonly error: number }>,
  opts: MintOptions = {},
): Promise<string> {
  const memo = (env.__bynkApiKeys ??= new Map<string, string>()) as Map<string, string>;
  const k = `${actor}:${sub}`;
  let key = memo.get(k);
  if (key === undefined) {
    const minted = await mintApiKey<{ readonly tag: "Ok"; readonly value: { readonly key: BynkSecret } } | { readonly tag: "Err" }>(
      peppers,
      pepperName,
      prefix,
      "bynk-test",
      nowMs,
      limit,
      store,
      opts,
    );
    if (minted.tag !== "Ok") throw new Error(`bynk: the test identity \`${sub}\` holds the most keys it may`);
    key = minted.value.key.reveal();
    memo.set(k, key);
  }
  return key;
}

/**
 * The key a failure principal presents, made from a real minted key `real`:
 *
 * - `malformed`: the real key with its last checksum character changed (refused before lookup);
 * - `unknownkey`: the real key's handle with a fresh key number and secret (never minted);
 * - `unknownhandle`: a fresh handle, key number and secret (no store holds the handle);
 * - `forged`: the real key's id with a fresh secret the store does not hold.
 *
 * Every key but `malformed` is well formed, its checksum recomputed, so the seam reaches the store.
 */
export function apiKeyBadKey(kind: string, real: string, prefix: string): string {
  const parsed = parseApiKey(real, prefix);
  if (parsed === null) throw new Error("bynk: the test key is not a key of this actor");
  const secret = crypto.getRandomValues(new Uint8Array(API_KEY_SECRET_BYTES));
  const keyNo = hex(crypto.getRandomValues(new Uint8Array(API_KEY_NUMBER_BYTES)));
  switch (kind) {
    case "malformed": {
      const last = real.at(-1) === "0" ? "1" : "0";
      return real.slice(0, -1) + last;
    }
    case "unknownkey":
      return formatApiKey(prefix, parsed.handle, keyNo, secret);
    case "unknownhandle":
      return formatApiKey(prefix, newApiKeyHandle(), keyNo, secret);
    case "forged":
      return formatApiKey(prefix, parsed.handle, parsed.keyNo, secret);
    default:
      throw new Error(`bynk: no such bad API key \`${kind}\``);
  }
}

/** `by Revoked` (slice 2): the real key `real`, revoked through `revoke` (the participant's
 *  in-process `<Actor>KeyOwner.revoke`) before it is presented. Revoking deletes the record, so the
 *  seam then compares the key once against the dummy digest, as for a key never minted. */
export async function apiKeyRevokedForTest(
  real: string,
  prefix: string,
  revoke: (handle: string, keyNo: string) => Promise<boolean>,
): Promise<string> {
  const parsed = parseApiKey(real, prefix);
  if (parsed === null) throw new Error("bynk: the test key is not a key of this actor");
  await revoke(parsed.handle, parsed.keyNo);
  return real;
}

/** The key store's comparisons so far: every `Crypto.equal`, and those against the dummy digest. */
export interface ApiKeyCompares {
  all: number;
  dummy: number;
}

const COUNTED = Symbol.for("bynk.apiKeyCompares");

function isDummy(b: unknown): boolean {
  return b instanceof Uint8Array && b.length === API_KEY_DUMMY_DIGEST.length && b.every((x) => x === 0);
}

/** Count every comparison the key store makes, by wrapping the platform primitive `apiKeyEqual`
 *  reaches on Node (`node:crypto`'s `timingSafeEqual`, through `process.getBuiltinModule`), once
 *  per process. The test tier runs on Node, so every key-store comparison passes through it. */
export function apiKeyCountCompares(): ApiKeyCompares {
  const proc = (globalThis as unknown as { process?: Record<PropertyKey, unknown> & { getBuiltinModule?: (id: string) => unknown } })
    .process;
  if (proc === undefined || typeof proc.getBuiltinModule !== "function") {
    throw new Error("bynk: the API key comparison counter needs Node's process.getBuiltinModule");
  }
  const existing = proc[COUNTED] as ApiKeyCompares | undefined;
  if (existing !== undefined) return existing;
  const counts: ApiKeyCompares = { all: 0, dummy: 0 };
  const load = proc.getBuiltinModule.bind(proc);
  proc.getBuiltinModule = (id: string): unknown => {
    const m = load(id);
    if (id !== "node:crypto" || m === null || typeof m !== "object") return m;
    const native = (m as { timingSafeEqual: (a: Uint8Array, b: Uint8Array) => boolean }).timingSafeEqual;
    return new Proxy(m as object, {
      get(target, key, receiver) {
        if (key !== "timingSafeEqual") return Reflect.get(target, key, receiver);
        return (a: Uint8Array, b: Uint8Array): boolean => {
          counts.all += 1;
          if (isDummy(a) || isDummy(b)) counts.dummy += 1;
          return native(a, b);
        };
      },
    });
  };
  proc[COUNTED] = counts;
  return counts;
}

/** Fail the case unless the request that ran since `before` made exactly the comparisons `want`. */
export function apiKeyExpectCompares(
  counts: ApiKeyCompares,
  before: { readonly all: number; readonly dummy: number },
  want: { readonly all: number; readonly dummy: number },
  principal: string,
): void {
  const all = counts.all - before.all;
  const dummy = counts.dummy - before.dummy;
  if (all !== want.all || dummy !== want.dummy) {
    throw new Error(
      `bynk: \`by ${principal}\` made the key store compare ${all} time(s), ${dummy} against the dummy digest; the seam must compare ${want.all} time(s), ${want.dummy} against the dummy`,
    );
  }
}

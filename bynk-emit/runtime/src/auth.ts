import { Ok, Err, type Result, type Option } from "./result.ts";

// v0.47: Bearer-token verification (the actors slice-2 seam). Verifies a JWT's
// HS256 signature against `secret` using WebCrypto (constant-time
// `crypto.subtle.verify`), enforces `exp`/`nbf`, and returns the `sub` claim.
// Any failure is an `Err` the caller maps to 401 — fail-closed. The raw token
// never leaves this function; only the verified `sub` flows out.
function __bynkB64UrlToBytes(s: string): Uint8Array {
  const b64 = s.replace(/-/g, "+").replace(/_/g, "/").padEnd(Math.ceil(s.length / 4) * 4, "=");
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

export async function verifyBearerJwtHs256(
  token: string,
  secret: string,
): Promise<Result<{ readonly sub: string; readonly claims: Record<string, unknown> }, string>> {
  const parts = token.split(".");
  if (parts.length !== 3) return Err("malformed token");
  const [headerB64, payloadB64, sigB64] = parts;
  let header: { alg?: unknown };
  try {
    header = JSON.parse(new TextDecoder().decode(__bynkB64UrlToBytes(headerB64)));
  } catch {
    return Err("malformed header");
  }
  // Reject algorithm confusion / `alg: none` — this seam only verifies HS256.
  if (header.alg !== "HS256") return Err("unsupported alg");
  const enc = new TextEncoder();
  const key = await crypto.subtle.importKey(
    "raw",
    enc.encode(secret) as BufferSource,
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["verify"],
  );
  let ok: boolean;
  try {
    ok = await crypto.subtle.verify(
      "HMAC",
      key,
      __bynkB64UrlToBytes(sigB64) as BufferSource,
      enc.encode(`${headerB64}.${payloadB64}`) as BufferSource,
    );
  } catch {
    return Err("verify failed");
  }
  if (!ok) return Err("bad signature");
  let payload: { sub?: unknown; exp?: unknown; nbf?: unknown };
  try {
    payload = JSON.parse(new TextDecoder().decode(__bynkB64UrlToBytes(payloadB64)));
  } catch {
    return Err("malformed payload");
  }
  const now = Math.floor(Date.now() / 1000);
  // RFC 7519: `exp`/`nbf` are NumericDate (a number). `exp` is required — a
  // token with no expiry never ages out, so a leaked bearer cannot be revoked
  // by time; parity with the OIDC seam below. A present-but-non-number `exp`
  // (or `nbf`) is malformed — reject rather than silently skip the time check.
  if (payload.exp === undefined) return Err("missing exp");
  if (typeof payload.exp !== "number") return Err("malformed exp");
  if ((payload.exp as number) < now) return Err("token expired");
  if (payload.nbf !== undefined && typeof payload.nbf !== "number") return Err("malformed nbf");
  if (payload.nbf !== undefined && (payload.nbf as number) > now) return Err("token not yet valid");
  if (typeof payload.sub !== "string" || payload.sub.length === 0) return Err("missing sub");
  // v0.53: surface the full verified claims for refinement-actor authorisation
  // (`actor Admin = User where hasClaim(...)`). The identity stays `sub`-minted
  // and sealed; claims are an authorisation-time input, checked at the boundary.
  return Ok({ sub: payload.sub, claims: payload as Record<string, unknown> });
}

// v0.51: Signature (webhook) verification — recompute an HMAC-SHA256 over the
// raw body (or `<timestamp>.<body>` when a timestamp is bound) and compare it,
// constant-time (`crypto.subtle.verify`), against the request's signature
// header (accepting a `sha256=<hex>` prefix or a bare hex digest). When a
// timestamp is bound, it is signed (so it cannot be forged) and checked within
// `toleranceSecs` for replay defence. Returns `true` iff the request is
// authentic; the caller maps `false` to 401. The body never reaches the handler
// unverified.
function __bynkHexToBytes(hex: string): Uint8Array {
  const clean = hex.startsWith("sha256=") ? hex.slice(7) : hex;
  if (clean.length === 0 || clean.length % 2 !== 0 || /[^0-9a-fA-F]/.test(clean)) {
    return new Uint8Array(0);
  }
  const out = new Uint8Array(clean.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(clean.slice(i * 2, i * 2 + 2), 16);
  return out;
}

export async function verifySignatureHmacSha256(
  body: string | Uint8Array,
  secret: string,
  signatureHeader: string | null,
  timestamp: string | null,
  toleranceSecs: number | null,
): Promise<boolean> {
  if (signatureHeader === null) return false;
  // When a timestamp is bound it is part of the signed string (so it cannot be
  // forged); a tolerance, if set, bounds replay.
  // A `@raw` route signs the octets as received (`Uint8Array`), a JSON route
  // the text read from them (`string`); the timestamp prefix is the same bytes
  // either way.
  const enc = new TextEncoder();
  let signed: Uint8Array = typeof body === "string" ? enc.encode(body) : body;
  if (timestamp !== null) {
    const ts = Number(timestamp);
    if (!Number.isFinite(ts)) return false;
    if (toleranceSecs !== null) {
      const now = Math.floor(Date.now() / 1000);
      if (Math.abs(now - ts) > toleranceSecs) return false;
    }
    if (typeof body === "string") {
      signed = enc.encode(`${timestamp}.${body}`);
    } else {
      const prefix = enc.encode(`${timestamp}.`);
      signed = new Uint8Array(prefix.length + body.length);
      signed.set(prefix, 0);
      signed.set(body, prefix.length);
    }
  }
  const sigBytes = __bynkHexToBytes(signatureHeader);
  if (sigBytes.length === 0) return false;
  const key = await crypto.subtle.importKey(
    "raw",
    enc.encode(secret) as BufferSource,
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["verify"],
  );
  try {
    return await crypto.subtle.verify(
      "HMAC",
      key,
      sigBytes as BufferSource,
      signed as BufferSource,
    );
  } catch {
    return false;
  }
}

// v0.151: OIDC/JWKS verification (the actors OIDC slice). Verifies an
// asymmetrically-signed JWT (RS256 or ES256) against the provider's published
// key set: fetch the JWKS (cached, refetched once on a `kid` miss so key
// rotation heals without a redeploy), import the matching public key via
// WebCrypto, verify the signature (`crypto.subtle.verify`), then enforce the
// trust contract — `iss` equals the declared issuer, `aud` contains the
// declared audience, `exp` is present and in the future, `nbf` (if present) has
// passed — and return the `sub` claim. Any failure is an `Err` the caller maps
// to 401 — fail-closed. Unlike the Bearer/Signature seams there is **no shared
// secret**: the trust root is the provider's public JWKS. `alg: none` and
// symmetric algorithms (HS*) are rejected — a symmetric alg against a public
// key is the classic algorithm-confusion forgery.
type __BynkJwk = JsonWebKey & { kid?: string; alg?: string; use?: string };

interface __BynkJwksCacheEntry {
  keys: __BynkJwk[];
  /** When `keys` were last successfully fetched — the TTL basis. */
  fetchedAt: number;
  /** When the network was last hit (success or failure) — the refetch-cooldown
   *  basis; bounds `kid`-miss refetch amplification. */
  lastAttemptAt: number;
}

const __bynkJwksCache = new Map<string, __BynkJwksCacheEntry>();
const __BYNK_JWKS_TTL_MS = 10 * 60 * 1000;
// A forced (`kid`-miss) refetch is rate-limited to at most once per this window
// per URI. `kid` is attacker-controlled and not integrity-protected before
// verification, so without a cooldown an unauthenticated caller could drive one
// uncached JWKS fetch per request just by varying `kid`. Mirrors `jose`'s
// `cooldownDuration`.
const __BYNK_JWKS_REFETCH_COOLDOWN_MS = 30 * 1000;
// Clock-skew leeway (seconds) applied to `exp`/`nbf`.
const __BYNK_CLOCK_SKEW_S = 60;

async function __bynkFetchJwks(jwksUri: string, force: boolean): Promise<__BynkJwk[] | null> {
  const now = Date.now();
  const cached = __bynkJwksCache.get(jwksUri);
  if (cached !== undefined) {
    // A normal read serves cached keys within the TTL.
    if (!force && now - cached.fetchedAt < __BYNK_JWKS_TTL_MS) return cached.keys;
    // A forced (`kid`-miss) refetch is skipped while within the cooldown, so a
    // flood of forged tokens with novel `kid`s cannot amplify into fetches.
    if (force && now - cached.lastAttemptAt < __BYNK_JWKS_REFETCH_COOLDOWN_MS) return cached.keys;
    // Record this attempt up front so a concurrent/subsequent forced call (or a
    // fetch that then fails) still honours the cooldown.
    cached.lastAttemptAt = now;
  }
  let res: Response;
  try {
    res = await fetch(jwksUri);
  } catch {
    return cached?.keys ?? null;
  }
  if (!res.ok) return cached?.keys ?? null;
  let body: { keys?: unknown };
  try {
    body = (await res.json()) as { keys?: unknown };
  } catch {
    return cached?.keys ?? null;
  }
  if (!Array.isArray(body.keys)) return cached?.keys ?? null;
  const keys = body.keys as __BynkJwk[];
  __bynkJwksCache.set(jwksUri, { keys, fetchedAt: now, lastAttemptAt: now });
  return keys;
}

function __bynkAlgParams(alg: string): {
  importAlg: RsaHashedImportParams | EcKeyImportParams;
  verifyAlg: AlgorithmIdentifier | EcdsaParams;
  kty: string;
} | null {
  if (alg === "RS256") {
    return {
      importAlg: { name: "RSASSA-PKCS1-v1_5", hash: "SHA-256" },
      verifyAlg: "RSASSA-PKCS1-v1_5",
      kty: "RSA",
    };
  }
  if (alg === "ES256") {
    return {
      importAlg: { name: "ECDSA", namedCurve: "P-256" },
      verifyAlg: { name: "ECDSA", hash: "SHA-256" },
      kty: "EC",
    };
  }
  return null;
}

export async function verifyOidcJwt(
  token: string,
  issuer: string,
  audience: string,
  jwksUri: string,
): Promise<Result<{ readonly sub: string; readonly claims: Record<string, unknown> }, string>> {
  const parts = token.split(".");
  if (parts.length !== 3) return Err("malformed token");
  const [headerB64, payloadB64, sigB64] = parts;
  let header: { alg?: unknown; kid?: unknown };
  try {
    header = JSON.parse(new TextDecoder().decode(__bynkB64UrlToBytes(headerB64)));
  } catch {
    return Err("malformed header");
  }
  // Only asymmetric OIDC signing algorithms — reject `none`, HS* (symmetric,
  // algorithm-confusion against the public key), and anything unlisted.
  if (typeof header.alg !== "string") return Err("missing alg");
  const params = __bynkAlgParams(header.alg);
  if (params === null) return Err("unsupported alg");
  const kid = typeof header.kid === "string" ? header.kid : null;

  const enc = new TextEncoder();
  const signed = enc.encode(`${headerB64}.${payloadB64}`) as BufferSource;
  const sig = __bynkB64UrlToBytes(sigB64) as BufferSource;

  // Try each candidate key (matched by `kid` when the header names one, else by
  // key type). `hadCandidate` distinguishes a `kid` miss — a refetch may heal a
  // key rotation — from a genuine bad signature against a published key, which
  // never refetches. The `kid`-miss refetch is itself rate-limited by the
  // cooldown in `__bynkFetchJwks` (a novel `kid` is attacker-controlled).
  const attempt = async (keys: __BynkJwk[]): Promise<{ verified: boolean; hadCandidate: boolean }> => {
    const candidates = keys.filter(
      (k) =>
        k.kty === params.kty &&
        (k.use === undefined || k.use === "sig") &&
        (kid === null || k.kid === undefined || k.kid === kid),
    );
    for (const jwk of candidates) {
      let key: CryptoKey;
      try {
        key = await crypto.subtle.importKey("jwk", jwk, params.importAlg, false, ["verify"]);
      } catch {
        continue;
      }
      try {
        if (await crypto.subtle.verify(params.verifyAlg, key, sig, signed)) {
          return { verified: true, hadCandidate: true };
        }
      } catch {
        // treat an import/verify throw as a non-match and keep trying
      }
    }
    return { verified: false, hadCandidate: candidates.length > 0 };
  };

  const keys = await __bynkFetchJwks(jwksUri, false);
  if (keys === null) return Err("jwks unavailable");
  let verified = false;
  const first = await attempt(keys);
  verified = first.verified;
  if (!verified && !first.hadCandidate) {
    const refetched = await __bynkFetchJwks(jwksUri, true);
    if (refetched !== null) verified = (await attempt(refetched)).verified;
  }
  if (!verified) return Err("bad signature");

  let payload: { sub?: unknown; iss?: unknown; aud?: unknown; exp?: unknown; nbf?: unknown };
  try {
    payload = JSON.parse(new TextDecoder().decode(__bynkB64UrlToBytes(payloadB64)));
  } catch {
    return Err("malformed payload");
  }
  // Issuer and audience are the trust contract — a token from another issuer or
  // for another audience is rejected even though its signature is authentic.
  if (payload.iss !== issuer) return Err("issuer mismatch");
  const audOk =
    payload.aud === audience ||
    (Array.isArray(payload.aud) && (payload.aud as unknown[]).includes(audience));
  if (!audOk) return Err("audience mismatch");
  const now = Math.floor(Date.now() / 1000);
  // A small leeway absorbs clock skew between this Worker and the issuer, so a
  // valid token is not spuriously rejected at the second boundary (RFC 7519
  // permits "some small leeway, usually no more than a few minutes").
  const skew = __BYNK_CLOCK_SKEW_S;
  // OIDC tokens carry an `exp`; a missing/non-number one is malformed, not a
  // token that never expires.
  if (typeof payload.exp !== "number") return Err("missing exp");
  if ((payload.exp as number) < now - skew) return Err("token expired");
  if (payload.nbf !== undefined && typeof payload.nbf !== "number") return Err("malformed nbf");
  if (payload.nbf !== undefined && (payload.nbf as number) > now + skew) return Err("token not yet valid");
  if (typeof payload.sub !== "string" || payload.sub.length === 0) return Err("missing sub");
  return Ok({ sub: payload.sub, claims: payload as Record<string, unknown> });
}

// #2380 (L5): an `Oidc` actor's verified token claims, as its binder's
// `.claims` reads them — or, in a test, `"name: value"` lines (`TokenClaims.of`).
export interface TokenClaims {
  get(name: string): Option<string>;
  has(name: string): boolean;
}

export function tokenClaims(source: Readonly<Record<string, unknown>> | readonly string[]): TokenClaims {
  let claims: Record<string, unknown>;
  if (Array.isArray(source)) {
    claims = {};
    for (const line of source as readonly string[]) {
      const i = line.indexOf(":");
      if (i > 0) claims[line.slice(0, i).trim()] = line.slice(i + 1).trim();
    }
  } else {
    claims = { ...(source as Record<string, unknown>) };
  }
  return {
    get: (name) => {
      const v = Object.prototype.hasOwnProperty.call(claims, name) ? claims[name] : undefined;
      return typeof v === "string" ? { tag: "Some", value: v } : { tag: "None" };
    },
    has: (name) => Object.prototype.hasOwnProperty.call(claims, name),
  };
}

// #2380 (L5): an `Oidc` actor's trust roots, overridden by its `trustFrom`
// environment variable when that is set: JSON `{"issuer", "jwks", "audience"?}`.
// Unset leaves the declared roots. Set but malformed is `null`: the seam then
// refuses every request (fail closed) rather than fall back to the declaration.
export function oidcTrust(
  raw: unknown,
  issuer: string,
  audience: string,
  jwks: string,
): { readonly issuer: string; readonly audience: string; readonly jwks: string } | null {
  if (raw === undefined || raw === null || raw === "") return { issuer, audience, jwks };
  if (typeof raw !== "string") {
    console.error("bynk: an Oidc actor's trustFrom variable isn't a string; every request is refused");
    return null;
  }
  let parsed: { issuer?: unknown; audience?: unknown; jwks?: unknown };
  try {
    parsed = JSON.parse(raw);
  } catch {
    console.error("bynk: an Oidc actor's trustFrom variable isn't JSON; every request is refused");
    return null;
  }
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
    console.error("bynk: an Oidc actor's trustFrom variable isn't a JSON object; every request is refused");
    return null;
  }
  if (typeof parsed.issuer !== "string" || typeof parsed.jwks !== "string") {
    console.error("bynk: an Oidc actor's trustFrom variable needs string `issuer` and `jwks`; every request is refused");
    return null;
  }
  if (parsed.audience !== undefined && typeof parsed.audience !== "string") {
    console.error("bynk: an Oidc actor's trustFrom `audience` must be a string; every request is refused");
    return null;
  }
  return { issuer: parsed.issuer, jwks: parsed.jwks, audience: parsed.audience ?? audience };
}

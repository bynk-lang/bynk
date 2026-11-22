// #3052: the `system` driver's test OIDC issuer. Test-only: it lives in the
// generated test module and never in an emitted Worker. It generates an ES256
// key pair, answers the Worker's JWKS `fetch` for the actor's `jwks` URL, and
// mints the tokens a call presents. The Worker's own verifier runs unmodified
// against it.
type __BynkOidcTrust = { issuer: string; audience: string; jwks: string };
type __BynkOidcMint = {
  sub: string;
  claims: globalThis.Record<string, unknown>;
  // Which credential: signed by the published key (`valid`), by an unpublished
  // key with the same `kid` (`forged`), or published-key tokens whose `exp`,
  // `iss` or `aud` the seam must refuse. `algnone` and `alghs256` carry an
  // otherwise valid payload under an algorithm the seam must refuse: `none`
  // (no signature), and `HS256` keyed with the public key (algorithm
  // confusion). `emptysub` and `nosub` are correctly signed with a `sub` the
  // seam must refuse: empty, and absent.
  kind: "valid" | "forged" | "expired" | "wrongissuer" | "wrongaudience" | "algnone" | "alghs256" | "emptysub" | "nosub";
};
interface __BynkOidcIssuer {
  kid: string;
  jwk: globalThis.Record<string, unknown>;
  signKey: CryptoKey;
  forgedKey: CryptoKey;
  // The published public key, DER-encoded: what an algorithm-confusion attacker
  // holds, and so what `alghs256` uses as its HMAC secret.
  publicSpki: Uint8Array;
  jwksUris: Set<string>;
}
function __bynkOidcB64(bytes: Uint8Array): string { let bin = ""; for (const b of bytes) bin += String.fromCharCode(b); return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, ""); }
function __bynkOidcText(s: string): string { return __bynkOidcB64(new TextEncoder().encode(s)); }
// A claim list is `"name: value"` lines, read as `TokenClaims.of` reads them in
// process: every value is a trimmed string (`"n: 1"` is the string `"1"`), so
// the `system` and `integration` tiers see the same claims.
function __bynkOidcClaims(lines: readonly string[]): globalThis.Record<string, unknown> {
  const claims: globalThis.Record<string, unknown> = {};
  for (const line of lines) {
    const i = line.indexOf(":");
    if (i > 0) claims[line.slice(0, i).trim()] = line.slice(i + 1).trim();
  }
  return claims;
}
// The trust roots the seam checks: the actor's declaration, replaced by its
// `trustFrom` variable's JSON when the case's `env` (or the process) sets it,
// exactly as the Worker's `__oidcTrust` reads it.
function __bynkOidcTrustOf(declared: __BynkOidcTrust, raw: string | undefined): __BynkOidcTrust {
  if (raw === undefined || raw === "") return declared;
  let parsed: { issuer?: unknown; audience?: unknown; jwks?: unknown };
  try { parsed = JSON.parse(raw); } catch { throw new globalThis.Error("an Oidc actor's trustFrom variable isn't JSON"); }
  if (typeof parsed !== "object" || parsed === null || typeof parsed.issuer !== "string" || typeof parsed.jwks !== "string") {
    throw new globalThis.Error("an Oidc actor's trustFrom variable needs string `issuer` and `jwks`");
  }
  if (parsed.audience !== undefined && typeof parsed.audience !== "string") {
    throw new globalThis.Error("an Oidc actor's trustFrom `audience` must be a string");
  }
  return { issuer: parsed.issuer, jwks: parsed.jwks, audience: (parsed.audience as string | undefined) ?? declared.audience };
}
// One issuer per test process, shared by every test module in it. The Worker's
// JWKS cache is keyed by URL with a ten-minute TTL, so a key that changed
// between suites would be refused from the cache.
async function __bynkOidcIssuerOf(): Promise<__BynkOidcIssuer> {
  const g = globalThis as unknown as { __bynkTestOidcIssuer?: Promise<__BynkOidcIssuer> };
  g.__bynkTestOidcIssuer ??= (async (): Promise<__BynkOidcIssuer> => {
    const gen = (): Promise<CryptoKeyPair> => crypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, true, ["sign", "verify"]) as Promise<CryptoKeyPair>;
    const pair = await gen();
    const forged = await gen();
    const kid = "bynk-test-issuer";
    const jwk = { ...((await crypto.subtle.exportKey("jwk", pair.publicKey)) as globalThis.Record<string, unknown>), kid, alg: "ES256", use: "sig" };
    const publicSpki = new Uint8Array((await crypto.subtle.exportKey("spki", pair.publicKey)) as ArrayBuffer);
    const issuer: __BynkOidcIssuer = { kid, jwk, signKey: pair.privateKey, forgedKey: forged.privateKey, publicSpki, jwksUris: new Set() };
    // Answer the Worker's JWKS fetch for the registered URLs; anything else
    // goes to the real `fetch`.
    const real = globalThis.fetch.bind(globalThis);
    globalThis.fetch = (async (input: globalThis.RequestInfo | globalThis.URL, init?: globalThis.RequestInit): Promise<globalThis.Response> => {
      const url = typeof input === "string" ? input : input instanceof globalThis.URL ? input.href : input.url;
      const method = (init?.method ?? (typeof input === "object" && "method" in input ? input.method : "GET")).toUpperCase();
      if (method === "GET" && issuer.jwksUris.has(url)) {
        return new globalThis.Response(JSON.stringify({ keys: [jwk] }), { status: 200, headers: { "content-type": "application/json" } });
      }
      return real(input, init);
    }) as typeof globalThis.fetch;
    return issuer;
  })();
  return g.__bynkTestOidcIssuer;
}
// Mint the credential a call presents and make the Worker's JWKS fetch for
// `trust.jwks` answer with the test issuer's key.
async function __bynkOidcToken(remote: boolean, trust: __BynkOidcTrust, o: __BynkOidcMint): Promise<string> {
  if (remote) {
    throw new globalThis.Error("an `Oidc` route is driven against an in-process Worker: a remote target fetches its own JWKS, which the driver cannot answer");
  }
  const issuer = await __bynkOidcIssuerOf();
  issuer.jwksUris.add(trust.jwks);
  const now = __bynkNow();
  const payload: globalThis.Record<string, unknown> = {
    ...o.claims,
    sub: o.sub,
    iss: o.kind === "wrongissuer" ? `${trust.issuer}/not-the-issuer` : trust.issuer,
    aud: o.kind === "wrongaudience" ? `${trust.audience}-not-the-audience` : trust.audience,
    iat: now,
    exp: o.kind === "expired" ? now - 3600 : now + 3600,
  };
  // `emptysub` and `nosub` are the only kinds whose `sub` is not the call's.
  if (o.kind === "emptysub") payload.sub = "";
  if (o.kind === "nosub") delete payload.sub;
  const alg = o.kind === "algnone" ? "none" : o.kind === "alghs256" ? "HS256" : "ES256";
  const head = __bynkOidcText(JSON.stringify({ alg, typ: "JWT", kid: issuer.kid }));
  const body = __bynkOidcText(JSON.stringify(payload));
  const signed = new TextEncoder().encode(`${head}.${body}`);
  if (o.kind === "algnone") return `${head}.${body}.`;
  if (o.kind === "alghs256") {
    // Algorithm confusion: the public key, which the JWKS publishes, as the
    // HMAC secret of a symmetric token.
    const mac = await crypto.subtle.importKey("raw", issuer.publicSpki as BufferSource, { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
    return `${head}.${body}.${__bynkOidcB64(new Uint8Array(await crypto.subtle.sign("HMAC", mac, signed)))}`;
  }
  // WebCrypto's ECDSA signature is the raw `r || s` a JWS carries.
  const sig = await crypto.subtle.sign({ name: "ECDSA", hash: "SHA-256" }, o.kind === "forged" ? issuer.forgedKey : issuer.signKey, signed);
  return `${head}.${body}.${__bynkOidcB64(new Uint8Array(sig))}`;
}

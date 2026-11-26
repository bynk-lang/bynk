export function __bynkNow(): number { return Math.floor(Date.now() / 1000); }
export function __b64url(s: string): string { return __bytesB64url(new TextEncoder().encode(s)); }
export function __bytesB64url(bytes: Uint8Array): string { let bin = ""; for (const b of bytes) bin += String.fromCharCode(b); return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, ""); }
export async function __bynkSignHs256(payload: Record<string, unknown>, secret: string): Promise<string> {
  const h = __b64url(JSON.stringify({ alg: "HS256", typ: "JWT" }));
  const p = __b64url(JSON.stringify(payload));
  const enc = new TextEncoder();
  const key = await crypto.subtle.importKey("raw", enc.encode(secret) as BufferSource, { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  const sig = await crypto.subtle.sign("HMAC", key, enc.encode(`${h}.${p}`) as BufferSource);
  return `${h}.${p}.${__bytesB64url(new Uint8Array(sig))}`;
}
// A claim list is `"name: value"` lines, read as `TokenClaims.of` reads them in
// process: every value is a trimmed string (`"n: 1"` is the string `"1"`), so
// the `system` and `integration` tiers see the same claims.
export function __bynkClaimLines(lines: readonly string[]): globalThis.Record<string, unknown> {
  const claims: globalThis.Record<string, unknown> = {};
  for (const line of lines) {
    const i = line.indexOf(":");
    if (i > 0) claims[line.slice(0, i).trim()] = line.slice(i + 1).trim();
  }
  return claims;
}

function __bynkNow(): number { return Math.floor(Date.now() / 1000); }
function __b64url(s: string): string { return __bytesB64url(new TextEncoder().encode(s)); }
function __bytesB64url(bytes: Uint8Array): string { let bin = ""; for (const b of bytes) bin += String.fromCharCode(b); return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, ""); }
async function __bynkSignHs256(payload: Record<string, unknown>, secret: string): Promise<string> {
  const h = __b64url(JSON.stringify({ alg: "HS256", typ: "JWT" }));
  const p = __b64url(JSON.stringify(payload));
  const enc = new TextEncoder();
  const key = await crypto.subtle.importKey("raw", enc.encode(secret) as BufferSource, { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  const sig = await crypto.subtle.sign("HMAC", key, enc.encode(`${h}.${p}`) as BufferSource);
  return `${h}.${p}.${__bytesB64url(new Uint8Array(sig))}`;
}

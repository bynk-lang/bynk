// #2135: the test-only `Signature` signer. It signs exactly what the Worker's
// seam (`verifySignatureHmacSha256` in the runtime's `auth.ts`) verifies: an
// HMAC-SHA256 over the raw body, or over `<timestamp>.<body>` when the actor
// declares a timestamp header, sent as `sha256=<hex>`. The timestamp is in
// seconds, `ageSecs` before now (a stale request is signed in the past). A `@raw`
// route signs the octets it sends (`Uint8Array`), a JSON route the text.
async function __bynkSignHmacSha256(body: string | Uint8Array, secret: string, timestamped: boolean, ageSecs: number): Promise<{ signature: string; timestamp: string }> {
  const timestamp = String(Math.floor(Date.now() / 1000) - ageSecs);
  const enc = new TextEncoder();
  const octets = typeof body === "string" ? enc.encode(body) : body;
  const prefix = timestamped ? enc.encode(`${timestamp}.`) : new Uint8Array(0);
  const signed = new Uint8Array(prefix.length + octets.length);
  signed.set(prefix, 0);
  signed.set(octets, prefix.length);
  const key = await crypto.subtle.importKey("raw", enc.encode(secret) as BufferSource, { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  const mac = new Uint8Array(await crypto.subtle.sign("HMAC", key, signed as BufferSource));
  let hex = "";
  for (const b of mac) hex += b.toString(16).padStart(2, "0");
  return { signature: `sha256=${hex}`, timestamp };
}

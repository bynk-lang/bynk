// #2135: the test-only `Signature` signer. It signs exactly what the Worker's
// seam (`verifySignatureHmacSha256` in the runtime's `auth.ts`) verifies: an
// HMAC-SHA256 over the raw body, or over `<timestamp>.<body>` when the actor
// declares a timestamp header, sent as `sha256=<hex>`. The timestamp is in
// seconds, `ageSecs` before now (a stale request is signed in the past).
async function __bynkSignHmacSha256(body: string, secret: string, timestamped: boolean, ageSecs: number): Promise<{ signature: string; timestamp: string }> {
  const timestamp = String(Math.floor(Date.now() / 1000) - ageSecs);
  const signed = timestamped ? `${timestamp}.${body}` : body;
  const enc = new TextEncoder();
  const key = await crypto.subtle.importKey("raw", enc.encode(secret) as BufferSource, { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  const mac = new Uint8Array(await crypto.subtle.sign("HMAC", key, enc.encode(signed) as BufferSource));
  let hex = "";
  for (const b of mac) hex += b.toString(16).padStart(2, "0");
  return { signature: `sha256=${hex}`, timestamp };
}

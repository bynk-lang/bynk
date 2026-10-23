// A stand-in for GitHub Actions' OIDC issuer (R4, #2381), for
// scripts/check-trusted.sh: an RSA key, its JWKS, and RS256 tokens with the
// claims asked for.
//
//   node fake-oidc.mjs <port-file>
//
//   GET  /jwks                     the key set
//   POST /token {claims…, aud?, ttl?}  a token: iss is this server, aud
//                                  defaults to `bynk-registry`, ttl (seconds,
//                                  may be negative) to 300
import { createServer } from "node:http";
import { writeFileSync } from "node:fs";

const { subtle } = globalThis.crypto;
const keys = await subtle.generateKey(
  { name: "RSASSA-PKCS1-v1_5", modulusLength: 2048, publicExponent: new Uint8Array([1, 0, 1]), hash: "SHA-256" },
  true,
  ["sign", "verify"],
);
const jwk = { ...(await subtle.exportKey("jwk", keys.publicKey)), kid: "fake-1", alg: "RS256", use: "sig" };
const b64url = (bytes) => Buffer.from(bytes).toString("base64url");

let issuer = "";
const server = createServer(async (req, res) => {
  const send = (status, body, type = "application/json") => {
    res.writeHead(status, { "content-type": type });
    res.end(typeof body === "string" ? body : JSON.stringify(body));
  };
  if (req.method === "GET" && req.url === "/jwks") return send(200, { keys: [jwk] });
  if (req.method === "POST" && req.url === "/token") {
    let raw = "";
    for await (const chunk of req) raw += chunk;
    const { aud = "bynk-registry", ttl = 300, ...claims } = JSON.parse(raw || "{}");
    const now = Math.floor(Date.now() / 1000);
    const header = b64url(JSON.stringify({ alg: "RS256", typ: "JWT", kid: "fake-1" }));
    const payload = b64url(JSON.stringify({ iss: issuer, aud, iat: now, exp: now + ttl, ...claims }));
    const signed = new TextEncoder().encode(`${header}.${payload}`);
    const sig = await subtle.sign("RSASSA-PKCS1-v1_5", keys.privateKey, signed);
    return send(200, `${header}.${payload}.${b64url(new Uint8Array(sig))}`, "text/plain");
  }
  send(404, { message: "Not Found" });
});
server.listen(0, "127.0.0.1", () => {
  const { port } = server.address();
  issuer = `http://127.0.0.1:${port}`;
  writeFileSync(process.argv[2], String(port));
});

// #3899: signed queue messages. A Cloudflare queue message carries only a body (no headers), so a
// signature travels inside it. A signed message's body is
//
//   { "bynk": { "v": 1, "from": "<context or actor>", "id": "<message id>", "ts": <seconds>, "sig": "<hex>" },
//     "body": <the message> }
//
// and the HMAC-SHA256 covers the version, the sender, the id, the timestamp, the NAME OF THE SOURCE
// QUEUE and the canonical body, so a message cannot be moved to another queue and still verify. The
// source queue (not the delivering queue) is the one named, so a dead-letter consumer can verify a
// message the platform moved. Confidentiality is not provided: anyone who can read the queue can
// read the body. The format is documented beside the call wire format so that a producer that is not
// Bynk can implement it.

const encoder = new TextEncoder();

export const QUEUE_SIGNING_VERSION = 1;

export interface SignedQueueEnvelope {
  readonly bynk: {
    readonly v: number;
    readonly from: string;
    readonly id: string;
    readonly ts: number;
    readonly sig: string;
  };
  readonly body: unknown;
}

/**
 * The canonical form of a body: JSON Canonicalization Scheme (RFC 8785). Object keys are sorted
 * (by UTF-16 code unit) at every depth, with no whitespace, strings escaped as `JSON.stringify`
 * does, and numbers written as ECMAScript does. The input must be JSON data; a value with a
 * `toJSON` (a `Date`) is converted first, as the transport's own `JSON.stringify` would.
 */
export function canonicalJson(value: unknown): string {
  if (value !== null && typeof value === "object" && typeof (value as { toJSON?: unknown }).toJSON === "function") {
    return canonicalJson((value as { toJSON(): unknown }).toJSON());
  }
  if (value === null || typeof value !== "object") return JSON.stringify(value) ?? "null";
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
  const entries = Object.entries(value as Record<string, unknown>)
    .filter(([, v]) => v !== undefined)
    .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
  return `{${entries.map(([k, v]) => `${JSON.stringify(k)}:${canonicalJson(v)}`).join(",")}}`;
}

/** The text the signature covers. */
export function queueSigningInput(
  from: string,
  id: string,
  ts: number,
  queue: string,
  body: unknown,
): string {
  return `bynk-queue-v${QUEUE_SIGNING_VERSION}\n${from}\n${id}\n${ts}\n${queue}\n${canonicalJson(body)}`;
}

function toHex(bytes: ArrayBuffer): string {
  return [...new Uint8Array(bytes)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

function fromHex(hex: string): Uint8Array<ArrayBuffer> | null {
  if (hex.length % 2 !== 0 || !/^[0-9a-fA-F]*$/.test(hex)) return null;
  const out = new Uint8Array(new ArrayBuffer(hex.length / 2));
  for (let i = 0; i < out.length; i++) out[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  return out;
}

async function hmacKey(secret: string, usage: "sign" | "verify"): Promise<CryptoKey> {
  return crypto.subtle.importKey("raw", encoder.encode(secret), { name: "HMAC", hash: "SHA-256" }, false, [usage]);
}

/** Wrap `body` in a signed envelope for the queue named `queue`, under `secret`. */
export async function signQueueMessage(
  secret: string,
  parts: { from: string; id: string; ts: number; queue: string; body: unknown },
): Promise<SignedQueueEnvelope> {
  const key = await hmacKey(secret, "sign");
  const sig = toHex(
    await crypto.subtle.sign("HMAC", key, encoder.encode(queueSigningInput(parts.from, parts.id, parts.ts, parts.queue, parts.body))),
  );
  return { bynk: { v: QUEUE_SIGNING_VERSION, from: parts.from, id: parts.id, ts: parts.ts, sig }, body: parts.body };
}

/** Why a message was refused. None of them is ever handled, retried or dead-lettered. */
export type QueueRefusal = "unsigned" | "malformed" | "version" | "unknown_sender" | "bad_signature";

export type QueueVerification =
  | { readonly ok: true; readonly from: string; readonly id: string; readonly ts: number; readonly body: unknown }
  | { readonly ok: false; readonly reason: QueueRefusal };

/**
 * Verify a received body. `keysFor(from)` gives the secrets a sender's messages may be signed under,
 * the current one first and the previous one after it while a rotation is in its window (#3899 H),
 * or an empty list for a sender the consumer does not admit. `queue` is the SOURCE queue's name.
 */
export async function verifyQueueMessage(
  received: unknown,
  queue: string,
  keysFor: (from: string) => readonly string[],
): Promise<QueueVerification> {
  if (received === null || typeof received !== "object" || Array.isArray(received)) return { ok: false, reason: "unsigned" };
  const envelope = (received as Record<string, unknown>).bynk;
  if (envelope === undefined) return { ok: false, reason: "unsigned" };
  if (envelope === null || typeof envelope !== "object" || !("body" in (received as object))) return { ok: false, reason: "malformed" };
  const { v, from, id, ts, sig } = envelope as Record<string, unknown>;
  // The version first: a later version may reshape the other fields, and "version" is the useful reason.
  if (v !== QUEUE_SIGNING_VERSION) return { ok: false, reason: "version" };
  // `ts` is an integer written in decimal. `from` and `id` carry no line feed, because the signing
  // input is newline-separated and must split unambiguously whatever the field order.
  if (
    typeof from !== "string" || from.includes("\n") ||
    typeof id !== "string" || id.includes("\n") ||
    typeof ts !== "number" || !Number.isSafeInteger(ts) ||
    typeof sig !== "string"
  ) {
    return { ok: false, reason: "malformed" };
  }
  const secrets = keysFor(from);
  if (secrets.length === 0) return { ok: false, reason: "unknown_sender" };
  const signature = fromHex(sig);
  if (signature === null) return { ok: false, reason: "malformed" };
  const body = (received as Record<string, unknown>).body;
  const input = encoder.encode(queueSigningInput(from, id, ts, queue, body));
  for (const secret of secrets) {
    const key = await hmacKey(secret, "verify");
    if (await crypto.subtle.verify("HMAC", key, signature, input)) return { ok: true, from, id, ts, body };
  }
  return { ok: false, reason: "bad_signature" };
}

/**
 * How long a verified message id stays claimed, in seconds: the queue's retention (14 days at most)
 * plus the longest a send can be delayed (24 hours), so a replayed message is still recognised
 * however long the original waited (#3899 G).
 */
export const QUEUE_DEDUP_WINDOW_SECONDS = (14 + 1) * 24 * 60 * 60;

/** The key a consumer claims a message id under, per consumer and source queue. */
export function queueReplayKey(consumer: string, queue: string, id: string): string {
  return `bynk.queue.${consumer}.${queue}.${id}`;
}

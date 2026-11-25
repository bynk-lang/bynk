// #3899 (3b): what a signed queue consumer does with a received body before it decodes it. The
// signature is verified (queue_signing.ts), then the message id is claimed once, so a replayed
// message is recognised however long the original waited. A refused message is never handled,
// retried or dead-lettered: the entry acknowledges it and reports it as a fault.

import { QUEUE_DEDUP_WINDOW_SECONDS, queueReplayKey, verifyQueueMessage, type QueueRefusal } from "./queue_signing.ts";

export type QueueAdmitRefusal = QueueRefusal | "replayed";

export type QueueAdmission =
  | { readonly ok: true; readonly from: string; readonly id: string; readonly body: unknown }
  | { readonly ok: false; readonly reason: QueueAdmitRefusal };

/** What the entry supplies: the consumer's secrets per admitted sender, and its id claim. */
export interface QueueAdmitPorts {
  /**
   * The secrets a sender's messages may be signed under, the current one first and the previous one
   * while a rotation is in its window. An empty list for a sender the consumer's actor does not
   * admit, which refuses the message as `unknown_sender`.
   */
  readonly keysFor: (from: string) => readonly string[];
  /** Claim `key` for `ttlSeconds`: true the first time, false if it is already claimed. */
  readonly claim: (key: string, ttlSeconds: number) => Promise<boolean>;
}

/**
 * Admit `received` to `consumer`. `sourceQueue` is the queue the message was first sent to, not the
 * queue delivering it: a dead-letter consumer passes the source queue, so a message the platform
 * moved still verifies, and one moved between queues by hand does not.
 */
export async function queueAdmit(
  received: unknown,
  consumer: string,
  sourceQueue: string,
  ports: QueueAdmitPorts,
): Promise<QueueAdmission> {
  const verified = await verifyQueueMessage(received, sourceQueue, ports.keysFor);
  if (!verified.ok) return verified;
  // Claimed only after the signature verified, so unsigned traffic cannot fill the store with ids.
  const fresh = await ports.claim(queueReplayKey(consumer, sourceQueue, verified.id), QUEUE_DEDUP_WINDOW_SECONDS);
  if (!fresh) return { ok: false, reason: "replayed" };
  return { ok: true, from: verified.from, id: verified.id, body: verified.body };
}

/** The fault text a refused message is reported with. */
export function queueRefusalDetail(consumer: string, reason: QueueAdmitRefusal): string {
  switch (reason) {
    case "unsigned": return `queue ${consumer} refused a message: it is not signed`;
    case "malformed": return `queue ${consumer} refused a message: its signature envelope is malformed`;
    case "version": return `queue ${consumer} refused a message: unsupported signature version`;
    case "unknown_sender": return `queue ${consumer} refused a message: its sender is not admitted`;
    case "bad_signature": return `queue ${consumer} refused a message: the signature does not verify`;
    case "replayed": return `queue ${consumer} refused a message: its id was already delivered`;
  }
}

/**
 * The secrets a foreign sender's messages are signed under, read from the Worker's `env`:
 * `secrets` maps each admitted actor's name (the envelope's `from`) to the name of its secret.
 * `<SECRET>_PREVIOUS`, when set, is the key before a rotation and is tried after the current one.
 * A secret that is missing or empty gives no key, which refuses the sender: a consumer whose
 * deploy lacks the secret admits nothing rather than everything.
 */
export function queueForeignKeys(env: unknown, secrets: Readonly<Record<string, string>>): (from: string) => readonly string[] {
  const read = (name: string): string | undefined => {
    const value = env !== null && typeof env === "object" ? (env as Record<string, unknown>)[name] : undefined;
    return typeof value === "string" && value !== "" ? value : undefined;
  };
  return (from) => {
    if (!Object.prototype.hasOwnProperty.call(secrets, from)) return [];
    const name = secrets[from]!;
    const current = read(name);
    if (current === undefined) return [];
    const previous = read(`${name}_PREVIOUS`);
    return previous === undefined ? [current] : [current, previous];
  };
}

/** The part of D1 an id claim uses; declared here because the Worker's `env` is a plain value. */
interface ClaimD1 {
  prepare(sql: string): { bind(...values: unknown[]): unknown };
  batch(statements: unknown[]): Promise<Array<{ meta: { changes?: number } }>>;
}

/**
 * Claim `key` for `ttlSeconds` in the Worker's implicit Idempotency database (`env.D1_IDEMPOTENCY`),
 * the table `bynk.Idempotency.claim` uses: true for the first claim, false while it is held. A Worker
 * without the binding is a deploy that skipped the generated `[[d1_databases]]` stanza, and fails
 * loudly, so the message is retried and never admitted unchecked.
 */
export async function claimQueueMessage(env: unknown, key: string, ttlSeconds: number): Promise<boolean> {
  const db = env !== null && typeof env === "object" ? ((env as Record<string, unknown>).D1_IDEMPOTENCY as ClaimD1 | undefined) : undefined;
  if (!db) {
    throw new Error("a signed queue consumer requires a D1 database binding (env.D1_IDEMPOTENCY) — deploy with the generated [[d1_databases]] wrangler stanza");
  }
  const now = Date.now();
  const [claimed] = await db.batch([
    db.prepare("INSERT INTO idempotency_claims (key, expires_at) VALUES (?1, ?2) ON CONFLICT (key) DO UPDATE SET expires_at = excluded.expires_at WHERE idempotency_claims.expires_at <= ?3").bind(key, now + ttlSeconds * 1000, now),
    db.prepare("DELETE FROM idempotency_claims WHERE key IN (SELECT key FROM idempotency_claims WHERE expires_at <= ?1 LIMIT 20)").bind(now),
  ]);
  return (claimed?.meta.changes ?? 0) > 0;
}

/**
 * What a signed consumer's entry calls for each message: admit `received` to the consumer of `queue`
 * under the foreign senders `secrets` names. `sources` are the queues that dead-letter into this one:
 * a message the platform moved here was signed for its source, so it is verified under each in turn.
 * A message that verified but was already delivered reports `replayed`, the most telling refusal.
 */
export async function admitQueueBody(
  received: unknown,
  queue: string,
  sources: readonly string[],
  env: unknown,
  secrets: Readonly<Record<string, string>>,
): Promise<QueueAdmission> {
  const ports: QueueAdmitPorts = { keysFor: queueForeignKeys(env, secrets), claim: (key, ttl) => claimQueueMessage(env, key, ttl) };
  let first: QueueAdmission | undefined;
  for (const source of [queue, ...sources]) {
    const admitted = await queueAdmit(received, queue, source, ports);
    if (admitted.ok || (!admitted.ok && admitted.reason === "replayed")) return admitted;
    first ??= admitted;
  }
  return first as QueueAdmission;
}

// #3899 (3b): what a signed queue consumer does with a received body before it decodes it. The
// signature is verified (queue_signing.ts), the message is refused if it is too old or too far ahead,
// and its id is claimed, so a replayed message is recognised. A refused message is never handled,
// retried or dead-lettered: the entry acknowledges it and reports it as a fault.
//
// The claim is a short lease while the handler runs. The entry settles it: confirmed for the full
// window once the message is handled, released if the handler asks for a retry or throws, so a platform
// redelivery of a message that failed is admitted again and nothing is lost. A claim whose instance
// dies lapses with its lease, for the same reason.

import { QUEUE_DEDUP_WINDOW_SECONDS, queueReplayKey, verifyQueueMessage, type QueueRefusal } from "./queue_signing.ts";

/** How long a claim is held while the handler runs, in seconds: a Worker's longest run is 15 minutes. */
export const QUEUE_CLAIM_LEASE_SECONDS = 15 * 60;

/** How far ahead of the consumer's clock a message's `ts` may be, in seconds, for clock skew. */
export const QUEUE_CLOCK_SKEW_SECONDS = 5 * 60;

export type QueueAdmitRefusal = QueueRefusal | "replayed" | "stale";

export type QueueAdmission =
  | {
      readonly ok: true;
      readonly from: string;
      readonly id: string;
      readonly body: unknown;
      /** Settle the claim: `true` once the message was handled (kept for the window), `false` when it was not (released). */
      readonly settle: (handled: boolean) => Promise<void>;
    }
  | { readonly ok: false; readonly reason: QueueAdmitRefusal };

/** What the entry supplies: the consumer's secrets per admitted sender, and its id claim. */
export interface QueueAdmitPorts {
  /**
   * The secrets a sender's messages may be signed under, the current one first and the previous one
   * while a rotation is in its window. An empty list for a sender the consumer's actor does not
   * admit, which refuses the message as `unknown_sender`.
   */
  readonly keysFor: (from: string) => readonly string[];
  /** Claim `key` for `ttlSeconds`: true the first time, false while it is claimed. */
  readonly claim: (key: string, ttlSeconds: number) => Promise<boolean>;
  /** Keep a claim `key` for `ttlSeconds` more. */
  readonly confirm: (key: string, ttlSeconds: number) => Promise<void>;
  /** Drop the claim `key`, so the message can be admitted again. */
  readonly release: (key: string) => Promise<void>;
  /** The current time in milliseconds; the clock, unless a test supplies one. */
  readonly now?: () => number;
}

/**
 * Admit `received` to `consumer`. `sourceQueue` is the queue the message was first sent to, not the
 * queue delivering it: a dead-letter consumer passes the source queue, so a message the platform
 * moved still verifies, and one moved between queues by hand does not.
 *
 * A message whose `ts` is older than the replay window is refused as `stale`, because its claim may
 * have lapsed and could no longer recognise a replay: a claim made at the latest `QUEUE_DEDUP_WINDOW_SECONDS`
 * after `ts` outlives the message by that much again. One too far ahead is refused for the same reason.
 */
export async function queueAdmit(
  received: unknown,
  consumer: string,
  sourceQueue: string,
  ports: QueueAdmitPorts,
): Promise<QueueAdmission> {
  const verified = await verifyQueueMessage(received, sourceQueue, ports.keysFor);
  if (!verified.ok) return verified;
  const age = (ports.now?.() ?? Date.now()) / 1000 - verified.ts;
  if (age > QUEUE_DEDUP_WINDOW_SECONDS || age < -QUEUE_CLOCK_SKEW_SECONDS) return { ok: false, reason: "stale" };
  // Claimed only after the signature verified, so unsigned traffic cannot fill the store with ids.
  const key = queueReplayKey(consumer, sourceQueue, verified.id);
  if (!(await ports.claim(key, QUEUE_CLAIM_LEASE_SECONDS))) return { ok: false, reason: "replayed" };
  return {
    ok: true,
    from: verified.from,
    id: verified.id,
    body: verified.body,
    settle: (handled) => (handled ? ports.confirm(key, QUEUE_DEDUP_WINDOW_SECONDS) : ports.release(key)),
  };
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
    case "stale": return `queue ${consumer} refused a message: it was signed too long ago, or too far ahead`;
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

/** The Worker's Idempotency database; a Worker without it fails the message loudly rather than admitting it unchecked. */
function claimDatabase(env: unknown): ClaimD1 {
  const db = env !== null && typeof env === "object" ? ((env as Record<string, unknown>).D1_IDEMPOTENCY as ClaimD1 | undefined) : undefined;
  if (!db) {
    throw new Error("a signed queue consumer requires a D1 database binding (env.D1_IDEMPOTENCY) — deploy with the generated [[d1_databases]] wrangler stanza");
  }
  return db;
}

/**
 * Claim `key` for `ttlSeconds` in the Worker's implicit Idempotency database (`env.D1_IDEMPOTENCY`),
 * the table `bynk.Idempotency.claim` uses: true for the first claim, false while it is held.
 */
export async function claimQueueMessage(env: unknown, key: string, ttlSeconds: number): Promise<boolean> {
  const db = claimDatabase(env);
  const now = Date.now();
  const [claimed] = await db.batch([
    db.prepare("INSERT INTO idempotency_claims (key, expires_at) VALUES (?1, ?2) ON CONFLICT (key) DO UPDATE SET expires_at = excluded.expires_at WHERE idempotency_claims.expires_at <= ?3").bind(key, now + ttlSeconds * 1000, now),
    db.prepare("DELETE FROM idempotency_claims WHERE key IN (SELECT key FROM idempotency_claims WHERE expires_at <= ?1 LIMIT 20)").bind(now),
  ]);
  return (claimed?.meta.changes ?? 0) > 0;
}

/** Keep the claim `key` for `ttlSeconds` from now: the message was handled. */
export async function confirmQueueMessage(env: unknown, key: string, ttlSeconds: number): Promise<void> {
  const db = claimDatabase(env);
  await db.batch([
    db.prepare("INSERT INTO idempotency_claims (key, expires_at) VALUES (?1, ?2) ON CONFLICT (key) DO UPDATE SET expires_at = excluded.expires_at").bind(key, Date.now() + ttlSeconds * 1000),
  ]);
}

/** Drop the claim `key`: the message was not handled, so its redelivery is admitted. */
export async function releaseQueueMessage(env: unknown, key: string): Promise<void> {
  const db = claimDatabase(env);
  await db.batch([db.prepare("DELETE FROM idempotency_claims WHERE key = ?1").bind(key)]);
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
  const ports: QueueAdmitPorts = {
    keysFor: queueForeignKeys(env, secrets),
    claim: (key, ttl) => claimQueueMessage(env, key, ttl),
    confirm: (key, ttl) => confirmQueueMessage(env, key, ttl),
    release: (key) => releaseQueueMessage(env, key),
  };
  let first: QueueAdmission | undefined;
  for (const source of [queue, ...sources]) {
    const admitted = await queueAdmit(received, queue, source, ports);
    if (admitted.ok || (!admitted.ok && admitted.reason === "replayed")) return admitted;
    first ??= admitted;
  }
  return first as QueueAdmission;
}

// #1653: the `__` aliases emitted code imports.
export { admitQueueBody as __bynkQueueAdmit };

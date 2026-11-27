// #3899 (3b): what a signed queue consumer does with a received body before it decodes it. The
// signature is verified (queue_signing.ts), the message is refused if it is too old or too far ahead,
// and its id is claimed, so a replayed message is recognised. A refused message is never handled,
// retried or dead-lettered: the entry acknowledges it and reports it as a fault.
//
// The claim is a short lease while the handler runs. The entry settles it: confirmed for the full
// window once the message is handled, released if the handler asks for a retry or throws, so a platform
// redelivery of a message that failed is admitted again and nothing is lost. A claim whose instance
// dies lapses with its lease, for the same reason.

import { deserialiseEventEnvelope, edgeVerifyKeysOf, type JsonValue } from "./boundary.ts";
import { QUEUE_DEDUP_WINDOW_SECONDS, queueReplayKey, verifyQueueMessage, type QueueRefusal } from "./queue_signing.ts";
import type { Option, Result } from "./result.ts";
import { runInTrace } from "./traceparent.ts";

/** How long a claim is held while the handler runs, in seconds: a Worker's longest run is 15 minutes. */
export const QUEUE_CLAIM_LEASE_SECONDS = 15 * 60;

/** How far ahead of the consumer's clock a message's `ts` may be, in seconds, for clock skew. */
export const QUEUE_CLOCK_SKEW_SECONDS = 5 * 60;

export type QueueAdmitRefusal = QueueRefusal | "replayed" | "stale" | "in_flight";

export type QueueAdmission =
  | {
      readonly ok: true;
      readonly from: string;
      readonly id: string;
      readonly body: unknown;
      /** Whether the sender is a Bynk context (admitted by a `Producer` refinement) rather than a declared foreign sender. */
      readonly context?: boolean;
      /** Settle the claim: `true` once the message was handled (kept for the window), `false` when it was not (released). */
      readonly settle: (handled: boolean) => Promise<void>;
    }
  | {
      readonly ok: false;
      readonly reason: QueueAdmitRefusal;
      /** For `in_flight`: when the lease on the message's id ends, in whole seconds from now. The message is retried after it, not dropped. */
      readonly retryAfterSeconds?: number;
    };

/** The outcome of a claim: taken, or held by another delivery until `remainingSeconds` from now. */
export type QueueClaim = { readonly claimed: true } | { readonly claimed: false; readonly remainingSeconds: number };

/** What the entry supplies: the consumer's secrets per admitted sender, and its id claim. */
export interface QueueAdmitPorts {
  /**
   * The secrets a sender's messages may be signed under, the current one first and the previous one
   * while a rotation is in its window. An empty list for a sender the consumer's actor does not
   * admit, which refuses the message as `unknown_sender`.
   */
  readonly keysFor: (from: string) => readonly string[];
  /** Claim `key` for `ttlSeconds`: taken the first time, otherwise how much longer the existing claim holds. */
  readonly claim: (key: string, ttlSeconds: number) => Promise<QueueClaim>;
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
  const claim = await ports.claim(key, QUEUE_CLAIM_LEASE_SECONDS);
  if (!claim.claimed) {
    // A claim confirmed for the window outlasts any lease: that message was handled, so this is a replay.
    // One still on its lease is another delivery of the message mid-handler (or its instance died): the
    // redelivery is retried after the lease rather than acknowledged, so the message is never lost.
    if (claim.remainingSeconds > QUEUE_CLAIM_LEASE_SECONDS) return { ok: false, reason: "replayed" };
    return { ok: false, reason: "in_flight", retryAfterSeconds: Math.max(1, Math.ceil(claim.remainingSeconds)) };
  }
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
    case "in_flight": return `queue ${consumer} deferred a message: its id is held by a delivery still being handled`;
  }
}

/**
 * The secrets a foreign sender's messages are signed under, read from the Worker's `env`:
 * `secrets` maps each admitted actor's name (the envelope's `from`) to the name of its secret.
 * `<SECRET>_PREVIOUS`, when set, is the key before a rotation and is tried after the current one.
 * A secret that is missing or empty gives no key, which refuses the sender: a consumer whose
 * deploy lacks the secret admits nothing rather than everything.
 */
export function queueForeignKeys(
  env: unknown,
  secrets: Readonly<Record<string, string>>,
  admitsContext?: (context: string) => boolean,
  now: () => number = Date.now,
): (from: string) => readonly string[] {
  const read = (name: string): string | undefined => {
    const value = env !== null && typeof env === "object" ? (env as Record<string, unknown>)[name] : undefined;
    return typeof value === "string" && value !== "" ? value : undefined;
  };
  return (from) => {
    if (!Object.prototype.hasOwnProperty.call(secrets, from)) {
      // A Bynk context a `Producer` refinement admits signs under the key of its edge to this Worker's
      // context, which `bynk deploy` sets. No key, or a name outside the edge domain, admits nobody.
      // #4526: after a rotation the edge holds two keys, the new one and the old one until its window
      // ends (`edgeVerifyKeysOf`), so a message signed under the old key before the rotation is still
      // admitted rather than refused as `bad_signature` and acknowledged away.
      if (admitsContext === undefined || !admitsContext(from)) return [];
      return edgeVerifyKeysOf(env, from, now() / 1000);
    }
    const name = secrets[from]!;
    const current = read(name);
    if (current === undefined) return [];
    const previous = read(`${name}_PREVIOUS`);
    return previous === undefined ? [current] : [current, previous];
  };
}

/** The part of D1 an id claim uses; declared here because the Worker's `env` is a plain value. */
interface ClaimD1 {
  prepare(sql: string): { bind(...values: unknown[]): unknown & { first<T>(): Promise<T | null> } };
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
 * the table `bynk.Idempotency.claim` uses: taken for the first claim, otherwise how much longer the
 * claim that holds it lasts.
 */
export async function claimQueueMessage(env: unknown, key: string, ttlSeconds: number): Promise<QueueClaim> {
  const db = claimDatabase(env);
  const now = Date.now();
  const [claimed] = await db.batch([
    db.prepare("INSERT INTO idempotency_claims (key, expires_at) VALUES (?1, ?2) ON CONFLICT (key) DO UPDATE SET expires_at = excluded.expires_at WHERE idempotency_claims.expires_at <= ?3").bind(key, now + ttlSeconds * 1000, now),
    db.prepare("DELETE FROM idempotency_claims WHERE key IN (SELECT key FROM idempotency_claims WHERE expires_at <= ?1 LIMIT 20)").bind(now),
  ]);
  if ((claimed?.meta.changes ?? 0) > 0) return { claimed: true };
  // Held: read how long the holder's claim lasts, to tell a message mid-handler from one already handled.
  const holder = await db.prepare("SELECT expires_at FROM idempotency_claims WHERE key = ?1").bind(key).first<{ expires_at: number }>();
  return { claimed: false, remainingSeconds: Math.max(0, (Number(holder?.expires_at ?? now) - now) / 1000) };
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
  admitsContext?: (context: string) => boolean,
  now?: () => number,
): Promise<QueueAdmission> {
  const ports: QueueAdmitPorts = {
    now,
    keysFor: queueForeignKeys(env, secrets, admitsContext, now),
    claim: (key, ttl) => claimQueueMessage(env, key, ttl),
    confirm: (key, ttl) => confirmQueueMessage(env, key, ttl),
    release: (key) => releaseQueueMessage(env, key),
  };
  let first: QueueAdmission | undefined;
  for (const source of [queue, ...sources]) {
    const admitted = await queueAdmit(received, queue, source, ports);
    if (admitted.ok) return { ...admitted, context: !Object.prototype.hasOwnProperty.call(secrets, admitted.from) };
    if (admitted.reason === "replayed" || admitted.reason === "in_flight") return admitted;
    first ??= admitted;
  }
  return first as QueueAdmission;
}

// ---- Durable event delivery (#3898, slice 3b): a subscriber's event queue ----
//
// A `from Events(E)` service's Worker consumes its compiler-owned event queue (D2). Each message is a
// #3899 signed queue message whose body is `{ type, payload, envelope }`, signed by the context that
// declares `E` under its edge key to the subscriber. The entry verifies before it decodes, claims the
// `eventId` for the subscriber (D11), checks the body against the signer, decodes, runs the guard and
// the handler, and acknowledges or retries (D5). Nothing sends to these queues before the cut-over
// (slice 4): the legacy `/_bynk/event/` route still delivers.

/** D5: the retries of an event queue's consumer, Cloudflare's default (3b has no `delivery { }` on events). */
export const EVENT_QUEUE_RETRIES = 3;

/** D5: the delay a faulted delivery is retried after, per attempt, in seconds (Bynk's default). */
export const EVENT_QUEUE_RETRY_DELAY_SECONDS = 10;

/** Cloudflare's longest retry delay, in seconds (24 hours). */
const MAX_RETRY_DELAY_SECONDS = 86_400;

/**
 * D2 step 1: admit `received` to the subscribing service `subscriber` (`<context>.<service>`, the D11
 * key's consumer) from the event queue `queue` (the deployed name, which the signature covers). Exactly
 * one sender is admitted: `owner`, the context that declares the event, signing under its edge key.
 * The id claim is the Worker's `D1_IDEMPOTENCY`, as `admitQueueBody`'s is; an event queue has no
 * dead-letter sources to try, so `queueAdmit` is called once, directly.
 */
export function admitEventMessage(
  received: unknown,
  subscriber: string,
  queue: string,
  owner: string,
  env: unknown,
  now?: () => number,
): Promise<QueueAdmission> {
  const ports: QueueAdmitPorts = {
    now,
    keysFor: queueForeignKeys(env, {}, (context) => context === owner, now),
    claim: (key, ttl) => claimQueueMessage(env, key, ttl),
    confirm: (key, ttl) => confirmQueueMessage(env, key, ttl),
    release: (key) => releaseQueueMessage(env, key),
  };
  return queueAdmit(received, subscriber, queue, ports);
}

/** D9: whether `e` is D1's documented daily-limit error (Workers Free's row read or write cap). */
export function isD1DailyLimit(e: unknown): boolean {
  const text = e instanceof Error ? e.message : String(e);
  return /exceeded D1's free tier daily row (read|write) limit/i.test(text);
}

/** D9: the seconds from `nowMs` to the next 00:00 UTC, when D1's daily limit resets, capped at 24 hours. */
export function secondsUntilUtcMidnight(nowMs: number): number {
  const day = 86_400_000;
  const next = Math.floor(nowMs / day) * day + day;
  return Math.min(MAX_RETRY_DELAY_SECONDS, Math.max(1, Math.ceil((next - nowMs) / 1000)));
}

/** D5: `retryDelay × attempts`, clamped to Cloudflare's range of 0 to 24 hours. */
export function eventRetryDelaySeconds(retryDelaySeconds: number, attempts: number): number {
  const wanted = retryDelaySeconds * Math.max(1, attempts);
  return Number.isFinite(wanted) ? Math.min(MAX_RETRY_DELAY_SECONDS, Math.max(0, Math.ceil(wanted))) : MAX_RETRY_DELAY_SECONDS;
}

let d1LimitLogged = false;

/** One message of an event queue's batch, typed structurally as the entry's `batch` is. */
export interface EventQueueMessage {
  readonly body: unknown;
  readonly attempts: number;
  ack(): void;
  retry(options?: { delaySeconds: number }): void;
}

/** What the generated entry knows about one subscribing service at compile time. */
export interface EventQueueRoute {
  /** The subscribing service, qualified: `<context>.<service>`. */
  readonly subscriber: string;
  /** The event the service subscribes to, as the body's `type` names it. */
  readonly event: string;
  /** The context that declares the event, the only sender admitted (ADR 0289). */
  readonly owner: string;
  /** The `__BYNK_DLQ_<QUEUE>` producer binding of the service's dead-letter queue. */
  readonly deadLetter: string;
  readonly retries?: number;
  readonly retryDelaySeconds?: number;
}

/** The verified body decoded: the envelope, and the payload in the subscriber's own type. */
export function decodeEventBody(
  body: unknown,
  decodePayload: (payload: JsonValue) => Result<unknown, unknown>,
): Result<{ readonly payload: unknown; readonly envelope: { readonly eventId: string; readonly trace: Option<string> } }, unknown> {
  const { payload, envelope } = body as { payload: JsonValue; envelope: JsonValue };
  const env = deserialiseEventEnvelope(envelope ?? null, "$.envelope");
  if (env.tag === "Err") return env;
  const decoded = decodePayload(payload ?? null);
  if (decoded.tag === "Err") return decoded;
  return { tag: "Ok", value: { payload: decoded.value, envelope: env.value } };
}

function faultText(e: unknown): unknown {
  return e instanceof Error ? (e.stack || String(e)) : e;
}

/**
 * Settle one message of a subscriber's event queue. `queue` is `batch.queue`, the deployed name.
 * `report` reports a fault to the platform (the entry's `__bynkReportFault`). `decodePayload` is the
 * subscriber's own deserialiser of the event's payload (the envelope's is the runtime's); `run` runs the generated subscriber handler,
 * whose prologue is the pattern and `via schema` guard, so an excluded event completes as handled.
 *
 * - A refusal (unsigned, malformed, another version, a sender that is not the owner, a bad signature,
 *   or `stale`): acknowledged, logged and reported as a fault; never handled, retried or dead-lettered (D2).
 * - `replayed`: acknowledged without running anything (D11). `in_flight`: retried after the lease.
 * - Verified, but its `publisherId` or `type` disagrees with the signer, or it does not decode
 *   (deploy skew): sent unchanged to the dead-letter queue, acknowledged, and its claim released,
 *   so a redrive after the fix is admitted once (D2, D5).
 * - Handled (or excluded by the guard): the claim is confirmed, then the message acknowledged.
 * - A fault: the claim is released and the message retried after `retryDelay × attempts`; the last
 *   attempt logs `EventDelivery dead-lettered`, since the platform then moves it to the dead-letter queue.
 * - A claim that fails on D1's daily limit is retried at the next 00:00 UTC (D9): never run unchecked.
 */
export async function deliverEventMessage(
  msg: EventQueueMessage,
  queue: string,
  route: EventQueueRoute,
  env: unknown,
  report: (e: unknown) => void,
  decodePayload: (payload: JsonValue) => Result<unknown, unknown>,
  run: (payload: unknown, envelope: unknown) => Promise<unknown>,
  now: () => number = Date.now,
): Promise<void> {
  const retries = route.retries ?? EVENT_QUEUE_RETRIES;
  const delay = route.retryDelaySeconds ?? EVENT_QUEUE_RETRY_DELAY_SECONDS;
  const fields = { event: route.event, subscriber: route.subscriber };
  const faulted = (e: unknown, eventId: string | undefined): void => {
    console.error("EventDelivery faulted", { ...fields, eventId, attempts: msg.attempts }, faultText(e));
    report(e);
    if (msg.attempts >= retries + 1) {
      console.error("EventDelivery dead-lettered", { ...fields, eventId, attempts: msg.attempts });
    }
    msg.retry({ delaySeconds: eventRetryDelaySeconds(delay, msg.attempts) });
  };
  let admission: QueueAdmission;
  try {
    admission = await admitEventMessage(msg.body, route.subscriber, queue, route.owner, env, now);
  } catch (e) {
    // D9: deduplication fails closed. A claim refused by D1's daily limit waits out the reset
    // rather than spending its retries in a burst; any other failure is a fault.
    if (isD1DailyLimit(e)) {
      if (!d1LimitLogged) {
        d1LimitLogged = true;
        console.error("EventDelivery deferred: D1 daily limit", fields);
      }
      msg.retry({ delaySeconds: secondsUntilUtcMidnight(now()) });
      return;
    }
    faulted(e, undefined);
    return;
  }
  if (!admission.ok) {
    if (admission.reason === "in_flight") {
      console.error("EventDelivery deferred", { ...fields, reason: "in_flight" });
      msg.retry({ delaySeconds: Math.min(MAX_RETRY_DELAY_SECONDS, admission.retryAfterSeconds ?? 1) });
      return;
    }
    if (admission.reason === "replayed") {
      console.log("EventDelivery replayed", fields);
      msg.ack();
      return;
    }
    console.error("EventDelivery refused", { ...fields, reason: admission.reason });
    report(new Error(queueRefusalDetail(route.subscriber, admission.reason)));
    msg.ack();
    return;
  }
  const settle = admission.settle;
  const deadLetter = async (why: string, detail: Record<string, unknown>): Promise<void> => {
    const dlq = env !== null && typeof env === "object"
      ? ((env as Record<string, unknown>)[route.deadLetter] as { send(body: unknown, options?: { contentType?: string }): Promise<void> } | undefined)
      : undefined;
    try {
      // Released before anything else, so a redrive (or this message's retry) is admitted again.
      await settle(false);
      if (dlq === undefined) throw new Error(`the producer binding ${route.deadLetter} is missing from env`);
      await dlq.send(msg.body, { contentType: "json" });
    } catch (e) {
      console.error("EventDelivery dead-letter failed", { ...fields, eventId: admission.id }, faultText(e));
      msg.retry({ delaySeconds: eventRetryDelaySeconds(delay, msg.attempts) });
      return;
    }
    console.error(`EventDelivery ${why}`, { ...fields, eventId: admission.id, ...detail, outcome: "dead-lettered" });
    report(new Error(`event ${route.event} to ${route.subscriber} was dead-lettered: ${why}`));
    msg.ack();
  };
  // D2 step 2: the body must agree with its verified signer. A disagreement is a compiler or deploy
  // bug, not an attack (the signature held), so it is dead-lettered for a redrive, not dropped.
  const body = admission.body as { type?: unknown; envelope?: { publisherId?: unknown } } | null;
  const type = typeof body === "object" && body !== null ? body.type : undefined;
  const publisherId = typeof body === "object" && body !== null && typeof body.envelope === "object" && body.envelope !== null ? body.envelope.publisherId : undefined;
  if (type !== route.event || publisherId !== admission.from) {
    await deadLetter("mismatched", { type: typeof type === "string" ? type : null, publisherId: typeof publisherId === "string" ? publisherId : null, from: admission.from });
    return;
  }
  const decoded = decodeEventBody(admission.body, decodePayload);
  if (decoded.tag === "Err") {
    const cause = decoded.error as { readonly kind?: unknown; readonly path?: unknown } | null;
    await deadLetter("undecodable", {
      path: typeof cause === "object" && cause !== null && typeof cause.path === "string" ? cause.path : "$",
      kind: typeof cause === "object" && cause !== null && typeof cause.kind === "string" ? cause.kind : "",
    });
    return;
  }
  const { payload, envelope } = decoded.value;
  // #3895: the handler runs in the emission's trace, which the signed envelope carries.
  const handle = async (): Promise<void> => {
    await run(payload, envelope);
    await settle(true);
  };
  try {
    await (envelope.trace.tag === "Some" ? runInTrace(envelope.trace.value, handle) : handle());
  } catch (e) {
    try {
      await settle(false);
    } catch (released) {
      // A claim that cannot be released lapses with its lease; the retry waits it out as `in_flight`.
      console.error("EventDelivery release failed", { ...fields, eventId: admission.id }, faultText(released));
    }
    faulted(e, admission.id);
    return;
  }
  msg.ack();
}

// #1653: the `__` aliases emitted code imports.
export { admitQueueBody as __bynkQueueAdmit, deliverEventMessage as __bynkEventQueueDeliver };

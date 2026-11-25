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

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

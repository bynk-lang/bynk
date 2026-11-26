import { EDGE_FROM_PREFIX, edgeKeyOf } from "../boundary.ts";
import { canonicalJson, signQueueMessage } from "../queue_signing.ts";

// #3899: the test-only producer of a signed queue message. A `system` case that addresses a signed
// consumer `by <ForeignActor>` sends its message as that actor would: wrapped in the signed envelope,
// under the actor's secret read from the Worker's `env`. The message id is a digest of the sender, the
// queue and the message, so sending the same message twice replays it (the consumer refuses the second),
// and a different message is a different id. With no actor the message is sent unsigned.
//
// An actor that is a `Producer` refinement has no secret of its own: the case names the Bynk context it
// sends as (`by Orders("shop.orders")`, `identity`), which signs under its edge key to the consumer.
export async function __bynkSysQueueBody(env: unknown, secrets: Readonly<Record<string, string>>, as: string | undefined, queue: string, body: unknown, identity?: string): Promise<unknown> {
  if (as === undefined) return body;
  const name = Object.prototype.hasOwnProperty.call(secrets, as) ? secrets[as] : undefined;
  const context = name === undefined && typeof identity === "string";
  const from = context ? identity : as;
  const secret = name !== undefined ? (env as Record<string, unknown>)[name] : context ? edgeKeyOf(env, EDGE_FROM_PREFIX, identity) : undefined;
  if (typeof secret !== "string" || secret === "") {
    throw new globalThis.Error(context ? `the test environment has no edge key for the producing context "${identity}"` : `the test environment has no secret for the foreign producer "${as}"`);
  }
  const wire: unknown = globalThis.JSON.parse(globalThis.JSON.stringify(body ?? null));
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(`${from}\n${queue}\n${canonicalJson(wire)}`)));
  let id = "";
  for (const b of digest) id += b.toString(16).padStart(2, "0");
  return signQueueMessage(secret, { from, id, ts: Math.floor(Date.now() / 1000), queue, body: wire });
}

// #3899: the test-only producer of a signed queue message. A `system` case that addresses a signed
// consumer `by <ForeignActor>` sends its message as that actor would: wrapped in the signed envelope,
// under the actor's secret read from the Worker's `env`. The message id is a digest of the sender, the
// queue and the message, so sending the same message twice replays it (the consumer refuses the second),
// and a different message is a different id. With no actor the message is sent unsigned.
async function __bynkSysQueueBody(env: unknown, secrets: Readonly<Record<string, string>>, as: string | undefined, queue: string, body: unknown): Promise<unknown> {
  if (as === undefined) return body;
  const name = secrets[as];
  const secret = name === undefined ? undefined : (env as Record<string, unknown>)[name];
  if (typeof secret !== "string" || secret === "") {
    throw new globalThis.Error(`the test environment has no secret for the foreign producer "${as}"`);
  }
  const wire: unknown = globalThis.JSON.parse(globalThis.JSON.stringify(body ?? null));
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(`${as}\n${queue}\n${canonicalJson(wire)}`)));
  let id = "";
  for (const b of digest) id += b.toString(16).padStart(2, "0");
  return signQueueMessage(secret, { from: as, id, ts: Math.floor(Date.now() / 1000), queue, body: wire });
}

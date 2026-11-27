// #3899: the test-only producer of a signed queue message. A `system` case that addresses a signed
// consumer `by <ForeignActor>` sends its message as that actor would: wrapped in the signed envelope,
// under the actor's secret read from the Worker's `env`. The message id is a digest of the sender, the
// queue and the message, so sending the same message twice replays it (the consumer refuses the second),
// and a different message is a different id. With no actor the message is sent unsigned.
//
// An actor that is a `Producer` refinement has no secret of its own: the case names the Bynk context it
// sends as (`by Orders("shop.orders")`, `identity`), which signs under its edge key to the consumer.
async function __bynkSysQueueBody(env: unknown, secrets: Readonly<Record<string, string>>, as: string | undefined, queue: string, body: unknown, identity?: string): Promise<unknown> {
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

// #3898 (3b): a `system` case's `svc.event(e)` / `svc.event(e, env)`. The event is sent as its owner
// would send it to the subscriber's event queue: a signed queue message, under the owner's edge key to
// the subscriber, whose id is the envelope's `eventId` (fresh unless the case gives the envelope, so
// the same envelope twice is a replay the second time). It drives the subscriber Worker's real `queue`
// entry with a one-message batch. A delivery the entry acknowledges yields `()`; one it retries (the
// handler faulted) fails the case, as a faulting handler does at `unit`, naming the retry.
async function __bynkDeliverEvent(
  env: unknown,
  owner: string,
  queue: string,
  type: string,
  schemaVersion: number,
  payload: unknown,
  envelope: unknown,
  run: (batch: __BynkQueueBatch, exec: __BynkJobExec) => Promise<void>,
): Promise<void> {
  const secret = edgeKeyOf(env, EDGE_FROM_PREFIX, owner);
  if (typeof secret !== "string" || secret === "") throw new globalThis.Error(`the test environment has no edge key for the event's owner "${owner}"`);
  const given = envelope as { eventId: string; publisherId: string; emittedAt: number; schemaVersion: number; trace?: { tag: string; value?: string } } | undefined;
  const wireEnvelope: Record<string, unknown> = given === undefined
    ? { eventId: `test-event-${globalThis.crypto.randomUUID()}`, publisherId: owner, emittedAt: 0, schemaVersion }
    : { eventId: given.eventId, publisherId: given.publisherId, emittedAt: given.emittedAt, schemaVersion: given.schemaVersion };
  if (given?.trace?.tag === "Some") wireEnvelope.trace = given.trace.value;
  const body: unknown = globalThis.JSON.parse(globalThis.JSON.stringify({ type, payload: payload ?? null, envelope: wireEnvelope }));
  const signed = await signQueueMessage(secret, { from: owner, id: String(wireEnvelope.eventId), ts: Math.floor(Date.now() / 1000), queue, body });
  let verdict: { tag: "Ack" } | { tag: "Retry"; delaySeconds?: number } | undefined;
  const message: __BynkQueueMessage = {
    id: "bynk-test-message-1", timestamp: new globalThis.Date(0), attempts: 1, body: globalThis.JSON.parse(globalThis.JSON.stringify(signed)),
    ack(): void { verdict ??= { tag: "Ack" }; },
    retry(options?: { delaySeconds: number }): void { verdict ??= { tag: "Retry", delaySeconds: options?.delaySeconds }; },
  };
  const batch: __BynkQueueBatch = { queue, messages: [message], ackAll(): void { message.ack(); }, retryAll(): void { message.retry(); } };
  const exec = __bynkJobExec();
  // The entry's `EventDelivery …` records are the verdict's detail, not the case's output.
  const [error, log] = [globalThis.console.error, globalThis.console.log];
  const records: unknown[][] = [];
  const keep = (fallback: (...args: unknown[]) => void) => (...args: unknown[]): void => {
    if (typeof args[0] === "string" && args[0].startsWith("EventDelivery")) records.push(args);
    else fallback(...args);
  };
  globalThis.console.error = keep(error);
  globalThis.console.log = keep(log);
  try {
    await __bynkJobSettle(exec, run(batch, exec));
  } catch (e) {
    throw __bynkFault(`the subscriber of event "${type}"`, e);
  } finally {
    [globalThis.console.error, globalThis.console.log] = [error, log];
  }
  if (verdict?.tag === "Ack") return undefined;
  const faulted = records.find((r) => r[0] === "EventDelivery faulted")?.[2];
  const detail = typeof faulted === "string" ? faulted.split("\n")[0] : records.map((r) => r[0]).join(", ");
  throw new globalThis.Error(verdict === undefined
    ? `the subscriber of event "${type}" neither acknowledged nor retried it`
    : `the event "${type}" was not handled and is retried after ${verdict.delaySeconds ?? 0}s (attempt 1): ${detail}`);
}

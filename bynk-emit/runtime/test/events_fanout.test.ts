import { test } from "node:test";
import assert from "node:assert/strict";
import { dispatchToEventsFanout, type DurableObjectNamespace } from "../src/agent.ts";

// #4632: a recording fan-out namespace, and the console it logs to.
function recorder(status = 204): { binding: DurableObjectNamespace; bodies: string[] } {
  const bodies: string[] = [];
  const binding = {
    idFromName: (n: string) => n,
    get: () => ({
      fetch: async (_url: string, init: { body: string }) => {
        bodies.push(init.body);
        return new Response(null, { status });
      },
    }),
  } as unknown as DurableObjectNamespace;
  return { binding, bodies };
}

const envelope = (eventId: string) => ({ eventId, publisherId: "p", emittedAt: 1, schemaVersion: 1 });
const event = (type: string, payload: unknown, id = "e1") => ({ type, payload, envelope: envelope(id) });

async function captured(run: () => Promise<void>): Promise<unknown[][]> {
  const logged: unknown[][] = [];
  const original = console.error;
  console.error = (...args: unknown[]) => void logged.push(args);
  try {
    await run();
  } finally {
    console.error = original;
  }
  return logged;
}

test("dispatchToEventsFanout: the body is the batch as JSON", async () => {
  const { binding, bodies } = recorder();
  const events = [event("A", { id: "x" }, "e1"), event("B", { n: 2 }, "e2")];
  await dispatchToEventsFanout(binding, events);
  assert.equal(bodies.length, 1);
  assert.equal(bodies[0], JSON.stringify({ events }));
});

test("dispatchToEventsFanout: an event that does not encode is named and its siblings still go", async () => {
  const { binding, bodies } = recorder();
  const logged = await captured(() =>
    dispatchToEventsFanout(binding, [event("Plain", { id: "x" }, "e1"), event("Bad", { d: 1n }, "e2"), event("Also", { id: "y" }, "e3")]),
  );
  assert.equal(bodies.length, 1);
  const sent = JSON.parse(bodies[0]!) as { events: Array<{ type: string }> };
  assert.deepEqual(sent.events.map((e) => e.type), ["Plain", "Also"]);
  assert.equal(logged.length, 1);
  assert.equal(logged[0]![0], "EventsFanout dropped an event that does not encode");
  assert.deepEqual((logged[0]![1] as { type: string; eventId: string }).type, "Bad");
  assert.deepEqual((logged[0]![1] as { type: string; eventId: string }).eventId, "e2");
});

test("dispatchToEventsFanout: nothing is sent when no event encodes", async () => {
  const { binding, bodies } = recorder();
  await captured(() => dispatchToEventsFanout(binding, [event("Bad", { d: 1n })]));
  assert.equal(bodies.length, 0);
});

test("dispatchToEventsFanout: a failed send names the events it carried", async () => {
  const { binding } = recorder(500);
  const logged = await captured(() => dispatchToEventsFanout(binding, [event("A", {}, "e1"), event("B", {}, "e2")]));
  assert.equal(logged.length, 1);
  assert.equal(logged[0]![0], "EventsFanout dispatch failed");
  assert.deepEqual(logged[0]![1], {
    status: 500,
    events: [
      { type: "A", eventId: "e1" },
      { type: "B", eventId: "e2" },
    ],
  });
});

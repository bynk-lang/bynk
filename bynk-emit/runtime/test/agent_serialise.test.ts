import { test } from "node:test";
import assert from "node:assert/strict";
import {
  AGENT_CHAIN_HEADER,
  AGENT_CHAIN_MAX,
  AGENT_QUEUE_WAIT_MS,
  AgentQueue,
  makeIntegrationDoNamespace,
  serialiseAgentClass,
  serialiseBundleAgent,
  type DurableObjectNamespace,
} from "../src/agent.ts";
import { makeTestState, type DurableObjectState } from "../src/storage.ts";

// #3202: an agent instance runs one handler at a time.

const sleep = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms));
const quiet = async <T>(f: () => Promise<T>): Promise<T> => {
  const log = globalThis.console.error;
  globalThis.console.error = () => {};
  try {
    return await f();
  } finally {
    globalThis.console.error = log;
  }
};

type Env = Record<string, DurableObjectNamespace>;

// The load, await, commit shape the emitter produces, with an await that is not
// a storage operation between the load and the commit.
class Counter {
  state: DurableObjectState;
  env?: unknown;
  constructor(state: DurableObjectState, env?: unknown) {
    this.state = state;
    this.env = env;
  }
  async bump(): Promise<number> {
    const cur = ((await this.state.storage.get<{ n: number }>("state")) ?? { n: 0 }).n;
    await sleep(5);
    await this.state.storage.put("state", { n: cur + 1 });
    return cur + 1;
  }
  async sibling(): Promise<number> {
    await this.bump();
    return this.bump(); // a handler calling its own sibling through `this`
  }
  async fetch(request: Request): Promise<Response> {
    const m = new URL(request.url).pathname.split("/").pop() as "bump" | "sibling";
    return new Response(String(await this[m]()));
  }
}

const call = (ns: DurableObjectNamespace, key: string, path: string): Promise<Response> =>
  ns.get(ns.idFromName(key)).fetch(`https://_bynk/_bynk/agent/${path}`, { method: "POST" });
const text = async (p: Promise<Response>): Promise<string> => (await p).text();

test("without the queue, concurrent handlers lose updates (the defect)", async () => {
  const raw = new Counter(makeTestState("k"));
  await Promise.all([raw.bump(), raw.bump(), raw.bump(), raw.bump(), raw.bump()]);
  assert.equal(((await raw.state.storage.get<{ n: number }>("state")) as { n: number }).n, 1);
});

test("serialised: five concurrent handlers on one instance all count", async () => {
  const ns = makeIntegrationDoNamespace(Counter as never);
  const answers = await Promise.all([1, 2, 3, 4, 5].map(() => text(call(ns, "k", "bump"))));
  assert.deepEqual([...answers].sort(), ["1", "2", "3", "4", "5"]);
  assert.equal(await text(call(ns, "k", "bump")), "6");
});

test("serialised: instances do not wait for each other", async () => {
  const ns = makeIntegrationDoNamespace(Counter as never);
  const t0 = Date.now();
  await Promise.all(["a", "b", "c", "d"].map((k) => call(ns, k, "bump")));
  assert.ok(Date.now() - t0 < 100, "four instances run concurrently, not in series");
});

test("a sibling handler called through `this` does not deadlock", async () => {
  const ns = makeIntegrationDoNamespace(Counter as never);
  assert.equal(await text(call(ns, "k", "sibling")), "2");
});

test("par of two external calls to one instance completes", async () => {
  const ns = makeIntegrationDoNamespace(Counter as never);
  const [a, b] = await Promise.all([text(call(ns, "k", "bump")), text(call(ns, "k", "bump"))]);
  assert.deepEqual([a, b].sort(), ["1", "2"]);
});

// X reaches Y, which reaches X again.
class AgentX {
  state: DurableObjectState;
  env: Env;
  constructor(state: DurableObjectState, env: Env) {
    this.state = state;
    this.env = env;
  }
  async fetch(request: Request): Promise<Response> {
    const path = new URL(request.url).pathname.split("/").pop();
    const next =
      path === "self"
        ? call(this.env.X as DurableObjectNamespace, "x", "leaf") // its own instance, by key
        : call(this.env.Y as DurableObjectNamespace, "y", "back");
    // As the emitted dispatch does: a fault answers 500 with its message.
    const r = await next;
    return new Response(await r.text(), { status: r.status });
  }
}
class AgentY {
  state: DurableObjectState;
  env: Env;
  constructor(state: DurableObjectState, env: Env) {
    this.state = state;
    this.env = env;
  }
  async fetch(): Promise<Response> {
    const r = await call(this.env.X as DurableObjectNamespace, "x", "leaf");
    return new Response(await r.text(), { status: r.status });
  }
}

function xy(): Env {
  const env = {} as Env;
  env.X = makeIntegrationDoNamespace(AgentX as never, env);
  env.Y = makeIntegrationDoNamespace(AgentY as never, env);
  return env;
}

test("X -> Y -> X faults ReentrantAgentCall instead of hanging", async () => {
  const env = xy();
  const r = await quiet(() => call(env.X as DurableObjectNamespace, "x", "go"));
  assert.match(await r.text(), /ReentrantAgentCall/);
});

test("X calling its own instance by key faults ReentrantAgentCall", async () => {
  const env = xy();
  const r = await quiet(() => call(env.X as DurableObjectNamespace, "x", "self"));
  assert.match(await r.text(), /ReentrantAgentCall/);
});

test("the instance still serves calls after a re-entrant fault", async () => {
  const env = xy();
  await quiet(() => call(env.X as DurableObjectNamespace, "x", "go"));
  const r = await quiet(() => call(env.X as DurableObjectNamespace, "x", "go"));
  assert.match(await r.text(), /ReentrantAgentCall/, "the queue was released, so it answers, not hangs");
});

test("a different instance of the same class is not re-entrancy", async () => {
  const env = xy();
  const r = await quiet(() => call(env.X as DurableObjectNamespace, "x", "go"));
  assert.match(await r.text(), /ReentrantAgentCall/);
  class Pass {
    state: DurableObjectState;
  env: Env;
  constructor(state: DurableObjectState, env: Env) {
    this.state = state;
    this.env = env;
  }
    async fetch(): Promise<Response> {
      const other = this.env.P as DurableObjectNamespace;
      const path = this.state.id.name === "outer" ? "inner" : "leaf";
      return this.state.id.name === "outer" ? call(other, path, "x") : new Response("ok");
    }
  }
  const e2 = {} as Env;
  e2.P = makeIntegrationDoNamespace(Pass as never, e2);
  assert.equal(await text(call(e2.P as DurableObjectNamespace, "outer", "x")), "ok");
});

test("a chain header is honoured only on the internal agent route", async () => {
  const seen: string[] = [];
  class Echo {
    state: DurableObjectState;
  constructor(state: DurableObjectState) {
    this.state = state;
  }
    async fetch(request: Request): Promise<Response> {
      seen.push(new URL(request.url).pathname);
      return new Response("ok");
    }
  }
  const ns = makeIntegrationDoNamespace(Echo as never);
  const stub = ns.get(ns.idFromName("k"));
  const forged = { headers: { [AGENT_CHAIN_HEADER]: "Echo:k" } };
  // On the internal route a chain naming this instance is a re-entrant call.
  const internal = await quiet(() => stub.fetch("https://_bynk/_bynk/agent/m", forged));
  assert.match(await internal.text(), /ReentrantAgentCall/);
  // On any other route (a forwarded client request) it is ignored.
  const forwarded = await stub.fetch("https://_bynk/ws", forged);
  assert.equal(await forwarded.text(), "ok");
});

test("a call chain longer than the bound is refused", async () => {
  class Echo {
    state: DurableObjectState;
  constructor(state: DurableObjectState) {
    this.state = state;
  }
    async fetch(): Promise<Response> {
      return new Response("ok");
    }
  }
  const ns = makeIntegrationDoNamespace(Echo as never);
  const long = Array.from({ length: AGENT_CHAIN_MAX + 1 }, (_, i) => `A:${i}`).join(",");
  const r = await ns
    .get(ns.idFromName("k"))
    .fetch("https://_bynk/_bynk/agent/m", { headers: { [AGENT_CHAIN_HEADER]: long } });
  assert.match(await r.text(), /AgentCallChainTooLong/);
});

test("a call that waits too long faults AgentBusy, distinct from re-entrancy", async () => {
  class Slow {
    state: DurableObjectState;
  constructor(state: DurableObjectState) {
    this.state = state;
  }
    async fetch(): Promise<Response> {
      await sleep(300);
      return new Response("done");
    }
  }
  const ns = makeIntegrationDoNamespace(Slow as never, undefined, 50);
  const first = call(ns, "k", "m");
  const second = await quiet(() => call(ns, "k", "m"));
  const body = await second.text();
  assert.match(body, /AgentBusy: agent Slow busy: waited 0\.05 s behind 1 queued call$/);
  assert.doesNotMatch(body, /Reentrant/);
  // The timer measured queue wait only: the first call ran its full 300 ms.
  assert.equal(await text(first), "done");
  // The abandoned place was passed on, so the instance serves again.
  assert.equal(await text(call(ns, "k", "m")), "done");
});

test("AgentQueue: a handler's own run time is never timed out", async () => {
  const q = new AgentQueue();
  const r = await q.run("A", 20, async () => {
    await sleep(100);
    return "slow but alone";
  });
  assert.equal(r, "slow but alone");
});

test("the default queue wait is one named constant", () => {
  assert.equal(AGENT_QUEUE_WAIT_MS, 30_000);
});

test("bundle mode: external calls queue, a call through `this` does not", async () => {
  const state = makeTestState("k");
  const make = (): Counter => serialiseBundleAgent(state, new Counter(state));
  const r = await Promise.all([1, 2, 3, 4, 5].map(() => make().bump()));
  assert.deepEqual([...r].sort(), [1, 2, 3, 4, 5]);
  assert.equal(await make().sibling(), 7);
});

test("serialiseAgentClass passes extra constructor arguments and keeps the class name", () => {
  class Named {
    state: DurableObjectState;
  env: unknown;
  extra?: string;
  constructor(state: DurableObjectState, env: unknown, extra?: string) {
    this.state = state;
    this.env = env;
    this.extra = extra;
  }
    async fetch(): Promise<Response> {
      return new Response("ok");
    }
  }
  const S = serialiseAgentClass(Named as never) as unknown as new (...a: unknown[]) => Named;
  const inst = new S(makeTestState("k"), { A: 1 }, "x");
  assert.equal(inst.extra, "x");
  assert.deepEqual({ ...(inst.env as object) }, { A: 1 });
});

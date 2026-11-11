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

// #3189: lazy Map handlers await storage reads between the read and the write of a
// read-modify-write. The queue keeps those awaits from interleaving.
class MapCounter {
  state: DurableObjectState;
  env?: unknown;
  constructor(state: DurableObjectState, env?: unknown) {
    this.state = state;
    this.env = env;
  }
  async bump(): Promise<number> {
    const key = "m/tally/k";
    const cur = ((await this.state.storage.get<{ i: number; v: number }>(key)) ?? { i: 0, v: 0 }).v;
    await sleep(5);
    await this.state.storage.put(key, { i: 1, v: cur + 1 });
    return cur + 1;
  }
  async fetch(): Promise<Response> {
    return new Response(String(await this.bump()));
  }
}

test("map read-modify-write: raw handlers lose an update, serialised ones count both", async () => {
  const raw = new MapCounter(makeTestState("k"));
  await Promise.all([raw.bump(), raw.bump()]);
  assert.equal(((await raw.state.storage.get<{ v: number }>("m/tally/k")) as { v: number }).v, 1);
  const ns = makeIntegrationDoNamespace(MapCounter as never);
  const answers = await Promise.all([text(call(ns, "k", "bump")), text(call(ns, "k", "bump"))]);
  assert.deepEqual(answers.sort(), ["1", "2"]);
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

test("bundle mode: a call that waits too long faults AgentBusy", async () => {
  class Slow {
    state: DurableObjectState;
    constructor(state: DurableObjectState) {
      this.state = state;
    }
    async hold(): Promise<string> {
      await sleep(200);
      return "done";
    }
  }
  const state = makeTestState("k");
  const make = (): Slow => serialiseBundleAgent(state, new Slow(state), 30);
  const first = make().hold();
  await quiet(async () => {
    await assert.rejects(() => make().hold(), /AgentBusy: agent Slow busy/);
  });
  assert.equal(await first, "done");
});

test("the chain wrapper keeps the stub's other members and merges Headers and Request headers", async () => {
  const seen: Array<Record<string, string>> = [];
  class Peer {
    state: DurableObjectState;
    constructor(state: DurableObjectState) {
      this.state = state;
    }
    async fetch(request: Request): Promise<Response> {
      seen.push(Object.fromEntries(request.headers));
      return new Response("ok");
    }
  }
  const inner = makeIntegrationDoNamespace(Peer as never);
  const ns: DurableObjectNamespace = {
    idFromName: (n) => n,
    get: (id) => ({ ...inner.get(id), name: "peer" }) as never,
  };
  class Caller {
    state: DurableObjectState;
    env: Env;
    constructor(state: DurableObjectState, env: Env) {
      this.state = state;
      this.env = env;
    }
    async fetch(): Promise<Response> {
      const stub = (this.env.P as DurableObjectNamespace).get("p") as unknown as { name: string; fetch: (a: string, b?: unknown) => Promise<Response> };
      assert.equal(stub.name, "peer", "other stub members survive the wrapper");
      await stub.fetch("https://_bynk/_bynk/agent/m", { headers: new Headers({ "x-a": "1" }) });
      return new Response("ok");
    }
  }
  const env = { P: ns } as Env;
  const C = serialiseAgentClass(Caller as never) as unknown as new (s: DurableObjectState, e: Env) => Caller;
  await (new C(makeTestState("c"), env) as unknown as { fetch(r: Request): Promise<Response> }).fetch(
    new Request("https://_bynk/_bynk/agent/go"),
  );
  assert.equal(seen[0]?.["x-a"], "1", "a Headers instance is merged, not dropped");
  assert.equal(seen[0]?.[AGENT_CHAIN_HEADER], "Caller:c");
});

// A publisher awaits the events fan-out after it commits; the fan-out calls a
// subscriber that calls the publisher back.
// The shape the emitter produces for a handler that emits: events are buffered
// while the body runs, the state is committed, the buffered events go to the
// fan-out, and only the reply is left. A subscriber that calls the publisher back
// and mutates it must not lose the publisher's own write, nor be lost itself.
test("a subscriber mutating the publisher during the fan-out loses no write", async () => {
  const env = {} as Env;
  class Tally {
    state: DurableObjectState;
    env: Env;
    constructor(state: DurableObjectState, e: Env) {
      this.state = state;
      this.env = e;
    }
    async bump(emit: boolean): Promise<number> {
      const cur = ((await this.state.storage.get<{ n: number }>("state")) ?? { n: 0 }).n; // load
      await sleep(5); // a non-storage await, as a call to another agent would be
      await this.state.storage.put("state", { n: cur + 1 }); // commit
      if (emit) {
        await (this.env.FAN as DurableObjectNamespace)
          .get("singleton")
          .fetch("https://_bynk/_bynk/fanout", { method: "POST" });
      }
      return cur + 1; // only the reply is left
    }
    async fetch(request: Request): Promise<Response> {
      const path = new URL(request.url).pathname.split("/").pop();
      return new Response(String(await this.bump(path === "emit")));
    }
  }
  class Subscriber {
    state: DurableObjectState;
    constructor(state: DurableObjectState) {
      this.state = state;
    }
    async fetch(): Promise<Response> {
      // Reaches the publisher by a path that carries no chain, and mutates it.
      return call(env.TALLY as DurableObjectNamespace, "t", "quiet");
    }
  }
  env.TALLY = makeIntegrationDoNamespace(Tally as never, env, 2000);
  env.FAN = makeIntegrationDoNamespace(Subscriber as never, env, 2000);
  const tally = env.TALLY as DurableObjectNamespace;
  // One emitting handler (whose fan-out mutates again) alongside four plain ones.
  const replies = await Promise.all([
    text(call(tally, "t", "emit")),
    ...[1, 2, 3, 4].map(() => text(call(tally, "t", "quiet"))),
  ]);
  assert.equal(replies.length, 5);
  // 5 external bumps + 1 from the subscriber = 6, none overwritten.
  assert.equal(await text(call(tally, "t", "quiet")), "7");
});

test("events fan-out that calls the publisher back does not wait out the queue limit", async () => {
  const env = {} as Env;
  class Pub {
    state: DurableObjectState;
    env: Env;
    constructor(state: DurableObjectState, e: Env) {
      this.state = state;
      this.env = e;
    }
    async fetch(request: Request): Promise<Response> {
      const path = new URL(request.url).pathname.split("/").pop();
      if (path === "read") return new Response("read");
      await this.state.storage.put("state", { n: 1 }); // commit
      const r = await (this.env.FAN as DurableObjectNamespace)
        .get("singleton")
        .fetch("https://_bynk/_bynk/fanout", { method: "POST" });
      return new Response(await r.text());
    }
  }
  class Fan {
    state: DurableObjectState;
    constructor(state: DurableObjectState) {
      this.state = state;
    }
    async fetch(): Promise<Response> {
      // The subscriber reaches the publisher through a path that carries no chain.
      const pub = rawPub.get(rawPub.idFromName("p"));
      return new Response(await (await pub.fetch("https://_bynk/_bynk/agent/read", { method: "POST" })).text());
    }
  }
  env.PUB = makeIntegrationDoNamespace(Pub as never, env, 200);
  const rawPub = env.PUB as DurableObjectNamespace;
  env.FAN = makeIntegrationDoNamespace(Fan as never, env, 200);
  const t0 = Date.now();
  const r = await call(rawPub, "p", "emit");
  assert.equal(await r.text(), "read");
  assert.ok(Date.now() - t0 < 150, "the subscriber's call was served, not queued behind the fan-out");
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

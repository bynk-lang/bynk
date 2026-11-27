import { test } from "node:test";
import assert from "node:assert/strict";
import {
  AGENT_CHAIN_HEADER,
  AGENT_CHAIN_MAX,
  AGENT_QUEUE_WAIT_MS,
  AGENT_BUSY_HEADER,
  AgentQueue,
  agentBusy,
  agentBusyAfter,
  agentBusyResponse,
  callAgentOutcome,
  callDurableObjectMethod,
  reentrantAgentCall,
  serialiseAgentClass,
  serialiseBundleAgent,
  StateRegistry,
  stubAgents,
  stripRebuiltDeps,
  WIRE_SAFE_DEPS_KEYS,
  type DurableObjectNamespace,
  type DurableObjectStub,
} from "../src/agent.ts";
import { makeTestState, type DurableObjectState } from "../src/storage.ts";
import { makeIntegrationDoNamespace } from "../src/testing/bindings.ts";
import { boundaryError, rehydrationViolation } from "../src/boundary.ts";

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

// #3932: `@busyAfter(d)` lowers a class's queue wait, and a busy instance is load, not a fault.
class SlowOp {
  state: DurableObjectState;
  constructor(state: DurableObjectState) {
    this.state = state;
  }
  async hold(): Promise<string> {
    await sleep(200);
    return "done";
  }
  async fetch(): Promise<Response> {
    return new Response(JSON.stringify(await this.hold()));
  }
}

test("@busyAfter lowers a class's wait, where the class is wrapped", async () => {
  class Hurried extends SlowOp {}
  agentBusyAfter(Hurried, 30);
  const ns = makeIntegrationDoNamespace(Hurried as never);
  const first = call(ns, "k", "m");
  const second = await quiet(() => call(ns, "k", "m"));
  assert.match(await second.text(), /AgentBusy: agent Hurried busy: waited 0\.03 s/);
  assert.equal(await text(first), '"done"');
  // A class with no `@busyAfter` keeps the default, and an explicit wait still overrides.
  class Patient extends SlowOp {}
  const patient = makeIntegrationDoNamespace(Patient as never);
  const a = call(patient, "k", "m");
  assert.equal(await text(call(patient, "k", "m")), '"done"');
  assert.equal(await text(a), '"done"');
  const state = makeTestState("k");
  const make = (): Hurried => serialiseBundleAgent(state, new Hurried(state));
  const held = make().hold();
  await quiet(async () => {
    await assert.rejects(() => make().hold(), /AgentBusy: agent Hurried busy/);
  });
  assert.equal(await held, "done");
});

test("a busy instance answers 503 naming itself; a re-entrant call stays 500", async () => {
  class Busy extends SlowOp {}
  const ns = makeIntegrationDoNamespace(Busy as never, undefined, 20);
  const first = call(ns, "k", "m");
  const logged: unknown[][] = [];
  const log = globalThis.console.error;
  globalThis.console.error = (...args: unknown[]) => {
    logged.push(args);
  };
  let busy: Response;
  try {
    busy = await call(ns, "k", "m");
  } finally {
    globalThis.console.error = log;
  }
  // Logged once, as busy, with the queue depth.
  assert.deepEqual(logged, [["AgentBusy Busy", { agent: "Busy", kind: "AgentBusy", queued: 1 }]]);
  assert.equal(busy.status, 503);
  assert.equal(busy.headers.get(AGENT_BUSY_HEADER), "Busy");
  await first;
  const id = "Busy:k";
  const loop = await ns
    .get(ns.idFromName("k"))
    .fetch("https://_bynk/_bynk/agent/m", { headers: { [AGENT_CHAIN_HEADER]: id } });
  assert.equal(loop.status, 500);
  assert.equal(loop.headers.get(AGENT_BUSY_HEADER), null);
});

test("the caller reads a busy instance back as AgentBusy, which an entry answers 503 with Retry-After", async () => {
  class Busy extends SlowOp {}
  const ns = makeIntegrationDoNamespace(Busy as never, undefined, 20);
  const stub = ns.get(ns.idFromName("k"));
  const first = call(ns, "k", "m");
  const err = await quiet(() =>
    callDurableObjectMethod(stub, "m", [], {}).then(
      () => null,
      (e: unknown) => e,
    ),
  );
  await first;
  assert.deepEqual((err as { agentFault?: unknown }).agentFault, { kind: "AgentBusy", agent: "Busy" });
  const answer = agentBusyResponse(err, "POST /add/:id");
  assert.equal(answer?.status, 503);
  assert.equal(answer?.headers.get("retry-after"), "1");
  // A cross-context call's `503` carries no `X-Bynk-Fault`, so its caller reads it as unreachable.
  assert.equal(agentBusyResponse(err, "call quote")?.status, 503);
  assert.equal(agentBusyResponse(err, "call quote")?.headers.get("x-bynk-fault"), null);
  // An event delivery, a plain fault and a re-entrant call are not mapped.
  assert.equal(agentBusyResponse(err, "event audit"), null);
  assert.equal(agentBusyResponse(new Error("boom"), "POST /x"), null);
  const reentrant = Object.assign(new Error("r"), { agentFault: { kind: "ReentrantAgentCall", agent: "A" } });
  assert.equal(agentBusyResponse(reentrant, "POST /x"), null);
});

// #3933: `Agent(key).op.outcome(args)` reports an agent call's failure as a `CallOutcome`.
test("callAgentOutcome: an answer is Handled, nothing logged", async () => {
  const logged: unknown[][] = [];
  const log = globalThis.console.error;
  globalThis.console.error = (...a: unknown[]) => void logged.push(a);
  try {
    assert.deepEqual(await callAgentOutcome(Promise.resolve(3), "Basket.add"), { tag: "Handled", result: 3 });
  } finally {
    globalThis.console.error = log;
  }
  assert.deepEqual(logged, []);
});

test("callAgentOutcome: AgentBusy is Unreachable(None), a handler fault is Faulted and logged once", async () => {
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
  const make = (): Slow => serialiseBundleAgent(state, new Slow(state), 20);
  const first = make().hold();
  const busy = await quiet(() => callAgentOutcome(make().hold(), "Slow.hold"));
  assert.deepEqual(busy, { tag: "Unreachable", status: { tag: "None" } });
  assert.equal(await first, "done");

  const logged: unknown[][] = [];
  const log = globalThis.console.error;
  globalThis.console.error = (...a: unknown[]) => void logged.push(a);
  try {
    assert.deepEqual(await callAgentOutcome(Promise.reject(new Error("boom")), "Basket.add"), { tag: "Faulted" });
    // A stored state that no longer loads, and an invariant violation, are faults of the handler.
    const rehydration = rehydrationViolation("Basket", { kind: "MalformedJson", details: "x" });
    assert.deepEqual(await callAgentOutcome(Promise.reject(rehydration), "Basket.add"), { tag: "Faulted" });
  } finally {
    globalThis.console.error = log;
  }
  assert.ok(logged.some((l) => l[0] === "AgentFaulted Basket.add"));
});

test("callAgentOutcome: only the called agent's own busy queue is Unreachable, a nested agent's is Faulted", async () => {
  const logged: unknown[][] = [];
  const log = globalThis.console.error;
  globalThis.console.error = (...a: unknown[]) => void logged.push(a);
  try {
    // `A.op` was busy itself: it never started.
    assert.deepEqual(await callAgentOutcome(Promise.reject(agentBusy("A", 30_000, 1)), "A.op"), {
      tag: "Unreachable",
      status: { tag: "None" },
    });
    // `A.op` ran and called a busy `B`: the handler started, so a retry could repeat its effects.
    assert.deepEqual(await callAgentOutcome(Promise.reject(agentBusy("B", 30_000, 1)), "A.op"), { tag: "Faulted" });
  } finally {
    globalThis.console.error = log;
  }
  // The fault is logged with its error, so a handler fault stays diagnosable (#3933).
  const faulted = logged.find((l) => l[0] === "AgentFaulted A.op");
  assert.ok(faulted !== undefined);
  assert.match(String(faulted[1]), /AgentBusy: agent B busy/);
});

// #4215: a unit-tier agent stub (`stub Basket.add(_) faulted | unreachable`) is a class the test
// module emits: a method per stubbed operation that throws the outcome its clause stands for, or an
// error marked `bynkNoStub` when no clause matches, and the real operation runs.
class Stubbed {
  state: DurableObjectState;
  constructor(state: DurableObjectState) {
    this.state = state;
  }
  async add(n: number): Promise<number> {
    return n + 1;
  }
}
const stubbedAgent = (): Stubbed => {
  const state = makeTestState("k");
  return serialiseBundleAgent(state, new Stubbed(state));
};
const stubOutcome = (tag: string, agent: string) => (): never => {
  throw Object.assign(new Error("bynk: stubbed agent call"), { bynkCallStub: { tag, agent } });
};

test("agent stubs: unreachable and faulted are the outcomes the stubbed call stands for", async () => {
  stubAgents([[Stubbed, { add: stubOutcome("Unreachable", "Stubbed") }]]);
  assert.deepEqual(await callAgentOutcome(stubbedAgent().add(1), "Stubbed.add"), {
    tag: "Unreachable",
    status: { tag: "None" },
  });
  // A plain call to it faults, as a busy agent's does.
  await assert.rejects(() => stubbedAgent().add(1), /stubbed agent call/);

  stubAgents([[Stubbed, { add: stubOutcome("Faulted", "Stubbed") }]]);
  const logged: unknown[][] = [];
  const log = globalThis.console.error;
  globalThis.console.error = (...a: unknown[]) => void logged.push(a);
  try {
    assert.deepEqual(await callAgentOutcome(stubbedAgent().add(1), "Stubbed.add"), { tag: "Faulted" });
  } finally {
    globalThis.console.error = log;
  }
  assert.ok(logged.some((l) => l[0] === "AgentFaulted Stubbed.add"));
});

test("agent stubs: a call no clause matches, or that is not in scope, runs the real operation", async () => {
  const noMatch = (): never => {
    throw Object.assign(new Error("bynk: no stub clause matched"), { bynkNoStub: true });
  };
  stubAgents([[Stubbed, { add: noMatch }]]);
  assert.equal(await stubbedAgent().add(1), 2);
  // `__applies` is the stub class's own answer for the running case.
  stubAgents([[Stubbed, { __applies: () => false, add: stubOutcome("Faulted", "Stubbed") }]]);
  assert.equal(await stubbedAgent().add(1), 2);
  // An operation the stub does not name is the real one.
  stubAgents([[Stubbed, {}]]);
  assert.equal(await stubbedAgent().add(1), 2);
});

test("agent stubs: they apply to the stubbed agent alone, and a registry reset ends them", async () => {
  class Other {}
  stubAgents([[Other, { add: stubOutcome("Faulted", "Other") }]]);
  assert.equal(await stubbedAgent().add(1), 2);

  stubAgents([[Stubbed, { add: stubOutcome("Faulted", "Stubbed") }]]);
  await assert.rejects(() => stubbedAgent().add(1));
  // A history property's driver resets a registry before each sequence: the stubs stay.
  new StateRegistry<string>().reset(true);
  await assert.rejects(() => stubbedAgent().add(1));
  // A case boundary ends them.
  new StateRegistry<string>().reset();
  assert.equal(await stubbedAgent().add(1), 2);
});

test("agent stubs: the class is the key, so a same-named class of another context is not stubbed", async () => {
  const make = () => {
    class Stubbed {
      state: DurableObjectState;
      constructor(state: DurableObjectState) {
        this.state = state;
      }
      async add(n: number): Promise<number> {
        return n + 10;
      }
    }
    const state = makeTestState("k");
    return serialiseBundleAgent(state, new Stubbed(state));
  };
  stubAgents([[Stubbed, { add: stubOutcome("Faulted", "Stubbed") }]]);
  assert.equal(await make().add(1), 11);
  await assert.rejects(() => stubbedAgent().add(1));
  stubAgents([]);
});

test("agent stubs: a call nothing stubs runs in the same order with a stub registered as without", async () => {
  const run = async (): Promise<string[]> => {
    const order: string[] = [];
    class Order {
      state: DurableObjectState;
      constructor(state: DurableObjectState) {
        this.state = state;
      }
      async add(n: number): Promise<number> {
        order.push(`add${n}`);
        return n;
      }
    }
    const state = makeTestState("k");
    const agent = serialiseBundleAgent(state, new Order(state));
    const first = agent.add(1);
    order.push("after-call");
    const second = agent.add(2);
    await Promise.all([first, second]);
    return order;
  };
  stubAgents([]);
  const baseline = await run();
  // A stub of an operation this agent does not run, and a stub of another agent.
  stubAgents([[Stubbed, { other: stubOutcome("Faulted", "Stubbed") }]]);
  assert.deepEqual(await run(), baseline);
  stubAgents([]);
});

test("agent stubs: an unreachable stub of another agent, met inside a handler, is that handler's fault", async () => {
  // `A.op` ran and called a stubbed `B`: the call to `A` did start, so it is `Faulted`.
  const error = (() => {
    try {
      return stubOutcome("Unreachable", "B")();
    } catch (e) {
      return e;
    }
  })();
  assert.deepEqual(await quiet(() => callAgentOutcome(Promise.reject(error), "A.op")), { tag: "Faulted" });
});

test("callAgentOutcome: ReentrantAgentCall and a codec failure stay faults", async () => {
  const quietly = <T>(f: () => Promise<T>): Promise<T> => quiet(f);
  await quietly(async () => {
    await assert.rejects(
      () => callAgentOutcome(Promise.reject(reentrantAgentCall("Basket")), "Basket.add"),
      /ReentrantAgentCall/,
    );
    await assert.rejects(
      () => callAgentOutcome(Promise.reject(boundaryError({ kind: "MalformedJson", details: "x" })), "Basket.add"),
      /BoundaryError/,
    );
  });
});

test("callAgentOutcome: across a Durable Object boundary, a busy instance is Unreachable and a re-entrant one still faults", async () => {
  class Slow {
    state: DurableObjectState;
    constructor(state: DurableObjectState) {
      this.state = state;
    }
    async fetch(): Promise<Response> {
      await sleep(200);
      return new Response(JSON.stringify("done"));
    }
  }
  const ns = makeIntegrationDoNamespace(Slow as never, undefined, 20);
  const stub = ns.get(ns.idFromName("k"));
  const first = callDurableObjectMethod(stub, "m", [], {});
  const busy = await quiet(() => callAgentOutcome(callDurableObjectMethod(stub, "m", [], {}), "Slow.m"));
  assert.deepEqual(busy, { tag: "Unreachable", status: { tag: "None" } });
  assert.equal(await first, "done");
  // The same instance asked to re-enter itself answers a `500` naming `ReentrantAgentCall`.
  const loop = callDurableObjectMethod(
    { fetch: (u: string, init?: unknown) => stub.fetch(u, { ...(init as object), headers: { [AGENT_CHAIN_HEADER]: "Slow:k" } }) } as never,
    "m",
    [],
    {},
  );
  await quiet(async () => {
    await assert.rejects(() => callAgentOutcome(loop, "Slow.m"), /ReentrantAgentCall/);
  });
  // A `500` from a handler that faulted is Faulted.
  class Broken {
    state: DurableObjectState;
    constructor(state: DurableObjectState) {
      this.state = state;
    }
    async fetch(): Promise<Response> {
      return new Response("Internal Server Error", { status: 500 });
    }
  }
  const broken = makeIntegrationDoNamespace(Broken as never);
  const faulted = await quiet(() =>
    callAgentOutcome(callDurableObjectMethod(broken.get(broken.idFromName("k")), "m", [], {}), "Broken.m"),
  );
  assert.deepEqual(faulted, { tag: "Faulted" });
});

// #4554: no key material in an agent call's request body.

/** A stub that records the body of the one request it is sent. */
function recordingStub(): { stub: DurableObjectStub; bodies: string[] } {
  const bodies: string[] = [];
  const stub = {
    async fetch(_url: string, init?: { body?: string }): Promise<Response> {
      bodies.push(String(init?.body));
      return new Response("null", { status: 200 });
    },
  } as unknown as DurableObjectStub;
  return { stub, bodies };
}

// The deps a Worker compose builds for a context with a `SignedCookie` actor and the API-key pepper,
// plus the entries a Durable Object may legitimately be sent.
const SECRET = "s3cret-signing-key-current";
const PREVIOUS = "s3cret-signing-key-previous";
const PEPPER = "pepper-for-api-key-hashes";
const composeDeps = () => ({
  Clock: { now: 1 },
  env: { SESSION_SECRET: SECRET, VAR: "plain" },
  __session: { Visitors: { secret: SECRET, previous: PREVIOUS } },
  __apiKey: { pepper: PEPPER },
  __exec: { waitUntil() {} },
  __eventsDispatch: () => {},
  identity: "user-1",
});

test("an agent call's request body carries no signing key, previous key or pepper, by value", async () => {
  const { stub, bodies } = recordingStub();
  await callDurableObjectMethod(stub, "add", ["sid", 1], composeDeps());
  assert.equal(bodies.length, 1);
  for (const secret of [SECRET, PREVIOUS, PEPPER]) {
    assert.ok(!bodies[0]!.includes(secret), `the body must not contain ${secret}: ${bodies[0]}`);
  }
  // What the callee does read still crosses.
  assert.deepEqual(JSON.parse(bodies[0]!), {
    args: ["sid", 1],
    deps: { Clock: { now: 1 }, identity: "user-1" },
  });
});

test("a compose-private entry nobody has classified is dropped by default, at any depth", () => {
  const wire = stripRebuiltDeps({
    a: 1,
    __futureSecret: "boom",
    provider: { __alsoPrivate: "boom", kept: 2 },
  });
  assert.deepEqual(wire, { a: 1, provider: { kept: 2 } });
});

test("the wire-safe list is empty: nothing `__`-prefixed is sent to a Durable Object today", () => {
  // Adding a key here is a deliberate decision to put it in a request body; the strip's other
  // tests then need to say why it is safe.
  assert.equal(WIRE_SAFE_DEPS_KEYS.size, 0);
});

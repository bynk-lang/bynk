import { test } from "node:test";
import assert from "node:assert/strict";
import {
  serialiseAgentKey,
  StateRegistry,
  makeAgent,
  makeIntegrationDoNamespace,
  makeWorkersAgent,
  sweepAgentInstances,
  wakeForMigration,
  decodeAgentArgs,
  decodeAgentKey,
  encodeAgentResult,
  AGENT_WIRE_PASS,
  type AgentWire,
  type DurableObjectNamespace,
  type WireCodec,
} from "../src/agent.ts";
import type { BoundaryError, JsonValue } from "../src/boundary.ts";
import { Ok, Err } from "../src/result.ts";
import { makeIntegrationKv, type DurableObjectState } from "../src/storage.ts";

test("serialiseAgentKey: strings are identity, primitives are JSON", () => {
  assert.equal(serialiseAgentKey("abc"), "abc");
  assert.equal(serialiseAgentKey(42), "42");
  assert.equal(serialiseAgentKey(true), "true");
  assert.equal(serialiseAgentKey(null), "null");
});

test("serialiseAgentKey: records are canonical regardless of field order", () => {
  assert.equal(serialiseAgentKey({ a: 1, b: 2 }), serialiseAgentKey({ b: 2, a: 1 }));
  assert.notEqual(serialiseAgentKey({ a: 1 }), serialiseAgentKey({ a: 2 }));
});

test("serialiseAgentKey: arrays recurse positionally", () => {
  assert.equal(serialiseAgentKey([1, "x", { k: 1 }]), serialiseAgentKey([1, "x", { k: 1 }]));
  assert.notEqual(serialiseAgentKey([1, 2]), serialiseAgentKey([2, 1]));
});

test("StateRegistry: same key yields same state; reset clears", () => {
  const reg = new StateRegistry<{ id: number }>();
  const a = reg.getOrCreate({ id: 1 });
  const aAgain = reg.getOrCreate({ id: 1 });
  const b = reg.getOrCreate({ id: 2 });
  assert.equal(a, aAgain);
  assert.notEqual(a, b);
  reg.reset();
  assert.notEqual(reg.getOrCreate({ id: 1 }), a);
});

test("makeAgent: absent binding takes the bundle path", () => {
  const reg = new StateRegistry<string>();
  let built: DurableObjectState | null = null;
  const agent = makeAgent(reg, undefined, "k", (state) => {
    built = state;
    return { kind: "bundle" as const };
  });
  assert.equal(agent.kind, "bundle");
  assert.ok(built);
});

test("makeAgent: present binding takes the workers proxy path", async () => {
  // A namespace whose stub echoes the method name back as JSON.
  const ns: DurableObjectNamespace = {
    idFromName: (n) => n,
    get: () => ({
      fetch: async (input: string) =>
        new Response(JSON.stringify({ called: input }), {
          headers: { "content-type": "application/json" },
        }),
    }),
  };
  interface Counter {
    bump(n: number, deps: unknown): Promise<{ called: string }>;
  }
  const agent = makeAgent<Counter>(reg(), ns, "k", () => {
    throw new Error("should not construct bundle when binding present");
  });
  const res = await agent.bump(1, {});
  assert.match(res.called, /_bynk\/agent\/bump$/);
});

function reg(): StateRegistry<unknown> {
  return new StateRegistry<unknown>();
}

test("integration namespace + workers proxy: round-trips method calls and splits deps", async () => {
  // A tiny emitted-style DO: handles POST /_bynk/agent/<method> with {args, deps}.
  const ns = makeIntegrationDoNamespace(class {
    state: DurableObjectState;
    constructor(state: DurableObjectState) {
      this.state = state;
    }
    async fetch(request: Request): Promise<Response> {
      const state = this.state;
      const url = new URL(request.url);
      const method = url.pathname.split("/").pop();
      const { args, deps } = (await request.json()) as { args: unknown[]; deps: unknown };
      if (method === "add") {
        const prev = (await state.storage.get<number>("total")) ?? 0;
        const next = prev + (args[0] as number);
        await state.storage.put("total", next);
        return Response.json({ total: next, deps });
      }
      return new Response("no such method", { status: 404 });
    }
  });

  interface Acc {
    add(n: number, deps: unknown): Promise<{ total: number; deps: unknown }>;
  }
  const agent = makeWorkersAgent<Acc>(ns, "acc-1");
  const r1 = await agent.add(3, { trace: "t1" });
  assert.deepEqual(r1, { total: 3, deps: { trace: "t1" } });
  // state accumulates per key within the namespace
  const r2 = await agent.add(4, { trace: "t2" });
  assert.equal(r2.total, 7);
  // a different key is isolated
  const other = makeWorkersAgent<Acc>(ns, "acc-2");
  assert.equal((await other.add(1, {})).total, 1);
});

// #1678: a wire table whose single method takes and returns bytes, encoded as a
// hex string so the test can see that both ends ran.
const hexWire: AgentWire = {
  echo: {
    args: [
      {
        enc: ((v: Uint8Array) => Buffer.from(v).toString("hex")) as (v: never) => JsonValue,
        dec: (j: JsonValue) =>
          typeof j === "string"
            ? Ok(new Uint8Array(Buffer.from(j, "hex")))
            : Err({ kind: "StructuralMismatch", path: "$", expected: "hex", actual: typeof j } as BoundaryError),
      },
    ],
    result: {
      enc: ((v: Uint8Array) => Buffer.from(v).toString("hex")) as (v: never) => JsonValue,
      dec: (j: JsonValue) =>
        typeof j === "string"
          ? Ok(new Uint8Array(Buffer.from(j, "hex")))
          : Err({ kind: "StructuralMismatch", path: "$", expected: "hex", actual: typeof j } as BoundaryError),
    },
  },
};

// An emitted-style DO that decodes and encodes through the same table.
function hexEchoNamespace(): DurableObjectNamespace {
  return makeIntegrationDoNamespace(class {
    async fetch(request: Request): Promise<Response> {
      const method = new URL(request.url).pathname.slice("/_bynk/agent/".length);
      const { args } = (await request.json()) as { args: unknown[]; deps: unknown };
      const decoded = decodeAgentArgs(hexWire, method, args);
      return Response.json(encodeAgentResult(hexWire, method, decoded[0]));
    }
  });
}

test("#1678 workers proxy: arguments and result go through the wire table", async () => {
  interface Echo {
    echo(b: Uint8Array, deps: unknown): Promise<Uint8Array>;
  }
  const agent = makeWorkersAgent<Echo>(hexEchoNamespace(), "k", hexWire);
  const out = await agent.echo(new Uint8Array([1, 2, 255]), {});
  assert.ok(out instanceof Uint8Array, "the result is decoded back to bytes");
  assert.deepEqual([...out], [1, 2, 255]);
});

test("#1678 workers proxy: a result that fails to decode throws a boundary error", async () => {
  const ns: DurableObjectNamespace = {
    idFromName: (n) => n,
    get: () => ({ fetch: async () => Response.json(42) }),
  };
  interface Echo {
    echo(b: Uint8Array, deps: unknown): Promise<Uint8Array>;
  }
  const agent = makeWorkersAgent<Echo>(ns, "k", hexWire);
  await assert.rejects(agent.echo(new Uint8Array([1]), {}), /BoundaryError: StructuralMismatch/);
});

// #1818: an Int key's codec, so the sent key is distinguishable from the
// stringified `idFromName` name.
const intKeyCodec: WireCodec = {
  enc: ((v: number) => ({ n: v })) as (v: never) => JsonValue,
  dec: (j: JsonValue) =>
    typeof j === "object" && j !== null && "n" in j && typeof j.n === "number"
      ? Ok(j.n)
      : Err({ kind: "StructuralMismatch", path: "$", expected: "{ n }", actual: typeof j } as BoundaryError),
};

test("#1818 workers proxy: an agent that reads its key is sent it, encoded", async () => {
  const bodies: unknown[] = [];
  const ns: DurableObjectNamespace = {
    idFromName: (n) => n,
    get: () => ({
      fetch: async (_url: string, init?: RequestInit) => {
        bodies.push(JSON.parse(String(init?.body)));
        return Response.json(null);
      },
    }),
  };
  interface Reader {
    read(deps: unknown): Promise<unknown>;
  }
  await makeWorkersAgent<Reader>(ns, 7, undefined, intKeyCodec).read({});
  assert.deepEqual(bodies[0], { args: [], deps: {}, key: { n: 7 } });
  assert.equal(decodeAgentKey(intKeyCodec, (bodies[0] as { key: JsonValue }).key), 7);
  // An agent that does not read its key sends the body it always did.
  await makeWorkersAgent<Reader>(ns, 7).read({});
  assert.deepEqual(bodies[1], { args: [], deps: {} });
});

test("#1818 the DO side rejects a key that fails to decode", () => {
  assert.throws(() => decodeAgentKey(intKeyCodec, "7"), /BoundaryError: StructuralMismatch/);
});

test("#1678 a method absent from the wire table throws, including an inherited name", async () => {
  assert.throws(() => decodeAgentArgs(hexWire, "missing", []), /has no wire codec/);
  assert.throws(() => decodeAgentArgs(hexWire, "constructor", []), /has no wire codec/);
  assert.throws(() => encodeAgentResult(hexWire, "toString", 1), /has no wire codec/);
  interface Odd {
    missing(deps: unknown): Promise<unknown>;
  }
  const agent = makeWorkersAgent<Odd>(hexEchoNamespace(), "k", hexWire);
  await assert.rejects(agent.missing({}), /has no wire codec/);
});

test("#1678 the DO side rejects an argument that fails to decode", () => {
  assert.throws(() => decodeAgentArgs(hexWire, "echo", [7]), /BoundaryError: StructuralMismatch/);
});

test("#1678 a pass-through position is carried as is", () => {
  const wire: AgentWire = { m: { args: [AGENT_WIRE_PASS], result: AGENT_WIRE_PASS } };
  assert.deepEqual(decodeAgentArgs(wire, "m", [{ id: "c1" }]), [{ id: "c1" }]);
  assert.deepEqual(encodeAgentResult(wire, "m", { id: "c1" }), { id: "c1" });
});

test("integration KV: get, put, delete, and list by prefix", async () => {
  const kv = makeIntegrationKv();
  await kv.put("user:2", "bo");
  await kv.put("user:1", "al", { expirationTtl: 60 });
  await kv.put("order:1", "x");
  assert.equal(await kv.get("user:1"), "al");
  assert.equal(await kv.get("missing"), null);
  const page = await kv.list({ prefix: "user:" });
  // A key written with a TTL lists with its expiry, in epoch seconds (P1, #2590).
  assert.deepEqual(
    page.keys.map((k) => k.name),
    ["user:1", "user:2"],
  );
  assert.equal(page.list_complete, true);
  assert.equal(typeof page.keys[0].expiration, "number");
  assert.equal(page.keys[1].expiration, undefined);
  await kv.delete("user:1");
  assert.equal(await kv.get("user:1"), null);
});

test("integration KV: each namespace is its own, as each deployed Worker's is", async () => {
  const a = makeIntegrationKv();
  const b = makeIntegrationKv();
  await a.put("k", "v");
  assert.equal(await b.get("k"), null);
});

test("integration namespace: constructs the class with the env it was given", async () => {
  const env = { NAME: "participant" };
  const ns = makeIntegrationDoNamespace(class {
    env: unknown;
    constructor(_state: DurableObjectState, env?: unknown) {
      this.env = env;
    }
    async fetch(_request: Request): Promise<Response> {
      return Response.json(this.env);
    }
  }, env);
  const res = await ns.get(ns.idFromName("k")).fetch("https://_bynk/x", { method: "POST" });
  assert.deepEqual(await res.json(), env);
});

// State-migrations S4 (#2259 Decision D): the sweep's wake.

test("wakeForMigration: an older record loads and commits", async () => {
  const committed: unknown[] = [];
  const r = await wakeForMigration({ __v: 2, n: 1 }, 3, async () => ({ n: 1 }), async (s) => {
    committed.push(s);
  });
  assert.deepEqual(await r.json(), { before: 2, after: 3 });
  assert.deepEqual(committed, [{ n: 1 }]);
});

test("wakeForMigration: a current record is read, not written; absent __v is version 1", async () => {
  let commits = 0;
  const commit = async () => {
    commits++;
  };
  const current = await wakeForMigration({ __v: 3 }, 3, async () => ({}), commit);
  assert.deepEqual(await current.json(), { before: 3, after: 3 });
  assert.equal(commits, 0);
  const unmarked = await wakeForMigration({ n: 1 }, 3, async () => ({}), commit);
  assert.deepEqual(await unmarked.json(), { before: 1, after: 3 });
  assert.equal(commits, 1);
});

test("wakeForMigration: no record, and a fault reported by kind only", async () => {
  const none = await wakeForMigration(undefined, 3, async () => ({}), async () => {});
  assert.deepEqual(await none.json(), { before: null, after: null });
  const e = new Error("RehydrationViolation");
  Object.defineProperty(e, "rehydrationViolation", {
    value: { kind: "RehydrationViolation", agent: "Cart", path: "<root>", detail: { kind: "NoMigrationPath" } },
  });
  const faulted = await wakeForMigration({ __v: 1, secret: "x" }, 3, async () => {
    throw e;
  }, async () => {});
  assert.deepEqual(await faulted.json(), { before: 1, fault: "NoMigrationPath" });
});

test("sweepAgentInstances: wakes each id, reports per id, and states current", async () => {
  const fetched: string[] = [];
  const binding = {
    idFromString(id: string) {
      if (id === "bad") throw new TypeError("Invalid Durable Object ID");
      return { id };
    },
    get(id: unknown) {
      const name = (id as { id: string }).id;
      return {
        async fetch(input: string) {
          fetched.push(`${name} ${input}`);
          if (name === "gone") throw new Error("down");
          return new Response(JSON.stringify({ before: 1, after: 2 }));
        },
      };
    },
  };
  const r = await sweepAgentInstances(binding, JSON.stringify({ ids: ["a", "bad", "gone"] }), 2);
  assert.deepEqual(await r.json(), {
    current: 2,
    results: [
      { id: "a", before: 1, after: 2 },
      { id: "bad", fault: "BadId" },
      { id: "gone", fault: "Unreachable" },
    ],
  });
  assert.deepEqual(fetched, ["a https://_bynk/_bynk/migrate", "gone https://_bynk/_bynk/migrate"]);
  const empty = await sweepAgentInstances(binding, JSON.stringify({ ids: [] }), 2);
  assert.deepEqual(await empty.json(), { current: 2, results: [] });
  const bad = await sweepAgentInstances(binding, "{\"ids\": [1]}", 2);
  assert.equal(bad.status, 400);
});

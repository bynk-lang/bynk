import { test } from "node:test";
import assert from "node:assert/strict";
import {
  TRACE_PARENT_HEADER,
  TRACE_PREFIX,
  TRACE_REQUEST_HEADER,
  TRACE_STEP_LIMIT,
  traceBinding,
  traceFault,
  traceFetch,
  traceStep,
} from "../src/trace.ts";

const span = { file: "src/shop.bynk", line: 14, col: 5 };

// Capture the one `__bynk_trace:` line a request prints.
async function traced(
  inner: (request: Request, env: unknown, ctx: unknown) => Promise<Response>,
): Promise<{ response: Response | undefined; error: unknown; line: Record<string, unknown> }> {
  const lines: string[] = [];
  const log = globalThis.console.log;
  globalThis.console.log = (...a: unknown[]) => void lines.push(String(a[0]));
  let response: Response | undefined;
  let error: unknown;
  try {
    response = await traceFetch("shop", inner, new Request("http://x/orders", { method: "POST" }), {}, {});
  } catch (e) {
    error = e;
  } finally {
    globalThis.console.log = log;
  }
  assert.equal(lines.length, 1);
  assert.ok(lines[0]!.startsWith(TRACE_PREFIX));
  return { response, error, line: JSON.parse(lines[0]!.slice(TRACE_PREFIX.length)) };
}

test("a step is recorded in order and returns exactly what the call returns", async () => {
  const { response, line } = await traced(async () => {
    const a = await traceStep("capability", "Db.query", span)(Promise.resolve(7));
    const b = traceStep("call", "pay.charge", null)("sync");
    assert.equal(a, 7);
    assert.equal(b, "sync");
    return new Response("ok", { status: 201 });
  });
  assert.equal(response?.status, 201);
  assert.equal(line.context, "shop");
  assert.equal(line.route, "POST /orders");
  assert.equal(line.status, 201);
  assert.equal(line.truncated, false);
  assert.equal(line.fault, null);
  const steps = line.steps as Array<Record<string, unknown>>;
  assert.deepEqual(
    steps.map((s) => [s.kind, s.target, s.outcome]),
    [
      ["capability", "Db.query", "ok"],
      ["call", "pay.charge", "ok"],
    ],
  );
  assert.deepEqual(steps[0]!.span, span);
  assert.equal(typeof steps[0]!.ms, "number");
});

test("a faulting step records the fault and passes the same rejection through", async () => {
  const boom = new Error("boom");
  const { line } = await traced(async () => {
    await assert.rejects(traceStep("agent", "Cart.add", span)(Promise.reject(boom)), (e) => e === boom);
    traceStep("call", "never.settled", null); // begun, never finished
    traceFault(boom);
    return new Response("no", { status: 500 });
  });
  const steps = line.steps as Array<Record<string, unknown>>;
  assert.deepEqual(
    steps.map((s) => s.outcome),
    ["fault", "pending"],
  );
  assert.equal(line.status, 500);
  assert.match(String(line.fault), /boom/);
});

test("steps are bounded and the request says so", async () => {
  const { line } = await traced(async () => {
    for (let i = 0; i < TRACE_STEP_LIMIT + 5; i++) traceStep("capability", `C.op${i}`, null)(i);
    return new Response("ok");
  });
  assert.equal((line.steps as unknown[]).length, TRACE_STEP_LIMIT);
  assert.equal(line.truncated, true);
});

test("a rejection from the handler is recorded as a 500 and rethrown", async () => {
  const boom = new Error("down");
  const { error, line } = await traced(async () => {
    throw boom;
  });
  assert.equal(error, boom);
  assert.equal(line.status, 500);
  assert.match(String(line.fault), /down/);
});

test("outside a traced request a step is exactly the call", async () => {
  assert.equal(await traceStep("call", "a.b", null)(Promise.resolve(3)), 3);
  assert.equal(traceStep("call", "a.b", null)("x"), "x");
});

test("concurrent requests keep their own steps", async () => {
  const lines: string[] = [];
  const log = globalThis.console.log;
  globalThis.console.log = (...a: unknown[]) => void lines.push(String(a[0]));
  try {
    const one = (name: string, wait: number) =>
      traceFetch(
        name,
        async () => {
          await traceStep("call", `${name}.op`, null)(new Promise((r) => setTimeout(r, wait)));
          return new Response("ok");
        },
        new Request("http://x/p"),
        {},
        {},
      );
    await Promise.all([one("a", 20), one("b", 1)]);
  } finally {
    globalThis.console.log = log;
  }
  const parsed = lines.map((l) => JSON.parse(l.slice(TRACE_PREFIX.length)));
  for (const r of parsed) assert.deepEqual(r.steps.map((s: { target: string }) => s.target), [`${r.context}.op`]);
});

// The lines `traceFetch` prints while `body` runs, parsed.
async function lines(body: () => Promise<void>): Promise<Array<Record<string, unknown>>> {
  const out: string[] = [];
  const log = globalThis.console.log;
  globalThis.console.log = (...a: unknown[]) => void out.push(String(a[0]));
  try {
    await body();
  } finally {
    globalThis.console.log = log;
  }
  return out.map((l) => JSON.parse(l.slice(TRACE_PREFIX.length)));
}

test("a call carries its ids to the callee, whose record names its parent", async () => {
  let callee: ((request: Request) => Promise<Response>) | undefined;
  const binding = { fetch: (request: Request) => callee!(request) };
  const got = await lines(async () => {
    // The callee Worker: serves the internal request under its own record.
    callee = (request) =>
      traceFetch("callee", async () => new Response("ok"), request, {}, {});
    await traceFetch(
      "caller",
      async () => {
        // As the emitter emits it: begin the step, then wrap the binding.
        await traceStep("call", "callee.op", span)(
          traceBinding(binding).fetch(new Request("http://internal/_bynk/call/op", { method: "POST", body: "x" })),
        );
        return new Response("ok");
      },
      new Request("http://x/go"),
      {},
      {},
    );
  });
  const calleeLine = got.find((l) => l.context === "callee")!;
  const callerLine = got.find((l) => l.context === "caller")!;
  const step = (callerLine.steps as Array<Record<string, unknown>>)[0]!;
  assert.equal(calleeLine.parent, callerLine.id);
  assert.equal(calleeLine.id, step.request, "the step names the callee's request id");
  assert.equal(callerLine.parent, null);
});

test("a request from outside has no parent, and a binding outside a request is untouched", async () => {
  const got = await lines(async () => {
    await traceFetch("only", async () => new Response("ok"), new Request("http://x/"), {}, {});
  });
  assert.equal(got[0]!.parent, null);
  const binding = { fetch: async (_: Request) => new Response("ok") };
  assert.equal(traceBinding(binding), binding);
  void TRACE_PARENT_HEADER;
  void TRACE_REQUEST_HEADER;
});

test("only a call step carries a request id", async () => {
  const got = await lines(async () => {
    await traceFetch(
      "c",
      async () => {
        traceStep("capability", "Db.q", null)(1);
        traceStep("agent", "A.m", null)(1);
        return new Response("ok");
      },
      new Request("http://x/"),
      {},
      {},
    );
  });
  for (const s of got[0]!.steps as Array<Record<string, unknown>>) assert.equal(s.request, undefined);
});

test("a request from outside cannot choose its id or claim a parent", async () => {
  const got = await lines(async () => {
    await traceFetch(
      "c",
      async () => new Response("ok"),
      new Request("http://x/orders", {
        headers: { [TRACE_REQUEST_HEADER]: "forged-id", [TRACE_PARENT_HEADER]: "forged-parent" },
      }),
      {},
      {},
    );
  });
  assert.notEqual(got[0]!.id, "forged-id");
  assert.equal(got[0]!.parent, null);
});

// #3787: each step carries `start_ms`, its start as an offset from the request's start, on the clock
// `duration_ms` is measured on, so a client can draw a true timeline.
const sleep = (ms: number): Promise<void> => new Promise((done) => setTimeout(done, ms));

type StepLine = { start_ms: number; ms: number; target: string };

test("a step carries its start offset from the request's start, on the clock of duration_ms", async () => {
  const { line } = await traced(async () => {
    await sleep(15);
    await traceStep("capability", "Db.query", null)(sleep(5));
    return new Response("ok");
  });
  const [step] = line.steps as StepLine[];
  // It began after the handler's own 15 ms, and inside the request.
  assert.ok(step!.start_ms >= 10, `start_ms ${step!.start_ms}`);
  assert.ok(step!.start_ms + step!.ms <= (line.duration_ms as number) + 1, "the step ends inside the request");
});

test("two concurrent steps report overlapping intervals", async () => {
  const { line } = await traced(async () => {
    await Promise.all([
      traceStep("call", "other.authorise", null)(sleep(40)),
      traceStep("agent", "Ledger.tally", null)(sleep(40)),
    ]);
    return new Response("ok");
  });
  const [a, b] = line.steps as StepLine[];
  // Each began before the other ended.
  assert.ok(a!.start_ms < b!.start_ms + b!.ms, JSON.stringify(line.steps));
  assert.ok(b!.start_ms < a!.start_ms + a!.ms, JSON.stringify(line.steps));
});

test("sequential steps do not overlap, and the handler's own time is the gap between them", async () => {
  const { line } = await traced(async () => {
    await traceStep("capability", "Db.first", null)(sleep(10));
    await sleep(30); // the handler's own computation
    await traceStep("capability", "Db.second", null)(sleep(10));
    return new Response("ok");
  });
  const [a, b] = line.steps as StepLine[];
  assert.ok(a!.start_ms + a!.ms <= b!.start_ms, JSON.stringify(line.steps));
  // The gap is the handler's 30 ms, give or take timer slack.
  assert.ok(b!.start_ms - (a!.start_ms + a!.ms) >= 25, JSON.stringify(line.steps));
});

test("steps are listed in the order they began, and a step that settles last still lists first", async () => {
  const { line } = await traced(async () => {
    await Promise.all([
      traceStep("call", "slow", null)(sleep(40)),
      traceStep("call", "fast", null)(sleep(1)),
    ]);
    return new Response("ok");
  });
  const steps = line.steps as StepLine[];
  assert.deepEqual(steps.map((s) => s.target), ["slow", "fast"]);
  for (let i = 1; i < steps.length; i++) assert.ok(steps[i]!.start_ms >= steps[i - 1]!.start_ms);
});

test("a callee's steps are relative to its own start, and it stays joinable through request and parent", async () => {
  const got = await lines(async () => {
    await traceFetch(
      "callee",
      async () => {
        await sleep(20); // the callee's own time before it makes a call
        traceStep("capability", "Db.query", null)(1);
        return new Response("ok");
      },
      new Request("http://x/_bynk/call/shop.pay.charge", {
        headers: { [TRACE_REQUEST_HEADER]: "c1", [TRACE_PARENT_HEADER]: "p0" },
      }),
      {},
      {},
    );
  });
  assert.equal(got[0]!.id, "c1");
  assert.equal(got[0]!.parent, "p0");
  const [step] = got[0]!.steps as StepLine[];
  // Measured from the callee's own start (its 20 ms of work), not from anything the caller's clock said.
  assert.ok(step!.start_ms >= 15 && step!.start_ms <= (got[0]!.duration_ms as number), JSON.stringify(got[0]));
});

import { test } from "node:test";
import assert from "node:assert/strict";
import { TRACE_PREFIX, TRACE_STEP_LIMIT, traceFault, traceFetch, traceStep } from "../src/trace.ts";

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

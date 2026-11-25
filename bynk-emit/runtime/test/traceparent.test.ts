import { test } from "node:test";
import assert from "node:assert/strict";
import {
  TRACEPARENT_HEADER,
  admitTrace,
  admittedTrace,
  currentTraceId,
  formatTraceparent,
  isTraceId,
  mintTraceId,
  outgoingTraceparent,
  parseTraceparent,
  runInTrace,
} from "../src/traceparent.ts";
import {
  callService,
  deliverEvent,
  deserialiseEventEnvelope,
  eventEnvelopeOf,
  signedHeaders,
  verifyCall,
  type JsonValue,
  type ServiceBinding,
} from "../src/boundary.ts";
import { Ok, None, Some, type Result } from "../src/result.ts";

const TRACE = "0af7651916cd43dd8448eb211c80319c";
const SPAN = "b7ad6b7169203331";

test("mintTraceId: 32 lowercase hex digits, not all zero, fresh each time", () => {
  const a = mintTraceId();
  assert.match(a, /^[0-9a-f]{32}$/);
  assert.ok(isTraceId(a));
  assert.notEqual(a, mintTraceId());
});

test("formatTraceparent: version 00, the trace, a fresh span, flags 01", () => {
  const a = formatTraceparent(TRACE);
  assert.match(a, new RegExp(`^00-${TRACE}-[0-9a-f]{16}-01$`));
  assert.notEqual(a, formatTraceparent(TRACE));
});

test("parseTraceparent: the trace of a well-formed header, undefined for anything else", () => {
  assert.equal(parseTraceparent(`00-${TRACE}-${SPAN}-01`), TRACE);
  assert.equal(parseTraceparent(`00-${TRACE}-${SPAN}-00`), TRACE);
  for (const bad of [
    undefined,
    null,
    "",
    "garbage",
    `01-${TRACE}-${SPAN}-01`,
    `00-${TRACE.toUpperCase()}-${SPAN}-01`,
    `00-${TRACE}-${SPAN}`,
    `00-${TRACE.slice(1)}-${SPAN}-01`,
    `00-${"0".repeat(32)}-${SPAN}-01`,
    `00-${TRACE}-${"0".repeat(16)}-01`,
    `00-${TRACE}-${SPAN}-01-extra`,
  ]) {
    assert.equal(parseTraceparent(bad), undefined, String(bad));
  }
});

test("the request scope: a trace is visible inside runInTrace, across awaits, and nowhere else", async () => {
  assert.equal(currentTraceId(), undefined);
  assert.equal(outgoingTraceparent(), undefined);
  await runInTrace(TRACE, async () => {
    assert.equal(currentTraceId(), TRACE);
    await new Promise((r) => setTimeout(r, 1));
    assert.equal(currentTraceId(), TRACE);
    assert.match(outgoingTraceparent() ?? "", new RegExp(`^00-${TRACE}-`));
  });
  assert.equal(currentTraceId(), undefined);
});

const ENV = { BYNK_CALL_TO_SHOP__PAYMENT: "k" };
const CALLEE_ENV = { BYNK_CALL_FROM_SHOP__ORDERS: "k" };
const deser = (json: JsonValue): Result<number, never> => Ok(json as number);

test("callService: sends a traceparent inside a trace, none outside, and the callee reads it only once verified", async () => {
  const seen: Array<string | null> = [];
  const binding: ServiceBinding = {
    fetch: async (req) => {
      seen.push(req.headers.get(TRACEPARENT_HEADER));
      const body = await verifyCall(req, CALLEE_ENV, "shop.payment");
      assert.equal(typeof body, "string");
      seen.push(admittedTrace(req) ?? null);
      return new Response(JSON.stringify(1));
    },
  };
  await callService(binding, "svc", null, deser, "shop.orders", "", ENV, "shop.payment");
  assert.deepEqual(seen, [null, null]);
  seen.length = 0;
  await runInTrace(TRACE, () => callService(binding, "svc", null, deser, "shop.orders", "", ENV, "shop.payment"));
  assert.match(seen[0] ?? "", new RegExp(`^00-${TRACE}-[0-9a-f]{16}-01$`));
  assert.equal(seen[1], TRACE);
});

test("admitTrace: a traceparent on a request that is not verified is never read", async () => {
  const headers = await signedHeaders(ENV, "shop.orders", "shop.payment", "/_bynk/call/svc", "", "1", "{}", "test");
  const forged = new Request("http://internal/_bynk/call/svc", {
    method: "POST",
    headers: { ...headers, "X-Bynk-Signature": "0".repeat(64), [TRACEPARENT_HEADER]: `00-${TRACE}-${SPAN}-01` },
    body: "{}",
  });
  const refused = await verifyCall(forged, CALLEE_ENV, "shop.payment");
  assert.equal(typeof refused, "object");
  assert.equal(admittedTrace(forged), undefined);
  const plain = new Request("http://localhost/", { headers: { [TRACEPARENT_HEADER]: `00-${TRACE}-${SPAN}-01` } });
  assert.equal(admittedTrace(plain), undefined);
  admitTrace(plain);
  assert.equal(admittedTrace(plain), TRACE, "admitTrace is what records it");
});

test("a malformed traceparent on a verified call is ignored, not refused", async () => {
  const headers = await signedHeaders(ENV, "shop.orders", "shop.payment", "/_bynk/call/svc", "", "1", "{}", "test");
  const req = new Request("http://internal/_bynk/call/svc", {
    method: "POST",
    headers: { ...headers, [TRACEPARENT_HEADER]: "not-a-traceparent" },
    body: "{}",
  });
  assert.equal(await verifyCall(req, CALLEE_ENV, "shop.payment"), "{}");
  assert.equal(admittedTrace(req), undefined);
});

test("deliverEvent: sends the traceparent of the envelope's trace, which the delivery outside the request has no scope for", async () => {
  let header: string | null = null;
  const binding: ServiceBinding = {
    fetch: async (req) => {
      header = req.headers.get(TRACEPARENT_HEADER);
      return new Response(null, { status: 204 });
    },
  };
  await deliverEvent(binding, "svc", {}, { trace: TRACE }, { BYNK_CALL_TO_SHOP__AUDIT: "k" }, "shop.orders", "shop.audit");
  assert.match(header ?? "", new RegExp(`^00-${TRACE}-`));
  header = null;
  await deliverEvent(binding, "svc", {}, {}, { BYNK_CALL_TO_SHOP__AUDIT: "k" }, "shop.orders", "shop.audit");
  assert.equal(header, null);
  await deliverEvent(binding, "svc", {}, { trace: "nope" }, { BYNK_CALL_TO_SHOP__AUDIT: "k" }, "shop.orders", "shop.audit");
  assert.equal(header, null, "a malformed trace is not sent");
});

const BASE = { eventId: "e", publisherId: "shop.orders", emittedAt: 1, schemaVersion: 1 };

test("deserialiseEventEnvelope: trace is optional and additive", () => {
  const some = deserialiseEventEnvelope({ ...BASE, trace: TRACE });
  assert.deepEqual(some.tag === "Ok" && some.value.trace, Some(TRACE));
  for (const extra of [{}, { trace: null }, { trace: "short" }, { trace: 7 }, { trace: "0".repeat(32) }]) {
    const r = deserialiseEventEnvelope({ ...BASE, ...extra } as JsonValue);
    assert.deepEqual(r.tag === "Ok" && r.value.trace, None, JSON.stringify(extra));
  }
});

test("eventEnvelopeOf: a minted envelope's bare id becomes an Option, and an Option is kept", () => {
  assert.deepEqual(eventEnvelopeOf({ ...BASE, trace: TRACE }).trace, Some(TRACE));
  assert.deepEqual(eventEnvelopeOf({ ...BASE, trace: undefined }).trace, None);
  assert.deepEqual(eventEnvelopeOf(BASE).trace, None);
  const kept = { ...BASE, trace: Some(TRACE) };
  assert.equal(eventEnvelopeOf(kept), kept);
});

// #3895 part 2: an entry point starts a trace, and a verified call replaces it.

import { enterTrace, withTrace, withTraceField } from "../src/traceparent.ts";

test("enterTrace: starts a fresh trace for the work it runs, and none outside it", async () => {
  assert.equal(currentTraceId(), undefined);
  const seen = await enterTrace(async () => {
    await new Promise((r) => setTimeout(r, 1));
    return currentTraceId();
  });
  assert.ok(isTraceId(seen));
  assert.equal(currentTraceId(), undefined);
  const again = await enterTrace(async () => currentTraceId());
  assert.notEqual(again, seen, "each entry starts its own trace");
});

test("enterTrace: a signed call's trace replaces the one the entry started, for the rest of the request", async () => {
  const headers = await signedHeaders(ENV, "shop.orders", "shop.payment", "/_bynk/call/svc", "", "1", "{}", "test");
  const req = new Request("http://internal/_bynk/call/svc", {
    method: "POST",
    headers: { ...headers, [TRACEPARENT_HEADER]: `00-${TRACE}-${SPAN}-01` },
    body: "{}",
  });
  const [before, after] = await enterTrace(async () => {
    const started = currentTraceId();
    assert.equal(await verifyCall(req, CALLEE_ENV, "shop.payment"), "{}");
    await new Promise((r) => setTimeout(r, 1));
    return [started, currentTraceId()];
  });
  assert.notEqual(before, TRACE);
  assert.equal(after, TRACE);
});

test("enterTrace: a public request's traceparent is not read, so the trace is the entry's own", async () => {
  const claimed = new Request("http://localhost/", { headers: { [TRACEPARENT_HEADER]: `00-${TRACE}-${SPAN}-01` } });
  const seen = await enterTrace(async () => {
    // A public route never reaches `verifyCall`, so nothing admits the header.
    assert.equal(admittedTrace(claimed), undefined);
    return currentTraceId();
  });
  assert.notEqual(seen, TRACE);
});

test("withTrace and withTraceField: a log line names the trace inside one, and is untouched outside", async () => {
  assert.equal(withTrace("CallUnreachable a -> b"), "CallUnreachable a -> b");
  const fields = { site: "s" };
  assert.equal(withTraceField(fields), fields);
  await runInTrace(TRACE, async () => {
    assert.equal(withTrace("CallUnreachable a -> b"), `CallUnreachable a -> b trace=${TRACE}`);
    assert.deepEqual(withTraceField({ site: "s", trace: "mine" }), { site: "s", trace: TRACE });
  });
});


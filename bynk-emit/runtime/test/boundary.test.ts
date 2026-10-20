import { test } from "node:test";
import assert from "node:assert/strict";
import {
  CALLER_HEADER,
  MAX_BODY_BYTES,
  SIGNATURE_HEADER,
  TIMESTAMP_HEADER,
  callService,
  checkStoredSchemaVersion,
  deliverEvent,
  deserialiseEventEnvelope,
  edgeSuffix,
  rehydrationViolation,
  reportUnknownStoredKeys,
  signedHeaders,
  verifyCall,
  WIRE_VERSION,
  WIRE_VERSION_HEADER,
  type BoundaryError,
  type ServiceBinding,
} from "../src/boundary.ts";
import { Ok, Err, type Result } from "../src/result.ts";

function bindingReturning(body: unknown, init?: ResponseInit): ServiceBinding {
  return { fetch: async () => new Response(JSON.stringify(body), init) };
}

// I2 (#1971): a caller's `env`, holding its key for the edge to `shop.payment`,
// and the callee's, holding the same key for the edge from `shop.orders`.
const ENV = { BYNK_CALL_TO_SHOP__PAYMENT: "s3cret" };
const CALLEE_ENV = { BYNK_CALL_FROM_SHOP__ORDERS: "s3cret" };

// A deserialiser that expects { ok: T } | { err: E }.
const deser =
  <T, E>() =>
  (json: unknown): Result<Result<T, E>, BoundaryError> => {
    const j = json as { ok?: T; err?: E };
    if (j && "ok" in j) return Ok(Ok(j.ok as T));
    if (j && "err" in j) return Ok(Err(j.err as E));
    return Err({ kind: "MalformedJson", details: "shape" });
  };

test("callService: unwraps a successful inner Ok", async () => {
  const r = await callService(bindingReturning({ ok: 7 }), "svc", null, deser<number, string>(), "shop.orders", "", ENV, "shop.payment");
  assert.deepEqual(r, { tag: "Ok", value: 7 });
});

test("callService: returns the inner Err to the caller", async () => {
  const r = await callService(bindingReturning({ err: "nope" }), "svc", null, deser<number, string>(), "shop.orders", "", ENV, "shop.payment");
  assert.deepEqual(r, { tag: "Err", error: "nope" });
});

test("callService: non-2xx response throws a Transport BoundaryError", async () => {
  const binding = bindingReturning({}, { status: 503 });
  await assert.rejects(
    () => callService(binding, "svc", null, deser<number, string>(), "shop.orders", "", ENV, "shop.payment"),
    (e: Error) => (e as any).boundaryError.kind === "Transport" && (e as any).boundaryError.status === 503,
  );
});

test("callService: invalid JSON body throws MalformedJson", async () => {
  const binding: ServiceBinding = { fetch: async () => new Response("not json", { status: 200 }) };
  await assert.rejects(
    () => callService(binding, "svc", null, deser<number, string>(), "shop.orders", "", ENV, "shop.payment"),
    (e: Error) => (e as any).boundaryError.kind === "MalformedJson",
  );
});

test("callService: a deserialiser BoundaryError is thrown", async () => {
  const binding = bindingReturning({ unexpected: true });
  await assert.rejects(
    () => callService(binding, "svc", null, deser<number, string>(), "shop.orders", "", ENV, "shop.payment"),
    (e: Error) => (e as any).boundaryError.kind === "MalformedJson",
  );
});

test("callService: stamps the caller context header", async () => {
  let seen: string | null = null;
  const binding: ServiceBinding = {
    fetch: async (req) => {
      seen = req.headers.get("X-Bynk-Caller");
      return new Response(JSON.stringify({ ok: 1 }));
    },
  };
  await callService(binding, "svc", null, deser<number, string>(), "shop.orders", "", ENV, "shop.payment");
  assert.equal(seen, "shop.orders");
});

// v0.177 (#643): the contract seam.

test("callService: a 409 ContractMismatch surfaces as the named error", async () => {
  const binding = bindingReturning(
    {
      kind: "ContractMismatch",
      service: "whoami",
      expected: "317bdd3de84d2176",
      actual: "0000000000000000",
    },
    { status: 409 },
  );
  await assert.rejects(
    () => callService(binding, "whoami", null, deser<string, string>(), "app.a", "0000000000000000", { BYNK_CALL_TO_APP__B: "k" }, "app.b"),
    (e: Error) => {
      const be = (e as { boundaryError?: { kind: string; service?: string } }).boundaryError;
      assert.equal(be?.kind, "ContractMismatch");
      assert.equal(be?.service, "whoami");
      return true;
    },
  );
});

// #1826: the caller logs the skew, with both hashes, before it faults; a
// 409 that is not ours logs nothing.
test("callService: a 409 ContractMismatch is logged before it throws", async () => {
  const detail = {
    kind: "ContractMismatch",
    service: "whoami",
    expected: "317bdd3de84d2176",
    actual: "0000000000000000",
  };
  const logged: unknown[][] = [];
  const original = globalThis.console.error;
  globalThis.console.error = (...args: unknown[]) => {
    logged.push(args);
  };
  try {
    await assert.rejects(() =>
      callService(bindingReturning(detail, { status: 409 }), "whoami", null, deser<string, string>(), "app.a", "0000000000000000", { BYNK_CALL_TO_APP__B: "k" }, "app.b"),
    );
    await assert.rejects(() =>
      callService(bindingReturning({ kind: "SomethingElse" }, { status: 409 }), "whoami", null, deser<string, string>(), "shop.orders", "", ENV, "shop.payment"),
    );
  } finally {
    globalThis.console.error = original;
  }
  assert.deepEqual(logged, [["ContractMismatch app.a -> whoami", detail]]);
});

// The body stream is consumed on first read, so reading it twice throws
// `TypeError: Body is unusable`. A 409 that is *not* ours must still produce a
// `Transport` error naming the status — replacing that with an opaque TypeError
// from inside the runtime would bury exactly the diagnosis this seam exists to
// give.
test("callService: a 409 that is not a ContractMismatch stays a Transport error", async () => {
  const binding = bindingReturning({ kind: "SomethingElse" }, { status: 409 });
  await assert.rejects(
    () => callService(binding, "whoami", null, deser<string, string>(), "app.a", "h", { BYNK_CALL_TO_APP__B: "k" }, "app.b"),
    (e: Error) => {
      const be = (e as { boundaryError?: { kind: string; status?: number } }).boundaryError;
      assert.equal(be?.kind, "Transport", `got ${e.message}`);
      assert.equal(be?.status, 409);
      return true;
    },
  );
});

test("callService: a 409 with an unparseable body stays a Transport error", async () => {
  const binding: ServiceBinding = {
    fetch: async () => new Response("<html>gateway</html>", { status: 409 }),
  };
  await assert.rejects(
    () => callService(binding, "whoami", null, deser<string, string>(), "app.a", "h", { BYNK_CALL_TO_APP__B: "k" }, "app.b"),
    (e: Error) => {
      const be = (e as { boundaryError?: { kind: string; details?: string } }).boundaryError;
      assert.equal(be?.kind, "Transport", `got ${e.message}`);
      assert.match(be?.details ?? "", /gateway/);
      return true;
    },
  );
});

// #973: the Events boundary had no runtime validation at all — a malformed
// payload/envelope reached the subscriber's handler via a bare `as any` cast.
// `deserialiseEventEnvelope` is the envelope half of the fix (the payload
// half is an ordinary generated `deserialise_<Type>`, covered by
// `events_workers_wiring.rs`, not here).
const validEnvelope = {
  eventId: "e1",
  publisherId: "commerce.order",
  emittedAt: 1700000000000,
  schemaVersion: 1,
};

test("deserialiseEventEnvelope: accepts a well-formed envelope", () => {
  const r = deserialiseEventEnvelope(validEnvelope);
  assert.deepEqual(r, { tag: "Ok", value: validEnvelope });
});

test("deserialiseEventEnvelope: rejects a non-object", () => {
  const r = deserialiseEventEnvelope("nope");
  assert.equal(r.tag, "Err");
  assert.equal((r as { error: BoundaryError }).error.kind, "StructuralMismatch");
});

test("deserialiseEventEnvelope: rejects a missing eventId", () => {
  const { eventId: _drop, ...rest } = validEnvelope;
  const r = deserialiseEventEnvelope(rest);
  assert.equal(r.tag, "Err");
  const err = (r as { error: BoundaryError & { path: string } }).error;
  assert.equal(err.kind, "StructuralMismatch");
  assert.equal(err.path, "$.eventId");
});

test("deserialiseEventEnvelope: rejects a non-string publisherId", () => {
  const r = deserialiseEventEnvelope({ ...validEnvelope, publisherId: 7 });
  assert.equal(r.tag, "Err");
  assert.equal((r as { error: BoundaryError & { path: string } }).error.path, "$.publisherId");
});

test("deserialiseEventEnvelope: rejects a fractional emittedAt", () => {
  const r = deserialiseEventEnvelope({ ...validEnvelope, emittedAt: 1.5 });
  assert.equal(r.tag, "Err");
  const err = (r as { error: BoundaryError & { path: string; expected: string } }).error;
  assert.equal(err.path, "$.emittedAt");
  assert.equal(err.expected, "integer");
});

test("deserialiseEventEnvelope: rejects a missing schemaVersion", () => {
  const { schemaVersion: _drop, ...rest } = validEnvelope;
  const r = deserialiseEventEnvelope(rest);
  assert.equal(r.tag, "Err");
  assert.equal((r as { error: BoundaryError & { path: string } }).error.path, "$.schemaVersion");
});

test("deserialiseEventEnvelope: reports the custom path prefix on failure", () => {
  const r = deserialiseEventEnvelope("nope", "$.envelope");
  assert.equal((r as { error: BoundaryError & { path: string } }).error.path, "$.envelope");
});

// #1827: a rehydration violation is logged with the agent, the field path and
// the failure's kind, never the offending value.
test("rehydrationViolation: logs the agent, path and kind, never the value", () => {
  const logged: unknown[][] = [];
  const original = globalThis.console.error;
  globalThis.console.error = (...args: unknown[]) => {
    logged.push(args);
  };
  let e: Error;
  try {
    e = rehydrationViolation("Tracking", {
      kind: "RefinementViolation",
      path: "connections",
      violation: { field: "CustomerId", message: "must be non-empty", value: "secret-key" },
    } as BoundaryError);
  } finally {
    globalThis.console.error = original;
  }
  assert.deepEqual(logged, [
    ["RehydrationViolation Tracking", { agent: "Tracking", path: "connections", kind: "RefinementViolation" }],
  ]);
  assert.ok(!JSON.stringify(logged).includes("secret-key"));
  assert.match(e.message, /^RehydrationViolation: Tracking RefinementViolation at connections$/);
  // #1825 review: readable, but not enumerable, so logging `e` hides `detail`.
  assert.equal((e as { rehydrationViolation?: { agent: string } }).rehydrationViolation?.agent, "Tracking");
  assert.ok(!Object.keys(e).includes("rehydrationViolation"));
});

// State-migrations S1 (#2211): the `__v` marker check. An absent marker is
// version 1; `oldestReadable..=current` passes; newer is `NewerSchema`;
// anything else, malformed included, is `NoMigrationPath`.
test("checkStoredSchemaVersion: passes readable versions, faults the rest", () => {
  const quiet = globalThis.console.error;
  const logged: unknown[][] = [];
  globalThis.console.error = (...args: unknown[]) => {
    logged.push(args);
  };
  const kind = (stored: object, current: number, oldest: number): string => {
    try {
      checkStoredSchemaVersion("Cart", stored, current, oldest);
      return "ok";
    } catch (e) {
      return (e as { rehydrationViolation: { detail: { kind: string } } }).rehydrationViolation
        .detail.kind;
    }
  };
  try {
    assert.equal(kind({}, 1, 1), "ok");
    assert.equal(kind({}, 3, 1), "ok");
    assert.equal(kind({}, 3, 2), "NoMigrationPath");
    assert.equal(kind({ __v: 2 }, 3, 1), "ok");
    assert.equal(kind({ __v: 3 }, 3, 3), "ok");
    assert.equal(kind({ __v: 4 }, 3, 1), "NewerSchema");
    for (const bad of ["2", 0, -1, 1.5, null, Number.NaN]) {
      assert.equal(kind({ __v: bad }, 3, 1), "NoMigrationPath", `__v = ${String(bad)}`);
    }
  } finally {
    globalThis.console.error = quiet;
  }
  // The log line carries the versions, which are not user data.
  assert.deepEqual(logged[0], [
    "RehydrationViolation Cart",
    { agent: "Cart", path: "<root>", kind: "NoMigrationPath", stored: 1, current: 3 },
  ]);
});

// State-migrations S1 (#2211): an undeclared stored key is reported once per
// isolate per (agent, key), by name only; `__v` and the zero record's own keys
// never are.
test("reportUnknownStoredKeys: once per key, by name only", () => {
  const quiet = globalThis.console.warn;
  const warned: string[] = [];
  globalThis.console.warn = (...args: unknown[]) => {
    warned.push(args.join(" "));
  };
  try {
    const zero = { count: 0, posting: {} };
    const stored = { count: 1, posting: {}, owner: "secret-owner", __v: 2 };
    reportUnknownStoredKeys("Ledger", stored, zero);
    reportUnknownStoredKeys("Ledger", stored, zero);
    reportUnknownStoredKeys("Other", stored, zero);
  } finally {
    globalThis.console.warn = quiet;
  }
  assert.equal(warned.length, 2, warned.join("\n"));
  assert.ok(warned[0].includes('agent Ledger has stored field "owner"'));
  assert.ok(warned[1].includes('agent Other has stored field "owner"'));
  assert.ok(!warned.some((w) => w.includes("secret-owner") || w.includes("__v")));
});

// #1825 review: a fault's payload is non-enumerable, so logging the error
// object (a Worker's fault catch) prints neither a callee's body nor a value.
test("boundaryError: the payload is readable but not enumerable", async () => {
  const { inspect } = await import("node:util");
  const binding = { fetch: async () => new Response("secret-body", { status: 502 }) };
  await assert.rejects(
    () => callService(binding, "svc", null, deser<number, string>(), "shop.orders", "", ENV, "shop.payment"),
    (e: Error) => {
      assert.equal((e as { boundaryError?: { kind: string } }).boundaryError?.kind, "Transport");
      assert.ok(!Object.keys(e).includes("boundaryError"));
      assert.ok(!inspect(e).includes("secret-body"), inspect(e));
      return true;
    },
  );
});

// I2 (#1971): per-edge request signatures.

// A request as `callService` would send it, signed by `caller` for `callee`
// under `env`'s key, with `tweak` applied to the headers after signing.
async function signedRequest(opts: {
  env?: unknown;
  caller?: string;
  callee?: string;
  body?: string;
  tweak?: (h: Record<string, string>) => void;
  url?: string;
} = {}): Promise<Request> {
  const body = opts.body ?? "{}";
  const headers: Record<string, string> = {
    "X-Bynk-Contract": "0123456789abcdef",
    ...(await signedHeaders(opts.env ?? ENV, opts.caller ?? "shop.orders", opts.callee ?? "shop.payment", "/_bynk/call/svc", "0123456789abcdef", "1", body, "test")),
  };
  opts.tweak?.(headers);
  return new Request(opts.url ?? "http://internal/_bynk/call/svc", { method: "POST", headers, body });
}

test("edgeSuffix: keeps a dot and an underscore apart, and refuses ambiguous names", () => {
  assert.equal(edgeSuffix("shop.orders"), "SHOP__ORDERS");
  assert.equal(edgeSuffix("a_b.c2"), "A_B__C2");
  assert.notEqual(edgeSuffix("a.b"), edgeSuffix("a_b"));
  for (const bad of ["", "a..b", "a__b", "_a", "a_", "a._b", "A.b", "a-b", "1a", "__proto__"]) {
    assert.equal(edgeSuffix(bad), undefined, bad);
  }
});

test("verifyCall: accepts a request signed for this callee, returning its body", async () => {
  assert.equal(await verifyCall(await signedRequest({ body: "[1,2]" }), CALLEE_ENV, "shop.payment"), "[1,2]");
});

test("verifyCall: refuses a request with any signed field changed", async () => {
  const tamper: Record<string, (h: Record<string, string>) => void> = {
    caller: (h) => (h[CALLER_HEADER] = "shop.payment"),
    contract: (h) => (h["X-Bynk-Contract"] = "fedcba9876543210"),
    wire: (h) => (h["X-Bynk-Wire"] = "2"),
    timestamp: (h) => (h[TIMESTAMP_HEADER] = String(Number(h[TIMESTAMP_HEADER]) - 1)),
    signature: (h) => (h[SIGNATURE_HEADER] = h[SIGNATURE_HEADER].replace(/^./, (c) => (c === "0" ? "1" : "0"))),
  };
  for (const [what, tweak] of Object.entries(tamper)) {
    const env = { ...CALLEE_ENV, BYNK_CALL_FROM_SHOP__PAYMENT: "s3cret" };
    const res = await verifyCall(await signedRequest({ tweak }), env, "shop.payment");
    assert.ok(res instanceof Response && res.status === 401, what);
  }
  // The path and the body.
  let res = await verifyCall(await signedRequest({ url: "http://internal/_bynk/call/other" }), CALLEE_ENV, "shop.payment");
  assert.ok(res instanceof Response && res.status === 401, "path");
  const req = await signedRequest({ body: "{}" });
  const swapped = new Request(req.url, { method: "POST", headers: req.headers, body: "{ }" });
  res = await verifyCall(swapped, CALLEE_ENV, "shop.payment");
  assert.ok(res instanceof Response && res.status === 401, "body");
});

test("verifyCall: a signature for one callee does not verify at another", async () => {
  // The same key at both, so only the signed callee name tells them apart.
  const res = await verifyCall(await signedRequest(), CALLEE_ENV, "shop.ledger");
  assert.ok(res instanceof Response && res.status === 401);
});

test("verifyCall: refuses a key held for another edge, and fails closed with none", async () => {
  // Signed by `shop.orders` under its key to `shop.payment`, but this callee
  // holds a different key for `shop.orders`.
  let res = await verifyCall(await signedRequest(), { BYNK_CALL_FROM_SHOP__ORDERS: "other" }, "shop.payment");
  assert.ok(res instanceof Response && res.status === 401, "another edge's key");
  for (const env of [undefined, {}, { BYNK_CALL_FROM_SHOP__ORDERS: "" }, { BYNK_CALL_FROM_SHOP__ORDERS: 7 }]) {
    res = await verifyCall(await signedRequest(), env, "shop.payment");
    assert.ok(res instanceof Response && res.status === 401, JSON.stringify(env));
  }
  // No caller, or one outside the edge-name domain, finds no key.
  for (const caller of [undefined, "", "SHOP.ORDERS", "shop_orders"]) {
    res = await verifyCall(
      await signedRequest({ tweak: (h) => (caller === undefined ? delete h[CALLER_HEADER] : (h[CALLER_HEADER] = caller)) }),
      { ...CALLEE_ENV, BYNK_CALL_FROM_SHOP_ORDERS: "s3cret" },
      "shop.payment",
    );
    assert.ok(res instanceof Response && res.status === 401, String(caller));
  }
});

test("verifyCall: refuses a stale or future timestamp", async () => {
  const realNow = Date.now;
  try {
    for (const skew of [-301_000, 301_000]) {
      Date.now = () => realNow() + skew;
      const req = await signedRequest();
      Date.now = realNow;
      const res = await verifyCall(req, CALLEE_ENV, "shop.payment");
      assert.ok(res instanceof Response && res.status === 401, String(skew));
    }
    Date.now = () => realNow() - 299_000;
    const req = await signedRequest();
    Date.now = realNow;
    assert.equal(await verifyCall(req, CALLEE_ENV, "shop.payment"), "{}", "inside the window");
  } finally {
    Date.now = realNow;
  }
});

test("verifyCall: refuses a body over the cap with 413, before authentication", async () => {
  const big = "x".repeat(MAX_BODY_BYTES + 1);
  // Unsigned: the cap is checked before the signature. A declared length
  // over the cap is refused before the body is read at all: this body
  // faults if anything reads it. (Node does not derive `content-length` for
  // a constructed Request, so the header is set by hand, as a sender would.)
  const unread = new ReadableStream<Uint8Array>({
    pull() {
      throw new Error("the body was read");
    },
  });
  const declared = new Request("http://internal/_bynk/call/svc", {
    method: "POST",
    headers: { "content-length": String(MAX_BODY_BYTES + 1) },
    body: unread,
    duplex: "half",
  } as RequestInit);
  let res = await verifyCall(declared, {}, "shop.payment");
  assert.ok(res instanceof Response && res.status === 413, "a declared length");
  // Streamed, with no length to refuse up front.
  const stream = new ReadableStream<Uint8Array>({
    start(c) {
      c.enqueue(new TextEncoder().encode(big));
      c.close();
    },
  });
  const streamed = new Request("http://internal/_bynk/call/svc", { method: "POST", body: stream, duplex: "half" } as RequestInit);
  res = await verifyCall(streamed, {}, "shop.payment");
  assert.ok(res instanceof Response && res.status === 413, "a streamed body");
  // At the cap is fine.
  const at = "x".repeat(MAX_BODY_BYTES);
  assert.equal(await verifyCall(await signedRequest({ body: at }), CALLEE_ENV, "shop.payment"), at);
});

test("callService: signs what it sends, and the callee verifies it", async () => {
  let verified: string | Response | undefined;
  const binding: ServiceBinding = {
    fetch: async (req) => {
      verified = await verifyCall(req, CALLEE_ENV, "shop.payment");
      return new Response(JSON.stringify({ ok: 1 }));
    },
  };
  await callService(binding, "svc", { a: 1 }, deser<number, string>(), "shop.orders", "0123456789abcdef", ENV, "shop.payment");
  assert.equal(verified, JSON.stringify({ a: 1 }));
});

test("callService: states the wire version it speaks, as a POST", async () => {
  let version: string | null = null;
  let method = "";
  const binding: ServiceBinding = {
    fetch: async (req) => {
      version = req.headers.get(WIRE_VERSION_HEADER);
      method = req.method;
      return new Response(JSON.stringify({ ok: 1 }));
    },
  };
  await callService(binding, "svc", null, deser<number, string>(), "shop.orders", "", ENV, "shop.payment");
  assert.equal(version, WIRE_VERSION);
  assert.equal(version, "1");
  assert.equal(method, "POST");
});

test("callService: refuses to send without the edge's key, naming it", async () => {
  let sent = false;
  const binding: ServiceBinding = {
    fetch: async () => {
      sent = true;
      return new Response(JSON.stringify({ ok: 1 }));
    },
  };
  await assert.rejects(
    () => callService(binding, "svc", null, deser<number, string>(), "shop.orders", "", {}, "shop.payment"),
    /BYNK_CALL_TO_SHOP__PAYMENT is not set/,
  );
  assert.equal(sent, false);
});

test("deliverEvent: signs the delivery for the subscriber, as the publisher", async () => {
  let verified: string | Response | undefined;
  const binding: ServiceBinding = {
    fetch: async (req) => {
      verified = await verifyCall(req, { BYNK_CALL_FROM_SHOP__ORDERS: "s3cret" }, "shop.audit");
      return new Response(null, { status: 204 });
    },
  };
  await deliverEvent(binding, "svc", { n: 1 }, {}, { BYNK_CALL_TO_SHOP__AUDIT: "s3cret" }, "shop.orders", "shop.audit");
  assert.equal(verified, JSON.stringify({ payload: { n: 1 }, envelope: {} }));
});

// I3 (#1971): foreign callers, checked under their own declared secret.

const FOREIGN = { Legacy: "LEGACY_CALL_KEY" };

test("edgeSuffix: no capitalised name is an edge name, so a foreign actor never selects an edge key", () => {
  for (const name of ["Legacy", "L", "Legacy.orders", "LEGACY", "Ledger_2"]) {
    assert.equal(edgeSuffix(name), undefined, name);
  }
});

test("verifyCall: a foreign caller verifies under its declared secret, not an edge key", async () => {
  const req = await signedRequest({ caller: "Legacy", env: { BYNK_CALL_TO_SHOP__PAYMENT: "legacy-key" } });
  assert.equal(await verifyCall(req, { LEGACY_CALL_KEY: "legacy-key" }, "shop.payment", FOREIGN), "{}");
  // Without the table, `Legacy` is no caller this Worker knows.
  const again = await signedRequest({ caller: "Legacy", env: { BYNK_CALL_TO_SHOP__PAYMENT: "legacy-key" } });
  const res = await verifyCall(again, { LEGACY_CALL_KEY: "legacy-key" }, "shop.payment");
  assert.ok(res instanceof Response && res.status === 401);
});

test("verifyCall: a foreign caller's secret does not admit it under another name", async () => {
  // Signed as `Legacy`, under Legacy's secret, but claiming a context.
  const req = await signedRequest({
    caller: "Legacy",
    env: { BYNK_CALL_TO_SHOP__PAYMENT: "legacy-key" },
    tweak: (h) => (h[CALLER_HEADER] = "shop.orders"),
  });
  const res = await verifyCall(req, { LEGACY_CALL_KEY: "legacy-key", BYNK_CALL_FROM_SHOP__ORDERS: "legacy-key" }, "shop.payment", FOREIGN);
  assert.ok(res instanceof Response && res.status === 401);
  // A caller named after an Object.prototype member finds no key.
  for (const name of ["constructor", "toString", "__proto__"]) {
    const r = await verifyCall(await signedRequest({ tweak: (h) => (h[CALLER_HEADER] = name) }), CALLEE_ENV, "shop.payment", FOREIGN);
    assert.ok(r instanceof Response && r.status === 401, name);
  }
  // An unset secret fails closed.
  const unset = await signedRequest({ caller: "Legacy", env: { BYNK_CALL_TO_SHOP__PAYMENT: "legacy-key" } });
  const r = await verifyCall(unset, {}, "shop.payment", FOREIGN);
  assert.ok(r instanceof Response && r.status === 401);
});

// #3051: the case-side `TestConnection` — what `svc.open(tc, …)` remembers, and
// the one-owner rule that stops a handle being opened and handed out too.
import { test } from "node:test";
import assert from "node:assert/strict";
import { TestConnection } from "../src/connection.ts";

test("message/close before open fail the case", () => {
  const tc = new TestConnection<string>();
  assert.throws(() => tc.opened(), /before `open`/);
});

test("open remembers the route values and deps", () => {
  const tc = new TestConnection<string>();
  tc.remember(["room-1"], { identity: "alice" });
  assert.deepEqual(tc.opened(), { route: ["room-1"], deps: { identity: "alice" } });
});

test("a connection opens once", () => {
  const tc = new TestConnection<string>();
  tc.remember([], {});
  assert.throws(() => tc.remember([], {}), /already opened/);
});

test("a connection has one owner: connection() once, and not after open", () => {
  const a = new TestConnection<string>();
  a.mint();
  assert.throws(() => a.mint(), /one socket has one owner/);
  assert.throws(() => a.remember([], {}), /already opened or handed/);
  const b = new TestConnection<string>();
  b.remember([], {});
  assert.throws(() => b.mint(), /already handed out or opened/);
});

test("sent frames are recorded and a closed connection refuses more", async () => {
  const tc = new TestConnection<string>();
  await tc.send("a");
  await tc.close();
  assert.deepEqual(tc.sent, ["a"]);
  assert.equal(tc.closed, true);
  await assert.rejects(tc.send("b"), /closed/);
});

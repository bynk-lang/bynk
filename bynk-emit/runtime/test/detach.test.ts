import test from "node:test";
import assert from "node:assert/strict";
import { __bynkDetach } from "../src/settle.ts";
import type { __BynkDetachReport } from "../src/settle.ts";

// #3437: `e.detach()` hands a running effect to `waitUntil` and reports a failure.
function exec() {
  const handed: Promise<unknown>[] = [];
  const reports: __BynkDetachReport[] = [];
  return {
    handed,
    reports,
    waitUntil: (p: Promise<unknown>) => void handed.push(p),
    report: (r: __BynkDetachReport) => void reports.push(r),
  };
}

test("detach returns at once, with the effect handed to waitUntil", async () => {
  const e = exec();
  let finished = false;
  const slow = new Promise<number>((r) => setTimeout(() => ((finished = true), r(1)), 20));
  await __bynkDetach(slow, e, "s:1:1");
  assert.equal(finished, false, "detach does not wait");
  assert.equal(e.handed.length, 1);
  await Promise.allSettled(e.handed);
  assert.equal(finished, true);
  assert.deepEqual(e.reports, []);
});

test("an Ok and a non-Result payload report nothing", async () => {
  const e = exec();
  await __bynkDetach(Promise.resolve({ tag: "Ok", value: 1 }), e, "s:1:1", "MailError");
  await __bynkDetach(Promise.resolve(5), e, "s:1:1");
  await __bynkDetach(Promise.resolve({ tag: "Err", error: 1 }), e, "s:1:1"); // not a Result: no errType
  await Promise.allSettled(e.handed);
  assert.deepEqual(e.reports, []);
});

test("an Err reports the type and variant, never the payload", async () => {
  const e = exec();
  await __bynkDetach(
    Promise.resolve({ tag: "Err", error: { tag: "Bounced", reason: "https://x/?key=secret" } }),
    e,
    "s:2:3",
    "MailError",
  );
  await Promise.allSettled(e.handed);
  assert.deepEqual(e.reports, [{ kind: "err", site: "s:2:3", type: "MailError", variant: "Bounced" }]);
  assert.doesNotMatch(JSON.stringify(e.reports), /secret/);
});

test("an Err whose error has no tag reports a null variant", async () => {
  const e = exec();
  await __bynkDetach(Promise.resolve({ tag: "Err", error: { after: 5 } }), e, "s:1:1", "TimedOut");
  await Promise.allSettled(e.handed);
  assert.deepEqual(e.reports, [{ kind: "err", site: "s:1:1", type: "TimedOut", variant: null }]);
});

test("without a report hook an Err is one structured console.error record", async () => {
  const records: unknown[] = [];
  const original = console.error;
  console.error = (r: unknown) => void records.push(r);
  try {
    const handed: Promise<unknown>[] = [];
    await __bynkDetach(
      Promise.resolve({ tag: "Err", error: { tag: "Refused" } }),
      { waitUntil: (p) => void handed.push(p) },
      "s:1:1",
      "MailError",
    );
    await Promise.allSettled(handed);
  } finally {
    console.error = original;
  }
  assert.deepEqual(records, [
    {
      message: "a detached effect returned Err",
      level: "error",
      fields: { site: "s:1:1", type: "MailError", variant: "Refused" },
    },
  ]);
});

test("a fault is passed on unchanged as a rejection into waitUntil, labelled with the site", async () => {
  const e = exec();
  const boom = new Error("boom");
  await __bynkDetach(Promise.reject(boom), e, "s:9:9");
  // handled: no unhandled rejection, and the rejection reaches waitUntil intact
  await assert.rejects(e.handed[0]!, (x) => x === boom);
  assert.equal((boom as unknown as { detachedAt: string }).detachedAt, "s:9:9");
  assert.deepEqual(e.reports, [{ kind: "fault", site: "s:9:9", type: null, variant: null }]);
});

test("a frozen or non-object fault is still passed on", async () => {
  const e = exec();
  const frozen = Object.freeze(new Error("frozen"));
  await __bynkDetach(Promise.reject(frozen), e, "s:1:1");
  await __bynkDetach(Promise.reject("plain"), e, "s:1:1");
  await assert.rejects(e.handed[0]!, (x) => x === frozen);
  await assert.rejects(e.handed[1]!, (x) => x === "plain");
});

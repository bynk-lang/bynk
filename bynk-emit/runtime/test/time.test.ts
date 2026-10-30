import { test } from "node:test";
import assert from "node:assert/strict";
import { Some, None } from "../src/result.ts";
import { __bynkInstantFromIso, __bynkInstantToIso } from "../src/time.ts";

// Standard-library K3 (#2592).
test("toIso writes UTC with three fraction digits and Z", () => {
  assert.equal(__bynkInstantToIso(0), "1970-01-01T00:00:00.000Z");
  assert.equal(__bynkInstantToIso(-1), "1969-12-31T23:59:59.999Z");
  assert.equal(__bynkInstantToIso(Date.UTC(2028, 1, 29, 12, 30, 5, 7)), "2028-02-29T12:30:05.007Z");
  assert.equal(__bynkInstantToIso(Date.UTC(9999, 11, 31, 23, 59, 59, 999)), "9999-12-31T23:59:59.999Z");
  assert.equal(__bynkInstantToIso(Date.UTC(10000, 0, 1)), "+010000-01-01T00:00:00.000Z");
  assert.equal(__bynkInstantToIso(-62167219200000), "0000-01-01T00:00:00.000Z");
});

test("toIso faults past the range a date can name", () => {
  assert.equal(__bynkInstantToIso(8.64e15), "+275760-09-13T00:00:00.000Z");
  assert.throws(() => __bynkInstantToIso(8.64e15 + 1), /beyond the range a date can name/);
  assert.throws(() => __bynkInstantToIso(-8.64e15 - 1), /beyond the range a date can name/);
});

test("fromIso reads the RFC 3339 profile, converting an offset to UTC", () => {
  const at = Date.UTC(2026, 9, 30, 12, 0, 0);
  for (const s of [
    "2026-10-30T12:00:00Z",
    "2026-10-30t12:00:00z",
    "2026-10-30T12:00:00.000Z",
    "2026-10-30T14:00:00+02:00",
    "2026-10-30T07:30:00-04:30",
    "2026-10-30T12:00:00-00:00",
  ]) {
    assert.deepEqual(__bynkInstantFromIso(s), Some(at), s);
  }
  assert.deepEqual(__bynkInstantFromIso("2026-10-30T12:00:00.5Z"), Some(at + 500));
  // Truncated toward the earlier instant, never rounded up.
  assert.deepEqual(__bynkInstantFromIso("2026-10-30T12:00:00.9999Z"), Some(at + 999));
  assert.deepEqual(__bynkInstantFromIso("2028-02-29T00:00:00Z"), Some(Date.UTC(2028, 1, 29)));
  assert.deepEqual(__bynkInstantFromIso("2000-02-29T00:00:00Z"), Some(Date.UTC(2000, 1, 29)));
  assert.deepEqual(__bynkInstantFromIso("0042-01-01T00:00:00Z"), Some(-60841756800000));
});

test("fromIso is None outside the profile or the calendar", () => {
  for (const s of [
    "",
    "2026-10-30",
    "12:00:00Z",
    "2026-10-30 12:00:00Z",
    "2026-10-30T12:00:00",
    "20261030T120000Z",
    "2026-W44-5T12:00:00Z",
    "2026-303T12:00:00Z",
    "+002026-10-30T12:00:00Z",
    "2026-10-30T12:00Z",
    "2026-10-30T12:00:00.Z",
    "2026-10-30T12:00:00+0200",
    "2026-13-01T00:00:00Z",
    "2026-00-01T00:00:00Z",
    "2026-02-29T00:00:00Z",
    "1900-02-29T00:00:00Z",
    "2026-04-31T00:00:00Z",
    "2026-10-30T24:00:00Z",
    "2026-10-30T12:60:00Z",
    "2026-12-31T23:59:60Z",
    "2026-10-30T12:00:00+24:00",
    "2026-10-30T12:00:00+02:60",
    " 2026-10-30T12:00:00Z",
  ]) {
    assert.deepEqual(__bynkInstantFromIso(s), None, s);
  }
});

test("fromIso(toIso(i)) is Some(i) across years 0000 to 9999", () => {
  const low = -62167219200000;
  const high = Date.UTC(9999, 11, 31, 23, 59, 59, 999);
  const samples = [low, high, 0, -1, 1, 951782400000, 1793348090123];
  // A fixed stride, odd in milliseconds, so every run checks the same 2000
  // instants spread over the whole range.
  const stride = Math.floor((high - low) / 2000) + 7;
  for (let i = 0; i < 2000; i++) {
    samples.push(low + i * stride);
  }
  for (const ms of samples) {
    assert.deepEqual(__bynkInstantFromIso(__bynkInstantToIso(ms)), Some(ms), String(ms));
  }
});

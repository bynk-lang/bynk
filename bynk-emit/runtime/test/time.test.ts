import { test } from "node:test";
import assert from "node:assert/strict";
import { Some, None } from "../src/result.ts";
import { __bynkInstantFromIso, __bynkInstantFromPgText, __bynkInstantToIso, __bynkIsIsoDate, __bynkPgRow } from "../src/time.ts";

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

// #2867: `IsoDate`.
test("isIsoDate admits real dates and refuses impossible or malformed ones", () => {
  for (const ok of ["2026-11-03", "2028-02-29", "2000-02-29", "0000-01-01", "9999-12-31", "2026-04-30"]) {
    assert.equal(__bynkIsIsoDate(ok), true, ok);
  }
  for (const bad of [
    "2026-02-30", "2026-02-29", "1900-02-29", "2026-04-31", "2026-13-01", "2026-00-10", "2026-01-00",
    "2026-1-1", "yesterday", "", "2026-11-03 ", " 2026-11-03", "2026-11-03\n", "2026-11-03T00:00:00Z",
    "20261103", "2026/11/03", "+2026-11-03", "２０２６-11-03", "2026-11-03x",
  ]) {
    assert.equal(__bynkIsIsoDate(bad), false, JSON.stringify(bad));
  }
});

test("isIsoDate agrees with Instant.fromIso at midnight over generated strings", () => {
  // A small deterministic generator over the alphabet that matters: digits near
  // the calendar's edges, the separators, and a few stray characters.
  let seed = 2867;
  const next = () => (seed = (seed * 1103515245 + 12345) & 0x7fffffff);
  const pick = (xs: string[]) => xs[next() % xs.length];
  const years = ["0000", "0004", "1900", "2000", "2026", "2028", "2100", "9999", "26", "20260"];
  const months = ["00", "01", "02", "04", "12", "13", "1", "99"];
  const days = ["00", "01", "28", "29", "30", "31", "32", "1", "99"];
  const seps = ["-", "-", "-", "/", ""];
  for (let i = 0; i < 20000; i++) {
    const d = `${pick(years)}${pick(seps)}${pick(months)}${pick(seps)}${pick(days)}`;
    const viaInstant = __bynkInstantFromIso(`${d}T00:00:00Z`).tag === "Some";
    assert.equal(__bynkIsIsoDate(d), viaInstant, d);
  }
});

// Hyperdrive H3 (#3320): the forms a Postgres `timestamptz` arrives as.
const NOON = Date.UTC(2026, 10, 11, 12, 34, 56, 789);

test("fromPgText reads the driver's ISO form and the server's own text, each as UTC", () => {
  const forms: [string, number][] = [
    ["2026-11-11T12:34:56.789Z", NOON],
    ["2026-11-11T12:34:56.789z", NOON],
    ["2026-11-11 12:34:56.789+00", NOON],
    ["2026-11-11 12:34:56.789+00:00", NOON],
    ["2026-11-11 12:34:56.789+0000", NOON],
    ["2026-11-11 13:34:56.789+01", NOON],
    ["2026-11-11 18:04:56.789+05:30", NOON],
    ["2026-11-11 18:04:56.789+0530", NOON],
    ["2026-11-11 04:34:56.789-08", NOON],
    ["2026-11-11 04:34:56.789-08:00", NOON],
    ["2026-11-11 12:34:56.789123+00", NOON],
    ["2026-11-11 12:34:56+00", NOON - 789],
    ["2026-11-11 12:34:56.7+00", NOON - 89],
    ["2026-11-11 18:28:24.789+05:53:28", NOON],
    ["2026-11-11T12:34:56.789000+00:00", NOON],
    ["1970-01-01 00:00:00+00", 0],
    ["1969-12-31 23:59:59.999+00", -1],
  ];
  for (const [text, ms] of forms) {
    assert.equal(__bynkInstantFromPgText(text), ms, text);
  }
});

test("fromPgText is null for anything that names no instant", () => {
  for (const text of [
    "2026-11-11 12:34:56",
    "2026-11-11 12:34:56.789",
    "infinity",
    "-infinity",
    "0044-03-15 12:00:00+00 BC",
    "2026-02-30 12:00:00+00",
    "2026-11-11 24:00:00+00",
    "2026-11-11 12:34:56+24",
    "2026-11-11 12:34:56+05:60",
    "12345-11-11 12:34:56+00",
    "yesterday",
    "",
  ]) {
    assert.equal(__bynkInstantFromPgText(text), null, text);
  }
});

test("pgRow converts the named Instant and Int columns and leaves everything else to the codec", () => {
  const row: Record<string, unknown> = {
    at: "2026-11-11 12:34:56.789+00",
    bad: "yesterday",
    n: "9007199254740991",
    neg: "-12",
    zero: "-0",
    big: "9007199254740993",
    frac: "1.5",
    already: 7,
    other: "2026-11-11 12:34:56.789+00",
  };
  __bynkPgRow(row, ["at", "bad", "already", "missing"], ["n", "neg", "zero", "big", "frac", "already"]);
  assert.equal(row.at, NOON);
  assert.equal(row.bad, "yesterday");
  assert.equal(row.n, 9007199254740991);
  assert.equal(row.neg, -12);
  assert.ok(Object.is(row.zero, 0));
  assert.equal(row.big, "9007199254740993");
  assert.equal(row.frac, "1.5");
  assert.equal(row.already, 7);
  assert.equal(row.other, "2026-11-11 12:34:56.789+00");
  // Not a row: nothing to convert, nothing thrown.
  for (const v of [null, 3, "x", [1]]) __bynkPgRow(v, ["at"], ["n"]);
});

import type { Option } from "./result.ts";
import { Some, None } from "./result.ts";

// Standard-library K3 (#2592): ISO-8601 for `Instant`, which erases to a
// `number` of Unix epoch milliseconds (ADR 0114). UTC only: an offset is read
// and converted, never kept, so no time-zone data is needed.

// The furthest a `Date` reaches either side of the epoch, ±100,000,000 days.
const MAX_EPOCH_MS = 8.64e15;

// Total over the `Date` range: `YYYY-MM-DDTHH:mm:ss.sssZ`, always three
// fraction digits and `Z` (so the text sorts in time order), with the expanded
// `±YYYYYY` year outside 0000 to 9999. Past the range no date names the
// instant, so it faults.
export function __bynkInstantToIso(ms: number): string {
  if (!(Math.abs(ms) <= MAX_EPOCH_MS)) {
    throw new Error(`Instant.toIso: ${ms} ms is beyond the range a date can name (±8.64e15 ms)`);
  }
  return new Date(ms).toISOString();
}

// The RFC 3339 date-time profile: `T` or `t`, a fraction of any length, and
// `Z`, `z` or a `±HH:mm` offset. Nothing else.
const ISO =
  /^(\d{4})-(\d{2})-(\d{2})[Tt](\d{2}):(\d{2}):(\d{2})(?:\.(\d+))?(?:([Zz])|([+-])(\d{2}):(\d{2}))$/;

function daysIn(year: number, month: number): number {
  if (month === 2) {
    const leap = (year % 4 === 0 && year % 100 !== 0) || year % 400 === 0;
    return leap ? 29 : 28;
  }
  return [4, 6, 9, 11].includes(month) ? 30 : 31;
}

// #2867: the calendar rule, once. `IsoDate` and `Instant.fromIso` both ask
// whether a year/month/day names a date that exists.
function isRealDate(year: number, month: number, day: number): boolean {
  return month >= 1 && month <= 12 && day >= 1 && day <= daysIn(year, month);
}

// The `IsoDate` refinement (#2867): exactly `YYYY-MM-DD`, years 0000 to 9999,
// and a date that exists. The date half of what `__bynkInstantFromIso` reads.
const ISO_DATE = /^(\d{4})-(\d{2})-(\d{2})$/;

export function __bynkIsIsoDate(s: string): boolean {
  const m = ISO_DATE.exec(s);
  return m !== null && isRealDate(Number(m[1]), Number(m[2]), Number(m[3]));
}

// Partial: `None` for anything outside the profile or the calendar. A leap
// second (`:60`) is `None`, since epoch milliseconds cannot name it, and a
// fraction past milliseconds is truncated toward the earlier instant. Hand
// written rather than `Date.parse`, whose accepted forms differ by engine.
export function __bynkInstantFromIso(s: string): Option<number> {
  const m = ISO.exec(s);
  if (m === null) {
    return None;
  }
  const [year, month, day, hour, minute, second] = m.slice(1, 7).map(Number);
  if (!isRealDate(year, month, day)) {
    return None;
  }
  if (hour > 23 || minute > 59 || second > 59) {
    return None;
  }
  const millis = m[7] === undefined ? 0 : Number(m[7].slice(0, 3).padEnd(3, "0"));
  let offsetMinutes = 0;
  if (m[9] !== undefined) {
    const offsetHour = Number(m[10]);
    const offsetMinute = Number(m[11]);
    if (offsetHour > 23 || offsetMinute > 59) {
      return None;
    }
    offsetMinutes = (m[9] === "-" ? -1 : 1) * (offsetHour * 60 + offsetMinute);
  }
  // `Date.UTC` reads a year below 100 as 19xx, so the year is set apart.
  const date = new Date(Date.UTC(2000, month - 1, day, hour, minute, second, millis));
  date.setUTCFullYear(year);
  return Some(date.getTime() - offsetMinutes * 60_000);
}

// Hyperdrive H3 (#3320, ADR 0799 F): the `Instant` a Postgres `timestamptz`
// arrives as. The boundary codec reads an `Instant` as whole epoch milliseconds
// (ADR 0114) and has no ISO-8601 rule, and `Json.decode`'s wire format stays
// as it is, so a typed `Postgres` read converts the text *before* the codec
// runs, here, as `Json.decodeSql` does for SQLite's flags.
//
// The forms read are the ones a `timestamptz` renders as: the driver's
// `toISOString()` (`2026-11-11T12:34:56.789Z`), and the server's own text
// (`::text`, `to_char`, a `jsonb` member): a space for the `T`, a fraction of up
// to six digits, and an offset that may be `+00`, `+05:30`, `-0800`, or (for a
// zone with a historic offset) `+05:53:28`. The result is always UTC epoch
// milliseconds, a fraction past milliseconds truncated toward the earlier
// instant. A form with no offset (a `timestamp`, which names no instant), the
// special values `infinity` and `-infinity`, a `BC` year and anything else
// is `null`. The driver keeps those as text (`PG_DATE_TYPES` in `postgres.ts`): only a
// finite `timestamptz` reaches here as a `Date`'s ISO text, so a `timestamp`, a `date`
// and an infinite value are refused by the `Instant` field, not read as UTC or `null`.
const PG_TIMESTAMP =
  /^(\d{4})-(\d{2})-(\d{2})[ Tt](\d{2}:\d{2}:\d{2}(?:\.\d+)?)(?:([Zz])|([+-])(\d{2})(?::?(\d{2})(?::?(\d{2}))?)?)$/;

export function __bynkInstantFromPgText(s: string): number | null {
  const m = PG_TIMESTAMP.exec(s);
  if (m === null) {
    return null;
  }
  const local = __bynkInstantFromIso(`${m[1]}-${m[2]}-${m[3]}T${m[4]}Z`);
  if (local.tag !== "Some") {
    return null;
  }
  if (m[5] !== undefined) {
    return local.value;
  }
  const hours = Number(m[7]);
  const minutes = m[8] === undefined ? 0 : Number(m[8]);
  const seconds = m[9] === undefined ? 0 : Number(m[9]);
  if (hours > 23 || minutes > 59 || seconds > 59) {
    return null;
  }
  const offsetMs = (hours * 3600 + minutes * 60 + seconds) * 1000;
  return local.value - (m[6] === "-" ? -offsetMs : offsetMs);
}

// A typed `Postgres` read's pass over the parsed rows, before the codec: at
// each `Instant` column (`instants`) a text becomes epoch milliseconds, and at
// each `Int` column (`ints`) the text of an `int8` becomes a number when it is
// a whole number within ±(2^53 − 1). Anything else is left alone, so the codec
// reports it: an `int8` past 2^53 fails the decode rather than losing digits.
export function __bynkPgRow(row: unknown, instants: readonly string[], ints: readonly string[]): void {
  if (row === null || typeof row !== "object" || Array.isArray(row)) {
    return;
  }
  const o = row as Record<string, unknown>;
  for (const k of instants) {
    const v = o[k];
    if (typeof v === "string") {
      const ms = __bynkInstantFromPgText(v);
      if (ms !== null) {
        o[k] = ms;
      }
    }
  }
  for (const k of ints) {
    const v = o[k];
    if (typeof v === "string" && /^-?\d+$/.test(v) && Number.isSafeInteger(Number(v))) {
      o[k] = Number(v) + 0;
    }
  }
}


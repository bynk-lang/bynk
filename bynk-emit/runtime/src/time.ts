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
  if (month < 1 || month > 12 || day < 1 || day > daysIn(year, month)) {
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

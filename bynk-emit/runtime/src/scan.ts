// A lazy scan over a store collection's values (#4036). `first`, `any`, `all` and
// `take(n).collect()` over a `Map` or `Log` that no stage ahead of them needs whole stop
// pulling pages as soon as the answer is known: each stage is an async generator over the
// one before, and a terminal that returns closes the chain it pulled from.
//
// The order matters. A `Log`'s keys are its append times, so a scan of its pages is its
// append order. A `Map`'s keys are the keys the program chose, so its pages are in key
// order and its insertion order exists only once every page is read: for a `Map` only the
// order-free terminals (`any`, `all`) are given this form.
import { None, Some, type Option } from "./result.ts";

/** What a store handle's `lazyValues()` gives: the values, a page of storage at a time. */
export type ScanSource<T> = AsyncIterable<T> | Iterable<T>;

export async function* scanFilter<T>(source: ScanSource<T>, keep: (x: T) => boolean): AsyncGenerator<T> {
  for await (const x of source) if (keep(x)) yield x;
}

export async function* scanMap<T, U>(source: ScanSource<T>, f: (x: T) => U): AsyncGenerator<U> {
  for await (const x of source) yield f(x);
}

/** `f` gives a `Query`: a thunk of the values to splice in. */
export async function* scanFlatMap<T, U>(
  source: ScanSource<T>,
  f: (x: T) => () => PromiseLike<readonly U[]> | readonly U[],
): AsyncGenerator<U> {
  for await (const x of source) for (const y of await f(x)()) yield y;
}

export async function* scanSkip<T>(source: ScanSource<T>, n: number): AsyncGenerator<T> {
  let left = Math.max(0, n);
  for await (const x of source) {
    if (left > 0) left -= 1;
    else yield x;
  }
}

export async function scanAny<T>(source: ScanSource<T>, p: (x: T) => boolean): Promise<boolean> {
  for await (const x of source) if (p(x)) return true;
  return false;
}

export async function scanAll<T>(source: ScanSource<T>, p: (x: T) => boolean): Promise<boolean> {
  for await (const x of source) if (!p(x)) return false;
  return true;
}

export async function scanFirst<T>(source: ScanSource<T>): Promise<Option<T>> {
  for await (const x of source) return Some(x);
  return None;
}

export async function scanFirstOrElse<T>(source: ScanSource<T>, fallback: T): Promise<T> {
  for await (const x of source) return x;
  return fallback;
}

/** `take(n).collect()`: the first `n` values, reading no further than the nth. */
export async function scanTake<T>(source: ScanSource<T>, n: number): Promise<T[]> {
  const out: T[] = [];
  const count = Math.max(0, n);
  if (count === 0) return out;
  for await (const x of source) {
    out.push(x);
    if (out.length >= count) break;
  }
  return out;
}

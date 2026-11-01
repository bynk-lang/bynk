// Concurrent effects, slice 0 (#2640): the join behind a `par { … }` block.
//
// Each effect of the block arrives as a thunk. `__bynkSettleAll` calls them in
// source order, so every effect is issued before any is awaited and before the
// next binding is evaluated, then waits for ALL of them to settle. It yields the
// values in source order, or raises the FIRST rejection in source order,
// whichever settled first in time. A thunk that throws synchronously (a check
// that fails before a promise exists) is a rejected entry like any other, so an
// earlier effect is never left unawaited. A fast failure therefore waits for a
// slow sibling: nothing the block started is still running when it returns or
// throws, so no work outlives the handler, and the fault a program sees does
// not depend on timing.
//
// Later rejections are attached to the raised one as `suppressed` (an array, in
// source order) so a log can show them, but they are not catchable separately.
// Not part of ADR 0310's published ABI: it is codegen.
export async function __bynkSettleAll<const T extends readonly (() => unknown)[]>(
  thunks: T,
): Promise<{ -readonly [P in keyof T]: Awaited<ReturnType<T[P]>> }> {
  const issued: Promise<unknown>[] = [];
  for (const thunk of thunks) {
    try {
      issued.push(Promise.resolve(thunk()));
    } catch (e) {
      issued.push(Promise.reject(e));
    }
  }
  const settled = await Promise.allSettled(issued);
  const failures: unknown[] = [];
  for (const r of settled) {
    if (r.status === "rejected") failures.push(r.reason);
  }
  if (failures.length > 0) {
    const [first, ...rest] = failures;
    if (rest.length > 0 && typeof first === "object" && first !== null) {
      try {
        Object.defineProperty(first, "suppressed", {
          value: rest,
          configurable: true,
          writable: true,
        });
      } catch {
        // A frozen reason cannot carry them; the first rejection still stands.
      }
    }
    throw first;
  }
  return settled.map((r) => (r as { value: unknown }).value) as {
    -readonly [P in keyof T]: Awaited<ReturnType<T[P]>>;
  };
}


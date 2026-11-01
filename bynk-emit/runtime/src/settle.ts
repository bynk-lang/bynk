// Concurrent effects, slice 0 (#2640): the join behind a `par { … }` block.
//
// Every effect of the block has already been issued, in source order, by the
// time its promise is in `ps`. `__bynkSettleAll` waits for ALL of them to
// settle, then yields the values in source order, or raises the FIRST rejection
// in source order, whichever settled first in time. A fast failure therefore
// waits for a slow sibling: nothing the block started is still running when it
// returns or throws, so no work outlives the handler, and the fault a program
// sees does not depend on timing.
//
// Later rejections are attached to the raised one as `suppressed` (an array, in
// source order) so a log can show them, but they are not catchable separately.
// Not part of ADR 0310's published ABI: it is codegen.
export async function __bynkSettleAll<const T extends readonly unknown[]>(
  ps: T,
): Promise<{ -readonly [P in keyof T]: Awaited<T[P]> }> {
  const settled = await Promise.allSettled(ps);
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
    -readonly [P in keyof T]: Awaited<T[P]>;
  };
}

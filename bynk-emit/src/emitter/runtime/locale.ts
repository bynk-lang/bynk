// Locale capability track, slice 2 (#882): RFC 4647 basic filtering of an
// `Accept-Language` header against a bundle's declared locale set — exact
// match, then successive rightmost-subtag truncation (`pt-BR` -> `pt`),
// falling back to the bundle's reference locale. Total: never throws, and
// always returns either a member of `declared` or `reference` verbatim
// (both already-valid `LocaleTag`s the compiler itself emitted), never a
// derived string of its own.

export function negotiateLocale(
  header: string | null | undefined,
  declared: readonly string[],
  reference: string,
): string {
  if (!header) return reference;
  const ranges = header
    .split(",")
    .map((part) => {
      const [rangeRaw, ...params] = part.split(";");
      const range = rangeRaw.trim();
      let q = 1;
      for (const param of params) {
        const [k, v] = param.trim().split("=");
        if (k === "q") {
          const n = Number(v);
          if (Number.isFinite(n)) q = n;
        }
      }
      return { range, q };
    })
    .filter((r) => r.range.length > 0 && r.range !== "*")
    // Array.prototype.sort is spec-guaranteed stable, so equal-q ranges keep
    // the header's own left-to-right preference order.
    .sort((a, b) => b.q - a.q);

  const declaredLower = declared.map((d) => d.toLowerCase());
  for (const { range } of ranges) {
    let candidate = range.toLowerCase();
    while (candidate.length > 0) {
      const idx = declaredLower.indexOf(candidate);
      if (idx !== -1) return declared[idx];
      const lastDash = candidate.lastIndexOf("-");
      if (lastDash === -1) break;
      candidate = candidate.slice(0, lastDash);
    }
  }
  return reference;
}


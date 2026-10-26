// message-bundles slice 3 (#878): ICU MessageFormat formatting helpers.
// Delegate entirely to the host JS engine's own `Intl` object — no CLDR
// data is bundled here (Decision A). Emitted `messages`-bundle code calls
// these directly; see `bynk-emit/src/emitter/emit.rs`'s `emit_icu_placeholder`.

/**
 * Selects a plural category's arm using the host `Intl.PluralRules` for
 * `tag`. Falls back to `"other"` if `Intl.PluralRules` selects a category
 * `arms` doesn't declare — the checker (`bynk.messages.malformed_icu_syntax`)
 * already guarantees every declared `plural` placeholder has an `other` arm,
 * so `arms["other"]` is always defined; this is a real runtime fallback (a
 * locale's actual CLDR category set can differ from the arms an author
 * happened to write), not dead code.
 *
 * The `?? arms["other"]` here is safe where the emitted `select` dispatch's
 * was not (#900): `category` comes from `Intl.PluralRules().select()`, whose
 * return is closed over `zero|one|two|few|many|other` — none an
 * `Object.prototype` member — so it can never resolve off the prototype chain.
 * A `select` arm, by contrast, is keyed by an arbitrary runtime `MessageArg`
 * value and needs an own-property (`Object.hasOwn`) check instead.
 */
export function selectPluralArm(
  tag: string,
  n: number,
  arms: Record<string, string>,
): string {
  const category = new Intl.PluralRules(tag).select(n);
  return arms[category] ?? arms["other"];
}

export function formatIcuNumber(
  tag: string,
  n: number,
  style?: "integer" | "percent",
): string {
  const options: Intl.NumberFormatOptions =
    style === "integer"
      ? { maximumFractionDigits: 0 }
      : style === "percent"
        ? { style: "percent" }
        : {};
  return new Intl.NumberFormat(tag, options).format(n);
}

export function formatIcuDate(
  tag: string,
  epochMillis: number,
  style?: "short" | "medium" | "long" | "full",
): string {
  const options: Intl.DateTimeFormatOptions =
    style !== undefined ? { dateStyle: style } : {};
  return new Intl.DateTimeFormat(tag, options).format(new Date(epochMillis));
}

// #1653: the `__` aliases emitted code imports.
export { formatIcuDate as __formatIcuDate, formatIcuNumber as __formatIcuNumber, selectPluralArm as __selectPluralArm };

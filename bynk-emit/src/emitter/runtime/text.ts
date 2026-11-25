// String measures that the language defines independently of the host's UTF-16
// representation (#3949).

/** A string's length in code points: what `MinLength`/`MaxLength`/`Length` count (#3949). */
export function __bynkCodePointLength(s: string): number {
  let n = s.length;
  for (let i = 0; i < s.length; i += 1) {
    const c = s.charCodeAt(i);
    // A high surrogate followed by a low one is one code point, not two units.
    if (c >= 0xd800 && c < 0xdc00 && i + 1 < s.length) {
      const d = s.charCodeAt(i + 1);
      if (d >= 0xdc00 && d < 0xe000) {
        n -= 1;
        i += 1;
      }
    }
  }
  return n;
}

const regExps = new Map<string, RegExp>();

/**
 * The `RegExp` for a source and flags, built once (#3950). A refinement's
 * `Matches` check runs on every construction, so it shares one compiled
 * pattern; a non-global, non-sticky `RegExp`'s `test` keeps no state, which
 * makes sharing safe.
 */
export function __bynkRegExp(source: string, flags: string): RegExp {
  const key = `${flags}/${source}`;
  let re = regExps.get(key);
  if (re === undefined) {
    re = new RegExp(source, flags);
    regExps.set(key, re);
  }
  return re;
}

/**
 * `String.padStart` / `padEnd` (#3978). The width counts code points, not UTF-16 units,
 * so an astral character is one position; `fill` is cut to fit (by code points too).
 * An empty `fill` has nothing to repeat and faults.
 */
export function __bynkPad(s: string, width: number, fill: string, atStart: boolean): string {
  const unit = [...fill];
  if (unit.length === 0) throw new Error("String.padStart/padEnd: `fill` is empty");
  const need = width - [...s].length;
  if (need <= 0) return s;
  let pad = "";
  for (let i = 0; i < need; i += 1) pad += unit[i % unit.length];
  return atStart ? pad + s : s + pad;
}

/** `String.repeat` (#3978): `n` copies; a negative `n` faults. */
export function __bynkRepeat(s: string, n: number): string {
  if (n < 0) throw new Error(`String.repeat: a negative count (${n})`);
  return s.repeat(n);
}

/**
 * `String.lines` (#3978): split on `\n` and `\r\n` (not a lone `\r`); a trailing line
 * break gives no empty last element, and the empty string gives `[]`.
 */
export function __bynkLines(s: string): string[] {
  if (s === "") return [];
  const parts = s.split(/\r?\n/);
  if (parts[parts.length - 1] === "") parts.pop();
  return parts;
}


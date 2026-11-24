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

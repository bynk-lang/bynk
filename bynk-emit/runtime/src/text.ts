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

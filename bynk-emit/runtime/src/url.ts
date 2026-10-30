import type { Option } from "./result.ts";
import { Some, None } from "./result.ts";

// Standard-library K2 (#2594): percent-encoding, the part of URL handling Bynk
// cannot write itself, since it works on a string's UTF-8 bytes. `bynk.url`
// builds the rest in Bynk.

// RFC 3986's unreserved set, kept as is; every other byte is `%XX`.
const UNRESERVED = /[A-Za-z0-9\-._~]/;

// Total: each character outside the unreserved set as the `%XX` of its UTF-8
// bytes, upper-case hex. Stricter than `encodeURIComponent`, which leaves
// `! ' ( ) *` alone, so the result is safe in any URL component. A lone
// surrogate is encoded as U+FFFD's bytes rather than thrown on.
export function __bynkPercentEncode(s: string): string {
  const bytes = new TextEncoder().encode(s);
  let out = "";
  for (const b of bytes) {
    const c = String.fromCharCode(b);
    out += b < 0x80 && UNRESERVED.test(c) ? c : "%" + b.toString(16).toUpperCase().padStart(2, "0");
  }
  return out;
}

// Partial: each `%XX` (either case) is a byte, everything else its UTF-8.
// `None` for a `%` without two hex digits after it, or for bytes that are not
// UTF-8. `+` is left alone: reading it as a space is query-string semantics,
// which `bynk.url` applies itself.
export function __bynkPercentDecode(s: string): Option<string> {
  const raw = new TextEncoder().encode(s);
  const bytes: number[] = [];
  for (let i = 0; i < raw.length; i++) {
    if (raw[i] !== 0x25) {
      bytes.push(raw[i]);
      continue;
    }
    const hex = String.fromCharCode(raw[i + 1] ?? 0, raw[i + 2] ?? 0);
    if (!/^[0-9A-Fa-f]{2}$/.test(hex)) {
      return None;
    }
    bytes.push(parseInt(hex, 16));
    i += 2;
  }
  // `ignoreBOM`: the default strips a leading U+FEFF, which would break
  // `percentDecode(percentEncode(s)) == Some(s)` for a string starting with one.
  try {
    return Some(new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(new Uint8Array(bytes)));
  } catch {
    return None;
  }
}

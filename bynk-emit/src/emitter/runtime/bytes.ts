import type { Option } from "./result.ts";
import { Some, None } from "./result.ts";

// v0.110 (ADR 0142): the `Bytes` primitive runtime. A `Bytes` erases to a
// `Uint8Array` (an immutable finite octet sequence at the Bynk level), so
// unlike the number-erased base types its equality is by content, not host
// `===`, and its wire form is a base64 string. These helpers back the kernel
// (`==`, `toBase64`, `decodeUtf8`) and the JSON codec (`fromBase64`).

// Content equality — the byte-for-byte compare the emitter lowers `==` to. The
// `===` fast path catches the identity case; otherwise length then each octet.
export function __bynkBytesEqual(a: Uint8Array, b: Uint8Array): boolean {
  if (a === b) return true;
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) {
    if (a[i] !== b[i]) return false;
  }
  return true;
}

// Total encode: the standard base64 of the octet sequence. Every byte is
// 0..255, so the Latin-1 string handed to `btoa` never overflows.
export function __bynkBytesToBase64(bytes: Uint8Array): string {
  let bin = "";
  for (let i = 0; i < bytes.length; i++) {
    bin += String.fromCharCode(bytes[i]);
  }
  return btoa(bin);
}

// Partial decode: `Some(bytes)` for a valid standard-base64 string, `None`
// otherwise. The alphabet/padding are validated up front so a malformed string
// is a clean `None`, not a silently truncated buffer (some `atob`
// implementations are lenient). The empty string decodes to empty bytes — the
// round-trip of `Bytes.empty`.
export function __bynkBytesFromBase64(s: string): Option<Uint8Array> {
  if (s.length % 4 !== 0 || !/^[A-Za-z0-9+/]*={0,2}$/.test(s)) {
    return None;
  }
  let bin: string;
  try {
    bin = atob(s);
  } catch {
    return None;
  }
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) {
    out[i] = bin.charCodeAt(i);
  }
  return Some(out);
}

// Partial decode to text: `Some(string)` when the octets are valid UTF-8,
// `None` otherwise. `fatal: true` makes an invalid sequence throw rather than
// emit replacement characters, so the partiality is real and surfaced.
export function __bynkBytesDecodeUtf8(bytes: Uint8Array): Option<string> {
  try {
    return Some(new TextDecoder("utf-8", { fatal: true }).decode(bytes));
  } catch {
    return None;
  }
}

// #2016 (standard-library K1): the `Bytes` operations. Each is pure,
// synchronous and uses only typed arrays and `btoa`/`atob`, which Cloudflare,
// Node and browsers all provide. `slice` lowers inline (`Uint8Array.slice`,
// clamped like `String.slice`), so it has no helper here.

// A fresh buffer holding `a`'s octets then `b`'s. It never aliases either input.
export function __bynkBytesConcat(a: Uint8Array, b: Uint8Array): Uint8Array {
  const out = new Uint8Array(a.length + b.length);
  out.set(a, 0);
  out.set(b, a.length);
  return out;
}

// Unsigned lexicographic order: -1, 0 or 1. The first differing octet decides,
// and a proper prefix sorts first. Not constant-time: it returns at the first
// difference, so it must never compare secrets.
export function __bynkBytesCompare(a: Uint8Array, b: Uint8Array): number {
  const n = Math.min(a.length, b.length);
  for (let i = 0; i < n; i++) {
    if (a[i] !== b[i]) return a[i] < b[i] ? -1 : 1;
  }
  return a.length === b.length ? 0 : a.length < b.length ? -1 : 1;
}

const HEX_DIGITS = "0123456789abcdef";

// Total encode: two lowercase hex digits per octet, no prefix or separators.
export function __bynkBytesToHex(bytes: Uint8Array): string {
  let out = "";
  for (let i = 0; i < bytes.length; i++) {
    out += HEX_DIGITS[bytes[i] >> 4] + HEX_DIGITS[bytes[i] & 0x0f];
  }
  return out;
}

// Partial decode: either case, an even number of digits, nothing else (no
// `0x`, no whitespace, no separators). `""` decodes to empty bytes, so
// `fromHex(toHex(b))` round-trips every `b`.
export function __bynkBytesFromHex(s: string): Option<Uint8Array> {
  if (!/^(?:[0-9a-fA-F]{2})*$/.test(s)) {
    return None;
  }
  const out = new Uint8Array(s.length / 2);
  for (let i = 0; i < out.length; i++) {
    out[i] = parseInt(s.slice(i * 2, i * 2 + 2), 16);
  }
  return Some(out);
}

// Total encode: RFC 4648 §5 base64url (`-` and `_`) with no padding, the JWT
// (RFC 7515) form.
export function __bynkBytesToBase64Url(bytes: Uint8Array): string {
  return __bynkBytesToBase64(bytes).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

// Partial decode: the URL-safe alphabet, with no padding or exactly the padding
// the length needs. A standard-alphabet character (`+`, `/`), wrong padding, an
// impossible length (`length % 4 == 1`) or non-zero spare bits in the last
// character is `None`. That last rule makes the decode canonical: `atob` drops
// the spare bits, so without it `"Zh"` and `"Zg"` would both decode to `"f"`,
// and a signed token segment could be re-spelled without changing its bytes.
export function __bynkBytesFromBase64Url(s: string): Option<Uint8Array> {
  const m = /^([A-Za-z0-9_-]*)(={0,2})$/.exec(s);
  if (m === null) {
    return None;
  }
  const body = m[1];
  const pad = m[2];
  if (body.length % 4 === 1) {
    return None;
  }
  const need = (4 - (body.length % 4)) % 4;
  if (pad.length !== 0 && pad.length !== need) {
    return None;
  }
  const decoded = __bynkBytesFromBase64(body.replace(/-/g, "+").replace(/_/g, "/") + "=".repeat(need));
  if (decoded.tag === "Some" && __bynkBytesToBase64Url(decoded.value) !== body) {
    return None;
  }
  return decoded;
}


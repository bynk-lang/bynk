// #2480: the playground deep link, as a pure module (no `vscode` import) so it
// is unit-tested under plain Mocha and shared by the desktop and web entries.
//
// The format is the playground's own (ADR 0140, D5), the one By Example's
// "Open in playground" links already use (site/src/lib/examples.mjs):
//   https://playground.bynk-lang.dev/#<base64url( deflate-raw( utf8(source) ) )>
// The code lives in the URL fragment, which a browser never sends to a server.

export const PLAYGROUND_ORIGIN = "https://playground.bynk-lang.dev";

/**
 * The longest link we will produce, in characters (DECISION A). Browsers take far
 * longer URLs, but chat tools, issue trackers and mail clients start to truncate
 * or break them well before; a link that is cut short opens the wrong program, so
 * past this we refuse rather than truncate. Deflate takes typical source to about
 * a quarter, so this admits tens of kilobytes of code.
 */
export const MAX_LINK_LENGTH = 16_384;

async function pipeThrough(bytes: Uint8Array, stream: CompressionStream | DecompressionStream) {
  const writer = stream.writable.getWriter();
  void writer.write(bytes as BufferSource);
  void writer.close();
  return new Uint8Array(await new Response(stream.readable).arrayBuffer());
}

function toBase64Url(bytes: Uint8Array): string {
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

function fromBase64Url(text: string): Uint8Array {
  const bin = atob(text.replace(/-/g, "+").replace(/_/g, "/"));
  return Uint8Array.from(bin, (c) => c.charCodeAt(0));
}

/** The fragment (without the `#`) that opens `source` in the playground. */
export async function encodeFragment(source: string): Promise<string> {
  const utf8 = new TextEncoder().encode(source);
  return toBase64Url(await pipeThrough(utf8, new CompressionStream("deflate-raw")));
}

/** The inverse of {@link encodeFragment}; used by the tests to prove a round trip. */
export async function decodeFragment(fragment: string): Promise<string> {
  const inflated = await pipeThrough(fromBase64Url(fragment), new DecompressionStream("deflate-raw"));
  return new TextDecoder().decode(inflated);
}

export type PlaygroundLink =
  | { ok: true; url: string }
  | { ok: false; length: number; max: number };

/** The link for `source`, or the reason there isn't one: it never truncates. */
export async function playgroundLink(
  source: string,
  max: number = MAX_LINK_LENGTH,
): Promise<PlaygroundLink> {
  const url = `${PLAYGROUND_ORIGIN}/#${await encodeFragment(source)}`;
  return url.length > max ? { ok: false, length: url.length, max } : { ok: true, url };
}

/** The unit names a file imports with `uses <unit>`. */
export function usedUnits(text: string): string[] {
  return [...text.matchAll(/^[ \t]*uses[ \t]+([A-Za-z_][\w.]*)/gm)].map((m) => m[1]);
}

/**
 * The units the file imports that a selection leaves out (DECISION B). `uses`
 * brings a unit's names in unqualified, so a selection can't be checked for
 * mentions of them without resolving names; what it can tell is that, lacking
 * the clause, any such name in the selection would not resolve in the playground.
 */
export function missingUses(documentText: string, selection: string): string[] {
  const carried = new Set(usedUnits(selection));
  return [...new Set(usedUnits(documentText))].filter((unit) => !carried.has(unit));
}

// `Yaml.decode[T]` (#3214): a pure YAML 1.2 core-schema subset parser.
//
// It parses text to the same JSON-shaped value tree `JSON.parse` yields, which
// the generated `deserialise_<T>` then decodes, so a YAML document and a JSON
// document reach one typed decoder, with one set of path-naming errors.
//
// Accepted: block and flow mappings and sequences; plain, single-quoted and
// double-quoted scalars (multi-line included); literal `|` and folded `>` block
// scalars with chomping and indentation indicators; comments; one document,
// optionally between `---` and `...`; scalar mapping keys.
//
// Refused with a distinct, positioned error, never a partial value: anchors
// (`&`), aliases (`*`), merge keys (`<<`), tags (`!`), a second document,
// directives and complex keys (`?`), `.inf` / `.nan`, and duplicate keys. Input
// over MAX_BYTES, or nested deeper than MAX_DEPTH, is refused up front.
//
// Core-schema resolution is by shape, for plain scalars only: `null`/`Null`/
// `NULL`/`~`/empty, `true`/`True`/`TRUE`/`false`/`False`/`FALSE`, decimal, `0o`
// octal and `0x` hex integers, and floats. Everything else is a string, so `on`,
// `off`, `yes` and `no` stay strings. A quoted scalar is always a string.
//
// Nothing here is shared with the rest of the runtime except `Ok`/`Err` and the
// two value types, so a project that never calls `Yaml.decode` never emits it.
import { Ok, Err } from "./result.ts";
import type { Result } from "./result.ts";
import type { JsonValue } from "./boundary.ts";
import type { DecodeError } from "./errors.ts";

/** The input cap, in UTF-8 bytes (the HTTP body cap, `MAX_BODY_BYTES`). */
export const YAML_MAX_BYTES = 1048576;
/** The nesting cap, the same as the recursive-codec bound (#2656). */
export const YAML_MAX_DEPTH = 256;

interface Line {
  /** The column of `text[0]` in the source line. */
  indent: number;
  text: string;
  /** 1-based source line number. */
  no: number;
}

interface Failure {
  readonly yamlFailure: true;
  readonly kind: string;
  readonly path: string;
  readonly message: string;
}

function fail(kind: string, path: string, line: Line | undefined, col: number, message: string): never {
  const where = line === undefined ? "" : ` (line ${line.no}, column ${col + 1})`;
  const f: Failure = { yamlFailure: true, kind, path, message: `${message}${where}` };
  throw f;
}

function utf8Length(s: string): number {
  let n = 0;
  for (let i = 0; i < s.length; i++) {
    const c = s.charCodeAt(i);
    if (c < 0x80) n += 1;
    else if (c < 0x800) n += 2;
    else if (c >= 0xd800 && c <= 0xdbff && i + 1 < s.length) {
      n += 4;
      i++;
    } else n += 3;
  }
  return n;
}

const isSpace = (c: string | undefined): boolean => c === " " || c === "\t";
const isBlank = (l: Line): boolean => l.text.trim() === "";

/** A document marker: `---` or `...` at column 0, alone or before whitespace. */
function marker(l: Line, m: "---" | "..."): boolean {
  return l.indent === 0 && (l.text === m || l.text.startsWith(`${m} `) || l.text.startsWith(`${m}\t`));
}

const isSeqEntry = (t: string): boolean => t === "-" || t.startsWith("- ") || t.startsWith("-\t");

function resolvePlain(raw: string, path: string, line: Line, col: number): JsonValue {
  if (raw === "" || raw === "~" || raw === "null" || raw === "Null" || raw === "NULL") return null;
  if (raw === "true" || raw === "True" || raw === "TRUE") return true;
  if (raw === "false" || raw === "False" || raw === "FALSE") return false;
  if (/^[-+]?[0-9]+$/.test(raw)) return Number(raw);
  if (/^0o[0-7]+$/.test(raw)) return parseInt(raw.slice(2), 8);
  if (/^0x[0-9a-fA-F]+$/.test(raw)) return parseInt(raw.slice(2), 16);
  if (/^[-+]?(\.[0-9]+|[0-9]+(\.[0-9]*)?)([eE][-+]?[0-9]+)?$/.test(raw)) return Number(raw);
  if (/^[-+]?\.(inf|Inf|INF)$/.test(raw) || /^\.(nan|NaN|NAN)$/.test(raw)) {
    fail(
      "UnsupportedValue",
      path,
      line,
      col,
      `\`${raw}\` is not a finite number, and a decoded value cannot hold it`,
    );
  }
  return raw;
}

function setKey(obj: { [k: string]: JsonValue }, key: string, value: JsonValue): void {
  // `defineProperty`, not assignment: a `__proto__` key is data, as in JSON.parse.
  Object.defineProperty(obj, key, { value, enumerable: true, writable: true, configurable: true });
}

const ESCAPES: { readonly [c: string]: string } = {
  "0": "\0",
  a: "\x07",
  b: "\b",
  t: "\t",
  "\t": "\t",
  n: "\n",
  v: "\v",
  f: "\f",
  r: "\r",
  e: "\x1b",
  " ": " ",
  '"': '"',
  "/": "/",
  "\\": "\\",
  N: "\u0085",
  _: " ",
  L: " ",
  P: " ",
};

class Parser {
  private readonly lines: Line[];
  private i = 0;

  constructor(lines: Line[]) {
    this.lines = lines;
  }

  // ---- entry ----------------------------------------------------------------

  document(): JsonValue {
    this.skip();
    let l = this.lines[this.i];
    if (l !== undefined && l.indent === 0 && l.text.startsWith("%")) {
      fail("UnsupportedSyntax", "$", l, 0, "directives (`%YAML`, `%TAG`) are not supported");
    }
    if (l !== undefined && marker(l, "---")) {
      const rest = l.text.slice(3);
      const lead = rest.length - rest.trimStart().length;
      const body = rest.trimStart();
      if (body === "" || body.startsWith("#")) this.i++;
      else {
        const virtual = { indent: 3 + lead, text: body, no: l.no };
        if (isSeqEntry(body) || this.scanKey(virtual) !== null) {
          fail("Malformed", "$", l, 3 + lead, "a block collection cannot start on the `---` line");
        }
        this.lines[this.i] = virtual;
      }
    }
    const value = this.node(0, "$", 0);
    this.skip();
    l = this.lines[this.i];
    if (l !== undefined && marker(l, "...")) {
      const after = l.text.slice(3).trim();
      if (after !== "" && !after.startsWith("#")) {
        fail("Malformed", "$", l, 3, "unexpected content after the document end marker `...`");
      }
      this.i++;
      this.skip();
      l = this.lines[this.i];
      if (l !== undefined) {
        fail("MultipleDocuments", "$", l, l.indent, "a YAML stream with more than one document is not supported");
      }
    } else if (l !== undefined) {
      if (marker(l, "---")) {
        fail("MultipleDocuments", "$", l, 0, "a YAML stream with more than one document is not supported");
      }
      fail("Malformed", "$", l, l.indent, "unexpected content after the document's value (check the indentation)");
    }
    return value;
  }

  // ---- line cursor ----------------------------------------------------------

  /** Step over blank lines and comment-only lines. */
  private skip(): void {
    while (this.i < this.lines.length) {
      const l = this.lines[this.i]!;
      if (isBlank(l) || l.text.startsWith("#")) this.i++;
      else break;
    }
  }

  private cur(): Line | undefined {
    this.skip();
    const l = this.lines[this.i];
    if (l === undefined || marker(l, "---") || marker(l, "...")) return undefined;
    // A tab is content inside a scalar or a flow collection, but never block indentation.
    if (l.text[0] === "\t") fail("Malformed", "$", l, l.indent, "tabs are not allowed for indentation");
    return l;
  }

  private checkDepth(depth: number, path: string, l: Line, col: number): void {
    if (depth + 1 > YAML_MAX_DEPTH) {
      fail("LimitExceeded", path, l, col, `nesting is deeper than ${YAML_MAX_DEPTH} levels`);
    }
  }

  // ---- block structure ------------------------------------------------------

  /** A node whose first line is at column `minIndent` or deeper; null when absent. */
  private node(minIndent: number, path: string, depth: number): JsonValue {
    const l = this.cur();
    if (l === undefined || l.indent < minIndent) return null;
    const c = l.text[0]!;
    if (c === "&") fail("UnsupportedAnchor", path, l, l.indent, "anchors (`&name`) are not supported");
    if (c === "*") fail("UnsupportedAlias", path, l, l.indent, "aliases (`*name`) are not supported");
    if (c === "!") fail("UnsupportedTag", path, l, l.indent, "tags (`!tag`) are not supported");
    if (isSeqEntry(l.text)) return this.sequence(l.indent, path, depth);
    if (this.scanKey(l) !== null) return this.mapping(l.indent, path, depth);
    return this.inline(minIndent - 1, path, depth);
  }

  private sequence(indent: number, path: string, depth: number): JsonValue {
    const first = this.lines[this.i]!;
    this.checkDepth(depth, path, first, indent);
    const items: JsonValue[] = [];
    for (;;) {
      const l = this.cur();
      if (l === undefined || l.indent < indent) break;
      if (l.indent > indent) fail("Malformed", path, l, l.indent, "bad indentation of a sequence entry");
      if (!isSeqEntry(l.text)) break;
      const rest = l.text.slice(1);
      const body = rest.trimStart();
      const here = `${path}[${items.length}]`;
      if (rest.slice(0, rest.length - body.length).includes("\t") && isSeqEntry(body)) {
        fail("Malformed", here, l, l.indent + 1, "a tab cannot separate a `-` from a nested sequence");
      }
      if (body === "" || body.startsWith("#")) {
        this.i++;
        items.push(this.node(indent + 1, here, depth + 1));
      } else {
        const lead = 1 + (rest.length - body.length);
        this.lines[this.i] = { indent: l.indent + lead, text: body, no: l.no };
        items.push(this.node(indent + 1, here, depth + 1));
      }
    }
    return items;
  }

  private mapping(indent: number, path: string, depth: number): JsonValue {
    const first = this.lines[this.i]!;
    this.checkDepth(depth, path, first, indent);
    const obj: { [k: string]: JsonValue } = {};
    const seen = new Set<string>();
    for (;;) {
      const l = this.cur();
      if (l === undefined || l.indent < indent) break;
      if (l.indent > indent) fail("Malformed", path, l, l.indent, "bad indentation of a mapping entry");
      if (isSeqEntry(l.text)) {
        fail("Malformed", path, l, l.indent, "a sequence entry cannot follow the entries of a mapping");
      }
      const c = l.text[0]!;
      if (c === "&") fail("UnsupportedAnchor", path, l, l.indent, "anchors (`&name`) are not supported");
      if (c === "*") fail("UnsupportedAlias", path, l, l.indent, "aliases (`*name`) are not supported");
      if (c === "!") fail("UnsupportedTag", path, l, l.indent, "tags (`!tag`) are not supported");
      const k = this.scanKey(l);
      if (k === null) fail("Malformed", path, l, l.indent, "expected a `key: value` entry");
      const here = `${path}.${k.key}`;
      if (!k.quoted && k.key === "<<") {
        fail("UnsupportedMergeKey", here, l, l.indent, "merge keys (`<<`) are not supported");
      }
      if (seen.has(k.key)) fail("DuplicateKey", here, l, l.indent, `the key \`${k.key}\` appears twice`);
      seen.add(k.key);
      const rest = l.text.slice(k.end);
      const body = rest.trimStart();
      let value: JsonValue;
      if (body === "" || body.startsWith("#")) {
        this.i++;
        const n = this.cur();
        if (n !== undefined && n.indent === indent && isSeqEntry(n.text)) {
          value = this.sequence(indent, here, depth + 1);
        } else {
          value = this.node(indent + 1, here, depth + 1);
        }
      } else {
        this.lines[this.i] = { indent: l.indent + k.end + (rest.length - body.length), text: body, no: l.no };
        value = this.inline(indent, here, depth + 1);
      }
      setKey(obj, k.key, value);
    }
    return obj;
  }

  /**
   * An implicit mapping key at the start of `l.text`: its string, the offset just
   * past the `:`, and whether it was quoted. Null when the line is not a key line.
   */
  private scanKey(l: Line): { key: string; end: number; quoted: boolean } | null {
    const t = l.text;
    const c = t[0]!;
    if (c === "?" && (t.length === 1 || isSpace(t[1]))) {
      fail("UnsupportedSyntax", "$", l, l.indent, "complex mapping keys (`? key`) are not supported");
    }
    if (c === '"' || c === "'") {
      const q = this.quotedOnLine(l, 0);
      if (q === null) return null;
      let j = q.end;
      while (isSpace(t[j])) j++;
      if (t[j] === ":" && (j + 1 === t.length || isSpace(t[j + 1]))) {
        return { key: q.value, end: j + 1, quoted: true };
      }
      return null;
    }
    if (c === "[" || c === "{" || c === "|" || c === ">") return null;
    for (let j = 0; j < t.length; j++) {
      const ch = t[j]!;
      if (ch === ":" && (j + 1 === t.length || isSpace(t[j + 1]))) {
        const key = t.slice(0, j).trimEnd();
        if (key === "") fail("Malformed", "$", l, l.indent + j, "a mapping key is empty");
        return { key, end: j + 1, quoted: false };
      }
      if (ch === "#" && j > 0 && isSpace(t[j - 1])) return null;
    }
    return null;
  }

  /** A quoted scalar that must close on this line (an implicit key); null if it does not. */
  private quotedOnLine(l: Line, col: number): { value: string; end: number } | null {
    const r = new Flow([l], 0, col);
    try {
      return { value: r.quoted("$") as string, end: r.col };
    } catch (e) {
      if (typeof e === "object" && e !== null && (e as Failure).yamlFailure === true) return null;
      throw e;
    }
  }

  // ---- inline values: scalars, flow collections, block scalars --------------

  private inline(parentIndent: number, path: string, depth: number): JsonValue {
    const l = this.lines[this.i]!;
    const t = l.text;
    const c = t[0]!;
    if (c === "&") fail("UnsupportedAnchor", path, l, l.indent, "anchors (`&name`) are not supported");
    if (c === "*") fail("UnsupportedAlias", path, l, l.indent, "aliases (`*name`) are not supported");
    if (c === "!") fail("UnsupportedTag", path, l, l.indent, "tags (`!tag`) are not supported");
    if (c === "|" || c === ">") return this.blockScalar(parentIndent, path);
    if (c === "[" || c === "{") {
      const r = new Flow(this.lines, this.i, 0);
      const v = r.value(path, depth);
      this.endOfNode(r.line, r.col, path);
      return v;
    }
    if (c === '"' || c === "'") {
      const r = new Flow(this.lines, this.i, 0);
      const v = r.quoted(path);
      this.endOfNode(r.line, r.col, path);
      return v;
    }
    if (c === "%" || c === "@" || c === "`") {
      fail("Malformed", path, l, l.indent, `\`${c}\` cannot start a plain value; quote it`);
    }
    return this.plain(parentIndent, path);
  }

  /** After a flow or quoted node ends at (line, col): only a comment may follow. */
  private endOfNode(lineIdx: number, col: number, path: string): void {
    const l = this.lines[lineIdx]!;
    const rest = l.text.slice(col).trim();
    if (rest !== "" && !(rest.startsWith("#") && isSpace(l.text[col]))) {
      if (rest.startsWith(":")) {
        fail("UnsupportedSyntax", path, l, l.indent + col, "a collection or quoted value used as a mapping key is not supported");
      }
      fail("Malformed", path, l, l.indent + col, "unexpected content after the value");
    }
    this.i = lineIdx + 1;
  }

  private plain(parentIndent: number, path: string): JsonValue {
    const first = this.lines[this.i]!;
    const parts: string[] = [];
    let blanks = 0;
    let idx = this.i;
    let l: Line = first;
    for (;;) {
      let t = l.text;
      for (let j = 1; j < t.length; j++) {
        if (t[j] === "#" && isSpace(t[j - 1])) {
          t = t.slice(0, j);
          break;
        }
      }
      t = t.trim();
      if (/:(\s|$)/.test(t)) {
        fail("Malformed", path, l, l.indent, "a mapping value is not allowed here (a `key:` inside a plain value)");
      }
      if (parts.length > 0) parts.push(blanks > 0 ? "\n".repeat(blanks) : " ");
      parts.push(t);
      blanks = 0;
      idx++;
      // A continuation: a deeper, non-comment line, with blank lines folded to newlines.
      let n = idx;
      let b = 0;
      while (n < this.lines.length && isBlank(this.lines[n]!)) {
        n++;
        b++;
      }
      const next = this.lines[n];
      if (
        next === undefined ||
        next.indent <= parentIndent ||
        next.text.startsWith("#") ||
        marker(next, "---") ||
        marker(next, "...") ||
        isSeqEntry(next.text) && next.indent <= first.indent
      ) {
        break;
      }
      blanks = b;
      idx = n;
      l = next;
    }
    this.i = idx;
    return resolvePlain(parts.join(""), path, first, first.indent);
  }

  private blockScalar(parentIndent: number, path: string): string {
    const head = this.lines[this.i]!;
    const literal = head.text[0] === "|";
    const m = /^[|>]([1-9]?)([+-]?)([1-9]?)(\s+#.*)?$/.exec(head.text.replace(/\s+$/, ""));
    if (m === null || (m[1] !== "" && m[3] !== "")) {
      fail("Malformed", path, head, head.indent, "a block scalar header may hold only an indentation digit and a chomping sign");
    }
    const digit = m[1] !== "" ? m[1]! : m[3]!;
    const chomp = m[2]!;
    this.i++;
    let indent = digit !== "" ? parentIndent + Number(digit) : -1;
    const body: Line[] = [];
    let j = this.i;
    while (j < this.lines.length) {
      const l = this.lines[j]!;
      if (marker(l, "---") || marker(l, "...")) break;
      if (isBlank(l)) {
        body.push(l);
        j++;
        continue;
      }
      if (indent < 0) {
        if (l.indent <= parentIndent) break;
        indent = l.indent;
      }
      if (l.indent < indent) break;
      body.push(l);
      j++;
    }
    // Trailing blank lines belong to the chomp; leave non-content lines unread.
    this.i = j;
    if (indent < 0 && body.some((l) => l.text.includes("\t"))) {
      fail("UnsupportedSyntax", path, head, head.indent, "a block scalar whose only content is a tab-only line is not supported");
    }
    // A whitespace-only line is empty unless it runs past the content indent, where
    // the extra whitespace is content.
    const rows = body.map((l) => {
      if (!isBlank(l)) return " ".repeat(l.indent - indent) + l.text;
      const raw = " ".repeat(l.indent) + l.text;
      return indent >= 0 && raw.length > indent ? raw.slice(indent) : "";
    });
    let end = rows.length;
    while (end > 0 && rows[end - 1] === "") end--;
    const trailing = rows.length - end;
    let out = "";
    let prev: "none" | "text" | "spaced" = "none";
    let blanks = 0;
    for (let k = 0; k < end; k++) {
      const r = rows[k]!;
      if (r === "") {
        blanks++;
        continue;
      }
      const kind = r[0] === " " || r[0] === "\t" ? "spaced" : "text";
      if (prev === "none") out += "\n".repeat(blanks);
      else if (literal || prev === "spaced" || kind === "spaced") out += "\n".repeat(blanks + 1);
      else out += blanks === 0 ? " " : "\n".repeat(blanks);
      out += r;
      prev = kind;
      blanks = 0;
    }
    if (end === 0) return chomp === "+" ? "\n".repeat(trailing) : "";
    if (chomp === "-") return out;
    if (chomp === "+") return `${out}\n${"\n".repeat(trailing)}`;
    return `${out}\n`;
  }
}

/** A character cursor over `lines` for flow collections and quoted scalars. */
class Flow {
  line: number;
  col: number;
  private readonly lines: Line[];

  constructor(lines: Line[], line: number, col: number) {
    this.lines = lines;
    this.line = line;
    this.col = col;
  }

  private text(): string {
    return this.lines[this.line]!.text;
  }

  private at(): Line {
    return this.lines[this.line]!;
  }

  private err(kind: string, path: string, msg: string): never {
    return fail(kind, path, this.at(), this.at().indent + this.col, msg);
  }

  /** Step over whitespace, line breaks and comments; false at the end of input. */
  private ws(): boolean {
    for (;;) {
      const t = this.text();
      while (this.col < t.length && isSpace(t[this.col])) this.col++;
      if (this.col < t.length && t[this.col] === "#" && (this.col === 0 || isSpace(t[this.col - 1]))) {
        this.col = t.length;
      }
      if (this.col < t.length) return true;
      if (this.line + 1 >= this.lines.length) return false;
      this.line++;
      this.col = 0;
      this.noMarker();
    }
  }

  /** A document marker at column 0 cannot sit inside a flow collection or a quoted value. */
  private noMarker(): void {
    const l = this.at();
    if (marker(l, "---") || marker(l, "...")) {
      this.err("Malformed", "$", "a document marker (`---` or `...`) inside a flow collection or quoted value");
    }
  }

  value(path: string, depth: number): JsonValue {
    if (!this.ws()) this.err("Malformed", path, "a flow collection is not closed");
    const t = this.text();
    const c = t[this.col]!;
    if (c === "[") return this.seq(path, depth);
    if (c === "{") return this.map(path, depth);
    if (c === '"' || c === "'") return this.quoted(path);
    if (c === "&") this.err("UnsupportedAnchor", path, "anchors (`&name`) are not supported");
    if (c === "*") this.err("UnsupportedAlias", path, "aliases (`*name`) are not supported");
    if (c === "!") this.err("UnsupportedTag", path, "tags (`!tag`) are not supported");
    if (c === "|" || c === ">" || c === "%" || c === "@" || c === "`") {
      this.err("Malformed", path, `\`${c}\` cannot start a value here; quote it`);
    }
    return this.plainFlow(path);
  }

  private seq(path: string, depth: number): JsonValue {
    if (depth + 1 > YAML_MAX_DEPTH) {
      this.err("LimitExceeded", path, `nesting is deeper than ${YAML_MAX_DEPTH} levels`);
    }
    this.col++;
    const items: JsonValue[] = [];
    for (;;) {
      if (!this.ws()) this.err("Malformed", path, "a flow sequence is not closed");
      if (this.text()[this.col] === "]") {
        this.col++;
        return items;
      }
      const v = this.value(`${path}[${items.length}]`, depth + 1);
      items.push(v);
      if (!this.ws()) this.err("Malformed", path, "a flow sequence is not closed");
      const c = this.text()[this.col];
      if (c === ":" ) {
        this.err("UnsupportedSyntax", path, "a `key: value` pair inside a flow sequence is not supported");
      }
      if (c === ",") this.col++;
      else if (c !== "]") this.err("Malformed", path, "expected `,` or `]` in a flow sequence");
    }
  }

  private map(path: string, depth: number): JsonValue {
    if (depth + 1 > YAML_MAX_DEPTH) {
      this.err("LimitExceeded", path, `nesting is deeper than ${YAML_MAX_DEPTH} levels`);
    }
    this.col++;
    const obj: { [k: string]: JsonValue } = {};
    const seen = new Set<string>();
    for (;;) {
      if (!this.ws()) this.err("Malformed", path, "a flow mapping is not closed");
      if (this.text()[this.col] === "}") {
        this.col++;
        return obj;
      }
      const c = this.text()[this.col]!;
      if (c === "?" && isSpace(this.text()[this.col + 1])) {
        this.err("UnsupportedSyntax", path, "complex mapping keys (`? key`) are not supported");
      }
      if (c === "[" || c === "{") {
        this.err("UnsupportedSyntax", path, "a collection used as a mapping key is not supported");
      }
      let key: string;
      let quoted = false;
      if (c === '"' || c === "'") {
        key = this.quoted(path) as string;
        quoted = true;
      } else {
        if (c === "&") this.err("UnsupportedAnchor", path, "anchors (`&name`) are not supported");
        if (c === "*") this.err("UnsupportedAlias", path, "aliases (`*name`) are not supported");
        if (c === "!") this.err("UnsupportedTag", path, "tags (`!tag`) are not supported");
        key = this.plainText(path, true);
      }
      const here = `${path}.${key}`;
      if (!quoted && key === "<<") {
        this.err("UnsupportedMergeKey", here, "merge keys (`<<`) are not supported");
      }
      if (seen.has(key)) this.err("DuplicateKey", here, `the key \`${key}\` appears twice`);
      seen.add(key);
      if (!this.ws()) this.err("Malformed", path, "a flow mapping is not closed");
      let value: JsonValue = null;
      if (this.text()[this.col] === ":") {
        this.col++;
        if (!this.ws()) this.err("Malformed", path, "a flow mapping is not closed");
        const n = this.text()[this.col];
        if (n !== "," && n !== "}") value = this.value(here, depth + 1);
      }
      setKey(obj, key, value);
      if (!this.ws()) this.err("Malformed", path, "a flow mapping is not closed");
      const d = this.text()[this.col];
      if (d === ",") this.col++;
      else if (d !== "}") this.err("Malformed", path, "expected `,` or `}` in a flow mapping");
    }
  }

  /** The raw text of a plain flow scalar, folded across lines. */
  private plainText(path: string, key: boolean): string {
    const parts: string[] = [];
    for (;;) {
      const t = this.text();
      const start = this.col;
      while (this.col < t.length) {
        const ch = t[this.col]!;
        if (ch === "," || ch === "[" || ch === "]" || ch === "{" || ch === "}") break;
        if (ch === ":" && (this.col + 1 === t.length || isSpace(t[this.col + 1]) || ",[]{}".includes(t[this.col + 1]!))) break;
        if (ch === "#" && this.col > 0 && isSpace(t[this.col - 1])) break;
        this.col++;
      }
      parts.push(t.slice(start, this.col).trim());
      if (this.col < t.length) break;
      // The line ended inside the scalar: continue on the next non-blank line.
      let n = this.line + 1;
      let blanks = 0;
      while (n < this.lines.length && isBlank(this.lines[n]!)) {
        n++;
        blanks++;
      }
      const next = this.lines[n];
      if (key || next === undefined) break;
      const first = next.text.trimStart()[0]!;
      if (first === "#" || first === "," || first === "]" || first === "}" || first === ":") break;
      parts.push(blanks > 0 ? "\n".repeat(blanks) : " ");
      this.line = n;
      this.col = 0;
    }
    const text = parts.join("");
    if (text === "" && !key) this.err("Malformed", path, "expected a value");
    return text;
  }

  private plainFlow(path: string): JsonValue {
    const line = this.at();
    const col = this.col;
    const t = this.text();
    if (t[col] === "-" && (col + 1 === t.length || isSpace(t[col + 1]) || ",[]{}".includes(t[col + 1]!))) {
      this.err("Malformed", path, "a `-` cannot be a plain value inside a flow collection; quote it");
    }
    return resolvePlain(this.plainText(path, false), path, line, line.indent + col);
  }

  /** A quoted scalar starting at the cursor; the cursor ends after the closing quote. */
  quoted(path: string): JsonValue {
    const q = this.text()[this.col]!;
    const double = q === '"';
    this.col++;
    let out = "";
    // Whitespace written as an escape (`\t`) is content; only literal whitespace
    // before a line break is trimmed.
    let escaped = 0;
    for (;;) {
      while (this.col < this.text().length) {
        const t = this.text();
        const ch = t[this.col]!;
        if (ch === q) {
          if (!double && t[this.col + 1] === "'") {
            out += "'";
            this.col += 2;
            continue;
          }
          this.col++;
          return out;
        }
        if (double && ch === "\\") {
          const e = t[this.col + 1];
          if (e === undefined) {
            // An escaped line break: join the next line with no space.
            if (this.line + 1 >= this.lines.length) this.err("Malformed", path, "a quoted value is not closed");
            this.line++;
            this.col = 0;
            this.noMarker();
            const nt = this.text();
            while (this.col < nt.length && isSpace(nt[this.col])) this.col++;
            continue;
          }
          const n = e === "x" ? 2 : e === "u" ? 4 : e === "U" ? 8 : 0;
          if (n > 0) {
            const hex = t.slice(this.col + 2, this.col + 2 + n);
            if (hex.length !== n || !/^[0-9a-fA-F]+$/.test(hex)) {
              this.err("Malformed", path, `a \`\\${e}\` escape needs ${n} hex digits`);
            }
            const cp = parseInt(hex, 16);
            if (cp > 0x10ffff || (cp >= 0xd800 && cp <= 0xdfff)) {
              this.err("Malformed", path, "an escape is not a Unicode scalar value");
            }
            out += String.fromCodePoint(cp);
            escaped = out.length;
            this.col += 2 + n;
            continue;
          }
          const mapped = ESCAPES[e];
          if (mapped === undefined) this.err("Malformed", path, `unknown escape \`\\${e}\``);
          out += mapped;
          escaped = out.length;
          this.col += 2;
          continue;
        }
        out += ch;
        this.col++;
      }
      // The line ended inside the scalar: fold the break.
      out = out.slice(0, escaped) + out.slice(escaped).replace(/[ \t]+$/, "");
      let n = this.line + 1;
      let blanks = 0;
      while (n < this.lines.length && isBlank(this.lines[n]!)) {
        n++;
        blanks++;
      }
      if (n >= this.lines.length) this.err("Malformed", path, "a quoted value is not closed");
      out += blanks > 0 ? "\n".repeat(blanks) : " ";
      this.line = n;
      this.col = 0;
      this.noMarker();
      const nt = this.text();
      while (this.col < nt.length && isSpace(nt[this.col])) this.col++;
    }
  }
}

/** Parse YAML text to a JSON-shaped value, or a positioned `DecodeError`. */
export function __bynkYamlParse(text: string): Result<JsonValue, DecodeError> {
  try {
    if (text.length > YAML_MAX_BYTES || utf8Length(text) > YAML_MAX_BYTES) {
      return Err({
        kind: "LimitExceeded",
        path: "$",
        message: `the input is longer than ${YAML_MAX_BYTES} bytes`,
      });
    }
    if (text.charCodeAt(0) === 0xfeff) text = text.slice(1);
    const lines: Line[] = [];
    const raw = text.split(/\r\n|\n|\r/);
    // A trailing line break ends the last line; it does not start another.
    if (raw.length > 1 && raw[raw.length - 1] === "") raw.pop();
    for (let k = 0; k < raw.length; k++) {
      const r = raw[k]!;
      let indent = 0;
      while (r[indent] === " ") indent++;
      const body = r.slice(indent);
      lines.push({ indent, text: body, no: k + 1 });
    }
    return Ok(new Parser(lines).document());
  } catch (e) {
    if (typeof e === "object" && e !== null && (e as Failure).yamlFailure === true) {
      const f = e as Failure;
      return Err({ kind: f.kind, path: f.path, message: f.message });
    }
    throw e;
  }
}

// A lightweight CodeMirror 6 stream highlighter for Bynk.
//
// NOTE (in-browser track Q4): the intended end-state is highlighting from
// `tree-sitter-bynk` compiled to web-tree-sitter wasm — the grammar + queries
// already exist (`tree-sitter-bynk/queries/highlights.scm`) and `scripts/
// build-grammar.sh` builds the wasm. At slice 4 that build needed `emcc` or a running
// docker daemon, which the environment lacked, so the MVP shipped this faithful
// stream highlighter; the tree-sitter CLI now fetches its own wasi-sdk, and this
// remains the fallback when the grammar wasm is absent.

import { HighlightStyle, StreamLanguage, syntaxHighlighting } from "@codemirror/language";
import { tags as t } from "@lezer/highlight";
import type { Extension } from "@codemirror/state";

// Lower-cased keywords, mirroring the lexer's `#[token(...)]` set
// (`bynk-syntax/src/lexer.rs`). Base-type keywords (`Int`, `String`, …) start
// uppercase and fall through to `TYPES`/the capitalised-identifier rule; the
// constructor/literal keywords (`true`, `Ok`, …) live in `ATOMS` below.
const KEYWORDS = new Set([
  "commons", "type", "fn", "where", "let", "if", "else", "enum", "match",
  "record", "self", "is", "opaque", "uses", "context", "consumes", "exports",
  "transparent", "as", "expect", "suite", "case", "stub", "property",
  "adapter", "binding", "agent", "capability", "do", "given", "on", "http",
  "cron", "queue", "from", "protocol", "provides", "service", "actor", "by",
  "invariant", "implies", "requires", "ensures", "transition",
]);

// Built-in type / kind names and well-known constructors.
const TYPES = new Set([
  "String", "Int", "Float", "Bool", "Bytes", "Effect", "Result", "Option",
  "Query", "Stream", "List", "Map", "Set", "Instant", "Duration", "Uuid",
  "Connection", "Request", "Response", "Method", "FetchError",
  "ValidationError", "JsonError",
]);
// Constructors, boolean literals, and HTTP verbs. The verbs are upper-cased to
// match the grammar's `http_method` choice (`GET`/`POST`/`PUT`/`PATCH`/`DELETE`,
// `tree-sitter-bynk/grammar.js`).
const ATOMS = new Set([
  "Ok", "Err", "Some", "None", "true", "false",
  "GET", "POST", "PUT", "PATCH", "DELETE",
]);

const bynk = StreamLanguage.define({
  name: "bynk",
  startState: () => ({}),
  token(stream) {
    if (stream.eatSpace()) return null;

    // Only indentation precedes the cursor on its line.
    const firstOnLine = stream.string.slice(0, stream.pos).trim() === "";
    // Doc blocks are delimited by `---` lines; highlight those markers line-wise.
    if (firstOnLine && stream.match(/^-{3,}\s*$/)) return "comment";
    // #2203: a `--- text` one-line doc, first on its line.
    if (firstOnLine && stream.match(/^-{3,}[ \t]+\S/)) {
      stream.skipToEnd();
      return "comment";
    }
    // A `//` line comment, anywhere (#2203). `--` is two `-` operators.
    if (stream.match("//")) {
      stream.skipToEnd();
      return "comment";
    }

    // Strings (double-quoted, with escapes).
    if (stream.match(/^"(?:[^"\\]|\\.)*"/)) return "string";
    if (stream.peek() === '"') {
      stream.next();
      while (!stream.eol()) {
        if (stream.next() === '"') break;
      }
      return "string";
    }

    // Numbers.
    if (stream.match(/^\d+(\.\d+)?/)) return "number";

    // Identifiers / keywords / types.
    const word = stream.match(/^[A-Za-z_][A-Za-z0-9_]*/);
    if (word) {
      // `match(regex)` returns the match array at runtime (the `true` arm is the
      // string-arg overload); index 0 is the matched word.
      const w = (word as RegExpMatchArray)[0];
      if (KEYWORDS.has(w)) return "keyword";
      if (ATOMS.has(w)) return "atom";
      if (TYPES.has(w) || /^[A-Z]/.test(w)) return "typeName";
      return "variableName";
    }

    // Operators / punctuation worth tinting.
    if (stream.match(/^(<-|->|=>|\+\+|::|[=+\-*/<>!&|?:.,;])/)) return "operator";

    stream.next();
    return null;
  },
});

// A dark highlight theme matching the playground shell.
const style = HighlightStyle.define([
  { tag: t.keyword, color: "#c792ea" },
  { tag: t.typeName, color: "#82aaff" },
  { tag: t.atom, color: "#f78c6c" },
  { tag: t.string, color: "#c3e88d" },
  { tag: t.number, color: "#f78c6c" },
  { tag: t.comment, color: "#5c6370", fontStyle: "italic" },
  { tag: t.operator, color: "#89ddff" },
  { tag: t.variableName, color: "#eeffff" },
]);

export function bynkHighlighting(): Extension {
  return [bynk, syntaxHighlighting(style)];
}

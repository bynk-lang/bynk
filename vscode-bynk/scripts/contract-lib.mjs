// Pure checks of the extension against the compiler's published editor
// contract (`bynk-editor-contract-<tag>.json`, a release asset). They replace
// the Rust drift guards that read this directory from the compiler's tests
// (`textmate_keywords`, `legend_drift`, `scaffolds_compile`), which only work
// while the extension lives in the compiler's repository. No I/O here: see
// check-contract.mjs for fetching, and contract-lib.test.mjs for the tests.

/** Whether `word` occurs as a whole word (alphanumeric/`_` boundaries) in `text`. */
export function wholeWordPresent(text, word) {
  const isWord = (c) => c !== undefined && /[\p{L}\p{N}_]/u.test(c);
  for (let at = text.indexOf(word); at !== -1; at = text.indexOf(word, at + 1)) {
    if (!isWord(text[at - 1]) && !isWord(text[at + word.length])) return true;
  }
  return false;
}

/**
 * Reserved keywords the TextMate grammar leaves dark. Only the converse is not
 * asserted: the grammar may highlight contextual words that are not reserved.
 */
export function missingKeywords(contract, tmLanguageText) {
  return contract.reservedKeywords.filter((w) => !wholeWordPresent(tmLanguageText, w));
}

/** The `id` of every entry in `contributes.<key>` of package.json. */
function declaredIds(pkg, key) {
  const entries = pkg?.contributes?.[key];
  if (!Array.isArray(entries)) throw new Error(`contributes.${key} is not an array`);
  return entries.map((e) => e.id);
}

/**
 * Differences between the custom semantic-token entries the server emits and
 * those package.json declares, as sets. An empty result means they agree. A
 * custom entry the extension lacks renders with no colour; one it declares that
 * the server no longer emits is stale.
 */
export function legendMismatches(contract, pkg) {
  const out = [];
  for (const [key, custom] of [
    ["semanticTokenTypes", "customTypes"],
    ["semanticTokenModifiers", "customModifiers"],
  ]) {
    const want = new Set(contract.semanticTokens[custom]);
    const have = new Set(declaredIds(pkg, key));
    for (const id of want) if (!have.has(id)) out.push(`${key}: the server emits \`${id}\` but package.json does not declare it`);
    for (const id of have) if (!want.has(id)) out.push(`${key}: package.json declares \`${id}\` but the server does not emit it`);
  }
  return out;
}

// -- snippets -----------------------------------------------------------------
// A port of bynk-lsp/tests/support/mod.rs: strip VS Code tab-stop syntax to a
// compilable skeleton, then wrap it in the minimal source it needs to parse.

/** What a bare placeholder (`$0`, `${1}`) becomes, given what surrounds it. */
function bareFill(outSoFar, after, exprBlock) {
  const prev = outSoFar.trimEnd().slice(-1);
  const next = after.trimStart().slice(0, 1);
  if (prev === "(" && next === ")") return "";
  if (exprBlock[exprBlock.length - 1] === true && next === "}") return "()";
  return "";
}

/**
 * `${N:default}` → `default`; `${N|a,b|}` → `a`; a bare `$N` / `${N}` → `()` if
 * it is the last thing before a `}` in an expression block (a body after `->`),
 * and empty otherwise.
 */
export function stripSnippetPlaceholders(body) {
  let out = "";
  let boundary = 0;
  const exprBlock = [];
  let i = 0;
  while (i < body.length) {
    const rest = body.slice(i);
    if (rest.startsWith("${")) {
      const end = rest.slice(2).indexOf("}");
      if (end !== -1) {
        const spec = rest.slice(2, 2 + end);
        const consumed = 2 + end + 1;
        const colon = spec.indexOf(":");
        const bar = spec.indexOf("|");
        if (colon !== -1) {
          out += spec.slice(colon + 1);
        } else if (bar !== -1) {
          const choices = spec.slice(bar + 1);
          const close = choices.lastIndexOf("|");
          out += (close === -1 ? choices : choices.slice(0, close)).split(",")[0];
        } else {
          out += bareFill(out, body.slice(i + consumed), exprBlock);
        }
        i += consumed;
        continue;
      }
    }
    if (rest.startsWith("$")) {
      const digits = /^\d+/.exec(rest.slice(1));
      if (digits) {
        out += bareFill(out, rest.slice(1 + digits[0].length), exprBlock);
        i += 1 + digits[0].length;
        continue;
      }
    }
    const ch = rest[0];
    if (ch === "{") {
      exprBlock.push(out.slice(boundary).includes("->"));
      out += ch;
      boundary = out.length;
    } else if (ch === "}") {
      exprBlock.pop();
      out += ch;
      boundary = out.length;
    } else {
      out += ch;
    }
    i += 1;
  }
  return out;
}

/** How the scaffold named `name` (its key in snippets/bynk.json) must be wrapped to parse. */
function classify(name) {
  if (["context", "commons", "adapter", "suite", "suite as"].includes(name)) return "unit";
  if (["property", "case", "case as", "stub"].includes(name)) return "suite";
  if (name === "advance Clock by") return "case";
  if (name === "uses" || name === "consumes") return "header";
  if (name === "match") return "expr";
  if (name.startsWith("on ")) return "handler";
  if (name.startsWith("service") && name !== "service") return "handler";
  return "item";
}

/** The minimal source a stripped scaffold body parses in. */
export function wrapForParse(name, body) {
  switch (classify(name)) {
    case "unit": return body;
    case "item": return `context Scaffold\n\n${body}\n`;
    case "handler": return `context Scaffold\n\nservice Scaffold {\n${body}\n}\n`;
    case "suite": return `suite Scaffold {\n${body}\n}\n`;
    case "case": return `suite Scaffold {\ncase "t" {\n${body}\n}\n}\n`;
    case "header": return `context Scaffold\n${body}\n`;
    case "expr": return `context Scaffold\n\nfn scaffold() -> Unit {\n${body}\n}\n`;
  }
}

/** The `[name, source]` skeleton of every snippet in a parsed snippets/bynk.json. */
export function snippetSkeletons(snippets) {
  const entries = Object.entries(snippets);
  if (entries.length === 0) throw new Error("snippets/bynk.json has no snippets");
  return entries.map(([name, entry]) => {
    if (!Array.isArray(entry.body)) throw new Error(`${name}: body is not an array`);
    return [name, wrapForParse(name, stripSnippetPlaceholders(entry.body.join("\n")))];
  });
}

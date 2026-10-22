// #855: the wire-contract peek's pure DOM-tree renderer — separated from
// `webview/wireContract.ts` (the bundle entry that reads
// `window.__BYNK_WIRE_CONTRACT__` and owns `acquireVsCodeApi()`) so this
// module has no dependency on the webview-bootstrapping concerns, only on
// the `Wc*` payload shape and plain DOM APIs.
//
// **Plain DOM, not Mermaid, not markdown-it.** A JSON shape tree is a nested
// list, not a graph and not prose — building it element-by-element means
// every node owns its own `WcLoc`/`Range` directly, so click-to-code is a
// direct lookup (`ref.kind === "Named" -> that type's own block`), never the
// DOM-order zip against a rendered SVG that `architecture.ts`'s Mermaid path
// needs. Neither library enters this bundle, keeping it the smallest of the
// four webview bundles.
//
// **Testability split.** Most of this module's line count is DOM
// construction (`document.createElement`), which this repo's test harness
// cannot exercise directly — Mocha here runs inside the extension host (a
// plain Node process with no `window`/`document`; there is no jsdom in this
// project, and none of `webview/main.ts` (#846), `webview/docview.ts` (#847)
// or `webview/architecture.ts` (#851) have direct DOM-level tests either, for
// the same reason). What *is* factored out and kept fully DOM-free — plain
// functions of plain data, `string`/`boolean` in and out — are the sentence-
// and label-generating functions below (`formatHeaderLine`,
// `bareEnvelopeSentence`, `revalidationSentence`, etc.), mirroring how
// `mermaid-gen.ts` (#846) and `architecture-gen.ts` (#851) keep their pure
// text-generation testable and leave the DOM/Mermaid wiring in their
// untested entry files. `test/suite/wire-render.unit.test.ts` covers these
// directly.

import type {
  WcBody,
  WcBoundaryKind,
  WcContractForm,
  WcField,
  WcHttpResponse,
  WcInst,
  WcJsonKind,
  WcModel,
  WcNoCrossContextReason,
  WcParam,
  WcPayload,
  WcRef,
  WcResponseOrigin,
  WcRevalidation,
  WcType,
  WcVariant,
} from "./wireContractTypes";

// ---------------------------------------------------------------------
// Pure text generation — no `document`, unit-tested directly.
// ---------------------------------------------------------------------

/** `WcJsonKind` as it appears on the wire (`"String"` -> `"string"`). */
export function jsonKindWire(json: WcJsonKind): string {
  return json.toLowerCase();
}

/** The header line for a handler's boundary kind, e.g. `on GET("/check/:client")`,
 *  `on call`, `on cron("0 * * * *")`. */
export function formatHeaderLine(kind: WcBoundaryKind): string {
  switch (kind.kind) {
    case "Http":
      return `on ${kind.method}("${kind.path}")`;
    case "Call":
      return "on call";
    case "Cron":
      return `on cron("${kind.expr}")`;
    case "Message":
      return "on message";
    case "Open":
      return "on open";
    case "Close":
      return "on close";
    case "Event":
      return "on event";
  }
}

export function emptyEnvelopeSentence(): string {
  return "This handler takes no parameters — the request body is empty.";
}

/** `Envelope::Bare` — one param, so the request body *is* that param's
 *  value, not `{ "<param>": … }` (DECISION per `wire.rs`'s three-case
 *  envelope; #855's Part 3 note that both the issue text and `contract.rs`
 *  understate this to two cases). */
export function bareEnvelopeSentence(param: string): string {
  return `The request body is the value of \`${param}\`.`;
}

/** `Envelope::Raw` — a `@raw(contentType:)` route: the request body is the
 *  octets of `param`, not JSON and not base64, under the declared media type
 *  (a wildcard accepts any). */
export function rawEnvelopeSentence(param: string, contentType: string): string {
  const type = contentType === "*/*" ? "any content type" : `\`${contentType}\``;
  return `The request body is the raw octets of \`${param}\` — not JSON, not base64 — sent as ${type}.`;
}

/** The contract card's closing sentence — the consequence of the hash: any
 *  edit to the normal form breaks already-deployed callers closed, not open. */
export function contractMismatchSentence(): string {
  return "Change any of this and already-deployed callers get a 409 ContractMismatch.";
}

export function noCrossContextSentence(reason: WcNoCrossContextReason): string {
  switch (reason) {
    case "NotACallHandler":
      return "No cross-context contract here — this handler isn't on call, so nothing hashes its shape.";
    case "SingleContext":
      return "No cross-context contract here — this project has a single context, so nothing crosses a boundary.";
  }
}

/** The re-validation sentence for one boundary scalar — the peek's whole
 *  reason for existing (#855's Context: "An author cannot see … that
 *  `ClientId` is re-validated as `NonEmpty` on entry, without reading
 *  generated `.ts`."). Four distinct truths, one per `Revalidation` arm; the
 *  `StructuralOnly` arm is the "refinement rendering that misstates the
 *  boundary check" risk the issue calls out — it must read as an honest
 *  counter-statement, not a muted version of the re-validated one. */
export function revalidationSentence(
  typeName: string,
  json: WcJsonKind,
  revalidation: WcRevalidation,
  predicates: string[],
): string {
  const wire = jsonKindWire(json);
  switch (revalidation) {
    case "ViaConstructor":
    case "Inline": {
      const preds = predicates.length > 0 ? predicates.join(", ") : "its predicates";
      return `${typeName} → wire ${wire}, re-validated ${preds} at the boundary (never trusted from the caller).`;
    }
    case "StructuralOnly":
      return `${typeName} → wire ${wire} — not re-validated here; opaque to this context; skew is caught by the contract hash instead.`;
    case "Base64Decode":
      return `${typeName} → wire ${wire} (base64), decoded not cast.`;
  }
}

/** A `WcResponseOrigin::BoundaryImplicit` row is visually muted; every other
 *  origin renders at full weight. */
export function responseRowMuted(origin: WcResponseOrigin): boolean {
  return origin.kind === "BoundaryImplicit";
}

/** The boundary-implicit reason text, `null` for any other origin (nothing
 *  to show — `DeclaredSuccess` and `Constructed` are self-evident from the
 *  status/variant columns, `Constructed` additionally carrying a
 *  click-to-code range). */
export function responseRowWhy(origin: WcResponseOrigin): string | null {
  return origin.kind === "BoundaryImplicit" ? origin.why : null;
}

// ---------------------------------------------------------------------
// DOM construction — untested directly (see module doc), built on the pure
// text above wherever the two meet.
// ---------------------------------------------------------------------

type RevealFn = (uri: string, range: WcModel["range"]) => void;

interface RenderCtx {
  payload: WcPayload;
  onReveal: RevealFn;
  instByKey: Map<string, WcInst>;
  /** `model.recursive` as a set, for an O(1) "does this type's own block
   *  need the (recursive) marker" check per type block. */
  recursiveNames: Set<string>;
  typeBlocks: Map<string, HTMLElement>;
}

function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  opts?: { className?: string; text?: string },
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (opts?.className) node.className = opts.className;
  if (opts?.text !== undefined) node.textContent = opts.text;
  return node;
}

function code(text: string): HTMLElement {
  return el("code", { text });
}

function makeClickable(node: HTMLElement, uri: string, range: WcModel["range"], ctx: RenderCtx): void {
  node.style.cursor = "pointer";
  node.addEventListener("click", () => ctx.onReveal(uri, range));
}

/** A clickable chip referencing another boundary type's own block — never
 *  re-expanded inline, only ever a jump target (per #855's Part 6 rendering
 *  note on recursive/named back-references). */
function typeChip(name: string, ctx: RenderCtx): HTMLElement {
  const chip = el("a", { className: "type-chip", text: name });
  chip.href = `#wc-type-${cssEscape(name)}`;
  chip.addEventListener("click", (ev) => {
    ev.preventDefault();
    const target = ctx.typeBlocks.get(name);
    if (!target) return;
    if (target instanceof HTMLDetailsElement) target.open = true;
    target.scrollIntoView({ behavior: "smooth", block: "start" });
  });
  return chip;
}

function cssEscape(name: string): string {
  return name.replace(/[^a-zA-Z0-9_-]/g, "_");
}

/** Append `parts` (mixed text and elements) to `parent` in order. */
function appendAll(parent: HTMLElement, parts: (Node | string)[]): void {
  for (const p of parts) parent.append(typeof p === "string" ? document.createTextNode(p) : p);
}

/** Render one `WcRef` inline as its wire-form parts (used for envelope
 *  params, record/variant fields, and generic-instantiation arguments). */
function refParts(ref: WcRef, ctx: RenderCtx): (Node | string)[] {
  switch (ref.kind) {
    case "Base":
      return [code(jsonKindWire(ref.json))];
    case "Bytes":
      return [code("string"), " (base64)"];
    case "Named":
      return [typeChip(ref.name, ctx)];
    case "Inst": {
      const inst = ctx.instByKey.get(ref.key);
      return inst ? instParts(inst, ctx) : [code(`<${ref.key}>`)];
    }
    case "Unit":
      return [code("null")];
    case "Unchecked":
      return [code(`<${ref.reason}>`)];
  }
}

function taggedObjParts(
  tag: string,
  fields: [string, WcRef][],
  ctx: RenderCtx,
): (Node | string)[] {
  const parts: (Node | string)[] = [`{ "kind": "${tag}"`];
  for (const [name, ref] of fields) {
    parts.push(`, "${name}": `, ...refParts(ref, ctx));
  }
  parts.push(" }");
  return parts;
}

function joinArgs(args: WcRef[], ctx: RenderCtx): (Node | string)[] {
  const parts: (Node | string)[] = [];
  args.forEach((a, i) => {
    if (i > 0) parts.push(", ");
    parts.push(...refParts(a, ctx));
  });
  return parts;
}

/** The four composite shapes invisible from source, rendered as their wire
 *  form (#855's Part 6 rendering note): `List` -> `[ <elem> ]`, `Map` -> an
 *  entries array, `Option`/`Result` -> `kind`-tagged alternatives. */
function instParts(inst: WcInst, ctx: RenderCtx): (Node | string)[] {
  switch (inst.kind) {
    case "ListInst":
      return ["[ ", ...refParts(inst.elem, ctx), " ]"];
    case "MapInst":
      return ["[[ ", ...refParts(inst.mapKey, ctx), ", ", ...refParts(inst.val, ctx), " ], …]"];
    case "OptionInst":
      return [
        ...taggedObjParts("Some", [["value", inst.inner]], ctx),
        " | ",
        ...taggedObjParts("None", [], ctx),
      ];
    case "ResultInst":
      return [
        ...taggedObjParts("Ok", [["value", inst.ok]], ctx),
        " | ",
        ...taggedObjParts("Err", [["error", inst.err]], ctx),
      ];
    case "RecordInst":
    case "SumInst":
      return [`${inst.name}<`, ...joinArgs(inst.args, ctx), ">"];
  }
}

/** One `"name": <shape>` entry, `name` suffixed `?` when the field has a
 *  default (a defaulted field is optional on the wire — a real fact the
 *  server sends via `WcField.default`, not decoration) and trailed with the
 *  default's own source rendering as a comment.
 *
 *  `WcField.pathSegment` is deliberately not surfaced here: today it is
 *  always identical to `name` (`bynk-check/src/wire.rs`'s `wire_type`
 *  derives it as `f.name.name.clone()`); it exists on the wire for a
 *  possible future where a field's JSON key diverges from its Bynk name.
 *  Nothing to show yet without inventing UI for a divergence that cannot
 *  currently occur. */
function objectFieldParts(
  name: string,
  shape: WcRef,
  defaultExpr: string | null,
  ctx: RenderCtx,
): (Node | string)[] {
  const key = defaultExpr !== null ? `"${name}"?` : `"${name}"`;
  const parts: (Node | string)[] = [`${key}: `, ...refParts(shape, ctx)];
  if (defaultExpr !== null) parts.push(` /* default: ${defaultExpr} */`);
  return parts;
}

/** A JSON object skeleton, one field per line in declaration order (#855's
 *  Part 6 rendering note: `record -> { "field": <shape>, ... }` one line per
 *  field) — used for record bodies, envelope params, and sum-variant
 *  payloads alike. */
function objectSkeletonParts(
  entries: { name: string; shape: WcRef; defaultExpr?: string | null }[],
  ctx: RenderCtx,
): (Node | string)[] {
  if (entries.length === 0) return ["{}"];
  const parts: (Node | string)[] = ["{\n  "];
  entries.forEach((e, i) => {
    if (i > 0) parts.push(",\n  ");
    parts.push(...objectFieldParts(e.name, e.shape, e.defaultExpr ?? null, ctx));
  });
  parts.push("\n}");
  return parts;
}

function fieldsParts(fields: WcField[], ctx: RenderCtx): (Node | string)[] {
  return objectSkeletonParts(
    fields.map((f) => ({ name: f.name, shape: f.shape, defaultExpr: f.default })),
    ctx,
  );
}

function variantParts(v: WcVariant, ctx: RenderCtx): (Node | string)[] {
  if (v.payload.length === 0) return [`{ "kind": "${v.name}" }`];
  const parts: (Node | string)[] = [`{\n  "kind": "${v.name}",\n  `];
  v.payload.forEach((f, i) => {
    if (i > 0) parts.push(",\n  ");
    parts.push(...objectFieldParts(f.name, f.shape, f.default, ctx));
  });
  parts.push("\n}");
  return parts;
}

function paramsParts(params: WcParam[], ctx: RenderCtx): (Node | string)[] {
  return objectSkeletonParts(
    params.map((p) => ({ name: p.name, shape: p.shape })),
    ctx,
  );
}

// -- header -------------------------------------------------------------

function renderHeader(ctx: RenderCtx): HTMLElement {
  const { model } = ctx.payload;
  const header = el("header");
  const h1 = el("h1");
  const kindLabel = el("span", { className: "wc-kind", text: `${model.service} · ${model.unit}` });
  const line = el("span", { text: formatHeaderLine(model.kind) });
  const lineNo = el("span", { className: "wc-muted", text: `line ${model.handlerLine}` });
  h1.append(kindLabel, line, lineNo);
  makeClickable(h1, ctx.payload.uri, model.range, ctx);
  header.append(h1);
  return header;
}

// -- contract card --------------------------------------------------------

function renderContractCard(ctx: RenderCtx): HTMLElement {
  const { model } = ctx.payload;
  const section = el("section", { className: "wc-card" });
  section.append(el("h2", { text: "Contract" }));

  if (model.contract) {
    const form: WcContractForm = model.contract;
    const pre = el("pre");
    pre.append(code(form.normalForm));
    section.append(pre);

    const hashLine = el("p");
    hashLine.append("hash ", el("span", { className: "wc-hash", text: form.hash }));
    section.append(hashLine);

    section.append(el("p", { text: contractMismatchSentence() }));
  } else {
    const reason = model.noCrossContext ?? "NotACallHandler";
    section.append(el("p", { className: "wc-muted", text: noCrossContextSentence(reason) }));
  }
  return section;
}

// -- request card ---------------------------------------------------------

function renderRequestCard(ctx: RenderCtx): HTMLElement {
  const { model } = ctx.payload;
  const section = el("section", { className: "wc-card" });
  section.append(el("h2", { text: "Request" }));

  const body = el("p", { className: "wc-shape" });
  switch (model.envelope.kind) {
    case "Empty":
      body.textContent = emptyEnvelopeSentence();
      break;
    case "Bare": {
      const sentence = el("span", { text: `${bareEnvelopeSentence(model.envelope.param)} ` });
      body.append(sentence, ...refParts(model.envelope.shape, ctx));
      break;
    }
    case "Keyed":
      appendAll(body, paramsParts(model.envelope.params, ctx));
      break;
    case "Raw":
      body.textContent = rawEnvelopeSentence(model.envelope.param, model.envelope.contentType);
      break;
  }
  section.append(body);
  return section;
}

// -- responses table --------------------------------------------------------

function renderResponses(ctx: RenderCtx): HTMLElement | null {
  const { model, uri } = ctx.payload;
  if (model.responses.length === 0) return null;

  const section = el("section", { className: "wc-card" });
  section.append(el("h2", { text: "Responses" }));

  const table = el("table", { className: "wc-responses" });
  const thead = el("thead");
  // `WcHttpResponse` carries `{status, variant, origin}` — no body-shape
  // field (the plan sketches this as a "body" column, but the server sends
  // *why* the response exists, not what it contains; the response's actual
  // payload shape is whatever the handler's own success/error types are,
  // not something this walk re-derives). The column is honestly labelled
  // `origin` rather than claiming a "body" the wire shape doesn't carry.
  const headRow = el("tr");
  headRow.append(el("th", { text: "status" }), el("th", { text: "variant" }), el("th", { text: "origin" }));
  thead.append(headRow);
  table.append(thead);

  const tbody = el("tbody");
  for (const resp of model.responses) {
    tbody.append(renderResponseRow(resp, uri, ctx));
  }
  table.append(tbody);
  section.append(table);
  return section;
}

function renderResponseRow(resp: WcHttpResponse, uri: string, ctx: RenderCtx): HTMLElement {
  const row = el("tr");
  if (responseRowMuted(resp.origin)) row.className = "wc-row-muted";

  row.append(el("td", { text: String(resp.status) }));
  row.append(el("td", { text: resp.variant }));

  const originCell = el("td");
  const why = responseRowWhy(resp.origin);
  if (why) {
    originCell.textContent = why;
  } else if (resp.origin.kind === "Constructed") {
    originCell.textContent = "constructed here";
    row.className = "wc-row-clickable";
    makeClickable(row, uri, resp.origin.range, ctx);
  } else {
    originCell.textContent = "declared return type";
  }
  row.append(originCell);
  return row;
}

// -- boundary types ---------------------------------------------------------

function renderPredicateBadges(predicates: string[]): HTMLElement[] {
  return predicates.map((p) => el("span", { className: "wc-badge", text: p }));
}

function renderScalarBody(name: string, body: Extract<WcBody, { kind: "Scalar" }>): HTMLElement {
  const wrap = el("div");

  const declaredBits = [body.base, body.opaque ? "opaque" : null].filter((s): s is string => s !== null);
  wrap.append(el("p", { className: "wc-muted", text: `declared as ${declaredBits.join(", ")}` }));
  wrap.append(el("p", { className: "wc-shape", text: `wire ${jsonKindWire(body.json)}` }));

  // `Integral`/`Finite` base guards are re-checked at the boundary the same
  // as a declared predicate (`emit_inline_refinement_checks`'s sibling
  // check) — surfaced here for the same reason predicates are: an author
  // should see every re-validation this scalar gets, not just the named ones.
  if (body.baseGuards.length > 0) {
    wrap.append(
      el("p", {
        className: "wc-muted",
        text: `base guard: ${body.baseGuards.join(", ")} (checked at the boundary)`,
      }),
    );
  }

  // Predicates are only rendered as unqualified "re-validated" badges when
  // this context actually re-checks them. `wire.rs`'s `WireScalar::predicates`
  // still carries them for a `StructuralOnly` scalar (only `contract.rs`'s
  // *hash* elides an opaque predicate — see Part 1's decision table); an
  // unqualified badge here, beside a sentence saying "not re-validated",
  // would be exactly the "refinement rendering that misstates the boundary
  // check" risk #855 calls out. So a `StructuralOnly` scalar's predicates
  // get a visibly distinct, explicitly-labelled badge instead.
  if (body.predicates.length > 0) {
    const badges = el("div");
    if (body.revalidation === "StructuralOnly") {
      badges.append(
        el("span", { className: "wc-muted", text: "declared upstream (not re-checked here): " }),
      );
      badges.append(
        ...body.predicates.map((p) => el("span", { className: "wc-badge wc-badge-opaque", text: p })),
      );
    } else {
      badges.append(...renderPredicateBadges(body.predicates));
    }
    wrap.append(badges);
  }

  wrap.append(
    el("p", {
      text: revalidationSentence(name, body.json, body.revalidation, body.predicates),
    }),
  );
  return wrap;
}

function renderRecordBody(body: Extract<WcBody, { kind: "Record" }>, ctx: RenderCtx): HTMLElement {
  const pre = el("pre");
  const codeEl = code("");
  appendAll(codeEl, fieldsParts(body.fields, ctx));
  pre.append(codeEl);
  return pre;
}

function renderSumBody(body: Extract<WcBody, { kind: "Sum" }>, ctx: RenderCtx): HTMLElement {
  const wrap = el("div");
  wrap.append(
    el("p", {
      className: "wc-muted",
      text: `wire discriminant "${body.wireDiscriminant}" (memory: "${body.memoryDiscriminant}")`,
    }),
  );
  const pre = el("pre");
  const codeEl = code("");
  body.variants.forEach((v, i) => {
    if (i > 0) appendAll(codeEl, [" | "]);
    appendAll(codeEl, variantParts(v, ctx));
  });
  pre.append(codeEl);
  wrap.append(pre);
  return wrap;
}

function renderTypeBlock(t: WcType, ctx: RenderCtx): HTMLDetailsElement {
  const details = el("details", { className: "wc-type" });
  details.id = `wc-type-${cssEscape(t.name)}`;
  details.open = true;

  const summary = el("summary");
  summary.append(t.name, " ", el("span", { className: "wc-kind", text: t.codecSuffix }));
  if (t.provenance.kind === "Consumed") {
    summary.append(" ", el("span", { className: "wc-muted", text: `(consumed from ${t.provenance.ownerUnit})` }));
  }
  if (ctx.recursiveNames.has(t.name)) {
    summary.append(" ", el("span", { className: "wc-muted", text: "(recursive)" }));
  }
  details.append(summary);

  if (t.loc) {
    makeClickable(summary, t.loc.uri, t.loc.range, ctx);
  }

  switch (t.body.kind) {
    case "Scalar":
      details.append(renderScalarBody(t.name, t.body));
      break;
    case "Record":
      details.append(renderRecordBody(t.body, ctx));
      break;
    case "Sum":
      details.append(renderSumBody(t.body, ctx));
      break;
  }
  return details;
}

function renderTypes(ctx: RenderCtx): HTMLElement | null {
  const { model } = ctx.payload;
  if (model.types.length === 0) return null;

  const section = el("section", { className: "wc-card" });
  section.append(el("h2", { text: "Boundary types" }));
  for (const t of model.types) {
    const block = renderTypeBlock(t, ctx);
    ctx.typeBlocks.set(t.name, block);
    section.append(block);
  }
  return section;
}

// -- entry ---------------------------------------------------------------

/** Render `payload` into `container`, wiring every click-to-code target
 *  through `onReveal`. Clears `container` first. */
export function renderWireContract(container: HTMLElement, payload: WcPayload, onReveal: RevealFn): void {
  container.textContent = "";

  const instByKey = new Map(payload.model.instantiations.map((i) => [i.key, i]));
  const recursiveNames = new Set(payload.model.recursive);
  const ctx: RenderCtx = { payload, onReveal, instByKey, recursiveNames, typeBlocks: new Map() };

  container.append(renderHeader(ctx));
  container.append(renderContractCard(ctx));
  container.append(renderRequestCard(ctx));
  const responses = renderResponses(ctx);
  if (responses) container.append(responses);
  const types = renderTypes(ctx);
  if (types) container.append(types);
}

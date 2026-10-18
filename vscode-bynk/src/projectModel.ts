// #2097: the project sidebar's model — the `vscode`-free half of
// `projectTree.ts`, so the unit tests build the tree from a pinned
// architecture-model fixture without a live server.
//
// DECISION A: structure comes from `bynk/architectureModel` (contexts,
// adapters and `consumes` edges) plus a cheap header scan of the project's
// `.bynk` files, which also finds the commons and test units the architecture
// model does not cover. Declarations are not here: the tree fetches them
// lazily from document symbols when a unit is expanded.

import type { ArchLoc, ArchModel, ArchNode } from "./webview/architectureTypes";

export type UnitKind = "context" | "commons" | "adapter" | "test";

/** A unit header found at the top of a `.bynk` file. */
export interface UnitHeader {
  kind: UnitKind;
  name: string;
  uri: string;
  /** Where the unit's name is, for click-to-source. */
  line: number;
  character: number;
}

const HEADER = /^(\s*)(context|commons|adapter|suite)\s+([A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*)/;
const FENCE = /^\s*-{3,}\s*$/;

/**
 * Find a file's unit header: the first `context`/`commons`/`adapter`/`suite`
 * line outside a `--- … ---` doc block. A `suite` (a test file) is reported
 * as kind `test`, named after the unit it tests. `undefined` when the file
 * has no recognisable header (a parse error, an empty file).
 */
export function parseUnitHeader(text: string): Omit<UnitHeader, "uri"> | undefined {
  const lines = text.split(/\r?\n/);
  let inDoc = false;
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    if (FENCE.test(line)) {
      inDoc = !inDoc;
      continue;
    }
    if (inDoc) continue;
    const m = HEADER.exec(line);
    if (!m) continue;
    const keyword = m[2];
    const kind: UnitKind = keyword === "suite" ? "test" : (keyword as UnitKind);
    // The name ends the match, so it starts where the match ends minus its length.
    return { kind, name: m[3], line: i, character: m[0].length - m[3].length };
  }
  return undefined;
}

/** One end of a `consumes` edge, seen from a unit. */
export interface EdgeRef {
  /** The unit at the other end. */
  unit: string;
  /** The braced capability selection; empty for a whole-unit `consumes`. */
  capabilities: string[];
  /** The `consumes` clause (in the consuming unit's file). */
  loc: ArchLoc;
}

export interface ProjectUnit {
  kind: UnitKind;
  name: string;
  /** The unit's files (a multi-file unit has several), sorted. */
  files: string[];
  /** The header to open on click: the architecture node's location when the
   *  model has one, else the first file's header. */
  loc: ArchLoc | undefined;
  /** The architecture node, for contexts and adapters the model covers. */
  arch: ArchNode | undefined;
  consumes: EdgeRef[];
  consumedBy: EdgeRef[];
}

export interface UnitGroup {
  kind: UnitKind;
  label: string;
  units: ProjectUnit[];
}

const GROUPS: { kind: UnitKind; label: string }[] = [
  { kind: "context", label: "Contexts" },
  { kind: "commons", label: "Commons" },
  { kind: "adapter", label: "Adapters" },
  { kind: "test", label: "Tests" },
];

function headerLoc(h: UnitHeader): ArchLoc {
  const start = { line: h.line, character: h.character };
  return { uri: h.uri, range: { start, end: { line: h.line, character: h.character + h.name.length } } };
}

/**
 * Join the header scan and the architecture model into the sidebar's unit
 * groups, each unit carrying its `consumes` edges in both directions
 * (DECISION C). Units the model knows but no scanned file declares (a
 * platform adapter such as `bynk.cloudflare`) are kept, at the model's
 * location. Empty groups are dropped; units sort by name.
 */
export function buildProjectModel(headers: readonly UnitHeader[], arch: ArchModel | null): UnitGroup[] {
  const units = new Map<string, ProjectUnit>();
  const key = (kind: UnitKind, name: string) => `${kind}:${name}`;

  for (const h of headers) {
    const k = key(h.kind, h.name);
    const u = units.get(k);
    if (u) {
      u.files.push(h.uri);
      continue;
    }
    units.set(k, {
      kind: h.kind,
      name: h.name,
      files: [h.uri],
      loc: headerLoc(h),
      arch: undefined,
      consumes: [],
      consumedBy: [],
    });
  }

  const byName = new Map<string, ProjectUnit>();
  for (const node of arch?.nodes ?? []) {
    const kind: UnitKind = node.kind === "Adapter" ? "adapter" : "context";
    const k = key(kind, node.name);
    let u = units.get(k);
    if (!u) {
      u = { kind, name: node.name, files: [node.loc.uri], loc: undefined, arch: undefined, consumes: [], consumedBy: [] };
      units.set(k, u);
    }
    u.arch = node;
    u.loc = node.loc;
    byName.set(node.name, u);
  }

  for (const e of arch?.edges ?? []) {
    byName.get(e.from)?.consumes.push({ unit: e.to, capabilities: e.capabilities, loc: e.loc });
    byName.get(e.to)?.consumedBy.push({ unit: e.from, capabilities: e.capabilities, loc: e.loc });
  }

  const groups: UnitGroup[] = [];
  for (const g of GROUPS) {
    const list = [...units.values()]
      .filter((u) => u.kind === g.kind)
      .sort((a, b) => a.name.localeCompare(b.name));
    for (const u of list) {
      u.files.sort();
      u.consumes.sort((a, b) => a.unit.localeCompare(b.unit));
      u.consumedBy.sort((a, b) => a.unit.localeCompare(b.unit));
    }
    if (list.length > 0) groups.push({ ...g, units: list });
  }
  return groups;
}

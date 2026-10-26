// The dependency-free core of dependencies in the editor (#2472): the JSON the
// driver prints for `bynk deps`, `bynk search`, `bynk add` and `bynk publish
// --dry-run`, the argument lists the editor runs, a scan of `bynk.toml` for the
// upgrade code lenses, and the publish plan's text. No `vscode` import, so the
// unit tests compile against it directly (see tsconfig.test.json);
// `dependencies.ts` wires it to the editor.
//
// DECISION A: every registry question goes through the driver. This module
// builds `bynk` argument lists and reads `bynk`'s output; nothing here (or
// anywhere in the extension) speaks HTTP to a registry, so routing, trust,
// tokens and caching stay in one place.

/** One `[dependencies]` / `[dev-dependencies]` entry, as `bynk deps` reports it. */
export interface DepRow {
  alias: string;
  /** `org/package`. */
  package: string;
  dev: boolean;
  kind: "registry" | "git" | "path";
  /** The version range, git revision or path the manifest asks for. */
  requested: string;
  /** The registry name, git URL or path. */
  source: string;
  /** The version `bynk.lock` pins, when it records one. */
  locked: string | null;
}

export interface SearchVersion {
  version: string;
  checksum: string;
  yanked: boolean;
  withdrawn: string | null;
}

/** `bynk search --format json`: an exact `org/package` lookup. */
export interface SearchResult {
  package: string;
  registry: string;
  /** The highest version that is neither yanked nor withdrawn. */
  latest: string | null;
  /** Oldest first, as the registry's index lists them. */
  versions: SearchVersion[];
}

export interface PublishPlan {
  package: string;
  version: string;
  checksum: string;
  registry: string;
  dryRun: boolean;
  files: { path: string; bytes: number }[];
}

function parseJson(stdout: string): Record<string, unknown> | undefined {
  try {
    const v: unknown = JSON.parse(stdout);
    return v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : undefined;
  } catch {
    return undefined;
  }
}

const str = (v: unknown): v is string => typeof v === "string";

/** `bynk deps --format json`, or `undefined` for anything else. */
export function parseDeps(stdout: string): DepRow[] | undefined {
  const doc = parseJson(stdout);
  if (!doc || !Array.isArray(doc.dependencies)) return undefined;
  const rows: DepRow[] = [];
  for (const d of doc.dependencies as Record<string, unknown>[]) {
    if (!str(d.alias) || !str(d.package) || !str(d.requested) || !str(d.source)) return undefined;
    if (d.kind !== "registry" && d.kind !== "git" && d.kind !== "path") return undefined;
    rows.push({
      alias: d.alias,
      package: d.package,
      dev: d.dev === true,
      kind: d.kind,
      requested: d.requested,
      source: d.source,
      locked: str(d.locked) ? d.locked : null,
    });
  }
  return rows;
}

/** `bynk search --format json`, or `undefined` for anything else. */
export function parseSearch(stdout: string): SearchResult | undefined {
  const doc = parseJson(stdout);
  if (!doc || !str(doc.package) || !str(doc.registry) || !Array.isArray(doc.versions)) return undefined;
  const versions: SearchVersion[] = [];
  for (const v of doc.versions as Record<string, unknown>[]) {
    if (!str(v.version)) return undefined;
    versions.push({
      version: v.version,
      checksum: str(v.checksum) ? v.checksum : "",
      yanked: v.yanked === true,
      withdrawn: str(v.withdrawn) ? v.withdrawn : null,
    });
  }
  return { package: doc.package, registry: doc.registry, latest: str(doc.latest) ? doc.latest : null, versions };
}

/** `bynk publish [--dry-run] --format json`, or `undefined` for anything else. */
export function parsePublishPlan(stdout: string): PublishPlan | undefined {
  const doc = parseJson(stdout);
  if (!doc || !str(doc.package) || !str(doc.version) || !str(doc.registry) || !Array.isArray(doc.files)) {
    return undefined;
  }
  const files: { path: string; bytes: number }[] = [];
  for (const f of doc.files as Record<string, unknown>[]) {
    if (!str(f.path)) return undefined;
    files.push({ path: f.path, bytes: typeof f.bytes === "number" ? f.bytes : 0 });
  }
  return {
    package: doc.package,
    version: doc.version,
    checksum: str(doc.checksum) ? doc.checksum : "",
    registry: doc.registry,
    dryRun: doc.dry_run === true,
    files,
  };
}

// --- the arguments the editor runs. One place, so each rule is tested once. ---

export const depsArgs = (): string[] => ["deps", "--format", "json"];
export const searchArgs = (pkg: string): string[] => ["search", pkg, "--format", "json"];
export const fetchArgs = (): string[] => ["fetch", "--format", "json"];
/** `range` is a version range (`^1.2.0`); omit it for the registry's newest. */
export const addArgs = (pkg: string, range?: string): string[] => [
  "add",
  range ? `${pkg}@${range}` : pkg,
  "--format",
  "json",
];
/** DECISION C: the plan is always `--dry-run` first; the real publish is a
 *  separate, explicitly confirmed run. */
export const publishPlanArgs = (): string[] => ["publish", "--dry-run", "--format", "json"];
export const publishArgs = (): string[] => ["publish", "--format", "json"];

/** Whether `s` looks like `org/package`: the driver validates properly, this
 *  only keeps an obviously wrong entry from starting a process. */
export function validPackageName(s: string): boolean {
  return /^[A-Za-z0-9][A-Za-z0-9_-]*\/[A-Za-z0-9][A-Za-z0-9_-]*$/.test(s);
}

// --- versions ---

interface Semver {
  major: number;
  minor: number;
  patch: number;
  pre: string[];
}

function parseSemver(s: string): Semver | undefined {
  const m = /^v?(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?(?:\+[0-9A-Za-z.-]+)?$/.exec(s.trim());
  if (!m) return undefined;
  return { major: +m[1], minor: +m[2], patch: +m[3], pre: m[4] ? m[4].split(".") : [] };
}

/** Semver precedence (a release outranks its pre-releases); `NaN` when either
 *  side is not a version. */
export function compareVersions(a: string, b: string): number {
  const x = parseSemver(a);
  const y = parseSemver(b);
  if (!x || !y) return NaN;
  for (const k of ["major", "minor", "patch"] as const) {
    if (x[k] !== y[k]) return x[k] < y[k] ? -1 : 1;
  }
  if (x.pre.length === 0 || y.pre.length === 0) {
    return x.pre.length === y.pre.length ? 0 : x.pre.length === 0 ? 1 : -1;
  }
  for (let i = 0; i < Math.max(x.pre.length, y.pre.length); i++) {
    const p = x.pre[i];
    const q = y.pre[i];
    if (p === undefined) return -1;
    if (q === undefined) return 1;
    if (p === q) continue;
    const pn = /^\d+$/.test(p);
    const qn = /^\d+$/.test(q);
    if (pn && qn) return +p < +q ? -1 : 1;
    if (pn !== qn) return pn ? -1 : 1;
    return p < q ? -1 : 1;
  }
  return 0;
}

/** The version a dependency is at: the lock's, else the first full version
 *  in its range (`^1.2.0` → `1.2.0`). `undefined` when neither names one. */
export function currentVersion(locked: string | null | undefined, requested: string | undefined): string | undefined {
  if (locked && parseSemver(locked)) return locked;
  const m = requested ? /\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?/.exec(requested) : null;
  return m ? m[0] : undefined;
}

/** The upgrade a lens offers: the registry's newest usable version when it is
 *  newer than where the dependency is, else `undefined`. */
export function upgradeTarget(current: string | undefined, found: SearchResult | undefined): string | undefined {
  if (!current || !found?.latest) return undefined;
  return compareVersions(found.latest, current) > 0 ? found.latest : undefined;
}

/** The range an upgrade writes: the newest version, caret-compatible, as
 *  `bynk add` writes it. */
export const rangeFor = (version: string): string => `^${version}`;

/** The versions a picker offers, newest first, never a yanked or withdrawn one. */
export function pickableVersions(found: SearchResult): SearchVersion[] {
  return found.versions
    .filter((v) => !v.yanked && v.withdrawn === null)
    .sort((a, b) => {
      const c = compareVersions(b.version, a.version);
      return Number.isNaN(c) ? b.version.localeCompare(a.version) : c;
    });
}

// --- bynk.toml ---

/** A registry dependency in `bynk.toml` whose version a lens can rewrite. */
export interface ManifestDependency {
  alias: string;
  /** `org/package`; absent in the manifest's `"org/package"` shorthand-free form. */
  package: string | undefined;
  /** The version range text, without quotes. */
  range: string;
  /** Zero-based line of the entry (the lens anchor). */
  line: number;
  /** Zero-based columns of the range text between the quotes, on `rangeLine`. */
  rangeLine: number;
  rangeStart: number;
  rangeEnd: number;
  dev: boolean;
}

/** The registry dependencies written in `text` (a `bynk.toml`): inline tables
 *  (`money = { package = "acme/money", version = "^1.4" }`, what `bynk add`
 *  writes) and `[dependencies.money]` sub-tables. Git and path entries have no
 *  `version` and are skipped. A forgiving line scan, not a TOML parser: an
 *  entry it cannot read is skipped, never mis-read. */
export function scanManifestDependencies(text: string): ManifestDependency[] {
  const out: ManifestDependency[] = [];
  const lines = text.split(/\r?\n/);
  let table: "deps" | "dev" | "sub" | "subdev" | "other" = "other";
  let sub: { alias: string; line: number; dev: boolean; package?: string; range?: ManifestDependency } | undefined;
  const flush = () => {
    if (sub?.range) out.push({ ...sub.range, alias: sub.alias, package: sub.package, line: sub.line, dev: sub.dev });
    sub = undefined;
  };
  const stringField = (src: string, key: string): { value: string; start: number; end: number } | undefined => {
    const m = new RegExp(`(?:^|[\\s{,])${key}\\s*=\\s*"([^"\\\\]*)"`).exec(src);
    if (!m) return undefined;
    const start = m.index + m[0].length - 1 - m[1].length;
    return { value: m[1], start, end: start + m[1].length };
  };
  for (let i = 0; i < lines.length; i++) {
    const raw = lines[i];
    const header = /^\s*\[\s*([^\]]+?)\s*\]\s*(?:#.*)?$/.exec(raw);
    if (header && !/^\s*\[\[/.test(raw)) {
      flush();
      const name = header[1];
      if (name === "dependencies") table = "deps";
      else if (name === "dev-dependencies") table = "dev";
      else {
        const s = /^(dependencies|dev-dependencies)\.(?:"([^"]+)"|([A-Za-z0-9_-]+))$/.exec(name);
        if (s) {
          table = s[1] === "dependencies" ? "sub" : "subdev";
          sub = { alias: s[2] ?? s[3], line: i, dev: s[1] !== "dependencies" };
        } else table = "other";
      }
      continue;
    }
    if (table === "deps" || table === "dev") {
      const entry = /^\s*([A-Za-z0-9_-]+|"[^"]+")\s*=\s*\{(.*)\}\s*(?:#.*)?$/.exec(raw);
      if (!entry) continue;
      const body = entry[2];
      const version = stringField(body, "version");
      if (!version) continue;
      const offset = raw.indexOf("{", raw.indexOf(entry[1]) + entry[1].length) + 1;
      out.push({
        alias: entry[1].replace(/^"|"$/g, ""),
        package: stringField(body, "package")?.value,
        range: version.value,
        line: i,
        rangeLine: i,
        rangeStart: offset + version.start,
        rangeEnd: offset + version.end,
        dev: table === "dev",
      });
    } else if (sub) {
      const version = stringField(raw, "version");
      if (version) {
        sub.range = {
          alias: sub.alias,
          package: undefined,
          range: version.value,
          line: i,
          rangeLine: i,
          rangeStart: version.start,
          rangeEnd: version.end,
          dev: sub.dev,
        };
      }
      const pkg = stringField(raw, "package");
      if (pkg) sub.package = pkg.value;
    }
  }
  flush();
  return out;
}

// --- the publish plan (DECISION C) ---

/** At most this many files are listed in the confirmation; the output channel
 *  has all of them. */
export const PLAN_FILES_SHOWN = 12;

/** The confirmation's text: identity, version, registry, checksum and file list. */
export function publishPlanText(plan: PublishPlan): string {
  const total = plan.files.reduce((n, f) => n + f.bytes, 0);
  const shown = plan.files.slice(0, PLAN_FILES_SHOWN).map((f) => `  ${f.path} (${formatBytes(f.bytes)})`);
  const more = plan.files.length - shown.length;
  return [
    `Package:   ${plan.package}`,
    `Version:   ${plan.version}`,
    `Registry:  ${plan.registry}`,
    `Checksum:  ${plan.checksum}`,
    `Files:     ${plan.files.length} (${formatBytes(total)})`,
    ...shown,
    ...(more > 0 ? [`  … and ${more} more`] : []),
  ].join("\n");
}

/** Every file, for the output channel. */
export function publishFilesText(plan: PublishPlan): string {
  return plan.files.map((f) => `${f.path}\t${f.bytes}`).join("\n");
}

export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KiB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MiB`;
}

/** The lens title: `0.3.1 → 0.4.0 available`. */
export const upgradeTitle = (current: string, latest: string): string => `${current} → ${latest} available`;

// The dependency-free core of `Bynk: Doctor` (#2095): the `bynk doctor --format
// json` report, the actions a remedy offers, and the checklist markup. No
// `vscode` import, so the unit tests compile against it directly (see
// tsconfig.test.json); `doctor.ts` wires it to the editor.
//
// The doctor is the single source of truth for "is this machine ready": the
// extension renders its verdict and never re-implements a probe.

import { escapeHtml } from "./cliCore";

export type Level = "ok" | "warn" | "fail" | "note";

export interface DoctorRow {
  label: string;
  level: Level;
  detail: string;
  remedy: string | null;
}

export interface DoctorCapability {
  /** `compile`, `test`, `deploy`, `editor`, `build`. */
  capability: string;
  optional: boolean;
  level: Level;
  rows: DoctorRow[];
}

export interface DoctorReport {
  driver: string;
  compiler: { resolved: boolean; path: string | null; version: string | null; skew: string | null };
  allOk: boolean;
  capabilities: DoctorCapability[];
}

const str = (v: unknown, d = ""): string => (typeof v === "string" ? v : d);
const arr = (v: unknown): unknown[] => (Array.isArray(v) ? v : []);
const rec = (v: unknown): Record<string, unknown> =>
  v && typeof v === "object" ? (v as Record<string, unknown>) : {};
const level = (v: unknown): Level =>
  v === "ok" || v === "warn" || v === "fail" || v === "note" ? v : "warn";

/** Parse the doctor's JSON. Lenient: unknown fields are ignored and an unknown
 *  level reads as `warn`. Throws only when the text has no `capabilities`. */
export function parseDoctorReport(text: string): DoctorReport {
  const o = rec(JSON.parse(text) as unknown);
  if (!Array.isArray(o.capabilities)) throw new Error("not a `bynk doctor` report (no `capabilities`)");
  const c = rec(o.compiler);
  return {
    driver: str(o.driver),
    compiler: {
      resolved: c.resolved === true,
      path: typeof c.path === "string" ? c.path : null,
      version: typeof c.version === "string" ? c.version : null,
      skew: typeof c.skew === "string" ? c.skew : null,
    },
    allOk: o.all_ok === true,
    capabilities: o.capabilities.map((cap) => {
      const x = rec(cap);
      return {
        capability: str(x.capability),
        optional: x.optional === true,
        level: level(x.level),
        rows: arr(x.rows).map((r) => {
          const y = rec(r);
          return {
            label: str(y.label),
            level: level(y.level),
            detail: str(y.detail),
            remedy: typeof y.remedy === "string" ? y.remedy : null,
          };
        }),
      };
    }),
  };
}

/** What a remedy can do from the editor. */
export interface RemedyActions {
  /** Shell commands the remedy names, each offered as "Run in terminal". */
  commands: string[];
  /** http(s) links it names, offered as "Open". */
  urls: string[];
  /** The extension setting that answers it, offered as "Open setting". */
  setting?: string;
}

/** Programs a remedy's command may start with. A clause that starts with
 *  anything else is prose ("install bynkc-lsp (or download …)"), not a command. */
const COMMAND_STARTS = /^(npm|npx|pnpm|yarn|cargo|rustup|brew|winget|choco|apt|apt-get|dnf)\s/;

/** The actions `row`'s remedy offers: commands it names (its leading clause,
 *  and any backticked one), links, and the setting for the row's tool. */
export function remedyActions(row: DoctorRow): RemedyActions {
  const remedy = row.remedy ?? "";
  const commands: string[] = [];
  const add = (cmd: string) => {
    const c = cmd.trim();
    if (COMMAND_STARTS.test(c) && !commands.includes(c)) commands.push(c);
  };
  // The leading clause, up to ", " or " (": `npm install -g wrangler@4, or …`.
  add(remedy.split(/,\s|\s\(/)[0]);
  for (const m of remedy.matchAll(/`([^`]+)`/g)) add(m[1]);
  const urls = [...new Set([...remedy.matchAll(/https?:\/\/[^\s),]+/g)].map((m) => m[0]))];
  let setting: string | undefined;
  if (row.label === "bynkc-lsp") setting = "bynk.executablePath";
  else if (row.label === "compiler" || row.label === "bynkc" || /BYNK_BYNKC/.test(remedy)) {
    setting = "bynk.compilerPath";
  }
  return setting ? { commands, urls, setting } : { commands, urls };
}

/** Every command any row of `report` offers: the host only runs one of these. */
export function offeredCommands(report: DoctorReport): Set<string> {
  const all = new Set<string>();
  for (const cap of report.capabilities) {
    for (const row of cap.rows) for (const c of remedyActions(row).commands) all.add(c);
  }
  return all;
}

const ICON: Record<Level, string> = { ok: "✓", warn: "!", fail: "✗", note: "·" };

const HEADING: Record<string, string> = {
  compile: "Compile",
  test: "Test",
  deploy: "Dev and deploy",
  editor: "Editor",
  build: "Build from source",
};

/** The checklist markup: one group per capability, a row per tool with its
 *  level, and remedy buttons on every row that is not ok. Buttons carry
 *  `data-action` for the host view's script. Every CLI string is escaped. */
export function renderDoctorHtml(report: DoctorReport, opts: { only?: string } = {}): string {
  const e = escapeHtml;
  const out: string[] = [];
  const verdict = report.allOk ? "Ready" : "Some tools need attention";
  out.push(
    `<header><h1 class="${report.allOk ? "ok" : "warn"}">${e(verdict)}</h1><p class="meta">bynk ${e(report.driver)}` +
      (report.compiler.version ? `, bynkc ${e(report.compiler.version)}` : "") +
      (report.compiler.skew && report.compiler.skew !== "match" ? ` (version skew: ${e(report.compiler.skew)})` : "") +
      (opts.only ? `, checking <code>${e(opts.only)}</code> only` : "") +
      `</p></header>`,
  );
  for (const cap of report.capabilities) {
    const heading = HEADING[cap.capability] ?? cap.capability;
    const rows = cap.rows.map((row) => {
      const parts = [
        `<span class="icon ${row.level}" aria-label="${row.level}">${ICON[row.level]}</span>`,
        `<span class="label">${e(row.label)}</span>`,
        `<span class="detail">${e(row.detail)}</span>`,
      ];
      if (row.level !== "ok" && row.remedy) {
        const a = remedyActions(row);
        const buttons = [
          ...a.commands.map(
            (c) => `<button data-action="run" data-command="${e(c)}">Run <code>${e(c)}</code></button>`,
          ),
          ...a.urls.map((u) => `<button data-action="open" data-url="${e(u)}">Open ${e(u)}</button>`),
          ...(a.setting
            ? [`<button data-action="setting" data-setting="${e(a.setting)}">Open setting <code>${e(a.setting)}</code></button>`]
            : []),
          `<button data-action="copy" data-text="${e(row.remedy)}">Copy</button>`,
        ];
        parts.push(`<div class="remedy"><p>${e(row.remedy)}</p><div class="buttons">${buttons.join("")}</div></div>`);
      }
      return `<li class="row ${row.level}">${parts.join("")}</li>`;
    });
    out.push(
      `<section class="cap"><h2><span class="icon ${cap.level}">${ICON[cap.level]}</span> ${e(heading)}${cap.optional ? ' <span class="optional">optional</span>' : ""}</h2><ul>${rows.join("")}</ul></section>`,
    );
  }
  out.push(`<footer><button class="primary" data-action="rerun">Check again</button></footer>`);
  return out.join("\n");
}

export const DOCTOR_CSS = `
  h1.ok { color: var(--vscode-testing-iconPassed); }
  h1.warn { color: var(--vscode-editorWarning-foreground); }
  .meta { opacity: 0.8; }
  section.cap ul { list-style: none; padding-left: 0; }
  li.row { margin: 6px 0; }
  .icon { display: inline-block; width: 1.4em; font-weight: bold; }
  .icon.ok { color: var(--vscode-testing-iconPassed); }
  .icon.warn { color: var(--vscode-editorWarning-foreground); }
  .icon.fail { color: var(--vscode-editorError-foreground); }
  .label { display: inline-block; min-width: 8em; font-family: var(--vscode-editor-font-family); }
  .detail { opacity: 0.85; }
  .optional { font-size: 0.75em; opacity: 0.7; font-weight: normal; }
  .remedy { margin: 4px 0 0 1.4em; padding-left: 8px; border-left: 3px solid var(--vscode-editorWarning-foreground); }
  .remedy p { margin: 2px 0 6px; }
  .buttons { display: flex; gap: 6px; flex-wrap: wrap; }
  button { color: var(--vscode-button-secondaryForeground); background: var(--vscode-button-secondaryBackground); border: none; padding: 3px 10px; cursor: pointer; }
  button.primary { color: var(--vscode-button-foreground); background: var(--vscode-button-background); }
  footer { margin-top: 16px; }
`;

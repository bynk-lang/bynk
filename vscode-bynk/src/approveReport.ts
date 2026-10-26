// The dependency-free core of `Bynk: Review Pending npm Requirements` (#2473):
// the `bynk approve --list --format json` listing, how a driver's output says a
// requirement is pending, what changed since the last approval, and the review
// markup. No `vscode` import, so the unit tests compile against it directly
// (see tsconfig.test.json); `approveRequirements.ts` wires it to the editor.
//
// Approval is a supply-chain decision: a dependency's adapter brings an npm
// package into the build, and the next `npm install` runs whatever it runs. So
// the review is per package (DECISION A); approve-all exists but is secondary,
// and the host confirms it. The driver stays the source of truth: the extension
// renders its listing and never reads `bynk.lock` to decide what is pending.

import { escapeHtml, parseShortDiagnostic } from "./cliCore";

export type RequirementState = "pending" | "approved" | "own";

/** One `[[npm]]` row of the lock: an npm package at one range. */
export interface Requirement {
  package: string;
  range: string;
  state: RequirementState;
  /** For a pending row, what changed since the last approval: `new` (no other
   *  range of the package is approved) or `range_changed` (another is). */
  change: "new" | "range_changed" | null;
  /** The other ranges of this package that are approved. */
  approvedRanges: string[];
  /** Who requires it, as `org/package:adapter`. */
  requiredBy: string[];
  /** How each requirer is reached from the project: `this project → … → by`. */
  chains: string[][];
  url: string;
  /** Set by `annotateChanges`: a range of the package this editor last saw
   *  approved, now superseded — the driver's lock drops an approved row once
   *  nothing requires its range, so the move is only visible from here. */
  previousRanges: string[];
}

export interface ApprovalListing {
  pending: number;
  requirements: Requirement[];
}

const str = (v: unknown, d = ""): string => (typeof v === "string" ? v : d);
const strs = (v: unknown): string[] => (Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : []);
const rec = (v: unknown): Record<string, unknown> =>
  v && typeof v === "object" ? (v as Record<string, unknown>) : {};

/** Parse the driver's listing. Lenient: unknown fields are ignored and an
 *  unknown state reads as `pending`, the cautious reading (it asks rather than
 *  assumes). Throws only when the text has no `requirements`. */
export function parseApprovalListing(text: string): ApprovalListing {
  const o = rec(JSON.parse(text) as unknown);
  if (!Array.isArray(o.requirements)) throw new Error("not a `bynk approve --list` document (no `requirements`)");
  const requirements = o.requirements.map((r): Requirement => {
    const x = rec(r);
    const state: RequirementState = x.state === "approved" || x.state === "own" ? x.state : "pending";
    const change = x.change === "new" || x.change === "range_changed" ? x.change : null;
    const chains = Array.isArray(x.chains) ? x.chains.map(strs) : [];
    return {
      package: str(x.package),
      range: str(x.range),
      state,
      change: state === "pending" ? (change ?? "new") : null,
      approvedRanges: strs(x.approved_ranges),
      requiredBy: strs(x.required_by),
      chains,
      url: str(x.url),
      previousRanges: [],
    };
  });
  return { pending: requirements.filter((r) => r.state === "pending").length, requirements };
}

/** The ranges of each package approved when last seen, kept per project so a
 *  later review can say what moved. */
export type ApprovedMemory = Record<string, string[]>;

/** Mark each pending row with the ranges of its package that were approved
 *  when last seen and no longer are (`previousRanges`), and turn its `change`
 *  into `range_changed`. */
export function annotateChanges(listing: ApprovalListing, memory: ApprovedMemory): ApprovalListing {
  const requirements = listing.requirements.map((r) => {
    if (r.state !== "pending") return r;
    const previousRanges = (memory[r.package] ?? []).filter((p) => p !== r.range && !r.approvedRanges.includes(p));
    if (previousRanges.length === 0) return r;
    return { ...r, change: "range_changed" as const, previousRanges };
  });
  return { ...listing, requirements };
}

/** The memory after seeing `listing`: a package's approved ranges when it has
 *  any; otherwise what was remembered, while the package is still pending (so
 *  the next review can still say what it replaced). */
export function rememberApproved(listing: ApprovalListing, memory: ApprovedMemory): ApprovedMemory {
  const next: ApprovedMemory = {};
  const pending = new Set(listing.requirements.filter((r) => r.state === "pending").map((r) => r.package));
  for (const r of listing.requirements) {
    if (r.state === "approved") next[r.package] = [...(next[r.package] ?? []), r.range];
  }
  for (const [pkg, ranges] of Object.entries(memory)) {
    if (!(pkg in next) && pending.has(pkg)) next[pkg] = ranges;
  }
  return next;
}

/** The code the compile gives a reached adapter's unapproved requirement. */
export const UNAPPROVED_CODE = "bynk.requires.unapproved";

/** Whether `output` — a `bynk check`/`dev`/`deploy` run's text — reports an
 *  unapproved requirement (DECISION B): a `--format short` diagnostic with its
 *  code, or the code named anywhere else (a deploy's failure text). */
export function reportsUnapproved(output: string): boolean {
  if (output.includes(UNAPPROVED_CODE)) return true;
  return output.split(/\r?\n/).some((line) => parseShortDiagnostic(line)?.code === UNAPPROVED_CODE);
}

/** The pending rows, grouped by package: approval is per package, so a package
 *  pending at two ranges is one decision. Sorted by package name. */
export interface PendingPackage {
  package: string;
  rows: Requirement[];
}

export function pendingPackages(listing: ApprovalListing): PendingPackage[] {
  const byName = new Map<string, Requirement[]>();
  for (const r of listing.requirements) {
    if (r.state === "pending") byName.set(r.package, [...(byName.get(r.package) ?? []), r]);
  }
  return [...byName.entries()]
    .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
    .map(([pkg, rows]) => ({ package: pkg, rows }));
}

/** A stable signature of what is pending, so a notification is shown once per
 *  distinct set rather than on every failing check. */
export function pendingSignature(listing: ApprovalListing): string {
  return listing.requirements
    .filter((r) => r.state === "pending")
    .map((r) => `${r.package}@${r.range}`)
    .sort()
    .join("\n");
}

/** The notification's text for `listing`. */
export function pendingMessage(listing: ApprovalListing): string {
  const names = pendingPackages(listing).map((p) => p.package);
  const list = names.length <= 3 ? names.map((n) => `\`${n}\``).join(", ") : `${names.length} packages`;
  return `Bynk: ${names.length === 1 ? "an npm package awaits" : "npm packages await"} your approval: ${list}. A build reaching ${names.length === 1 ? "it" : "them"} fails until approved.`;
}

/** The `bynk approve` arguments the host runs: one named package. Never
 *  `--all`, which approves whatever is pending when it runs rather than what
 *  the user was shown. */
export function approveArgs(pkg: string): string[] {
  return ["approve", pkg];
}

/** The review markup. Buttons carry `data-action` for the host view's script;
 *  every driver string is escaped. Pending packages come first, each with its
 *  own Approve; approve-all is a small secondary button the host confirms; what
 *  is already approved is folded away. */
export function renderApproveHtml(listing: ApprovalListing): string {
  const e = escapeHtml;
  const out: string[] = [];
  const packages = pendingPackages(listing);
  const approved = listing.requirements.filter((r) => r.state === "approved");
  const own = listing.requirements.filter((r) => r.state === "own");

  out.push(
    `<header><h1>${packages.length === 0 ? "Nothing awaits approval" : `Review ${packages.length} npm package${packages.length === 1 ? "" : "s"}`}</h1>` +
      `<p class="meta">A dependency's adapter brings these npm packages into your build, and the next <code>npm install</code> runs whatever they run. ` +
      `Approving one records it in <code>bynk.lock</code>; a changed range asks again.</p></header>`,
  );

  for (const p of packages) {
    const ranges = p.rows.map((r) => `<code>${e(r.range)}</code>`).join(", ");
    const parts: string[] = [`<h2><code>${e(p.package)}</code> ${ranges}</h2>`];
    for (const r of p.rows) {
      const badge =
        r.change === "range_changed"
          ? `<span class="badge changed">range changed${r.previousRanges.length > 0 ? `, was ${r.previousRanges.map((x) => `<code>${e(x)}</code>`).join(", ")}` : ""}</span>`
          : `<span class="badge new">new</span>`;
      parts.push(`<p class="change"><code>${e(r.range)}</code> ${badge}</p>`);
      parts.push(
        `<ul class="chains">${r.chains
          .map((c) => `<li>required through ${c.map((s) => `<code>${e(s)}</code>`).join(" → ")}</li>`)
          .join("")}</ul>`,
      );
    }
    const url = p.rows[0]?.url ?? "";
    parts.push(
      `<div class="buttons"><button class="primary" data-action="approve" data-package="${e(p.package)}">Approve <code>${e(p.package)}</code></button>` +
        (url ? `<button data-action="open" data-url="${e(url)}">View on npm</button>` : "") +
        `</div>`,
    );
    out.push(`<section class="pkg">${parts.join("")}</section>`);
  }

  if (packages.length > 1) {
    out.push(
      `<section class="all"><p>Reviewed them all? <button class="danger" data-action="approveAll">Approve all ${packages.length}…</button></p>` +
        `<p class="meta">You are asked to confirm first.</p></section>`,
    );
  }

  if (approved.length > 0) {
    out.push(
      `<details class="approved"><summary>Already approved (${approved.length})</summary><ul>${approved
        .map((r) => `<li><code>${e(r.package)}</code> <code>${e(r.range)}</code> <span class="meta">required by ${r.requiredBy.map(e).join(", ")}</span></li>`)
        .join("")}</ul></details>`,
    );
  }
  if (own.length > 0) {
    out.push(
      `<p class="meta">${own.length} more come only from your own adapters or path dependencies, which need no approval.</p>`,
    );
  }
  out.push(`<footer><button data-action="refresh">Refresh</button></footer>`);
  return out.join("\n");
}

export const APPROVE_CSS = `
  .meta { opacity: 0.8; }
  section.pkg { margin: 16px 0; padding: 8px 12px; border-left: 3px solid var(--vscode-editorWarning-foreground); }
  section.pkg h2 { margin: 0 0 6px; font-size: 1.1em; }
  .chains { margin: 2px 0 8px; padding-left: 20px; }
  .badge { font-size: 0.8em; padding: 0 6px; border-radius: 3px; background: var(--vscode-badge-background); color: var(--vscode-badge-foreground); }
  .badge.changed { background: var(--vscode-editorWarning-foreground); color: var(--vscode-editor-background); }
  .buttons { display: flex; gap: 6px; flex-wrap: wrap; }
  button { color: var(--vscode-button-secondaryForeground); background: var(--vscode-button-secondaryBackground); border: none; padding: 3px 10px; cursor: pointer; }
  button.primary { color: var(--vscode-button-foreground); background: var(--vscode-button-background); }
  button.danger { font-size: 0.9em; }
  section.all { margin-top: 24px; }
  details.approved { margin-top: 16px; }
  footer { margin-top: 16px; }
`;

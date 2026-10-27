// #2467: the contract-diff view's renderer. The sentence and label helpers
// are DOM-free and unit-tested (`contract-diff-render.unit.test.ts`); the DOM
// construction below them is not, for the reason `wire-render.ts` gives.

import type { CdChange, CdLedgerState, CdLoc, CdLockState, CdModel, CdPayload, CdService, Range } from "./contractDiffTypes";

/** `orders.Cart` -> `orders.Cart`; kept as a function so the label has one
 *  home if it ever needs to change. */
export function changeLabel(c: CdChange): string {
  return `${c.kind} ${c.unit}.${c.name}`;
}

/** One line for the header: how many of each, in the view's own words. */
export function summarySentence(model: CdModel): string {
  const breaking = model.changes.filter((c) => c.class === "breaking").length;
  const compatible = model.changes.length - breaking;
  const moved = model.services.length;
  if (model.changes.length === 0 && moved === 0) return "No boundary changes since the lock.";
  const parts: string[] = [];
  if (breaking > 0) parts.push(`${breaking} breaking`);
  if (compatible > 0) parts.push(`${compatible} compatible`);
  if (moved > 0) parts.push(`${moved} service contract${moved === 1 ? "" : "s"} changed`);
  return parts.join(", ") + ".";
}

/** What to say when there is no readable `bynk.schema.lock` to diff against;
 *  `null` when there is one. */
export function lockSentence(state: CdLockState): string | null {
  switch (state) {
    case "absent":
      return (
        "There is no bynk.schema.lock, so events and agent state have nothing to be compared with. " +
        "It is created the first time the project is built for deploy."
      );
    case "unreadable":
      return "bynk.schema.lock could not be read (see the diagnostic on bynk.toml), so events and agent state were not compared.";
    case "read":
      return null;
  }
}

export const NO_CHANGES_SENTENCE = "Every event and agent class matches the lock.";

export function ledgerSentence(state: CdLedgerState, rows: number): string | null {
  switch (state) {
    case "absent":
      return "There is no bynk.deploy.lock, so nothing is recorded as live: service contracts have nothing to be compared with.";
    case "unreadable":
      return "bynk.deploy.lock could not be read, so service contracts were not compared.";
    case "read":
      return rows === 0 ? "No service contract differs from what bynk.deploy.lock records as live." : null;
  }
}

export function serviceSentence(s: CdService): string {
  switch (s.status) {
    case "changed":
      return `${s.service}: contract ${short(s.live)} → ${short(s.current)}`;
    case "added":
      return `${s.service}: not in the live record (now ${short(s.current)})`;
    case "removed":
      return `${s.service}: live as ${short(s.live)}, no longer provided`;
  }
}

function short(hash: string | null): string {
  return hash ? hash.slice(0, 8) : "—";
}

/** The note under the services list: a hash says the contract moved, not
 *  whether the move breaks a caller. */
export const SERVICE_CAVEAT =
  "Service rows compare contract hashes with the deploy ledger. A changed hash means a live caller " +
  "would be refused (bynk.deploy.contract_skew); it is not classified as compatible or breaking.";

// ---------------------------------------------------------------------
// DOM construction.
// ---------------------------------------------------------------------

type Reveal = (uri: string, range: Range) => void;

function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  className?: string,
  text?: string,
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

function clickable(node: HTMLElement, loc: CdLoc | null, reveal: Reveal): void {
  if (!loc) return;
  node.classList.add("cd-clickable");
  node.addEventListener("click", () => reveal(loc.uri, loc.range));
}

export function renderContractDiff(root: HTMLElement, payload: CdPayload, reveal: Reveal): void {
  const model = payload.model;
  root.replaceChildren();
  root.append(el("h1", undefined, "Contract Diff"), el("p", "cd-summary", summarySentence(model)));

  const changes = el("section", "cd-card");
  changes.append(el("h2", undefined, "Events and agents — against bynk.schema.lock"));
  const lockNote = lockSentence(model.lock);
  if (lockNote) {
    changes.append(el("p", "cd-muted", lockNote));
  } else if (model.changes.length === 0) {
    changes.append(el("p", "cd-muted", NO_CHANGES_SENTENCE));
  } else {
    const table = el("table", "cd-table");
    for (const c of model.changes) {
      const row = el("tr", `cd-row cd-${c.class}`);
      row.append(
        el("td", "cd-class", c.class),
        el("td", "cd-name", changeLabel(c)),
        el("td", "cd-detail", `${c.detail} (locked at v${c.lockedVersion})`),
      );
      clickable(row, c.loc, reveal);
      table.append(row);
    }
    changes.append(table);
  }
  root.append(changes);

  const services = el("section", "cd-card");
  services.append(el("h2", undefined, "Services — against bynk.deploy.lock"));
  const note = ledgerSentence(model.ledger, model.services.length);
  if (note) services.append(el("p", "cd-muted", note));
  if (model.services.length > 0) {
    const table = el("table", "cd-table");
    for (const s of model.services) {
      const row = el("tr", "cd-row cd-changed");
      row.append(
        el("td", "cd-class", s.status),
        el("td", "cd-name", `${s.context} (${s.environment})`),
        el("td", "cd-detail", serviceSentence(s)),
      );
      clickable(row, s.loc, reveal);
      table.append(row);
    }
    services.append(table, el("p", "cd-muted", SERVICE_CAVEAT));
  }
  root.append(services);
}

// #2478: the Explorer file decorations' model — the `vscode`-free half of
// `fileDecorations.ts`, so the unit tests drive the badge rules and the
// failing-test bookkeeping without an extension host.
//
// DECISION A: the badge is a two-letter abbreviation of the owning context's
// name (the full name is the tooltip), shown by default only in a project with
// more than one context, where telling them apart is the point.

import type { UnitKind } from "./projectModel";

/** How `bynk.fileDecorations.contextBadge` is set. */
export type BadgeMode = "auto" | "always" | "never";

/** A scanned `.bynk` file: the project it is in and the unit it declares. */
export interface ScannedFile {
  /** The project root, as a URI string. */
  root: string;
  kind: UnitKind;
  name: string;
}

/**
 * A context's two-letter badge. A multi-word name (`order.billing`,
 * `order_billing`, `OrderBilling`) takes the initials of its first two words;
 * a single word takes its first two letters. Always upper case.
 */
export function abbreviate(name: string): string {
  const words = name
    .split(/[._]+|(?<=[a-z0-9])(?=[A-Z])/)
    .filter((w) => w.length > 0);
  const letters = words.length >= 2 ? words[0][0] + words[1][0] : (words[0] ?? name).slice(0, 2);
  return letters.toUpperCase();
}

/** The context a file belongs to: a context's own file, or a suite (a test
 *  file) naming the context it tests. `undefined` for commons, adapters, and
 *  a suite whose target is not a context of its project. */
export function owningContext(files: ReadonlyMap<string, ScannedFile>, uri: string): string | undefined {
  const file = files.get(uri);
  if (!file) return undefined;
  if (file.kind === "context") return file.name;
  if (file.kind !== "test") return undefined;
  for (const other of files.values()) {
    if (other.root === file.root && other.kind === "context" && other.name === file.name) return file.name;
  }
  return undefined;
}

/** The number of distinct contexts declared in a project. */
export function contextCount(files: ReadonlyMap<string, ScannedFile>, root: string): number {
  const names = new Set<string>();
  for (const f of files.values()) if (f.root === root && f.kind === "context") names.add(f.name);
  return names.size;
}

/** The context badge for a file, or `undefined` when it has none to show. */
export function contextBadge(
  files: ReadonlyMap<string, ScannedFile>,
  uri: string,
  mode: BadgeMode,
): { badge: string; context: string } | undefined {
  if (mode === "never") return undefined;
  const context = owningContext(files, uri);
  const file = files.get(uri);
  if (!context || !file) return undefined;
  if (mode === "auto" && contextCount(files, file.root) < 2) return undefined;
  return { badge: abbreviate(context), context };
}

/**
 * Which cases failed in the last run, per file. A case enters on a failure and
 * leaves when a later run passes it (or the case no longer exists), so a
 * passing re-run of the failures clears the file while a narrowed re-run of one
 * passing case leaves a still-failing sibling alone.
 */
export class FailureTracker {
  private readonly failed = new Map<string, Set<string>>();
  private readonly listeners = new Set<(uris: string[]) => void>();

  /** Called with the files whose state changed. */
  onDidChange(listener: (uris: string[]) => void): { dispose(): void } {
    this.listeners.add(listener);
    return { dispose: () => this.listeners.delete(listener) };
  }

  /** Record one case's outcome; `file` is the case's file, when it has one. */
  record(file: string | undefined, caseId: string, outcome: "passed" | "failed"): void {
    if (!file) return;
    const ids = this.failed.get(file);
    if (outcome === "failed") {
      if (ids?.has(caseId)) return;
      if (ids) ids.add(caseId);
      else this.failed.set(file, new Set([caseId]));
      this.emit([file]);
    } else if (ids?.delete(caseId)) {
      if (ids.size === 0) this.failed.delete(file);
      this.emit([file]);
    }
  }

  /** Forget every failed case whose id is not in `live` (a removed test). */
  retain(live: ReadonlySet<string>): void {
    const changed: string[] = [];
    for (const [file, ids] of this.failed) {
      for (const id of [...ids]) if (!live.has(id)) ids.delete(id);
      if (ids.size === 0) {
        this.failed.delete(file);
        changed.push(file);
      }
    }
    if (changed.length > 0) this.emit(changed);
  }

  hasFailures(file: string): boolean {
    return this.failed.has(file);
  }

  private emit(uris: string[]): void {
    for (const l of this.listeners) l(uris);
  }
}

/** The failures `testing.ts` records into and the decoration provider reads. */
export const testFailures = new FailureTracker();

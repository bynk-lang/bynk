// The Bynk Logs view (#2474): the workers' output while `bynk dev` runs, kept
// in a bounded in-memory buffer and shown in an output channel, filterable by
// context and level. The driver rewrites stack frames to `.bynk` locations
// before they get here; the output channel links an absolute `path:line:col`.
// The buffer is never persisted: it lives as long as the window.

import * as vscode from "vscode";

import {
  describeFilter,
  LogBuffer,
  LogEntry,
  LogFilter,
  MAX_LOG_ENTRIES,
  NO_FILTER,
  passes,
  renderEntry,
} from "./devLogs";
import type { LogLevel } from "./devEvents";

/** The context a session marker is filed under; it passes every context filter. */
const SESSION = "bynk dev";

export class DevLogs implements vscode.Disposable {
  private readonly buffer = new LogBuffer();
  private filter: LogFilter = NO_FILTER;
  private readonly channel = vscode.window.createOutputChannel("Bynk Logs");
  /** Lines appended to the channel since it was last rendered whole. The
   *  channel is bounded like the buffer: past this it is rendered afresh. */
  private appended = 0;

  push(entry: LogEntry): void {
    this.buffer.push(entry);
    if (!this.shown(entry)) return;
    if (++this.appended > MAX_LOG_ENTRIES) {
      this.render();
      return;
    }
    this.channel.appendLine(renderEntry(entry, true));
  }

  /** Mark where a session began, so a restart's logs do not run into the last's. */
  sessionStarted(args: string): void {
    this.push({ context: SESSION, level: "info", text: `--- bynk dev ${args} ---` });
  }

  show(preserveFocus = false): void {
    this.channel.show(preserveFocus);
  }

  clear(): void {
    this.buffer.clear();
    this.render();
  }

  /** The lines the view shows now, for tests: the channel's text is write-only. */
  lines(): string[] {
    return this.buffer.view(this.visible()).map((e) => renderEntry(e, true));
  }

  /** A quick pick for the level, then the contexts. */
  async pickFilter(): Promise<void> {
    type Level = vscode.QuickPickItem & { level: LogLevel };
    const level = await vscode.window.showQuickPick<Level>(
      [
        { label: "All levels", level: "info", picked: this.filter.minLevel === "info" },
        { label: "Warnings and errors", level: "warn", picked: this.filter.minLevel === "warn" },
        { label: "Errors only", level: "error", picked: this.filter.minLevel === "error" },
      ],
      { placeHolder: `Bynk Logs: showing ${describeFilter(this.filter)}` },
    );
    if (!level) return;
    let contexts: ReadonlySet<string> | undefined;
    const names = this.buffer.contexts().filter((c) => c !== SESSION);
    if (names.length > 1) {
      const chosen = await vscode.window.showQuickPick(
        names.map((label) => ({ label, picked: !this.filter.contexts || this.filter.contexts.has(label) })),
        { canPickMany: true, placeHolder: "Which contexts? (none selected shows all)" },
      );
      if (!chosen) return;
      if (chosen.length > 0 && chosen.length < names.length) {
        contexts = new Set(chosen.map((c) => c.label));
      }
    }
    this.setFilter({ minLevel: level.level, contexts });
  }

  setFilter(filter: LogFilter): void {
    this.filter = filter;
    this.render();
    this.show(true);
  }

  dispose(): void {
    this.channel.dispose();
  }

  /** The filter with session markers always passing, so a restart still shows. */
  private visible(): LogFilter {
    const { contexts } = this.filter;
    return contexts ? { ...this.filter, contexts: new Set([...contexts, SESSION]) } : this.filter;
  }

  private shown(entry: LogEntry): boolean {
    return passes(this.visible(), entry);
  }

  private render(): void {
    this.channel.clear();
    const lines = this.lines();
    this.appended = lines.length;
    if (lines.length > 0) this.channel.append(lines.join("\n") + "\n");
  }
}

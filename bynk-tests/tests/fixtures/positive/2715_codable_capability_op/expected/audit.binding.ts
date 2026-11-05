import type { Audit } from "./audit.js";

// The provider receives plain JSON for every `Codable` parameter and no witness.
export class MemoAudit implements Audit {
  readonly seen: unknown[] = [];

  async append(id: string, event: unknown): Promise<void> {
    this.seen.push({ id, event });
  }

  async appendAll(events: unknown): Promise<void> {
    this.seen.push(events);
  }

  async appendKeyed(entries: unknown): Promise<void> {
    this.seen.push(entries);
  }

  async note(text: string): Promise<void> {
    this.seen.push(text);
  }
}

import type { Idempotency } from "./idempotency.js";

export class MemoIdempotency implements Idempotency {
  private seen = new Set<string>();

  async claim(key: string): Promise<boolean> {
    if (this.seen.has(key)) return false;
    this.seen.add(key);
    return true;
  }
}

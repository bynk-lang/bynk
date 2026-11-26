import type { Idempotency } from "./idempotency.js";

export class MemoIdempotency implements Idempotency {
  private store = new Map<string, unknown>();

  async keep<T>(key: string, value: T): Promise<void> {
    this.store.set(key, value);
  }
}

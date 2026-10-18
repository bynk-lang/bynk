import type { Idempotency } from "./idempotency.js";
import { Some, None, type Option } from "./runtime.js";

export class MemoIdempotency implements Idempotency {
  private store = new Map<string, unknown>();

  async dedup<T>(key: string): Promise<Option<T>> {
    return this.store.has(key) ? Some(this.store.get(key) as T) : None;
  }
}

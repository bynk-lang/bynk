import type { Audit } from "./audit.js";

export class MemoAudit implements Audit {
  async note<T>(key: string): Promise<void> {}
}

import type { A } from "./alpha.js";
export class AImpl implements A {
  async a(): Promise<number> { return 0; }
}

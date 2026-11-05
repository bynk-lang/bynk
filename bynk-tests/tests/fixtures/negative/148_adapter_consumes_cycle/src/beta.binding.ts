import type { B } from "./beta.js";
export class BImpl implements B {
  async b(): Promise<number> { return 0; }
}

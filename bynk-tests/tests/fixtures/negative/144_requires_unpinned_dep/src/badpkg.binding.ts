import type { C } from "./badpkg.js";
export class Impl implements C {
  async f(): Promise<number> { return 0; }
}

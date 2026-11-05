import type { Jwt } from "./tokens.js";
import type { Log } from "./logging.js";

export class StubJwt implements Jwt {
  private deps: { Log: Log };
  constructor(deps: { Log: Log }) {
    this.deps = deps;
  }
  async sign(sub: string): Promise<string> {
    await this.deps.Log.info(`signing for ${sub}`);
    return `token:${sub}`;
  }
}

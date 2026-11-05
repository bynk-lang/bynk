import type { Greeter } from "./greet.js";
import type { Secrets } from "./bynk.js";

export class SecretGreeter implements Greeter {
  private deps: { Secrets: Secrets };
  constructor(deps: { Secrets: Secrets }) {
    this.deps = deps;
  }
  async hello(name: string): Promise<string> {
    const g = await this.deps.Secrets.get("GREETING");
    return `${g.tag === "Some" ? g.value : "hello"}, ${name}`;
  }
}

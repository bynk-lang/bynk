import type { Greeter, Secrets } from "./vault.js";
import type { Option } from "./runtime.js";

export class VaultSecrets implements Secrets {
  async get(_name: string): Promise<Option<string>> {
    return { tag: "None" };
  }
}

export class VaultGreeter implements Greeter {
  private deps: { Secrets: Secrets };
  constructor(deps: { Secrets: Secrets }) {
    this.deps = deps;
  }
  async hello(name: string): Promise<string> {
    const g = await this.deps.Secrets.get("GREETING");
    return `${g.tag === "Some" ? g.value : "hello"}, ${name}`;
  }
}

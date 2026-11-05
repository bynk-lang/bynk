import type { Greeter } from "./greet.js";
import type { Clock, Secrets } from "./bynk.js";

export class PlainGreeter implements Greeter {
  private readonly deps: { Clock: Clock; Secrets: Secrets };
  constructor(deps: { Clock: Clock; Secrets: Secrets }) {
    this.deps = deps;
  }
  async hello(name: string): Promise<string> {
    const key = await this.deps.Secrets.get("GREETING");
    const now = await this.deps.Clock.now();
    return `${key.tag === "Some" ? key.value : "hello"}, ${name} at ${now}`;
  }
}

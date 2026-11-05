import type { Relay } from "./relay.js";
import type { Greeter } from "./greet.js";

export class GreetRelay implements Relay {
  private deps: { Greeter: Greeter };
  constructor(deps: { Greeter: Greeter }) {
    this.deps = deps;
  }
  async hello(name: string): Promise<string> {
    return this.deps.Greeter.hello(name);
  }
}

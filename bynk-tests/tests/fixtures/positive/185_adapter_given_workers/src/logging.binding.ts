import type { Log } from "./logging.js";

export class ConsoleLog implements Log {
  async info(msg: string): Promise<void> {
    console.log(msg);
  }
}

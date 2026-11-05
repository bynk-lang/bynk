import type { Adder } from "./math.js";
import { loadWasm } from "./runtime.js";
import adder from "./adder.wasm.js";

const instance = await loadWasm(adder);
const add = instance.exports.add as (a: number, b: number) => number;

export class WasmAdder implements Adder {
  async add(a: number, b: number): Promise<number> {
    return add(a, b);
  }
}

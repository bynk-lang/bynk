// Native-vs-wasm parity (#2321, decision F). Replays the admission corpus that
// `cargo test -p bynk-registry-core` wrote to BYNK_PARITY_DIR through the
// standalone wasm module on Node, and requires every result to equal the
// native one byte for byte.
//
//   BYNK_PARITY_DIR=dir cargo test -p bynk-registry-core --test corpus
//   node scripts/registry-core-parity.mjs <bynk_registry_core.wasm> <dir>
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";

const [wasmPath, dir] = process.argv.slice(2);
if (!wasmPath || !dir) {
  console.error("usage: registry-core-parity.mjs <module.wasm> <corpus-dir>");
  process.exit(2);
}
const { instance } = await WebAssembly.instantiate(readFileSync(wasmPath), {});
const { memory, alloc, dealloc, admit, call } = instance.exports;

function put(bytes) {
  const ptr = alloc(bytes.length);
  new Uint8Array(memory.buffer, ptr, bytes.length).set(bytes);
  return ptr;
}

function viaWasm(upload, expected) {
  const up = put(upload);
  const ex = expected.length ? put(expected) : 0;
  const packed = admit(up, upload.length, ex, expected.length);
  const ptr = Number(packed >> 32n);
  const len = Number(packed & 0xffffffffn);
  const text = new TextDecoder().decode(new Uint8Array(memory.buffer, ptr, len).slice());
  dealloc(ptr, len);
  dealloc(up, upload.length);
  if (expected.length) dealloc(ex, expected.length);
  return text;
}

// #2336: a JSON request through `call`.
function viaCall(request) {
  const req = put(request);
  const packed = call(req, request.length);
  const ptr = Number(packed >> 32n);
  const len = Number(packed & 0xffffffffn);
  const text = new TextDecoder().decode(new Uint8Array(memory.buffer, ptr, len).slice());
  dealloc(ptr, len);
  dealloc(req, request.length);
  return text;
}

const ids = readdirSync(dir)
  .filter((f) => f.endsWith(".upload"))
  .map((f) => f.slice(0, -".upload".length))
  .sort();
if (ids.length === 0) {
  console.error(`no corpus cases in ${dir}`);
  process.exit(2);
}
let failed = 0;
for (const id of ids) {
  const upload = readFileSync(join(dir, `${id}.upload`));
  const expected = readFileSync(join(dir, `${id}.expected`));
  const native = readFileSync(join(dir, `${id}.result`), "utf8");
  const wasm = viaWasm(upload, expected);
  const same = native === wasm;
  if (!same) failed++;
  console.log(`${same ? "ok  " : "FAIL"} ${id}`);
  if (!same) console.log(`  native: ${native.slice(0, 300)}\n  wasm:   ${wasm.slice(0, 300)}`);
}
const calls = readdirSync(dir)
  .filter((f) => f.endsWith(".call"))
  .map((f) => f.slice(0, -".call".length))
  .sort();
for (const id of calls) {
  const request = readFileSync(join(dir, `${id}.call`));
  const native = readFileSync(join(dir, `${id}.callresult`), "utf8");
  const wasm = viaCall(request);
  const same = native === wasm;
  if (!same) failed++;
  console.log(`${same ? "ok  " : "FAIL"} ${id}`);
  if (!same) console.log(`  native: ${native.slice(0, 300)}\n  wasm:   ${wasm.slice(0, 300)}`);
}
if (failed) {
  console.error(`${failed} of ${ids.length + calls.length} cases differ between native and wasm`);
  process.exit(1);
}
console.log(`${ids.length + calls.length} cases identical, native and wasm`);

// The admission module's binding (#2336, decision A): `bynk-registry-core`'s C
// ABI (`alloc`/`dealloc`/`admit`/`call`), loaded once per isolate. Every
// answer is the module's JSON, turned into the capability's types here.
import type { Admit, Admitted, Line, Refusal, Requirement, Signer } from "./admission.js";
import { Err, Ok, loadWasm, type Result } from "../runtime.js";
import core from "./bynk_registry_core.wasm.js";

type Exports = {
  memory: WebAssembly.Memory;
  alloc(len: number): number;
  dealloc(ptr: number, len: number): void;
  admit(ptr: number, len: number, expectedPtr: number, expectedLen: number): bigint;
  call(ptr: number, len: number): bigint;
};

const instance = await loadWasm(core);
const ex = instance.exports as unknown as Exports;

// Copy `bytes` into the module's memory; the caller frees it.
function put(bytes: Uint8Array): [number, number] {
  const ptr = ex.alloc(bytes.length);
  new Uint8Array(ex.memory.buffer, ptr, bytes.length).set(bytes);
  return [ptr, bytes.length];
}

// Read and free a result (`ptr << 32 | len`).
function take(result: bigint): unknown {
  const ptr = Number(result >> 32n);
  const len = Number(result & 0xffffffffn);
  const text = new TextDecoder().decode(new Uint8Array(ex.memory.buffer, ptr, len));
  ex.dealloc(ptr, len);
  return JSON.parse(text);
}

function call(request: unknown): { ok?: Record<string, unknown>; error?: Refusal } {
  const [ptr, len] = put(new TextEncoder().encode(JSON.stringify(request)));
  try {
    return take(ex.call(ptr, len)) as { ok?: Record<string, unknown>; error?: Refusal };
  } finally {
    ex.dealloc(ptr, len);
  }
}

// A request the binding built itself that the module refused is a bug here,
// not a publisher's mistake.
function must(answer: { ok?: Record<string, unknown>; error?: Refusal }): Record<string, unknown> {
  if (answer.ok === undefined) {
    throw new Error(`the admission module refused a request: ${answer.error?.message ?? "no answer"}`);
  }
  return answer.ok;
}

function fromBase64(text: string): Uint8Array {
  const bin = atob(text);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

function lineJson(line: Line): Record<string, unknown> {
  return {
    name: line.name,
    version: line.version,
    checksum: line.checksum,
    deps: line.deps.map((r) => ({ package: r.package, range: r.range })),
    npm: line.npm.map((r) => ({ package: r.package, range: r.range })),
    toolchain: line.toolchain.tag === "Some" ? line.toolchain.value : null,
    yanked: line.yanked,
    ...(line.withdrawn.tag === "Some" ? { withdrawn: line.withdrawn.value } : {}),
    sig: line.sig,
  };
}

export class WasmAdmit implements Admit {
  async admit(upload: Uint8Array, name: string, version: string): Promise<Result<Admitted, Refusal>> {
    const [up, upLen] = put(upload);
    const [exp, expLen] = put(new TextEncoder().encode(JSON.stringify({ name, version })));
    let answer: { ok?: Record<string, unknown>; error?: Refusal };
    try {
      answer = take(ex.admit(up, upLen, exp, expLen)) as typeof answer;
    } finally {
      ex.dealloc(up, upLen);
      ex.dealloc(exp, expLen);
    }
    if (answer.ok === undefined) {
      return Err({ code: answer.error?.code ?? "invalid_package", message: answer.error?.message ?? "" });
    }
    const a = answer.ok as {
      name: string;
      version: string;
      checksum: string;
      deps: Requirement[];
      npm: Requirement[];
      archive: string;
    };
    return Ok({
      name: a.name,
      version: a.version,
      checksum: a.checksum,
      deps: a.deps,
      npm: a.npm,
      archive: fromBase64(a.archive),
    });
  }

  async payload(line: Line, registry: string): Promise<Uint8Array> {
    const ok = must(call({ op: "line_payload", line: lineJson(line), registry }));
    return fromBase64(ok.payload as string);
  }

  async render(line: Line): Promise<string> {
    return must(call({ op: "line_render", line: lineJson(line) })).text as string;
  }

  async signer(config: string, key: string): Promise<Result<Signer, Refusal>> {
    const answer = call({ op: "signer_info", config, key });
    if (answer.ok === undefined) {
      return Err({ code: answer.error?.code ?? "bad_request", message: answer.error?.message ?? "" });
    }
    const s = answer.ok as {
      id: string;
      chain_key: string;
      delegated: boolean;
      delegation_payload: string;
      delegation_sig: string | null;
      revoked: boolean;
    };
    return Ok({
      id: s.id,
      chainKey: s.chain_key,
      delegated: s.delegated,
      delegationPayload: fromBase64(s.delegation_payload),
      delegationSig: s.delegation_sig === null ? { tag: "None" } : { tag: "Some", value: s.delegation_sig },
      revoked: s.revoked,
    });
  }
}

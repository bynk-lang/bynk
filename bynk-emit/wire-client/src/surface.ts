// I4 (#1970): the part of @bynk/wire-client that is the package's own, beside
// the runtime modules it is bundled from (`../../runtime/src/{result,bytes,
// errors,boundary}.ts`). A generated client (`bynk-client.ts`) imports the
// names below under the same `__`-prefixed spellings a Bynk Worker's
// `handlers.ts` imports from `runtime.ts`, so the codec code bynkc writes into
// the client is the code it writes into a Bynk caller, byte for byte.
//
// The package follows the published wire format's version, not the compiler's
// and not ADR 0310's emit ABI (`design/archive/retired-tracks.md`, interop track Q7): its major
// version is the wire version it speaks.
import {
  boundaryError,
  callService,
  WIRE_VERSION,
  type BoundaryError,
  type CallError,
  type JsonValue,
  type ServiceBinding,
} from "../../runtime/src/boundary.ts";

// The wire format version this package speaks. A generated client declares
// `export const WIRE_VERSION: WireVersion = "<its version>"`, so a client
// generated for a wire version this package does not speak fails `tsc` in the
// caller's build rather than at the callee.
export type WireVersion = typeof WIRE_VERSION;

// The error a failed call threw, when it is one of the call boundary's own:
// the callee refused the contract (`ContractMismatch`), the callee's handler
// faulted (`CalleeFaulted`, naming its context), the transport failed or the
// callee refused the request (`Transport`, with the status), or a payload did
// not decode. `undefined` for anything else. A domain error is
// not thrown: a service returning `Result` returns its `Err`.
export function callErrorOf(e: unknown): CallError | undefined {
  if (typeof e !== "object" || e === null) return undefined;
  const found = (e as { boundaryError?: unknown }).boundaryError;
  return typeof found === "object" && found !== null ? (found as CallError) : undefined;
}

export { boundaryError as __boundaryError, callService as __callService };
export type { BoundaryError as __BoundaryError, JsonValue as __JsonValue, ServiceBinding as __ServiceBinding };

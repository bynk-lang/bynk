// The slice of the `bynk-wasm` wasm-bindgen glue the web worker uses (#2102).
//
// `npm run build:wasm` generates the real glue into `wasm/` (gitignored, like the
// playground's `src/vendor/`), and `build:web` points esbuild's `bynk-wasm`
// alias at it. Declaring it here keeps `npm run check` working without a Rust
// toolchain; the generated `wasm/bynk_wasm.d.ts` is the source of truth, and the
// web smoke test (test/web) fails if the two drift. Keep these signatures in
// step with bynk-wasm/src/lib.rs's `#[wasm_bindgen]` exports.
declare module "bynk-wasm" {
  /** `{ diagnostics: [{ from, to, line, col, severity, category, message, notes, labels }] }` */
  export function bynk_analyze(source: string, platform?: string | null): string;
  /** `{ ty: string | null }` at a UTF-8 byte offset. */
  export function bynk_hover(source: string, offset: number, platform?: string | null): string;
  /** `{ items: [{ label, kind, detail, insert_text }] }` at a UTF-8 byte offset. */
  export function bynk_complete(source: string, offset: number, platform?: string | null): string;
  /** `bynk_analyze` over a project, `{ "<path>": "<text>" }` JSON: `{ diagnostics, warnings }`; each diagnostic carries its `path`, and `warnings` are project-level notes (an ignored absolute `[paths]` entry). */
  export function bynk_analyze_project(files_json: string, platform?: string | null): string;
  /** `bynk_hover` in the project file at `path`. */
  export function bynk_hover_project(
    files_json: string,
    path: string,
    offset: number,
    platform?: string | null,
  ): string;
  /** `bynk_complete` in the project file at `path`. */
  export function bynk_complete_project(
    files_json: string,
    path: string,
    offset: number,
    platform?: string | null,
  ): string;
  /** Instantiate the module from its bytes (or a URL / Response). */
  export default function init(input: {
    module_or_path: BufferSource | URL | string | Response | WebAssembly.Module;
  }): Promise<unknown>;
}

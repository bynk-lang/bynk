// The names a generated test module supplies to the test runtime it splices (back-half paydown
// 1B.3a, #4047, Decision B). This is a contract, never spliced or emitted: `bundle.mjs` lists it
// as contract-only. A `testgen` unit test checks each declaration against what the generator
// writes (`test_module.rs`, `snapshot_harness`; `test_deps.rs`, `ASSET_FILES_NAME` and
// `ASSET_FILES_TYPE`), so a renamed or retyped generated name fails a test, not a fixture.

import type { __SnapshotFile } from "./snapshot.ts";

/** The stored text of each snapshot file a test module names, inlined by the compiler. */
export declare const __bynkSnapshotFiles: globalThis.Record<string, __SnapshotFile>;

/** The files a target's Worker ships, embedded base64 by served path, for a case's asset fetch. */
export declare const __bynkAssetFiles: Record<string, { readonly body: string; readonly contentType: string }>;

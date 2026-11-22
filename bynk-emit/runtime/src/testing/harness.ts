// The one name a generated test module supplies to the test runtime it splices (back-half paydown
// 1B.3a, #4047, Decision B). This is a contract, never spliced or emitted: `bundle.mjs` lists it
// as contract-only. A `testgen` unit test checks the declaration against the text the generator
// writes (`test_module.rs`, `snapshot_harness`), so a renamed generated name fails a test, not a
// fixture.

import type { __SnapshotFile } from "./snapshot.ts";

/** The stored text of each snapshot file a test module names, inlined by the compiler. */
export declare const __bynkSnapshotFiles: globalThis.Record<string, __SnapshotFile>;

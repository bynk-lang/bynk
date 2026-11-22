//! Stages runtime modules into a compiled `--emit js` output (1B.3b, #4047).
//!
//! A compile emits only the runtime modules the emitted files import. The test doubles
//! (`__bynk_runtime/testing/bindings.js`) are imported by a test build and by nothing else, so a
//! hand-written script that binds one beside a production compile has to stage the module itself.
//! The module and the modules it imports that are not already there are type-stripped into place.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

/// Stages the runtime's test doubles under `out` (a `--emit js` output root).
pub fn stage_test_doubles(out: &Path) {
    let reached = BTreeSet::from([("testing/bindings".to_string(), "js".to_string())]);
    for (path, source) in bynk_emit::emitter::closure_files(&reached) {
        let target = out.join(path.with_extension("js"));
        if target.exists() {
            continue;
        }
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        let js = bynk_strip::strip_types(&source, &path.to_string_lossy())
            .unwrap_or_else(|e| panic!("strip {}: {e:?}", path.display()));
        fs::write(&target, js).unwrap();
    }
}

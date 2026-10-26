//! #2471: `bynk/emittedOutput` — `emitted_output_for` driven directly over a
//! scratch project (as `sequence_request.rs` does for its request): the
//! requested file's generated TypeScript comes back with a decoded line map
//! whose entries point back into the right source lines.

use std::collections::HashMap;
use std::path::PathBuf;

use bynk_lsp::emitted_request::emitted_output_for;

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const SRC: &str = "context greet\n\nservice hello {\n  on call(name: String) -> Effect[Result[String, ()]] {\n    Ok(name)\n  }\n}\n";

fn project(tag: &str, src: &str) -> (Scratch, HashMap<PathBuf, String>) {
    let dir = std::env::temp_dir().join(format!(
        "bynk_lsp_emitted_request_{tag}_{}_{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("bynk.toml"), "").unwrap();
    std::fs::write(dir.join("src/greet.bynk"), src).unwrap();
    let dir = dir.canonicalize().unwrap();
    let snapshots = HashMap::from([(PathBuf::from("src/greet.bynk"), src.to_string())]);
    (Scratch(dir), snapshots)
}

#[test]
fn a_files_output_carries_its_text_and_a_line_map_into_the_source() {
    let (dir, snapshots) = project("ok", SRC);
    let out = emitted_output_for(&dir.0, &snapshots, &PathBuf::from("src/greet.bynk"))
        .expect("the project compiles");
    assert!(
        !out.files.is_empty(),
        "greet.bynk emits at least one module"
    );
    let f = &out.files[0];
    assert!(f.path.ends_with(".ts"), "{}", f.path);
    assert!(f.sources[f.source_index].ends_with("src/greet.bynk"));
    // Some generated line maps to the handler's `Ok(name)` body (source line 4).
    assert!(
        f.lines
            .iter()
            .flatten()
            .any(|[s, l]| *s == f.source_index && *l == 4),
        "no generated line maps to the handler body: {:?}",
        f.lines
    );
}

#[test]
fn an_unsaved_buffer_is_what_is_emitted() {
    let (dir, mut snapshots) = project("overlay", SRC);
    let edited = SRC.replace("Ok(name)", "Ok(\"edited\")");
    snapshots.insert(PathBuf::from("src/greet.bynk"), edited);
    let out = emitted_output_for(&dir.0, &snapshots, &PathBuf::from("src/greet.bynk")).unwrap();
    assert!(out.files.iter().any(|f| f.text.contains("edited")));
}

#[test]
fn a_project_that_does_not_compile_has_no_output() {
    let (dir, snapshots) = project("broken", "context greet\n\nservice hello {\n  on call(");
    assert!(emitted_output_for(&dir.0, &snapshots, &PathBuf::from("src/greet.bynk")).is_none());
}

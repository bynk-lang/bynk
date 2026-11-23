//! 1C.3a (#4040): the CLI's two ways of knowing a Worker's bindings agree over
//! the whole corpus. `bynk deploy` and `bynk dev` read each Worker's
//! declarations from the build's `WorkerBindings`; until 1C.3b deletes it they
//! also parse them back out of the `wrangler.toml` and sidecars the build
//! wrote. Compiling every Workers fixture and holding the two to each other
//! says the projection declares, for every Worker the goldens pin, exactly what
//! the parse reads.

use std::fs;
use std::path::{Path, PathBuf};

use bynk_driver::project::BuildTarget;

use super::e2e::{compile_fixture, fixture_target};

/// The `wrangler.toml` goldens of `fixture`'s Workers.
fn golden_workers(fixture: &Path) -> usize {
    let workers = fixture.join("expected").join("workers");
    fs::read_dir(workers).map_or(0, |entries| {
        entries
            .flatten()
            .filter(|e| e.path().join("wrangler.toml").is_file())
            .count()
    })
}

fn scratch(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("bynk-binding-model-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

#[test]
fn the_binding_model_agrees_with_the_emitted_files_over_every_workers_fixture() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/positive");
    let mut fixtures: Vec<PathBuf> = fs::read_dir(&root)
        .expect("the positive fixtures")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() && fixture_target(p) == BuildTarget::Workers)
        .collect();
    fixtures.sort();
    let (mut checked, mut pinned) = (0, 0);
    let mut failures = Vec::new();
    for fixture in &fixtures {
        let Ok(output) = compile_fixture(fixture, BuildTarget::Workers) else {
            continue;
        };
        let name = fixture.file_name().unwrap().to_string_lossy().into_owned();
        let build = output
            .workers
            .as_ref()
            .expect("a Workers build carries its binding model");
        let out = scratch(&name);
        bynk_driver::write_output(&output, &out).expect("the build writes");
        let workers_dir = out.join("workers");
        pinned += golden_workers(fixture);
        checked += fs::read_dir(&workers_dir).map_or(0, |entries| {
            entries
                .flatten()
                .filter(|e| e.path().join("wrangler.toml").is_file())
                .count()
        });
        if let Err(e) = bynk::deploy::check_binding_model(&workers_dir, build) {
            failures.push(format!("{name}: {e}"));
        }
        let _ = fs::remove_dir_all(&out);
    }
    assert!(
        failures.is_empty(),
        "the binding model and the emitted files disagree:\n{}",
        failures.join("\n")
    );
    assert!(
        checked > 0 && checked >= pinned,
        "checked {checked} Workers, but the goldens pin {pinned}"
    );
}

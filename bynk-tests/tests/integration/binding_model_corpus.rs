//! 1C.3b (#4040): the `wrangler.toml` the renderer writes names exactly the
//! bindings the model holds. `bynk deploy` and `bynk dev` read each Worker's
//! bindings from the build's `WorkerBindings`, not from the file, so a renderer
//! that dropped or added a binding would go unseen by them. This reads each
//! Workers fixture's rendered config generically, as a `toml::Table` with no
//! per-kind struct, so the check restates no kind's shape: the `binding` (or
//! `name`) of every array-of-tables entry is the set of the model's binding
//! names, less the singletons, which are plain tables.
//!
//! It runs in both directions over Workers: every directory the renderer wrote
//! is checked, and every Worker the model gives a binding to must have a
//! config that was. What it can see is a binding table keyed `binding` or
//! `name`; a future kind rendered under some other key would be missing from the
//! config side, so a dropped binding is caught but one added under such a key is
//! not, and the key must be added to [`entry_names`] with the kind.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use bynk_check::worker_bindings::BindingSource;
use bynk_driver::project::BuildTarget;

use super::e2e::{compile_fixture, fixture_target};

/// The `binding` or `name` of every table in every array of tables of `table`,
/// at any depth.
fn entry_names(table: &toml::Table, out: &mut BTreeSet<String>) {
    for value in table.values() {
        match value {
            toml::Value::Table(inner) => entry_names(inner, out),
            toml::Value::Array(items) => {
                for item in items {
                    if let toml::Value::Table(entry) = item {
                        if let Some(name) = entry
                            .get("binding")
                            .or_else(|| entry.get("name"))
                            .and_then(toml::Value::as_str)
                        {
                            out.insert(name.to_string());
                        }
                        entry_names(entry, out);
                    }
                }
            }
            toml::Value::String(_)
            | toml::Value::Integer(_)
            | toml::Value::Float(_)
            | toml::Value::Boolean(_)
            | toml::Value::Datetime(_) => {}
        }
    }
}

fn scratch(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("bynk-binding-names-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

#[test]
fn every_rendered_binding_is_the_models_and_every_model_binding_is_rendered() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/positive");
    let mut fixtures: Vec<PathBuf> = fs::read_dir(&root)
        .expect("the positive fixtures")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() && fixture_target(p) == BuildTarget::Workers)
        .collect();
    fixtures.sort();
    let mut checked = 0;
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
        let mut seen = BTreeSet::new();
        for entry in fs::read_dir(&workers_dir).into_iter().flatten().flatten() {
            let dir = entry.file_name().to_string_lossy().into_owned();
            let config = entry.path().join("wrangler.toml");
            let text = match fs::read_to_string(&config) {
                Ok(text) => text,
                Err(e) => {
                    failures.push(format!("{name}/{dir}: its config cannot be read: {e}"));
                    continue;
                }
            };
            seen.insert(dir.clone());
            let table: toml::Table = text.parse().expect("the rendered config parses");
            let mut rendered = BTreeSet::new();
            entry_names(&table, &mut rendered);
            let modelled: BTreeSet<String> = build
                .bindings
                .of_worker(&dir)
                .iter()
                .filter(|b| !matches!(b.source, BindingSource::Singleton { .. }))
                .map(|b| b.name.clone())
                .collect();
            if rendered != modelled {
                failures.push(format!(
                    "{name}/{dir}: the config names {rendered:?}, the model {modelled:?}"
                ));
            }
            checked += 1;
        }
        // The other direction: a Worker the model gives a binding to must have a
        // config the renderer wrote, under the directory the layout names.
        for context in build.bindings.by_context().keys() {
            let dir = build.layout.dir(context);
            let binds = build
                .bindings
                .of_worker(&dir)
                .iter()
                .any(|b| !matches!(b.source, BindingSource::Singleton { .. }));
            if binds && !seen.contains(&dir) {
                failures.push(format!(
                    "{name}/{dir}: the model has bindings for this Worker, but no config was rendered"
                ));
            }
        }
        let _ = fs::remove_dir_all(&out);
    }
    assert!(
        failures.is_empty(),
        "the rendered configuration and the binding model disagree:\n{}",
        failures.join("\n")
    );
    assert!(checked > 0, "no Workers fixture was checked");
}

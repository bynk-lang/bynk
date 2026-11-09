//! #3100: a build reports each context as it compiles, dependencies first, and
//! a build with no sink is exactly what it was.

use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use bynk_driver::project::{self, ProgressEvent};

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn project(tag: &str, files: &[(&str, &str)]) -> Scratch {
    let dir = std::env::temp_dir().join(format!(
        "bynk_3100_{tag}_{}_{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = fs::remove_dir_all(&dir);
    for (rel, body) in files {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, body).unwrap();
    }
    Scratch(dir)
}

const TWO_CONTEXTS: &[(&str, &str)] = &[
    (
        "bynk.toml",
        "[project]\nname = \"shop\"\nversion = \"0.1.0\"\n",
    ),
    ("src/shop/billing.bynk", "context shop.billing {\n}\n"),
    (
        "src/shop/orders.bynk",
        "context shop.orders {\n  consumes shop.billing\n}\n",
    ),
];

fn collect(root: &std::path::Path) -> (Vec<ProgressEvent>, project::ProjectOutput) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = {
        let events = Arc::clone(&events);
        Arc::new(move |e: &ProgressEvent| events.lock().unwrap().push(e.clone()))
    };
    let opts = bynk_driver::project_options(root)
        .expect("readable project")
        .progress(Some(sink));
    let out = project::compile_project(&opts).unwrap_or_else(|_| panic!("build must succeed"));
    let seen = events.lock().unwrap().clone();
    (seen, out)
}

#[test]
fn each_context_starts_and_finishes_dependencies_first() {
    let root = project("order", TWO_CONTEXTS);
    let (events, _) = collect(&root.0);
    let s = |n: &str| n.to_string();
    assert_eq!(
        events,
        vec![
            ProgressEvent::Started(s("shop.billing")),
            ProgressEvent::Finished(s("shop.billing")),
            ProgressEvent::Started(s("shop.orders")),
            ProgressEvent::Finished(s("shop.orders")),
        ]
    );
}

#[test]
fn a_sink_changes_nothing_about_the_output() {
    let root = project("same", TWO_CONTEXTS);
    let (_, with) = collect(&root.0);
    let opts = bynk_driver::project_options(&root.0).expect("readable project");
    let without = project::compile_project(&opts).unwrap_or_else(|_| panic!("build must succeed"));
    let read = |out: &project::ProjectOutput, name: &str| {
        let dir = root.0.join(name);
        bynk_driver::write_output(out, &dir).unwrap();
        let mut files: Vec<_> = walk(&dir)
            .into_iter()
            .map(|p| {
                (
                    p.strip_prefix(&dir).unwrap().to_path_buf(),
                    fs::read(&p).unwrap(),
                )
            })
            .collect();
        files.sort();
        files
    };
    assert_eq!(read(&with, "a"), read(&without, "b"));
}

#[test]
fn a_context_with_errors_starts_but_does_not_finish() {
    let root = project(
        "bad",
        &[
            (
                "bynk.toml",
                "[project]\nname = \"shop\"\nversion = \"0.1.0\"\n",
            ),
            (
                "src/shop/billing.bynk",
                "context shop.billing {\n  fn f() -> Int { \"no\" }\n}\n",
            ),
        ],
    );
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = {
        let events = Arc::clone(&events);
        Arc::new(move |e: &ProgressEvent| events.lock().unwrap().push(e.clone()))
    };
    let opts = bynk_driver::project_options(&root.0)
        .expect("readable")
        .progress(Some(sink));
    assert!(project::compile_project(&opts).is_err());
    assert_eq!(
        *events.lock().unwrap(),
        vec![ProgressEvent::Started("shop.billing".to_string())],
        "a failed context reports Started and no Finished"
    );
}

fn walk(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let p = entry.unwrap().path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else {
            out.push(p);
        }
    }
    out
}

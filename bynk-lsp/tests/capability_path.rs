//! #2469: the `given`-entry hover and the "Show capability path" action, over
//! real `diagnose_project` output. A context split over three files, so the
//! chain `BucketTakedown → Front.go → Index.withdraw → Files.write` crosses
//! files and the links must name the right one.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use bynk_lsp::capability_path::{PathView, all_paths, hover_section, show_action};
use bynk_lsp::hover;
use tower_lsp::lsp_types::{CodeActionOrCommand, Url};

const FILES: &str = "context shop\n\
    \n\
    capability Files {\n\
    \x20 fn write(name: String) -> Effect[()]\n\
    }\n\
    \n\
    provides Files = MemFiles {\n\
    \x20 fn write(name: String) -> Effect[()] {\n\
    \x20   Effect.pure(())\n\
    \x20 }\n\
    }\n\
    \n\
    agent Index {\n\
    \x20 key name: String\n\
    \x20 store hits: Cell[Int]\n\
    \x20 on call withdraw(version: String) -> Effect[()] given Files {\n\
    \x20   Files.write(version)\n\
    \x20 }\n\
    }\n";

const FRONT: &str = "context shop\n\
    \n\
    agent Front {\n\
    \x20 key name: String\n\
    \x20 store hits: Cell[Int]\n\
    \x20 on call go(version: String) -> Effect[()] {\n\
    \x20   Index(self.name).withdraw(version)\n\
    \x20 }\n\
    }\n";

/// `take` reaches `Files` two ways: through `Front.go` and straight to
/// `Index.withdraw`.
const TAKEDOWN: &str = "context shop\n\
    \n\
    capability Takedown {\n\
    \x20 fn take(name: String) -> Effect[()]\n\
    }\n\
    \n\
    provides Takedown = BucketTakedown given Files {\n\
    \x20 fn take(name: String) -> Effect[()] {\n\
    \x20   let _ <- Front(name).go(\"1.0.0\")\n\
    \x20   Index(name).withdraw(\"1.0.0\")\n\
    \x20 }\n\
    }\n";

struct Project {
    root: PathBuf,
    result: bynk_ide::ProjectDiagnostics,
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(self.root.parent().unwrap());
    }
}

fn project(tag: &str) -> Project {
    let top = std::env::temp_dir().join(format!("bynk_lsp_cappath_{}_{tag}", std::process::id()));
    let root = top.join("src");
    fs::create_dir_all(root.join("shop")).unwrap();
    for (name, text) in [
        ("files.bynk", FILES),
        ("front.bynk", FRONT),
        ("takedown.bynk", TAKEDOWN),
    ] {
        fs::write(root.join("shop").join(name), text).unwrap();
    }
    let sources =
        bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
    let result = bynk_ide::diagnose_project(&root, &sources);
    Project { root, result }
}

fn snapshots(p: &Project) -> HashMap<PathBuf, String> {
    p.result
        .capability_paths
        .keys()
        .map(|rel| {
            let text = p
                .result
                .files
                .iter()
                .find(|f| f.source_path.ends_with(rel) || rel.ends_with(&f.source_path))
                .map(|f| f.text.clone())
                .unwrap_or_default();
            (rel.clone(), text)
        })
        .collect()
}

fn view<'a>(p: &'a Project, snaps: &'a HashMap<PathBuf, String>, rel: &'a Path) -> PathView<'a> {
    PathView {
        graphs: &p.result.capability_paths,
        snapshots: snaps,
        project_root: &p.root,
        rel,
    }
}

fn rel_of(p: &Project, name: &str) -> PathBuf {
    p.result
        .capability_paths
        .keys()
        .find(|k| k.ends_with(name))
        .unwrap_or_else(|| panic!("{name} has no capability paths"))
        .clone()
}

/// The offset of the `Files` in `given Files` within `text`.
fn given_files(text: &str) -> usize {
    text.find("given Files").unwrap() + "given ".len() + 1
}

#[test]
fn hover_on_a_direct_given_shows_the_one_step_path() {
    let p = project("direct");
    let snaps = snapshots(&p);
    let rel = rel_of(&p, "files.bynk");
    let offset = given_files(&snaps[&rel]);
    let md = hover_section(&view(&p, &snaps, &rel), offset).expect("a path section");
    assert!(
        md.contains("`Index.withdraw` needs `Files` directly"),
        "{md}"
    );
    assert!(md.contains("calls `Files.write`"), "{md}");
    assert!(md.contains("files.bynk#L"), "link names the file: {md}");
}

#[test]
fn hover_on_a_transitive_given_lists_each_call_with_links_across_files() {
    let p = project("transitive");
    let snaps = snapshots(&p);
    let rel = rel_of(&p, "takedown.bynk");
    let offset = given_files(&snaps[&rel]);
    let md = hover_section(&view(&p, &snaps, &rel), offset).expect("a path section");
    // The shortest of the two routes: the provider calls `withdraw` itself.
    assert!(
        md.contains("`BucketTakedown` needs `Files` through 1 call(s)"),
        "{md}"
    );
    assert!(md.contains("calls [`Index.withdraw`]"), "{md}");
    assert!(
        md.contains("files.bynk#L"),
        "the use lies in files.bynk: {md}"
    );
}

#[test]
fn the_action_lists_every_path_as_peek_locations() {
    let p = project("action");
    let snaps = snapshots(&p);
    let rel = rel_of(&p, "takedown.bynk");
    let offset = given_files(&snaps[&rel]);
    let all = all_paths(&view(&p, &snaps, &rel), offset).expect("paths");
    assert_eq!(all.paths, 2, "via Front.go and straight to Index.withdraw");
    assert!(!all.truncated);
    // Two calls into `withdraw`/`go`, the `withdraw` call inside `go`, and the
    // one shared use.
    assert_eq!(all.locations.len(), 4, "{:?}", all.locations);
    // Per path, the shared use repeats: `go` → `withdraw` → use, and
    // `withdraw` → use.
    let lens: Vec<usize> = all.by_path.iter().map(Vec::len).collect();
    assert_eq!(lens, vec![2, 3], "shortest chain first");
    let last = |i: usize| all.by_path[i].last().cloned();
    assert_eq!(last(0), last(1), "both chains end at the one shared use");
    let uri = Url::from_file_path(p.root.join("shop/takedown.bynk")).unwrap();
    let Some(CodeActionOrCommand::CodeAction(action)) =
        show_action(&view(&p, &snaps, &rel), &uri, offset)
    else {
        panic!("an action on the given entry");
    };
    assert!(
        action
            .title
            .contains("Show capability paths for `Files` (2)"),
        "{}",
        action.title
    );
    let cmd = action.command.expect("a client command");
    assert_eq!(cmd.command, "bynk.showCapabilityPath");
    let args = cmd.arguments.unwrap();
    assert_eq!(args.len(), 4);
    assert_eq!(args[3], serde_json::json!(all.by_path));
}

#[test]
fn off_a_given_entry_there_is_nothing() {
    let p = project("off");
    let snaps = snapshots(&p);
    let rel = rel_of(&p, "takedown.bynk");
    let offset = snaps[&rel].find("capability Takedown").unwrap();
    assert!(hover_section(&view(&p, &snaps, &rel), offset).is_none());
    assert!(all_paths(&view(&p, &snaps, &rel), offset).is_none());
}

/// The path rides rung 1: the capability's own description stays, with the
/// path below it.
#[test]
fn the_full_ladder_keeps_the_capability_description_and_appends_the_path() {
    let p = project("ladder");
    let snaps = snapshots(&p);
    let rel = rel_of(&p, "files.bynk");
    let text = snaps[&rel].clone();
    let offset = given_files(&text);
    let uri = Url::parse("file:///files.bynk").unwrap();
    let md = hover::hover_content(&hover::HoverInput {
        analysis: Some(hover::HoverAnalysis {
            index: &p.result.index,
            snapshots: &snaps,
            locals: &p.result.locals,
            expr_types: &p.result.expr_types,
            tys: &p.result.ty_intern,
            rel: &rel,
            offset,
            capability_paths: &p.result.capability_paths,
            project_root: &p.root,
            doc_scope: &p.result.doc_scope,
            boundary_info: &p.result.boundary_info,
            context_count: 1,
        }),
        doc: Some((&text, offset)),
        path: uri.to_file_path().ok().as_deref(),
        files: None,
    })
    .expect("hover content");
    assert!(md.contains("capability Files"), "{md}");
    assert!(
        md.contains("`Index.withdraw` needs `Files` directly"),
        "{md}"
    );
}

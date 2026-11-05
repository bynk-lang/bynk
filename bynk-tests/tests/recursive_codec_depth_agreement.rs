//! #2704: a codec and the code that calls it agree about the `depth`
//! parameter (#2656).
//!
//! A codec on a type cycle takes a trailing `depth: number = 0` and passes it,
//! plus one, to every callee on a cycle. Which codecs those are is decided by
//! `bynk_check::wire::recursive_codec_suffixes`, which the emitter evaluates
//! once per emission site (the owner's helpers, a consumer's own view of a
//! callee's types in `client.rs`, the unit-test tier in `tests_emit.rs`), each
//! from the type table that site has. Were one site's table to lack a
//! declaration on a cycle, a codec there would not take `depth` while the one
//! it calls (in another module, from another site) did, and the call would
//! compile: JavaScript defaults the missing argument to 0, so the count would
//! restart at every call and the unbounded recursion #2656 removed would come
//! back with no error anywhere.
//!
//! This reads the blessed output of every positive fixture and checks that,
//! inside a codec that takes `depth`, every call to a codec that also takes it
//! passes it on, as `depth` from a container's codec and as `depth + 1` from a
//! record's or sum's (so the count both survives and rises). Calls are
//! resolved across modules through the emitted `import` and re-export lines,
//! so a call into another module's codec is checked against that module's
//! definition. Negative controls prove the check is not vacuous.
//!
//! Not checked here: a caller that does *not* take `depth` calling a callee
//! that does is skipped (a non-recursive record holding a recursive field does
//! exactly that legitimately, so the text alone cannot tell it from the
//! bug above). The opposite shape, a `depth` caller passing an extra
//! argument to a callee without the parameter, is an arity error that `tsc`
//! reports on the emitted tree; the behaviour tests in
//! `recursive_boundary_behaviour.rs` catch the lost bound at runtime.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// One emitted codec function.
struct Def {
    takes_depth: bool,
    /// The function body, up to its closing `}` at column 0.
    body: String,
}

#[derive(Default)]
struct Module {
    defs: HashMap<String, Def>,
    /// `name` -> the module specifier it is imported or re-exported from.
    named: HashMap<String, String>,
    /// `export * from "<specifier>"`.
    star: Vec<String>,
}

fn is_codec(name: &str) -> bool {
    name.starts_with("__serialise_") || name.starts_with("__deserialise_")
}

fn ident_at(s: &str) -> &str {
    let end = s
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(s.len());
    &s[..end]
}

fn parse(text: &str) -> Module {
    let mut m = Module::default();
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let header = line
            .strip_prefix("export function ")
            .or_else(|| line.strip_prefix("function "));
        if let Some(rest) = header
            && is_codec(ident_at(rest))
        {
            let name = ident_at(rest).to_string();
            let takes_depth = line.contains("depth: number");
            let mut body = String::new();
            i += 1;
            while i < lines.len() && lines[i] != "}" {
                body.push_str(lines[i]);
                body.push('\n');
                i += 1;
            }
            m.defs.insert(name, Def { takes_depth, body });
        } else if (line.starts_with("import {") || line.starts_with("export {"))
            && let Some((names, from)) = line.split_once("} from \"")
        {
            let spec = from.trim_end_matches(';').trim_end_matches('"').to_string();
            for n in names.split_once('{').map_or("", |(_, n)| n).split(',') {
                let n = n.trim().trim_start_matches("type ").trim();
                if is_codec(n) {
                    m.named.insert(n.to_string(), spec.clone());
                }
            }
        } else if let Some(rest) = line.strip_prefix("export * from \"") {
            m.star
                .push(rest.trim_end_matches(';').trim_end_matches('"').to_string());
        }
        i += 1;
    }
    m
}

/// The files of one fixture's `expected/` tree, `.ts` only, by relative path.
type Tree = HashMap<PathBuf, Module>;

fn load(root: &Path) -> Tree {
    fn walk(dir: &Path, root: &Path, out: &mut Tree) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, root, out);
            } else if path.extension().is_some_and(|e| e == "ts") {
                let text = fs::read_to_string(&path).unwrap();
                out.insert(path.strip_prefix(root).unwrap().to_path_buf(), parse(&text));
            }
        }
    }
    let mut tree = Tree::new();
    walk(root, root, &mut tree);
    tree
}

fn target_of(from: &Path, spec: &str) -> Option<PathBuf> {
    if !spec.starts_with('.') {
        return None;
    }
    let joined = from.parent()?.join(spec.trim_end_matches(".js"));
    // Normalise `a/../b`.
    let mut parts: Vec<std::ffi::OsString> = Vec::new();
    for c in joined.components() {
        match c {
            std::path::Component::ParentDir => {
                parts.pop();
            }
            std::path::Component::CurDir => {}
            other => parts.push(other.as_os_str().to_os_string()),
        }
    }
    let mut p: PathBuf = parts.iter().collect();
    p.set_extension("ts");
    Some(p)
}

/// Whether the codec `name`, as seen from `file`, takes `depth`; `None` when it
/// is not defined in the tree (a runtime helper, say).
fn takes_depth(tree: &Tree, file: &Path, name: &str, hops: u32) -> Option<bool> {
    if hops > 8 {
        return None;
    }
    let module = tree.get(file)?;
    if let Some(def) = module.defs.get(name) {
        return Some(def.takes_depth);
    }
    if let Some(spec) = module.named.get(name)
        && let Some(next) = target_of(file, spec)
        && let Some(found) = takes_depth(tree, &next, name, hops + 1)
    {
        return Some(found);
    }
    for spec in &module.star {
        if let Some(next) = target_of(file, spec)
            && let Some(found) = takes_depth(tree, &next, name, hops + 1)
        {
            return Some(found);
        }
    }
    None
}

/// The text between the `(` at `open` and its matching `)`.
fn call_args(s: &str, open: usize) -> Option<&str> {
    let mut nesting = 0usize;
    for (i, c) in s[open..].char_indices() {
        match c {
            '(' => nesting += 1,
            ')' => {
                nesting -= 1;
                if nesting == 0 {
                    return Some(&s[open + 1..open + i]);
                }
            }
            _ => {}
        }
    }
    None
}

/// A relative path spelt with `/` on every OS, for reports and comparisons.
fn slashed(p: &Path) -> String {
    p.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// The codecs of a container (`__deserialise_List_Dir`, `Map_String_Index`,
/// `Option_Tree`, …), as opposed to a named type's.
fn is_container_codec(name: &str) -> bool {
    let suffix = name
        .strip_prefix("__serialise_")
        .or_else(|| name.strip_prefix("__deserialise_"))
        .unwrap_or(name);
    ["List_", "Map_", "Set_", "Option_", "Result_"]
        .iter()
        .any(|p| suffix.starts_with(p))
}

/// Every place a codec taking `depth` calls a codec taking `depth` without
/// passing it on, as `file: caller -> callee`.
fn disagreements(tree: &Tree) -> Vec<String> {
    let mut found = Vec::new();
    for (file, module) in tree {
        for (caller, def) in &module.defs {
            if !def.takes_depth {
                continue;
            }
            let body = &def.body;
            let mut at = 0;
            while let Some(rel) = body[at..].find("__") {
                let start = at + rel;
                let name = ident_at(&body[start..]);
                at = start + name.len().max(2);
                if !is_codec(name) || !body[at..].starts_with('(') {
                    continue;
                }
                if takes_depth(tree, file, name, 0) != Some(true) {
                    continue;
                }
                let args = call_args(body, at).unwrap_or("").trim_end();
                // A container's codec (`List_Dir`) is not a level of nesting and
                // passes `depth` on unchanged; a record's or sum's counts one
                // and passes `depth + 1`. A codec that never increments loses
                // the bound as surely as one that drops the argument.
                let (want, how) = if is_container_codec(caller) {
                    (", depth", "`depth`")
                } else {
                    (", depth + 1", "`depth + 1`")
                };
                let passes = args.ends_with(want)
                    && (want == ", depth + 1" || !args.ends_with(", depth + 1"));
                if !passes {
                    found.push(format!(
                        "{}: {caller} calls {name} without passing {how}",
                        slashed(file)
                    ));
                }
            }
        }
    }
    found.sort();
    found
}

fn positive_fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/positive")
}

#[test]
fn every_call_between_codecs_that_take_depth_passes_it() {
    let mut checked = Vec::new();
    let mut all = Vec::new();
    let mut dirs: Vec<PathBuf> = fs::read_dir(positive_fixtures())
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    dirs.sort();
    for dir in dirs {
        let expected = dir.join("expected");
        if !expected.is_dir() {
            continue;
        }
        let tree = load(&expected);
        if !tree
            .values()
            .any(|m| m.defs.values().any(|d| d.takes_depth))
        {
            continue;
        }
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        all.extend(
            disagreements(&tree)
                .into_iter()
                .map(|d| format!("{name}: {d}")),
        );
        checked.push(name);
    }
    // Not vacuous: the fixtures that pin recursive codecs are in the sweep.
    for must in [
        "2613_recursive_types_at_boundary",
        "2656_recursive_nesting_bound",
        "2704_recursive_cycle_across_files",
        "2704_recursive_event_payload",
    ] {
        assert!(
            checked.iter().any(|c| c == must),
            "`{must}` carries depth-taking codecs but was not checked; checked: {checked:?}"
        );
    }
    assert!(
        all.is_empty(),
        "a caller and its callee disagree about `depth`, so the count restarts at 0 there \
         (JavaScript defaults a missing argument) and the nesting bound is lost:\n{}",
        all.join("\n")
    );
}

/// The check finds what it exists to find: a caller in one module that drops
/// `depth` on a call into another module's codec that takes it.
#[test]
fn a_call_that_drops_depth_across_modules_is_reported() {
    let expected = positive_fixtures().join("2704_recursive_cycle_across_files/expected");
    let mut tree = load(&expected);
    assert!(
        disagreements(&tree).is_empty(),
        "the blessed output agrees: {:?}",
        disagreements(&tree)
    );

    // `Stmt`'s decoder (cyc/shapes/stmt.ts) calls `Expr`'s (cyc/shapes/expr.ts)
    // with `depth + 1`. Rewrite that one call to omit it.
    let stmt = tree
        .get_mut(Path::new("cyc/shapes/stmt.ts"))
        .expect("the split commons emits stmt.ts");
    let def = stmt.defs.get_mut("__deserialise_Stmt").unwrap();
    assert!(
        def.body
            .contains("__deserialise_Expr(obj[\"expr\"], `${path}.expr`, depth + 1)")
    );
    def.body = def.body.replace(", depth + 1)", ")");
    assert_eq!(
        disagreements(&tree),
        vec![
            "cyc/shapes/stmt.ts: __deserialise_Stmt calls __deserialise_Expr without passing `depth + 1`"
        ],
    );

    // A named type's codec that passes `depth` unchanged never raises the
    // count, so the bound is lost although the argument is passed.
    let mut tree = load(&expected);
    let stmt = tree.get_mut(Path::new("cyc/shapes/stmt.ts")).unwrap();
    let def = stmt.defs.get_mut("__deserialise_Stmt").unwrap();
    def.body = def.body.replace(", depth + 1)", ", depth)");
    assert_eq!(
        disagreements(&tree),
        vec![
            "cyc/shapes/stmt.ts: __deserialise_Stmt calls __deserialise_Expr without passing `depth + 1`"
        ],
    );

    // And one through a re-export: a context's codec calling the commons'
    // `Stmt` codec by the name its own module imports.
    let mut tree = load(&expected);
    let handlers = tree
        .values_mut()
        .find(|m| m.defs.contains_key("__deserialise_List_Stmt"))
        .expect("a context emits the List[Stmt] decoder");
    let def = handlers.defs.get_mut("__deserialise_List_Stmt").unwrap();
    def.body = def.body.replace(", depth)", ")");
    assert_eq!(disagreements(&tree).len(), 1, "{:?}", disagreements(&tree));

    // A container's codec that increments counts a level that is not one.
    let mut tree = load(&expected);
    let handlers = tree
        .values_mut()
        .find(|m| m.defs.contains_key("__deserialise_List_Stmt"))
        .unwrap();
    let def = handlers.defs.get_mut("__deserialise_List_Stmt").unwrap();
    def.body = def.body.replace(", depth)", ", depth + 1)");
    assert_eq!(disagreements(&tree).len(), 1, "{:?}", disagreements(&tree));
}

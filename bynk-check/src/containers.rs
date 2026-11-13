//! `Container` instances (#3477, slice C1): the validation of each option of
//! `capability Renderer = Container @build("…") @port(8080) …`, and the name
//! of the Durable Object class generated for an instance.
//!
//! These are the pure rules. Whether `<build>/Dockerfile` exists is a question
//! about the project's files, which the driver answers
//! ([`dockerfile_problem`]); the checker has no filesystem.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use bynk_project::{ParsedFile, UnitKind};
use bynk_syntax::ast::CommonsItem;
use bynk_syntax::error::CompileError;

use crate::firstparty;
use crate::project_model::ErrorSink;
use crate::symbols::UnitTable;

/// The prefix of the class generated for a `Container` instance, reserved
/// beside `__EventsFanout`: `capability Renderer = Container` is
/// `__Container_Renderer`. A user `agent` may not use it
/// (`bynk.container.class_collision`).
pub const CONTAINER_CLASS_PREFIX: &str = "__Container_";

/// The name of the Durable Object class generated for the `Container`
/// instance `capability`.
pub fn container_class_name(capability: &str) -> String {
    format!("{CONTAINER_CLASS_PREFIX}{capability}")
}

/// Why `@build`'s value is not a directory the project can name, or `None`
/// when it is: relative, inside the project (no `..`, no absolute or drive
/// path, no backslash) and not empty. A trailing `/` and `.` segments are
/// tolerated, and [`normalise_build`] removes them.
pub fn build_path_problem(path: &str) -> Option<&'static str> {
    if path.is_empty() {
        return Some("is empty");
    }
    if path.contains('\\') {
        return Some("uses `\\`; write a `/`-separated path");
    }
    if path.contains(['\0', '\n', '\r']) || path.chars().any(char::is_control) {
        return Some("holds a control character");
    }
    if path.starts_with('/') || matches!(path.as_bytes(), [d, b':', ..] if d.is_ascii_alphabetic())
    {
        return Some("is absolute; it must be relative to the project root");
    }
    if path.split('/').any(|seg| seg == "..") {
        return Some("leaves the project with `..`");
    }
    if normalise_build(path).is_empty() {
        return Some("names the project root, not a directory beneath it");
    }
    None
}

/// `@build`'s directory as a project-relative path without a trailing `/`,
/// empty segments or `.` segments (`./containers//renderer/` becomes
/// `containers/renderer`).
pub fn normalise_build(path: &str) -> String {
    path.split('/')
        .filter(|seg| !seg.is_empty() && *seg != ".")
        .collect::<Vec<_>>()
        .join("/")
}

/// The Dockerfile a `@build` directory names: `<dir>/Dockerfile`.
pub fn dockerfile_path(build: &str) -> String {
    format!("{}/Dockerfile", normalise_build(build))
}

/// Why `@port` is not a port, or `None` when it is (1 to 65535).
pub fn port_problem(value: i64) -> Option<&'static str> {
    (!(1..=65535).contains(&value)).then_some("is a TCP port, 1 to 65535")
}

/// `@sleepAfter` as whole seconds: more than zero and at most 6 hours, with no
/// part second. `None` otherwise.
pub fn sleep_after_seconds(millis: i64) -> Option<i64> {
    (millis > 0
        && millis % 1000 == 0
        && millis / 1000 <= firstparty::CONTAINER_MAX_SLEEP_AFTER_SECONDS)
        .then_some(millis / 1000)
}

/// Whether `name` is a Worker secret `@forward` may name: an identifier a
/// process reads from its environment (letters, digits and `_`, not starting
/// with a digit), at most 128 characters.
pub fn is_forwardable_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.len() <= 128
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Why a `@forward` list is not one, or `None` when it is: at most
/// [`firstparty::CONTAINER_MAX_FORWARD`] names, each a plain identifier, none
/// repeated.
pub fn forward_problem(items: &[String]) -> Option<String> {
    if items.len() > firstparty::CONTAINER_MAX_FORWARD {
        return Some(format!(
            "`@forward` lists {} names, and a container takes at most {}",
            items.len(),
            firstparty::CONTAINER_MAX_FORWARD
        ));
    }
    items.iter().enumerate().find_map(|(i, item)| {
        if !is_forwardable_name(item) {
            return Some(format!(
                "`{}` is not a name a container's environment can hold",
                item.escape_debug()
            ));
        }
        items[..i]
            .contains(item)
            .then(|| format!("`{item}` is listed twice"))
    })
}

/// The problem with the `Dockerfile` of a `@build` directory, looked up by
/// `has_file` (project-relative path to whether it is a file), or `None` when
/// it is there.
pub fn dockerfile_problem(build: &str, has_file: &dyn Fn(&str) -> bool) -> Option<String> {
    let file = dockerfile_path(build);
    (!has_file(&file)).then(|| format!("`{file}` does not exist"))
}

/// `bynk.container.build_missing`: every `Container` instance's `@build`
/// directory holds a `Dockerfile` under `project_root`. A `@build` that is not
/// a project path is already refused when the option is read
/// (`bynk.container.build_path`), so it is skipped here.
pub fn phase_container_builds(
    groups: &BTreeMap<String, Vec<usize>>,
    kinds: &BTreeMap<String, UnitKind>,
    parsed: &[ParsedFile],
    unit_tables: &HashMap<String, UnitTable>,
    project_root: &Path,
    errors: &mut ErrorSink,
) {
    let has_file = |rel: &str| project_root.join(rel).is_file();
    for (name, indices) in groups {
        if kinds.get(name) != Some(&UnitKind::Context) {
            continue;
        }
        let Some(table) = unit_tables.get(name) else {
            continue;
        };
        for &i in indices {
            if parsed[i].is_synthetic() {
                continue;
            }
            let path = parsed[i].identity_path();
            for item in parsed[i].items() {
                let CommonsItem::Capability(c) = item else {
                    continue;
                };
                let Some(build) = table
                    .derived
                    .get(&c.name.name)
                    .and_then(|d| d.container_build())
                else {
                    continue;
                };
                let Some(problem) = dockerfile_problem(build, &has_file) else {
                    continue;
                };
                errors.push_for(
                    Some(&path),
                    CompileError::new(
                        "bynk.container.build_missing",
                        c.span,
                        format!(
                            "the container `{}` has no Dockerfile: {problem}",
                            c.name.name
                        ),
                    )
                    .with_note(format!(
                        "`@build(\"{build}\")` names the directory that holds the Dockerfile, relative to the project root; create `{}`",
                        dockerfile_path(build)
                    )),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_build_path_stays_inside_the_project() {
        assert_eq!(build_path_problem("containers/renderer"), None);
        assert_eq!(build_path_problem("./containers/renderer/"), None);
        assert!(build_path_problem("").is_some());
        assert!(build_path_problem("../x").is_some());
        assert!(build_path_problem("a/../../x").is_some());
        assert!(build_path_problem("/abs").is_some());
        assert!(build_path_problem("C:/abs").is_some());
        assert_eq!(build_path_problem("a:b/c"), None);
        assert!(build_path_problem("a\\b").is_some());
        assert!(build_path_problem(".").is_some());
        assert!(build_path_problem("a\nb").is_some());
    }

    #[test]
    fn a_build_path_is_normalised() {
        assert_eq!(
            normalise_build("./containers//renderer/"),
            "containers/renderer"
        );
        assert_eq!(
            dockerfile_path("containers/renderer/"),
            "containers/renderer/Dockerfile"
        );
    }

    #[test]
    fn sleep_after_is_whole_seconds_up_to_six_hours() {
        assert_eq!(sleep_after_seconds(300_000), Some(300));
        assert_eq!(sleep_after_seconds(21_600_000), Some(21_600));
        assert_eq!(sleep_after_seconds(21_601_000), None);
        assert_eq!(sleep_after_seconds(0), None);
        assert_eq!(sleep_after_seconds(1_500), None);
    }

    #[test]
    fn forwarded_names_are_plain_identifiers() {
        assert!(forward_problem(&["API_KEY".into(), "_x1".into()]).is_none());
        assert!(forward_problem(&["1X".into()]).is_some());
        assert!(forward_problem(&["A-B".into()]).is_some());
        assert!(forward_problem(&["A".into(), "A".into()]).is_some());
        assert!(port_problem(0).is_some() && port_problem(65536).is_some());
        assert!(port_problem(8080).is_none());
    }
}

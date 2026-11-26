//! #3941: the editor surfaces of an `env(NAME[, default])` constant: hover on the
//! call, completion of the variable name, and signature help.
//!
//! A constant's initialiser is rewritten to the environment's literal before the
//! checker runs, so no index entry sits on the call; these answers are lexical,
//! read off the `const` line under the cursor, and take the project's `bynk.toml`
//! variables as an argument.

use std::collections::HashMap;
use std::path::Path;

use bynk_project::vars::DEFAULT_ENVIRONMENT;
pub use bynk_project::vars::ProjectVars;

use crate::completion::{Completion, CompletionKind};

/// The project's `bynk.toml` variables, read from disk; `None` when the tables do
/// not parse (the build says why).
pub fn read_vars(project_root: &Path) -> Option<ProjectVars> {
    bynk_project::vars::try_read_vars_with(project_root, &HashMap::new()).ok()
}

/// The variables of `manifest_text` (an open buffer), or of the project's
/// `bynk.toml` on disk when there is none.
pub fn read_vars_from(project_root: &Path, manifest_text: Option<&str>) -> Option<ProjectVars> {
    let overlay: HashMap<std::path::PathBuf, String> = manifest_text
        .map(|t| (project_root.join("bynk.toml"), t.to_string()))
        .into_iter()
        .collect();
    bynk_project::vars::try_read_vars_with(project_root, &overlay).ok()
}

/// The label signature help shows for `env`.
pub const SIGNATURE: &str = "env(NAME, default)";

/// An `env(…)` call in a `const` initialiser, found on one line.
#[derive(Debug, PartialEq, Eq)]
pub struct EnvCall {
    /// The constant's name.
    pub constant: String,
    /// The constant's written type (`String`).
    pub ty: String,
    /// The variable name, and where it starts in the buffer.
    pub variable: String,
    pub variable_start: usize,
    /// The default as written (`"https://app"`), when the call has one.
    pub default: Option<String>,
    /// Where `env` starts, and where the call's name argument ends.
    pub call_start: usize,
    pub name_end: usize,
}

/// The `env(NAME…` call on the line holding `offset`, when that line is a
/// constant whose initialiser is the call.
pub fn call_on_line(text: &str, offset: usize) -> Option<EnvCall> {
    let line_start = text.get(..offset)?.rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[offset..].find('\n').map_or(text.len(), |i| offset + i);
    let line = &text[line_start..line_end];
    let rest = line.trim_start().strip_prefix("const ")?;
    let indent = line.len() - line.trim_start().len();
    let (head, init) = rest.split_once('=')?;
    let (constant, ty) = head.split_once(':')?;
    let init_trim = init.trim_start();
    let after = init_trim.strip_prefix("env(")?;
    let call_start =
        line_start + indent + "const ".len() + head.len() + 1 + (init.len() - init_trim.len());
    let name_start_in_line = call_start - line_start + "env(".len();
    let after_ws = after.trim_start();
    let variable_start = line_start + name_start_in_line + (after.len() - after_ws.len());
    let variable: String = after_ws
        .chars()
        .take_while(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == '_')
        .collect();
    let name_end = variable_start + variable.len();
    let tail = after_ws[variable.len()..].trim_start();
    let default = tail.strip_prefix(',').map(|d| default_text(d).to_string());
    Some(EnvCall {
        constant: constant.trim().to_string(),
        ty: ty.trim().to_string(),
        variable,
        variable_start,
        default: default.filter(|d| !d.is_empty()),
        call_start,
        name_end,
    })
}

/// The default argument in `rest`, the text after the call's comma: up to the
/// call's closing `)`, skipping string literals so a `)` or `,` inside the
/// default does not end it, and not taking in what follows the call (a comment).
fn default_text(rest: &str) -> &str {
    let (mut depth, mut in_str, mut escaped) = (0usize, false, false);
    for (i, c) in rest.char_indices() {
        if in_str {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_str = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '(' | '[' | '{' => depth += 1,
            ')' if depth == 0 => return rest[..i].trim(),
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    rest.trim()
}

/// Whether `offset` is where the call's variable name is being written: inside
/// the parentheses of `env(`, before any comma, on a constant's line.
pub fn in_name_position(text: &str, offset: usize) -> bool {
    let Some(ctx) = crate::signature_help::call_context(text, offset) else {
        return false;
    };
    ctx.callee == "env" && ctx.active_param == 0 && on_const_line(text, ctx.open_paren)
}

/// Whether `offset` is inside the argument list of an `env(` call on a constant's line.
pub fn in_call(text: &str, offset: usize) -> bool {
    crate::signature_help::call_context(text, offset)
        .is_some_and(|ctx| ctx.callee == "env" && on_const_line(text, ctx.open_paren))
}

fn on_const_line(text: &str, at: usize) -> bool {
    let start = text
        .get(..at)
        .and_then(|t| t.rfind('\n'))
        .map_or(0, |i| i + 1);
    text[start..at].trim_start().starts_with("const ")
}

/// The variables `bynk.toml` defines, as completion candidates: each with the
/// default environment's value and where it is set.
pub fn variable_candidates(vars: &ProjectVars) -> Vec<Completion> {
    let mut names: Vec<&str> = vars.names().collect();
    names.sort_unstable();
    names.dedup();
    names
        .into_iter()
        .map(|name| {
            let detail = match vars.resolve(DEFAULT_ENVIRONMENT, name) {
                Some((value, origin)) => format!("{} in {}", value.shown(), origin.table()),
                None => "set in a named environment only".to_string(),
            };
            Completion::item(name, CompletionKind::Constant, Some(detail))
        })
        .collect()
}

/// The hover for the call under `offset`: the variable, and what every
/// environment resolves it to.
pub fn describe_at(text: &str, offset: usize, vars: &ProjectVars) -> Option<String> {
    let call = call_on_line(text, offset)?;
    if offset < call.call_start || offset > call.name_end || call.variable.is_empty() {
        return None;
    }
    let mut envs: Vec<&str> = vec![DEFAULT_ENVIRONMENT];
    envs.extend(
        vars.environments
            .keys()
            .map(String::as_str)
            .filter(|e| *e != DEFAULT_ENVIRONMENT),
    );
    let rows: Vec<String> = envs
        .iter()
        .map(|env| {
            let shown = match vars.resolve(env, &call.variable) {
                Some((value, origin)) => format!("`{}` from `{}`", value.shown(), origin.table()),
                None => match &call.default {
                    Some(d) => format!("`{d}` (the constant's default)"),
                    None => "**required, and not set**: the build fails".to_string(),
                },
            };
            format!("- `{env}`: {shown}")
        })
        .collect();
    Some(format!(
        "```bynk\nenv({})\n```\n\nThe `bynk.toml` variable `{}` read by `{}: {}`. It is resolved at build time, per environment (`bynk build --env <name>`):\n\n{}",
        call.variable,
        call.variable,
        call.constant,
        call.ty,
        rows.join("\n"),
    ))
}

/// The byte span of the variable name in every `const NAME: T = env(VAR…)` line
/// of `text`: what a semantic-token pass marks.
pub fn variable_spans(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        if line.trim_start().starts_with("const ")
            && let Some(call) = call_on_line(text, at)
            && !call.variable.is_empty()
        {
            out.push((call.variable_start, call.name_end));
        }
        at += line.len();
    }
    out
}

/// The variable whose name the cursor is on, with its span.
pub fn variable_at(text: &str, offset: usize) -> Option<(String, usize, usize)> {
    let call = call_on_line(text, offset)?;
    (!call.variable.is_empty() && call.variable_start <= offset && offset <= call.name_end)
        .then_some((call.variable, call.variable_start, call.name_end))
}

/// The spans of the key `name` in `bynk.toml`'s `[vars]` and `[env.<n>.vars]`
/// tables, read off the table headers and `NAME = …` lines. A key written some
/// other way (a dotted key, an inline table) is not found here.
pub fn manifest_key_spans(manifest: &str, name: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut in_vars = false;
    let mut at = 0;
    for line in manifest.split_inclusive('\n') {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            let header_text = trimmed.split('#').next().unwrap_or(trimmed);
            let header: String = header_text.chars().filter(|c| !c.is_whitespace()).collect();
            in_vars =
                header == "[vars]" || (header.starts_with("[env.") && header.ends_with(".vars]"));
        } else if in_vars {
            let indent = line.len() - line.trim_start().len();
            let rest = &line[indent..];
            if let Some(after) = rest.strip_prefix(name)
                && after.trim_start().starts_with('=')
            {
                out.push((at + indent, at + indent + name.len()));
            }
        }
        at += line.len();
    }
    out
}

/// Every edit that renames the variable `name` to `new_name`: its `env(NAME…)`
/// in the project's sources and its key in `bynk.toml`, by file.
///
/// Refused, with the reason, when `new_name` is not a variable name, is already
/// a variable (the two would merge, and could disagree on a default), or a
/// `bynk.toml` key is written in a form the scan cannot edit, which would leave
/// that environment on the old name.
pub fn plan_rename(
    name: &str,
    new_name: &str,
    sources: &std::collections::HashMap<std::path::PathBuf, String>,
    manifest: Option<(&Path, &str)>,
    vars: &ProjectVars,
) -> Result<std::collections::BTreeMap<std::path::PathBuf, Vec<(usize, usize)>>, String> {
    if new_name == name {
        return Ok(std::collections::BTreeMap::new());
    }
    if !bynk_project::vars::is_var_name(new_name) {
        return Err(format!(
            "`{new_name}` is not a variable name: capital letters, digits and `_`, starting with a capital letter"
        ));
    }
    let taken = vars.names().any(|n| n == new_name)
        || sources.values().any(|t| {
            variable_spans(t)
                .into_iter()
                .any(|(s, e)| &t[s..e] == new_name)
        });
    if taken {
        return Err(format!(
            "`{new_name}` is already a variable: renaming would merge the two"
        ));
    }
    let mut plan = std::collections::BTreeMap::new();
    for (path, text) in sources {
        let spans: Vec<(usize, usize)> = variable_spans(text)
            .into_iter()
            .filter(|(s, e)| &text[*s..*e] == name)
            .collect();
        if !spans.is_empty() {
            plan.insert(path.clone(), spans);
        }
    }
    let defined = usize::from(vars.shared.contains_key(name))
        + vars
            .environments
            .values()
            .filter(|t| t.contains_key(name))
            .count();
    if let Some((path, text)) = manifest {
        let spans = manifest_key_spans(text, name);
        if spans.len() != defined {
            return Err(format!(
                "`{name}` is set in {defined} `bynk.toml` {}, but only {} could be found to edit: it is written in a form the rename cannot edit (a quoted, dotted or inline key), so rename it by hand in `bynk.toml`, then rename the constants",
                if defined == 1 { "table" } else { "tables" },
                spans.len()
            ));
        }
        if !spans.is_empty() {
            plan.insert(path.to_path_buf(), spans);
        }
    } else if defined > 0 {
        return Err("`bynk.toml` could not be read, so its key cannot be renamed".to_string());
    }
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars() -> ProjectVars {
        use bynk_project::vars::VarValue;
        use std::collections::BTreeMap;
        let table = |pairs: &[(&str, VarValue)]| -> BTreeMap<String, VarValue> {
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_string(), v.clone()))
                .collect()
        };
        ProjectVars {
            shared: table(&[("APP_ORIGIN", VarValue::Str("https://app".into()))]),
            environments: BTreeMap::from([
                (
                    "production".to_string(),
                    table(&[
                        ("APP_ORIGIN", VarValue::Str("https://prod".into())),
                        ("ONLY_PROD", VarValue::Int(3)),
                    ]),
                ),
                ("staging".to_string(), table(&[("OTHER", VarValue::Int(1))])),
            ]),
        }
    }

    const SRC: &str = "commons config\n\nconst Origin: String = env(APP_ORIGIN, \"x\")\nconst Port: Int = env(PORT)\n";

    fn at(src: &str, needle: &str) -> usize {
        src.find(needle).unwrap()
    }

    #[test]
    fn the_call_is_read_off_its_line() {
        let c = call_on_line(SRC, at(SRC, "APP_ORIGIN")).unwrap();
        assert_eq!(c.constant, "Origin");
        assert_eq!(c.ty, "String");
        assert_eq!(c.variable, "APP_ORIGIN");
        assert_eq!(c.default.as_deref(), Some("\"x\""));
        assert_eq!(&SRC[c.variable_start..c.name_end], "APP_ORIGIN");
        let p = call_on_line(SRC, at(SRC, "PORT)")).unwrap();
        assert_eq!(p.default, None);
        // What follows the call's closing paren is not the default, and a `)`
        // or `,` inside a string is part of it.
        let src = "commons c\n\nconst A: String = env(X, \"a), b\") // fallback (x)\n";
        let c = call_on_line(src, at(src, "X,")).unwrap();
        assert_eq!(c.default.as_deref(), Some("\"a), b\""));
        let src = "commons c\n\nconst L: List[String] = env(Y, [\"a\", \"b\"]) // list\n";
        let c = call_on_line(src, at(src, "Y,")).unwrap();
        assert_eq!(c.default.as_deref(), Some("[\"a\", \"b\"]"));
    }

    #[test]
    fn hover_lists_each_environments_value_and_its_origin() {
        let v = vars();
        let h = describe_at(SRC, at(SRC, "env(APP"), &v).unwrap();
        assert!(
            h.contains("`default`: `\"https://app\"` from `[vars]`"),
            "{h}"
        );
        assert!(
            h.contains("`production`: `\"https://prod\"` from `[env.production.vars]`"),
            "{h}"
        );
        assert!(
            h.contains("`staging`: `\"https://app\"` from `[vars]`"),
            "{h}"
        );
        // No default and no value: required, and said so.
        let h = describe_at(SRC, at(SRC, "PORT)"), &v).unwrap();
        assert!(h.contains("required, and not set"), "{h}");
        // A value in a named environment only: the others fall to the default.
        let src = "commons c\n\nconst N: Int = env(ONLY_PROD, 1)\n";
        let h = describe_at(src, at(src, "ONLY_PROD"), &v).unwrap();
        assert!(h.contains("`default`: `1` (the constant's default)"), "{h}");
        assert!(
            h.contains("`production`: `3` from `[env.production.vars]`"),
            "{h}"
        );
    }

    #[test]
    fn hover_is_only_on_the_call() {
        let v = vars();
        assert!(describe_at(SRC, at(SRC, "Origin:"), &v).is_none());
        assert!(describe_at(SRC, at(SRC, "\"x\""), &v).is_none());
    }

    #[test]
    fn the_name_position_is_inside_the_parentheses_before_a_comma() {
        let src = "commons c\n\nconst A: String = env(";
        assert!(in_name_position(src, src.len()));
        let src = "commons c\n\nconst A: String = env(APP_";
        assert!(in_name_position(src, src.len()));
        let src = "commons c\n\nconst A: String = env(APP, ";
        assert!(!in_name_position(src, src.len()));
        assert!(in_call(src, src.len()));
        let src = "commons c\n\nfn a() -> String { env(";
        assert!(!in_name_position(src, src.len()));
    }

    #[test]
    fn the_variables_complete_with_their_default_value() {
        let c = variable_candidates(&vars());
        let labels: Vec<&str> = c.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, ["APP_ORIGIN", "ONLY_PROD", "OTHER"]);
        assert_eq!(c[0].detail.as_deref(), Some("\"https://app\" in [vars]"));
        assert_eq!(
            c[1].detail.as_deref(),
            Some("set in a named environment only")
        );
    }

    #[test]
    fn variable_spans_cover_every_constant_line() {
        let spans = variable_spans(SRC);
        let names: Vec<&str> = spans.iter().map(|(s, e)| &SRC[*s..*e]).collect();
        assert_eq!(names, ["APP_ORIGIN", "PORT"]);
        let (n, s, e) = variable_at(SRC, at(SRC, "APP_ORIGIN") + 3).unwrap();
        assert_eq!((n.as_str(), &SRC[s..e]), ("APP_ORIGIN", "APP_ORIGIN"));
        assert!(variable_at(SRC, at(SRC, "Origin:")).is_none());
    }

    const MANIFEST: &str = "[project]\nname = \"p\"\n\n[vars]\nAPP_ORIGIN = \"https://app\"\nOTHER_APP_ORIGIN = 1\n\n[env.production.vars]\n  APP_ORIGIN = \"https://prod\"\n\n[env.staging.vars]\nOTHER = 1\n";

    #[test]
    fn manifest_keys_are_found_in_the_vars_tables_only() {
        let spans = manifest_key_spans(MANIFEST, "APP_ORIGIN");
        assert_eq!(spans.len(), 2);
        for (s, e) in &spans {
            assert_eq!(&MANIFEST[*s..*e], "APP_ORIGIN");
        }
        assert!(manifest_key_spans("[project]\nAPP_ORIGIN = 1\n", "APP_ORIGIN").is_empty());
        // A header with a trailing comment, as the manifest docs write it.
        let commented = "[vars]   # every environment\nAPP_ORIGIN = 1\n\n[env.staging.vars] # staging\nAPP_ORIGIN = 2\n";
        assert_eq!(manifest_key_spans(commented, "APP_ORIGIN").len(), 2);
    }

    fn sources() -> std::collections::HashMap<std::path::PathBuf, String> {
        std::collections::HashMap::from([(std::path::PathBuf::from("src/c.bynk"), SRC.to_string())])
    }

    #[test]
    fn renaming_a_variable_edits_its_constants_and_every_table() {
        let plan = plan_rename(
            "APP_ORIGIN",
            "SITE_ORIGIN",
            &sources(),
            Some((Path::new("bynk.toml"), MANIFEST)),
            &vars(),
        )
        .unwrap();
        assert_eq!(plan[Path::new("src/c.bynk")].len(), 1);
        assert_eq!(plan[Path::new("bynk.toml")].len(), 2);
    }

    #[test]
    fn a_rename_that_would_merge_or_miss_a_key_is_refused() {
        let m = Some((Path::new("bynk.toml"), MANIFEST));
        // Renaming a variable to its own name is no change, not a merge.
        assert!(
            plan_rename("APP_ORIGIN", "APP_ORIGIN", &sources(), m, &vars())
                .unwrap()
                .is_empty()
        );
        let e = plan_rename("APP_ORIGIN", "lower", &sources(), m, &vars()).unwrap_err();
        assert!(e.contains("not a variable name"), "{e}");
        let e = plan_rename("APP_ORIGIN", "OTHER", &sources(), m, &vars()).unwrap_err();
        assert!(e.contains("already a variable"), "{e}");
        let e = plan_rename("APP_ORIGIN", "PORT", &sources(), m, &vars()).unwrap_err();
        assert!(e.contains("already a variable"), "{e}");
        let dotted = "[project]\nname = \"p\"\n\n[vars]\nAPP_ORIGIN = \"x\"\n\n[env.production]\nvars.APP_ORIGIN = \"y\"\n";
        let e = plan_rename(
            "APP_ORIGIN",
            "SITE_ORIGIN",
            &sources(),
            Some((Path::new("bynk.toml"), dotted)),
            &vars(),
        )
        .unwrap_err();
        assert!(e.contains("rename it by hand"), "{e}");
    }
}

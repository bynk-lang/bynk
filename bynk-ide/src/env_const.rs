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

/// The label signature help shows for `env`.
pub const SIGNATURE: &str = "env(NAME, default)";

/// An `env(…)` call in a `const` initialiser, found on one line.
#[derive(Debug, PartialEq, Eq)]
pub struct EnvCall {
    /// The constant's name.
    pub constant: String,
    /// The constant's written type (`String`), when there is one.
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
    let default = tail.strip_prefix(',').map(|d| {
        let d = d.trim();
        d.strip_suffix(')').unwrap_or(d).trim().to_string()
    });
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
}

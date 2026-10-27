//! Each tool's model rendered as compact Markdown for an agent to read — the
//! Rust twin of `vscode-bynk/src/lmRender.ts` (#2463), so an agent sees the
//! same shapes in VS Code and outside it. Models read prose and short lists
//! better than raw JSON, so every renderer emits Markdown with
//! `path:line:column` locations (workspace-relative, 1-based), and every result
//! is capped ([`cap_text`]) with a note on how to narrow the call.

use std::path::Path;

use bynk_check::wire::{JsonKind, Provenance, UncheckedReason, WireBody, WireField, WireRef};
use bynk_ide::architecture::{ArchModel, CapabilityOrigin, Located};
use bynk_ide::documentation::DocModel;
use bynk_ide::wire_contract::{BoundaryKind, Envelope, ResponseOrigin, WireContractModel};
use bynk_syntax::ast::PredKind;
use bynk_syntax::span::Span;

use super::workspace::{Project, Workspace, line_col};

/// A result's ceiling, in characters (about 3,000 tokens).
pub const MAX_CHARS: usize = 12_000;

/// Turns a project-relative file and span into `path:line:column`.
pub struct Locator<'a> {
    pub ws: &'a Workspace,
    pub project: &'a Project,
}

impl Locator<'_> {
    pub fn at(&self, file: &Path, span: Span) -> String {
        let path = self.project.display(self.ws, file);
        match self.project.text(file) {
            Some(text) => {
                let (line, col) = line_col(text, span.start);
                format!("{path}:{line}:{col}")
            }
            None => path,
        }
    }

    fn located(&self, loc: &Located) -> String {
        self.at(&loc.file, loc.span)
    }
}

/// Cut `text` to `max` characters at a line boundary, saying so and how to ask
/// for less.
pub fn cap_text(text: &str, max: usize, narrower: Option<&str>) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let limit = text.char_indices().nth(max).map_or(text.len(), |(i, _)| i);
    let cut = text[..limit]
        .rfind('\n')
        .filter(|i| *i > 0)
        .unwrap_or(limit);
    let kept = &text[..cut];
    let hint = narrower.map(|n| format!(" {n}")).unwrap_or_default();
    format!(
        "{kept}\n\n…(truncated: {} more characters.{hint})",
        text.chars().count() - kept.chars().count()
    )
}

/// One diagnostic, as `bynk_get_diagnostics` and `bynk_check` list it.
pub struct DiagnosticLine {
    pub path: String,
    pub line: usize,
    pub column: usize,
    pub severity: Severity,
    pub code: Option<String>,
    pub message: String,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
}

impl Severity {
    fn word(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
        }
    }
}

/// A diagnostics list, errors first, then by path and position.
pub fn diagnostics(list: &[DiagnosticLine], scope: &str) -> String {
    if list.is_empty() {
        return format!("No Bynk diagnostics {scope}.\n");
    }
    let mut sorted: Vec<&DiagnosticLine> = list.iter().collect();
    sorted.sort_by(|a, b| {
        (a.severity, &a.path, a.line, a.column).cmp(&(b.severity, &b.path, b.line, b.column))
    });
    let errors = list
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .count();
    let warnings = list.len() - errors;
    let mut out = format!("{errors} error(s), {warnings} warning(s) {scope}.\n\n");
    for d in &sorted {
        let code = d
            .code
            .as_deref()
            .map(|c| format!("[{c}]"))
            .unwrap_or_default();
        out.push_str(&format!(
            "- {}:{}:{} {}{code}: {}\n",
            d.path,
            d.line,
            d.column,
            d.severity.word(),
            d.message
        ));
    }
    if let Some(code) = list.iter().find_map(|d| d.code.as_deref()) {
        out.push_str(&format!(
            "\nExplain a code with `bynk_explain_diagnostic` (e.g. `{code}`).\n"
        ));
    }
    out
}

/// `bynk_explain_diagnostic`: the toolchain's own explanation of a code.
pub fn explanation(code: &str, blurb: &str, example: &str, href: &str) -> String {
    format!(
        "# `{code}`\n\n{}\n\n## Example\n\n```bynk\n{}\n```\n\nBook: {href}\n",
        blurb.trim(),
        example.trim_end()
    )
}

/// `bynk_describe_symbol`: a position's hover and where the symbol is defined.
pub fn symbol(at: &str, hovers: &[String], definitions: &[String]) -> String {
    if hovers.is_empty() && definitions.is_empty() {
        return format!(
            "Nothing is known at {at} (not on a symbol, or the file has not been analysed).\n"
        );
    }
    let mut out = format!("# Symbol at {at}\n\n");
    if !hovers.is_empty() {
        let joined = hovers
            .iter()
            .map(|h| h.trim())
            .collect::<Vec<_>>()
            .join("\n\n---\n\n");
        out.push_str(&joined);
        out.push_str("\n\n");
    }
    if !definitions.is_empty() {
        out.push_str(&format!("Defined at: {}", definitions.join(", ")));
    }
    format!("{}\n", out.trim_end())
}

/// `bynk_get_documentation`: every declaration of a unit, with its signature
/// and doc comment. `loc` is each entry's `path:line:column`, in entry order.
pub fn documentation(model: &DocModel, locs: &[String]) -> String {
    let mut out = vec![
        format!("# {} `{}`", model.unit_kind, model.unit_name),
        String::new(),
    ];
    if let Some(doc) = &model.unit_doc {
        out.push(doc.trim().to_string());
        out.push(String::new());
    }
    for (entry, loc) in model.entries.iter().zip(locs) {
        let hashes = "#".repeat((2 + entry.depth as usize).min(6));
        out.push(format!("{hashes} {} `{}` ({loc})", entry.kind, entry.name));
        out.push(String::new());
        out.push(entry.markdown.trim().to_string());
        if !entry.documented {
            out.push(String::new());
            out.push("_(no doc comment)_".to_string());
        }
        out.push(String::new());
    }
    format!("{}\n", out.join("\n").trim_end())
}

/// `bynk_get_architecture`: the project's units, what each declares, and the
/// `consumes` edges between them.
pub fn architecture(model: &ArchModel, at: &Locator<'_>) -> String {
    let mut out = vec![
        format!(
            "# Architecture: {} unit(s), {} dependency edge(s)",
            model.nodes.len(),
            model.edges.len()
        ),
        String::new(),
    ];
    let mut nodes: Vec<_> = model.nodes.iter().collect();
    nodes.sort_by(|a, b| a.name.cmp(&b.name));
    for node in nodes {
        let kind = match node.kind {
            bynk_ide::architecture::NodeKind::Context => "context",
            bynk_ide::architecture::NodeKind::Adapter => "adapter",
        };
        out.push(format!(
            "## {kind} `{}` ({})",
            node.name,
            at.located(&node.loc)
        ));
        for s in &node.services {
            out.push(format!(
                "- service `{}`: {} handler(s) ({})",
                s.name,
                s.handler_count,
                at.located(&s.loc)
            ));
        }
        for a in &node.agents {
            out.push(format!(
                "- agent `{}`: {} handler(s) ({})",
                a.name,
                a.handler_count,
                at.located(&a.loc)
            ));
        }
        for c in &node.capabilities {
            let from = match &c.origin {
                CapabilityOrigin::Local => "declared here".to_string(),
                CapabilityOrigin::Consumed { from } => format!("from `{from}`"),
            };
            out.push(format!("- capability `{}` ({from})", c.name));
        }
        for p in &node.providers {
            let ext = if p.external { ", external" } else { "" };
            out.push(format!(
                "- provides `{}` as `{}`{ext}",
                p.capability, p.provider_name
            ));
        }
        for e in model.edges.iter().filter(|e| e.from == node.name) {
            let what = if e.capabilities.is_empty() {
                String::new()
            } else {
                format!(" {{ {} }}", e.capabilities.join(", "))
            };
            out.push(format!("- consumes `{}`{what}", e.to));
        }
        out.push(String::new());
    }
    format!("{}\n", out.join("\n").trim_end())
}

fn json_word(j: JsonKind) -> &'static str {
    match j {
        JsonKind::Number => "number",
        JsonKind::String => "string",
        JsonKind::Boolean => "boolean",
        JsonKind::Object => "object",
        JsonKind::Array => "array",
        JsonKind::Null => "null",
    }
}

fn unchecked_word(r: UncheckedReason) -> &'static str {
    match r {
        UncheckedReason::Effect => "Effect",
        UncheckedReason::ValidationError => "ValidationError",
        UncheckedReason::JsonError => "JsonError",
        UncheckedReason::HttpResult => "HttpResult",
        UncheckedReason::QueueResult => "QueueResult",
        UncheckedReason::Asset => "Asset",
    }
}

/// A predicate with its arguments (`MaxLength(20)`, not just `MaxLength`).
fn pred_text(p: &PredKind) -> String {
    match p {
        PredKind::Matches(re) => format!("Matches(\"{}\")", bynk_fmt::escape_string(re)),
        PredKind::InRange(a, b) => format!("InRange({}, {})", a.value, b.value),
        PredKind::InRangeF(a, b) => format!("InRange({}, {})", a.lexeme, b.lexeme),
        PredKind::MinLength(n) => format!("MinLength({n})"),
        PredKind::MaxLength(n) => format!("MaxLength({n})"),
        PredKind::Length(n) => format!("Length({n})"),
        PredKind::NonNegative => "NonNegative".to_string(),
        PredKind::Positive => "Positive".to_string(),
        PredKind::NonEmpty => "NonEmpty".to_string(),
    }
}

fn ref_text(r: &WireRef) -> String {
    match r {
        WireRef::Base { base, .. } => base.name().to_string(),
        WireRef::Bytes => "Bytes".to_string(),
        WireRef::Named { name } => bynk_check::symbols::display_type_name(name).to_string(),
        WireRef::Inst { key } => key.clone(),
        WireRef::Unit => "()".to_string(),
        WireRef::Unchecked { reason } => {
            format!("{} (not checked at the boundary)", unchecked_word(*reason))
        }
    }
}

fn envelope_text(env: &Envelope) -> String {
    match env {
        Envelope::Empty => "no parameters".to_string(),
        Envelope::Bare { param, shape } => format!("`{param}: {}`", ref_text(shape)),
        Envelope::Keyed { params } => params
            .iter()
            .map(|(n, s)| format!("`{n}: {}`", ref_text(s)))
            .collect::<Vec<_>>()
            .join(", "),
        Envelope::Raw {
            param,
            content_type,
        } => format!("`{param}`, raw {content_type}"),
    }
}

fn field_text(f: &WireField) -> String {
    let default = f
        .default
        .as_ref()
        .map(|(e, _)| format!(" = {}", bynk_fmt::expr_to_string(e)))
        .unwrap_or_default();
    format!("`{}`: {}{default}", f.name, ref_text(&f.shape))
}

/// A boundary type's body: a scalar or sum leads on the type's line; a record
/// lists its fields beneath.
fn body_lines(body: &WireBody) -> Vec<String> {
    match body {
        WireBody::Scalar(s) => {
            let preds = if s.predicates.is_empty() {
                String::new()
            } else {
                format!(
                    " where {}",
                    s.predicates
                        .iter()
                        .map(pred_text)
                        .collect::<Vec<_>>()
                        .join(" && ")
                )
            };
            let opaque = if s.opaque { "opaque " } else { "" };
            vec![format!(
                "{opaque}{}{preds}, sent as JSON {}",
                s.base.name(),
                json_word(s.json)
            )]
        }
        WireBody::Record { fields } => fields
            .iter()
            .map(|f| format!("  - {}", field_text(f)))
            .collect(),
        WireBody::Sum(sum) => {
            let mut lines = vec![format!(
                "sum, discriminated by `{}`:",
                sum.wire_discriminant
            )];
            for v in &sum.variants {
                let payload = v
                    .payload
                    .iter()
                    .map(|f| format!("{}: {}", f.name, ref_text(&f.shape)))
                    .collect::<Vec<_>>()
                    .join(", ");
                let payload = if payload.is_empty() {
                    String::new()
                } else {
                    format!("({payload})")
                };
                lines.push(format!("  - `{}`{payload}", v.name));
            }
            lines
        }
    }
}

/// `bynk_get_wire_contract`: what a handler accepts and answers on the wire.
/// `type_locs` gives each boundary type's declaration site, by type name.
pub fn wire_contract(
    model: &WireContractModel,
    handler_at: &str,
    type_locs: &dyn Fn(&str) -> Option<String>,
) -> String {
    let route = match &model.kind {
        BoundaryKind::Http { method, path } => format!("{} {path}", method.as_str()),
        BoundaryKind::Cron { expr } => format!("cron \"{expr}\""),
        BoundaryKind::Call => "call".to_string(),
        BoundaryKind::Message => "message".to_string(),
        BoundaryKind::Open => "open".to_string(),
        BoundaryKind::Close => "close".to_string(),
        BoundaryKind::Event => "event".to_string(),
        BoundaryKind::Rejected => "rejected".to_string(),
    };
    let mut out = vec![
        format!(
            "# `{}` · service `{}` · {route} ({handler_at})",
            model.unit, model.service
        ),
        String::new(),
        format!("Request: {}.", envelope_text(&model.envelope)),
    ];
    if !model.responses.is_empty() {
        out.push(String::new());
        out.push("Responses:".to_string());
        for r in &model.responses {
            let why = match &r.origin {
                ResponseOrigin::BoundaryImplicit { why } => {
                    format!(" (added by the boundary: {why})")
                }
                ResponseOrigin::DeclaredSuccess => " (declared success)".to_string(),
                ResponseOrigin::Constructed { .. } => String::new(),
            };
            out.push(format!("- {} `{}`{why}", r.status, r.variant));
        }
    }
    if let Some(contract) = &model.contract {
        out.push(String::new());
        out.push(format!("Cross-context contract hash: `{}`.", contract.hash));
    }
    if !model.boundary.types.is_empty() {
        out.push(String::new());
        out.push("Boundary types:".to_string());
        for t in &model.boundary.types {
            let name = bynk_check::symbols::display_type_name(&t.name);
            let where_ = type_locs(name)
                .map(|l| format!(" ({l})"))
                .unwrap_or_default();
            let owner = match &t.provenance {
                Provenance::Consumed { owner_unit } => format!(", from `{owner_unit}`"),
                Provenance::Owned => String::new(),
            };
            let mut lines = body_lines(&t.body);
            let lead = if matches!(t.body, WireBody::Record { .. }) {
                "record".to_string()
            } else {
                lines.remove(0)
            };
            out.push(format!("- `{name}`{owner}{where_}: {lead}"));
            out.extend(lines);
        }
    }
    format!("{}\n", out.join("\n").trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cap_text_cuts_at_a_line_and_says_how_to_narrow() {
        let text = "aaaa\nbbbb\ncccc\n";
        assert_eq!(cap_text(text, 100, None), text);
        let capped = cap_text(text, 7, Some("Pass `file`."));
        assert!(capped.starts_with("aaaa\n\n…(truncated: "), "{capped}");
        assert!(capped.ends_with("Pass `file`.)"), "{capped}");
    }

    #[test]
    fn diagnostics_list_errors_first_and_point_at_explain() {
        let lines = vec![
            DiagnosticLine {
                path: "b.bynk".into(),
                line: 2,
                column: 3,
                severity: Severity::Warning,
                code: Some("bynk.w".into()),
                message: "careful".into(),
            },
            DiagnosticLine {
                path: "a.bynk".into(),
                line: 9,
                column: 1,
                severity: Severity::Error,
                code: Some("bynk.resolve.unknown_type".into()),
                message: "no such type".into(),
            },
        ];
        let text = diagnostics(&lines, "in the workspace");
        let first = text.lines().nth(2).unwrap();
        assert_eq!(
            first,
            "- a.bynk:9:1 error[bynk.resolve.unknown_type]: no such type"
        );
        assert!(text.starts_with("1 error(s), 1 warning(s) in the workspace."));
        assert!(text.contains("`bynk_explain_diagnostic` (e.g. `bynk.w`)"));
        assert_eq!(diagnostics(&[], "in x"), "No Bynk diagnostics in x.\n");
    }

    #[test]
    fn symbol_says_so_when_nothing_is_known() {
        assert!(symbol("a.bynk:1:1", &[], &[]).starts_with("Nothing is known at a.bynk:1:1"));
        let s = symbol(
            "a.bynk:1:1",
            &["```bynk\nx\n```".into()],
            &["a.bynk:3:6".into()],
        );
        assert!(s.contains("Defined at: a.bynk:3:6"));
    }
}

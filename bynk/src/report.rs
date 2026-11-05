//! Render a [`Report`] in one of three shapes.
//!
//! The default human table is for a person at a terminal; `--format short` (one
//! `capability: level (remedy)` line) and `--format json` are the **pinned
//! scriptable surface** (golden-tested), siblings to `bynkc check --format
//! short` (ADR 0071).

use crate::doctor::{Capability, CapabilityReport, Level, Report};

/// Output selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Format {
    #[default]
    Human,
    Short,
    Json,
}

/// The displayed status word for a capability. Optional capabilities show
/// `note` instead of `fail` when their tool is merely absent — a missing editor
/// is a note, not a failure.
fn level_word(cap: &CapabilityReport) -> &'static str {
    match cap.level {
        Level::Ok => "ok",
        Level::Warn => "warn",
        Level::Fail if cap.optional => "note",
        Level::Fail => "fail",
    }
}

pub fn render(report: &Report, format: Format) -> String {
    match format {
        Format::Human => human(report),
        Format::Short => short(report),
        Format::Json => json(report),
    }
}

fn short(report: &Report) -> String {
    let mut out = String::new();
    for cap in &report.capabilities {
        let word = level_word(cap);
        // The first non-ok row's remedy is the actionable hint for the line.
        let remedy = cap
            .rows
            .iter()
            .find(|r| r.level != Level::Ok)
            .and_then(|r| r.remedy.as_deref());
        match remedy {
            Some(r) => out.push_str(&format!("{}: {word} ({r})\n", cap.capability.token())),
            None => out.push_str(&format!("{}: {word}\n", cap.capability.token())),
        }
    }
    out
}

fn human(report: &Report) -> String {
    let mut out = String::new();
    let header = if report.is_all_ok() {
        "bynk doctor — your environment is ready".to_string()
    } else {
        "bynk doctor — environment report".to_string()
    };
    out.push_str(&header);
    out.push('\n');
    out.push_str(&format!("driver: bynk {}\n", report.driver_version));
    // Slice 7: the compiler is linked in-process. Only a `BYNK_BYNKC` override
    // points the driver at an external binary worth naming here.
    match (report.compiler.origin, report.compiler.path.as_deref()) {
        (Some(crate::compiler::Origin::Override), Some(path)) => {
            out.push_str(&format!(
                "compiler: bynkc at {} (override)\n",
                path.display()
            ));
        }
        _ => out.push_str("compiler: in-process\n"),
    }
    out.push('\n');

    for cap in &report.capabilities {
        let mark = match cap.level {
            Level::Ok => "✓",
            Level::Warn => "!",
            Level::Fail if cap.optional => "·",
            Level::Fail => "✗",
        };
        out.push_str(&format!(
            "{mark} {} [{}]{}\n",
            cap.capability.token(),
            level_word(cap),
            if cap.optional { " (optional)" } else { "" }
        ));
        for row in &cap.rows {
            out.push_str(&format!("    {} — {}\n", row.label, row.detail));
            if let Some(remedy) = &row.remedy {
                out.push_str(&format!("      ↳ fix: {remedy}\n"));
            }
        }
    }
    out
}

// The JSON surface is built from `#[derive(Serialize)]` structs (not the
// `json!` macro): serde emits struct fields in declaration order regardless of
// serde_json's `preserve_order` feature (no workspace crate enables it now) —
// a map-based value would otherwise reorder under feature unification and
// break the golden. Field order here *is* the pinned contract.
#[derive(serde::Serialize)]
struct JsonReport<'a> {
    driver: &'a str,
    compiler: JsonCompiler,
    all_ok: bool,
    capabilities: Vec<JsonCap<'a>>,
}

#[derive(serde::Serialize)]
struct JsonCompiler {
    resolved: bool,
    path: Option<String>,
    version: Option<String>,
    origin: Option<&'static str>,
    skew: Option<&'static str>,
}

#[derive(serde::Serialize)]
struct JsonCap<'a> {
    capability: &'static str,
    optional: bool,
    level: &'static str,
    rows: Vec<JsonRow<'a>>,
}

#[derive(serde::Serialize)]
struct JsonRow<'a> {
    label: &'a str,
    level: &'static str,
    detail: &'a str,
    remedy: Option<&'a str>,
}

fn json(report: &Report) -> String {
    let compiler = &report.compiler;
    let value = JsonReport {
        driver: &report.driver_version,
        compiler: JsonCompiler {
            resolved: compiler.is_resolved(),
            path: compiler.path.as_ref().map(|p| p.display().to_string()),
            version: compiler.version.map(|v| v.to_string()),
            origin: compiler.origin.map(|o| o.token()),
            skew: compiler.skew.map(|s| s.token()),
        },
        all_ok: report.is_all_ok(),
        capabilities: report
            .capabilities
            .iter()
            .map(|cap| JsonCap {
                capability: cap.capability.token(),
                optional: cap.optional,
                level: level_word(cap),
                rows: cap
                    .rows
                    .iter()
                    .map(|r| JsonRow {
                        label: &r.label,
                        level: level_token(r.level),
                        detail: &r.detail,
                        remedy: r.remedy.as_deref(),
                    })
                    .collect(),
            })
            .collect(),
    };
    // Pretty-printed and trailing-newline-terminated for a stable golden.
    let mut s = serde_json::to_string_pretty(&value).expect("Report serialises");
    s.push('\n');
    s
}

fn level_token(level: Level) -> &'static str {
    match level {
        Level::Ok => "ok",
        Level::Warn => "warn",
        Level::Fail => "fail",
    }
}

/// Parse a `--only <capability>` token (shared with the CLI layer).
pub fn parse_capability(token: &str) -> Option<Capability> {
    match token {
        "compile" => Some(Capability::Compile),
        "test" => Some(Capability::Test),
        "deploy" => Some(Capability::Deploy),
        "editor" => Some(Capability::Editor),
        "build" => Some(Capability::BuildFromSource),
        _ => None,
    }
}

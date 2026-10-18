//! Bynk project configuration (`bynk.toml`).
//!
//! Parses the project's `bynk.toml` if one exists at the project root. All
//! fields have sensible defaults so an absent or minimal config is fine.
//!
//! `[fmt]` is **not** parsed here (#972). It is read by
//! [`bynk_fmt::FmtConfig`], the same reader `bynkc fmt` / `bynk fmt` layer
//! their flags over, so the editor's format-on-save and the command line
//! cannot drift apart on what a key means. This file owns only the mapping
//! from that shared config into the server's own [`ProjectConfig`], alongside
//! the `[lsp]` section, which is the server's alone.

use std::path::Path;

use bynk_fmt::{FmtConfig, FormatOptions, IndentStyle};
use serde::Deserialize;

/// Slice A: only the sections the **server** owns. `[paths]` is deliberately
/// absent — the project's trees are the *compiler's* to resolve
/// (`bynk_ide::AnalysisRoots::Project`, overlay-aware since content-ownership
/// track #1086 slice 2), and this file re-deriving them is precisely the
/// defect slice A removes. `[project]` is
/// absent because nothing read it. `[fmt]` is absent for the same shape of
/// reason (#972): it belongs to `bynk-fmt`, which the CLI reads too.
#[derive(Debug, Deserialize, Default)]
struct RawConfig {
    #[serde(default)]
    lsp: LspSection,
}

/// #2096: `bynk.toml`'s JSON Schema describes this table from
/// `bynk_project::manifest_schema` (the schema is assembled where this crate
/// can't be reached); `lsp_section_matches_the_manifest_schema` holds the two
/// to the same keys, defaults and modes.
#[derive(Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
struct LspSection {
    #[serde(default = "default_diagnostics_mode")]
    pub diagnostics_mode: String,
    #[serde(default = "default_diagnostics_debounce_ms")]
    pub diagnostics_debounce_ms: u64,
}

impl Default for LspSection {
    fn default() -> Self {
        Self {
            diagnostics_mode: default_diagnostics_mode(),
            diagnostics_debounce_ms: default_diagnostics_debounce_ms(),
        }
    }
}

fn default_diagnostics_mode() -> String {
    "live".into()
}
fn default_diagnostics_debounce_ms() -> u64 {
    300
}

/// Effective **server** configuration with all defaults resolved.
///
/// Slice A: this no longer carries a source root. It used to reduce `[paths]
/// include` to a single `src_dir` string — dropping every root but the first
/// and ignoring `exclude` — which made the LSP analyse a different project than
/// `bynkc` compiles. The trees now come from the compiler's own discovery; what
/// is left here is what the *server* owns: formatting and the diagnostics mode.
#[derive(Debug, Clone)]
pub struct ProjectConfig {
    pub indent: IndentStyle,
    pub max_line_width: u32,
    pub trailing_comma: bool,
    pub diagnostics_mode: DiagnosticsMode,
    pub diagnostics_debounce_ms: u64,
}

impl Default for ProjectConfig {
    fn default() -> Self {
        Self {
            indent: IndentStyle::Tab,
            max_line_width: 100,
            trailing_comma: true,
            diagnostics_mode: DiagnosticsMode::Live,
            diagnostics_debounce_ms: 300,
        }
    }
}

impl ProjectConfig {
    pub fn format_options(&self) -> FormatOptions {
        FormatOptions {
            indent: self.indent,
            max_line_width: self.max_line_width,
            trailing_comma: self.trailing_comma,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticsMode {
    Live,
    OnSave,
}

/// Load `bynk.toml` from the given project root.
pub fn load_config(root: &Path) -> Option<ProjectConfig> {
    let path = root.join(bynk_fmt::MANIFEST);
    let source = std::fs::read_to_string(&path).ok()?;
    let raw: RawConfig = toml::from_str(&source).ok()?;
    // `[fmt]` through the shared reader (#972), which parses the text a second
    // time. Deliberate: the alternative is to thread a `toml::Value` across the
    // crate boundary, coupling the two readers to one representation to save a
    // parse of a file read once per project root. A malformed section leaves
    // the formatter on its defaults rather than taking the server down — it
    // cannot refuse to serve — while the CLI reports the same error loudly,
    // which is where a typo gets noticed. The rest of the config applies either
    // way (`[lsp]` is read from `raw`, above).
    let fmt = FmtConfig::from_manifest_str(&source)
        .unwrap_or_default()
        .apply(FormatOptions::default());
    let diagnostics_mode = match raw.lsp.diagnostics_mode.as_str() {
        "on_save" => DiagnosticsMode::OnSave,
        _ => DiagnosticsMode::Live,
    };
    Some(ProjectConfig {
        indent: fmt.indent,
        max_line_width: fmt.max_line_width,
        trailing_comma: fmt.trailing_comma,
        diagnostics_mode,
        diagnostics_debounce_ms: raw.lsp.diagnostics_debounce_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `bynk.toml` in a fresh directory, returning the root.
    ///
    /// `std::env::temp_dir`, not `CARGO_TARGET_TMPDIR` — the latter is defined
    /// only for integration tests, and these are lib unit tests.
    fn project(name: &str, manifest: &str) -> std::path::PathBuf {
        // The pid keeps concurrent checkouts (or two users on a shared runner)
        // from racing on one fixed path under the shared temp directory.
        let dir =
            std::env::temp_dir().join(format!("bynk-lsp-project-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join(bynk_fmt::MANIFEST), manifest).expect("write");
        dir
    }

    #[test]
    fn the_fmt_section_still_reaches_format_on_save() {
        // #972 moved this parse out to `bynk-fmt`; format-on-save must still
        // apply what the project asked for.
        let root = project(
            "lsp-fmt-section",
            "[project]\nname = \"x\"\n\n[fmt]\nindent = \"spaces\"\nindent_width = 4\nmax_line_width = 120\ntrailing_comma = false\n",
        );
        let opts = load_config(&root).expect("config loads").format_options();
        assert_eq!(opts.indent, IndentStyle::Spaces(4));
        assert_eq!(opts.max_line_width, 120);
        assert!(!opts.trailing_comma);
    }

    #[test]
    fn the_editor_and_the_cli_resolve_a_manifest_identically() {
        // The property the shared reader exists to hold. Not circular: this
        // compares the *server's* resolved options against what the CLI's
        // layering produces from the same file with no flags in play.
        let manifest = "[fmt]\nindent = \"spaces\"\nindent_width = 3\nmax_line_width = 60\n";
        let root = project("lsp-fmt-parity", manifest);
        let editor = load_config(&root).expect("config loads").format_options();
        let cli_text =
            std::fs::read_to_string(root.join(bynk_fmt::MANIFEST)).expect("manifest reads");
        let cli = bynk_fmt::FmtConfig::from_manifest_str(&cli_text)
            .expect("manifest parses")
            .apply(FormatOptions::default());
        assert_eq!(editor, cli, "format-on-save and `fmt` must agree");
    }

    #[test]
    fn lsp_section_matches_the_manifest_schema() {
        // #2096: the schema's `[lsp]` table is written in `bynk-project`, apart
        // from this reader, so pin it here: the same keys, the defaults this
        // reader falls back to, and a mode for each value the schema offers.
        let mut generator = schemars::generate::SchemaSettings::draft07()
            .with(|s| s.inline_subschemas = true)
            .into_generator();
        let schema = bynk_project::manifest_schema::table_schema("lsp", &mut generator)
            .expect("the schema has an `[lsp]` table")
            .to_value();
        let ours = schemars::schema_for!(LspSection).to_value();
        let names = |s: &serde_json::Value| -> Vec<String> {
            let mut k: Vec<String> = s["properties"]
                .as_object()
                .expect("properties")
                .keys()
                .cloned()
                .collect();
            k.sort();
            k
        };
        assert_eq!(names(&schema), names(&ours), "`[lsp]` keys");

        let defaults = LspSection::default();
        let properties = &schema["properties"];
        assert_eq!(
            properties["diagnostics_mode"]["default"],
            defaults.diagnostics_mode
        );
        assert_eq!(
            properties["diagnostics_debounce_ms"]["default"],
            defaults.diagnostics_debounce_ms
        );

        let modes = properties["diagnostics_mode"]["enum"]
            .as_array()
            .expect("`diagnostics_mode` is an enum");
        let read: Vec<DiagnosticsMode> = modes
            .iter()
            .map(|mode| {
                let mode = mode.as_str().expect("a string");
                let root = project(
                    &format!("lsp-schema-{mode}"),
                    &format!("[lsp]\ndiagnostics_mode = \"{mode}\"\n"),
                );
                load_config(&root).expect("config loads").diagnostics_mode
            })
            .collect();
        assert_eq!(read, [DiagnosticsMode::Live, DiagnosticsMode::OnSave]);
    }

    #[test]
    fn an_absent_fmt_section_leaves_the_canonical_style() {
        let root = project("lsp-fmt-absent", "[project]\nname = \"x\"\n");
        let opts = load_config(&root).expect("config loads").format_options();
        assert_eq!(opts, FormatOptions::default());
    }

    #[test]
    fn a_broken_fmt_section_does_not_take_the_rest_of_the_config_with_it() {
        // The server keeps serving on a bad `[fmt]`; the CLI is where the same
        // typo is reported loudly.
        let root = project(
            "lsp-fmt-broken",
            "[fmt]\nindent = \"tabs\"\n\n[lsp]\ndiagnostics_debounce_ms = 900\n",
        );
        let config = load_config(&root).expect("config loads");
        assert_eq!(config.format_options(), FormatOptions::default());
        assert_eq!(config.diagnostics_debounce_ms, 900, "[lsp] still applies");
    }
}

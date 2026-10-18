//! The `[fmt]` section of a project's `bynk.toml`.
//!
//! One reader, two consumers (#972). The language server has read `[fmt]` for
//! format-on-save since v0.3; the `bynkc fmt` / `bynk fmt` CLI did not, so a
//! project that set a style there had the editor and the command line disagree
//! — and `bynk fmt --check` in CI gated on a style the editor never produced.
//! Wiring the CLI to a *second* parser of the same section would have made that
//! two implementations to keep in step, so the parser lives here, beside the
//! [`FormatOptions`] it produces, and both front-ends call it.
//!
//! Every key is optional, and reading yields a [`FmtConfig`] of `Option`s
//! rather than a filled-in [`FormatOptions`]. That is what lets a caller layer
//! the sources in the right order — spec default, then `bynk.toml`, then an
//! explicit CLI flag — with an absent key meaning "defer", not "reset to
//! default".

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::fmt::{FormatOptions, IndentStyle};

/// The manifest file a project is rooted by.
pub const MANIFEST: &str = "bynk.toml";

/// A `[fmt]` section, as read. Each field is `Some` only when the manifest
/// actually stated it, so [`FmtConfig::apply`] can leave the rest alone.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FmtConfig {
    pub indent: Option<IndentStyle>,
    pub max_line_width: Option<u32>,
    pub trailing_comma: Option<bool>,
}

/// Why a `bynk.toml` could not be turned into a [`FmtConfig`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// The file could not be read.
    Read(String),
    /// The file is not valid TOML, or `[fmt]` has a key of the wrong type.
    Parse(String),
    /// `[fmt] indent` is neither `"tab"` nor `"spaces"`.
    Indent(String),
    /// `[fmt] max_line_width` is zero — no width at all is not a width.
    MaxLineWidth(u32),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read(e) => write!(f, "{e}"),
            Self::Parse(e) => write!(f, "{e}"),
            Self::Indent(found) => write!(
                f,
                "`[fmt] indent` must be \"tab\" or \"spaces\", found \"{found}\""
            ),
            Self::MaxLineWidth(n) => {
                write!(f, "`[fmt] max_line_width` must be at least 1, found {n}")
            }
        }
    }
}

impl std::error::Error for ConfigError {}

/// The raw shape serde reads. Separate from [`FmtConfig`] because the manifest
/// spells indentation as two keys (`indent` + `indent_width`) while the
/// formatter models it as one [`IndentStyle`], and because `deny_unknown_fields`
/// on the section catches a typo (`max_line_length`) that would otherwise be
/// silently ignored.
#[derive(Debug, Deserialize, Default)]
struct RawManifest {
    #[serde(default)]
    fmt: RawFmt,
}

/// `deny_unknown_fields` on the *section* (not the manifest): a `[fmt]` key
/// that is not one of these four is a typo, and the failure it otherwise
/// produces — `max_line_length = 120` sitting in a manifest for months while
/// the formatter quietly uses 100 — is exactly what a config layer must not
/// do. Pre-1.0 this trades forward compatibility for that, deliberately: a
/// manifest naming a key an older binary lacks is refused rather than
/// half-applied.
///
/// #2096: with the `schema` feature this struct is also the source of the
/// `[fmt]` table in `bynk.toml`'s JSON Schema ([`fmt_table_schema`]), so the
/// field doc comments below are what an editor shows on hover. The defaults
/// they state are pinned to [`FormatOptions::default`] by `xtask`'s schema test.
#[derive(Debug, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    feature = "schema",
    schemars(
        description = "Formatter settings, read by format-on-save in the editor and by \
        `bynkc fmt` / `bynk fmt`, through one shared reader. A flag passed to `fmt` \
        overrides the key it names."
    )
)]
#[serde(deny_unknown_fields)]
struct RawFmt {
    /// Indentation style: `"tab"` (one tab per nesting level) or `"spaces"`
    /// (`indent_width` spaces per level). Default: `"tab"`.
    #[cfg_attr(
        feature = "schema",
        schemars(extend("enum" = ["tab", "spaces"], "default" = "tab"))
    )]
    indent: Option<String>,
    /// Spaces per nesting level. Only consulted when `indent = "spaces"`, where
    /// it defaults to `2`.
    #[cfg_attr(feature = "schema", schemars(extend("default" = 2)))]
    indent_width: Option<u8>,
    /// Soft target line width. A construct wider than this wraps across lines
    /// where the grammar allows. Must be at least 1. Default: `100`.
    #[cfg_attr(
        feature = "schema",
        schemars(range(min = 1), extend("default" = 100))
    )]
    max_line_width: Option<u32>,
    /// Emit trailing commas in multi-line records, sums, list literals and
    /// `exports` clauses. Default: `true`.
    #[cfg_attr(feature = "schema", schemars(extend("default" = true)))]
    trailing_comma: Option<bool>,
}

/// #2096: the JSON Schema of `bynk.toml`'s `[fmt]` table, derived from the
/// struct that reads it. `xtask manifest-schema` places it in the whole
/// manifest's schema beside `bynk-project`'s tables.
#[cfg(feature = "schema")]
pub fn fmt_table_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    generator.subschema_for::<RawFmt>()
}

impl FmtConfig {
    /// Read a `[fmt]` section out of a `bynk.toml`'s text. A manifest with no
    /// `[fmt]` section is not an error — it yields an empty config, which
    /// defers every field.
    pub fn from_manifest_str(text: &str) -> Result<Self, ConfigError> {
        // Only `[fmt]` is read here; the manifest's other sections belong to
        // other readers, so unknown *top-level* keys must stay tolerated.
        let raw: RawManifest =
            toml::from_str(text).map_err(|e| ConfigError::Parse(e.to_string()))?;
        let indent = match raw.fmt.indent.as_deref() {
            None => None,
            Some("tab") => Some(IndentStyle::Tab),
            // `indent_width` alongside `indent = "tab"` is ignored rather than
            // refused: a manifest is declarative and the key is commonly left
            // behind when a project switches back to tabs. (The CLI's
            // `--indent-width` *is* refused with `--indent tab` — passing a
            // flag is a deliberate act in a way that a stale file key is not.)
            Some("spaces") => Some(IndentStyle::Spaces(raw.fmt.indent_width.unwrap_or(2))),
            Some(other) => return Err(ConfigError::Indent(other.to_string())),
        };
        if let Some(0) = raw.fmt.max_line_width {
            return Err(ConfigError::MaxLineWidth(0));
        }
        Ok(Self {
            indent,
            max_line_width: raw.fmt.max_line_width,
            trailing_comma: raw.fmt.trailing_comma,
        })
    }

    /// Layer this config over `base`: a field the manifest stated wins, one it
    /// omitted leaves `base` untouched.
    ///
    /// `indent_width` is carried across when the manifest names `spaces`
    /// without a width *and* `base` already holds one, so a CLI `--indent
    /// spaces` over a manifest `indent_width = 4` lands on four spaces rather
    /// than silently resetting to two.
    pub fn apply(&self, base: FormatOptions) -> FormatOptions {
        FormatOptions {
            indent: self.indent.unwrap_or(base.indent),
            max_line_width: self.max_line_width.unwrap_or(base.max_line_width),
            trailing_comma: self.trailing_comma.unwrap_or(base.trailing_comma),
        }
    }
}

/// The nearest `bynk.toml` at or above `start`, or `None` when the walk reaches
/// the filesystem root without finding one.
///
/// The search starts at `start` itself when it is a directory, otherwise at its
/// parent — so a caller can hand over either a project root or the source file
/// being formatted. Formatting a file resolves the manifest that governs *that
/// file*, not the one the shell happens to be standing in, which is what makes
/// `bynk fmt ../other-project/src/x.bynk` obey the other project's style.
pub fn find_manifest(start: &Path) -> Option<PathBuf> {
    let mut dir = if start.is_dir() {
        Some(start)
    } else {
        start.parent()
    };
    while let Some(d) = dir {
        let candidate = d.join(MANIFEST);
        if candidate.is_file() {
            return Some(candidate);
        }
        dir = d.parent();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absent_fmt_section_defers_every_field() {
        let cfg = FmtConfig::from_manifest_str("[project]\nname = \"x\"\n").expect("parses");
        assert_eq!(cfg, FmtConfig::default());
        // Deferring means `apply` is the identity over any base.
        let base = FormatOptions {
            indent: IndentStyle::Spaces(3),
            max_line_width: 77,
            trailing_comma: false,
        };
        let out = cfg.apply(base);
        assert_eq!(out.indent, base.indent);
        assert_eq!(out.max_line_width, base.max_line_width);
        assert_eq!(out.trailing_comma, base.trailing_comma);
    }

    #[test]
    fn a_partial_section_overrides_only_what_it_states() {
        let cfg = FmtConfig::from_manifest_str("[fmt]\nmax_line_width = 120\n").expect("parses");
        let out = cfg.apply(FormatOptions::default());
        assert_eq!(out.max_line_width, 120);
        // Untouched fields keep the base's values.
        assert_eq!(out.indent, IndentStyle::Tab);
        assert!(out.trailing_comma);
    }

    #[test]
    fn spaces_takes_indent_width_and_defaults_to_two() {
        let four = FmtConfig::from_manifest_str("[fmt]\nindent = \"spaces\"\nindent_width = 4\n")
            .expect("parses");
        assert_eq!(four.indent, Some(IndentStyle::Spaces(4)));
        let bare = FmtConfig::from_manifest_str("[fmt]\nindent = \"spaces\"\n").expect("parses");
        assert_eq!(bare.indent, Some(IndentStyle::Spaces(2)));
    }

    #[test]
    fn indent_width_beside_tab_is_ignored_not_refused() {
        // Declarative, and commonly left behind when a project switches back.
        let cfg = FmtConfig::from_manifest_str("[fmt]\nindent = \"tab\"\nindent_width = 4\n")
            .expect("parses");
        assert_eq!(cfg.indent, Some(IndentStyle::Tab));
    }

    #[test]
    fn an_unknown_indent_word_is_an_error() {
        let err = FmtConfig::from_manifest_str("[fmt]\nindent = \"tabs\"\n").expect_err("refused");
        assert_eq!(err, ConfigError::Indent("tabs".into()));
        assert!(err.to_string().contains("\"tab\" or \"spaces\""), "{err}");
    }

    #[test]
    fn a_zero_max_line_width_is_an_error() {
        let err = FmtConfig::from_manifest_str("[fmt]\nmax_line_width = 0\n").expect_err("refused");
        assert_eq!(err, ConfigError::MaxLineWidth(0));
    }

    #[test]
    fn a_misspelled_fmt_key_is_an_error_not_a_silent_no_op() {
        // The failure this catches: `max_line_length = 120` sitting in a
        // manifest for months, formatting at 100 the whole time.
        let err =
            FmtConfig::from_manifest_str("[fmt]\nmax_line_length = 120\n").expect_err("refused");
        assert!(
            matches!(err, ConfigError::Parse(_)),
            "expected a parse error, got {err:?}"
        );
    }

    #[test]
    fn other_manifest_sections_are_left_to_their_own_readers() {
        // `[paths]` belongs to `bynk-emit::project`; reading `[fmt]` must not
        // reject a manifest for carrying it.
        let cfg = FmtConfig::from_manifest_str(
            "[project]\nname = \"x\"\n\n[paths]\ninclude = [\"src\"]\n\n[lsp]\ndiagnostics_mode = \"live\"\n\n[fmt]\ntrailing_comma = false\n",
        )
        .expect("parses");
        assert_eq!(cfg.trailing_comma, Some(false));
    }

    #[test]
    fn malformed_toml_is_reported_not_ignored() {
        let err = FmtConfig::from_manifest_str("[fmt\nindent = \"tab\"\n").expect_err("refused");
        assert!(matches!(err, ConfigError::Parse(_)), "{err:?}");
    }
}

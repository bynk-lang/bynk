//! The pure binding-index queries live in [`bynk_ide::index_queries`] (#2520),
//! where the hover ladder shared with `bynk mcp` reads them; this module keeps
//! the old path working and adds the one query that builds `lsp_types` values.
pub use bynk_ide::index_queries::*;

pub use crate::semantic_tokens::{semantic_tokens, semantic_tokens_legend};

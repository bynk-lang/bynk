//! `bynk-lsp` — the Bynk Language Server binary.
//!
//! Slice C (the `[lib]` seam): the server implementation lives in the crate's
//! library ([`bynk_lsp`]); this binary is a thin entry point so integration
//! tests can `use bynk_lsp::…` rather than `#[path]`-include source modules.

#[tokio::main]
async fn main() {
    let code = bynk_lsp::run().await;
    // #1667: exit here rather than returning, so a stdin read still pending
    // on a blocking thread can't hold the process open past `exit`.
    std::process::exit(if code == std::process::ExitCode::SUCCESS {
        0
    } else {
        1
    });
}

// bynkc — the final release of the `bynkc` crate (#3013, track E3).
//
// `bynkc` is now `bynk` under another name. This shim exists so `cargo install
// bynkc` still installs a working `bynkc` for one release; the crate is not
// published again after that. Use `cargo install bynk`.

use std::process::ExitCode;

// The stack `bynk` itself runs on (its `main.rs` says why): clap builds the whole
// command tree on parsing.
const STACK_BYTES: usize = 16 * 1024 * 1024;

fn main() -> ExitCode {
    match std::thread::Builder::new()
        .stack_size(STACK_BYTES)
        .spawn(bynk::bynkc_compat::run)
        .map(std::thread::JoinHandle::join)
    {
        Ok(Ok(code)) => code,
        Ok(Err(_)) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("bynkc: could not start: {e}");
            ExitCode::FAILURE
        }
    }
}

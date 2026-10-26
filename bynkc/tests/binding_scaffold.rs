//! Every first-party binding passes `tsc --strict` on every platform it ships
//! for, beside the whole standard library compiled for that platform
//! (standard-library S2, #1952).
//!
//! The bindings import the modules the compiler generates from the first-party
//! sources, so they cannot be checked on their own. Before this suite they were
//! checked only inside fixtures that happened to consume them (ADR 0086 left
//! "their standalone scaffold" as a follow-up). Here the project they sit in is
//! built for the purpose: `stdlib_scaffold` consumes every capability and uses
//! every commons the `bynk-std` registry lists for the platform, so a unit added
//! to the registry is type-checked without a fixture.
//!
//! Skips loudly without a TypeScript toolchain; `BYNK_REQUIRE_TSC=1` turns the
//! skip into a failure (CI).

mod require;
mod stdlib_scaffold;
mod tools;

use std::fs;

const REQUIRE_ENV: &str = "BYNK_REQUIRE_TSC";

#[test]
fn every_binding_passes_tsc_strict_on_every_platform() {
    let Some(_) = tools::tsc_command() else {
        tools::skip(
            "BINDING SCAFFOLD (tsc)",
            require::is_required(REQUIRE_ENV),
            REQUIRE_ENV,
            "no TypeScript toolchain (`tsc` or `npx`) is on PATH",
        );
        return;
    };
    let dir = stdlib_scaffold::scratch("binding-scaffold");
    for platform in stdlib_scaffold::PLATFORMS {
        let out = stdlib_scaffold::compile(platform, &dir, false);
        for (path, source) in stdlib_scaffold::bindings(platform) {
            let emitted = fs::read_to_string(out.join(path)).unwrap_or_else(|e| {
                panic!(
                    "[{}] the scaffold does not ship {path}: {e}",
                    platform.as_str()
                )
            });
            // The compiler rewrites a binding's runtime import to per-module
            // imports (ADR 0615); everything else is the registry's text.
            assert_eq!(
                emitted,
                bynk_emit::emitter::rewrite_runtime_imports(source),
                "[{}] {path} is not the registry's binding, verbatim",
                platform.as_str()
            );
        }
        let checked = tools::tsc_command()
            .expect("found above")
            .args(["--strict", "--noEmit", "-p"])
            .arg(out.join("tsconfig.json"))
            .output()
            .expect("run tsc");
        assert!(
            checked.status.success(),
            "[{}] the scaffold, with its bindings, fails tsc --strict:\n{}{}",
            platform.as_str(),
            String::from_utf8_lossy(&checked.stdout),
            String::from_utf8_lossy(&checked.stderr)
        );
    }
    let _ = fs::remove_dir_all(&dir);
}

/// The scaffold reaches every registry unit available on each platform, and a
/// platform-native unit only on its own platform.
#[test]
fn the_scaffold_covers_the_registry() {
    use bynk_check::firstparty::{Platform, UNITS};
    for platform in stdlib_scaffold::PLATFORMS {
        let source = stdlib_scaffold::source(platform);
        for unit in UNITS {
            let available = unit.platform.is_none_or(|p| p == platform);
            let named = source.lines().any(|l| {
                l == format!("uses {}", unit.name)
                    || l.starts_with(&format!("consumes {} {{", unit.name))
            });
            assert_eq!(
                named,
                available,
                "[{}] {} is {} the scaffold",
                platform.as_str(),
                unit.name,
                if available {
                    "missing from"
                } else {
                    "wrongly in"
                }
            );
        }
    }
    assert!(
        stdlib_scaffold::bindings(Platform::Cloudflare)
            .iter()
            .any(|(p, _)| stdlib_scaffold::stem(p) == "cloudflare.binding"),
        "the Cloudflare scaffold ships the platform adapter's binding"
    );
}

//! Spawning wrangler (back-half paydown 1D.3a, #4043 Decision J): the one
//! place a wrangler `Command` is built.
//!
//! An installed wrangler (on `PATH`, or the project's own
//! `node_modules/.bin`) runs directly. A provisionable one goes through the
//! package manager's runner (`npx --yes -p wrangler@4 wrangler`, `pnpm dlx`,
//! `bunx`), pinned to [`super::pins::WRANGLER_SPEC`]. A missing one gives
//! `None`.

use std::process::Command;

use crate::probe::Provenance;

/// `wrangler <subcommand>` for `provenance`, or `None` when wrangler is
/// missing.
pub fn command(provenance: &Provenance, subcommand: &str) -> Option<Command> {
    match provenance {
        Provenance::Path(p) | Provenance::ProjectLocal(p) => {
            let mut cmd = Command::new(p);
            cmd.arg(subcommand);
            Some(cmd)
        }
        Provenance::Runner(manager) => {
            // Yarn classic has no runner, so a probe never reports it; npx is
            // the historical default should one arrive.
            let program = manager.runner_program().unwrap_or("npx");
            // #1758: the resolved runner, so Windows runs its `.cmd` shim.
            let mut cmd = Command::new(crate::probe::program_path(program));
            // #524: pinned provisioning, per the repo's runner convention: an
            // unpinned `wrangler` here meant the dev server could drift from
            // the wrangler the tests and deploys run.
            cmd.args(manager.runner_args(super::pins::WRANGLER_SPEC, "wrangler"))
                .arg(subcommand);
            Some(cmd)
        }
        Provenance::Missing => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::npm_install::PackageManager;
    use std::path::PathBuf;

    fn argv(cmd: &Command) -> Vec<String> {
        cmd.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn an_installed_wrangler_runs_directly() {
        let cmd = command(&Provenance::Path(PathBuf::from("/bin/wrangler")), "kv").unwrap();
        assert_eq!(cmd.get_program(), "/bin/wrangler");
        assert_eq!(argv(&cmd), ["kv"]);
    }

    #[test]
    fn a_runner_provisions_the_pinned_spec() {
        let cmd = command(&Provenance::Runner(PackageManager::Npm), "dev").unwrap();
        assert_eq!(argv(&cmd), ["--yes", "-p", "wrangler@4", "wrangler", "dev"]);
    }

    #[test]
    fn a_missing_wrangler_has_no_command() {
        assert!(command(&Provenance::Missing, "dev").is_none());
    }
}

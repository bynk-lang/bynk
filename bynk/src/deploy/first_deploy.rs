//! #3562 (G4-2): wrangler's `[code: 10007]` after a first upload.
//!
//! `wrangler deploy` uploads a Worker and then looks it up. On the first deploy
//! of a name it can answer that lookup `This Worker does not exist on your
//! account [code: 10007]` and exit non-zero after the upload has landed: a race
//! between the script's creation and the lookup (seen on the scratch account
//! with `--secrets-file`, straight after the name was deleted; the same deploys
//! succeeded when run again). The upload is not what failed, so the run is not
//! failed by it.
//!
//! Never on wrangler's word alone. A same-named script can predate the deploy
//! (another project, a lost ledger), so the failure is forgiven only when
//!
//! 1. it is a first deploy (the ledger has no record of the Worker),
//! 2. wrangler's own output carries `[code: 10007]`,
//! 3. the account had **no** script of that name before the push, and
//! 4. it has **exactly one** version after it.
//!
//! The last two need no clock: Cloudflare's own state before and after says the
//! upload that failed to be confirmed is the one that created the script. If the
//! account cannot be asked (no API token, an unreadable answer), nothing is
//! forgiven and the failure stays as wrangler reported it.

use bynk_driver::emsg;
use std::cell::Cell;
use std::path::Path;

use crate::cloudflare::Rest;
use crate::cloudflare::{CloudflareApi, Exit, Failure};

/// What the account says about one script's versions.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ScriptState {
    Absent,
    /// The script exists with this many versions.
    Present(usize),
    /// It could not be told (network, permissions): what was said.
    Unknown(crate::cloudflare::CloudflareError),
}

/// The account's record of a script's versions.
pub(crate) trait ScriptVersions {
    fn state(&self, script: &str) -> ScriptState;
}

/// [`ScriptVersions`] over Cloudflare's REST API.
struct RestVersions(Rest);

impl ScriptVersions for RestVersions {
    fn state(&self, script: &str) -> ScriptState {
        match self.0.get(&format!("/workers/scripts/{script}/versions")) {
            Ok(body) => ScriptState::Present(
                body["result"]["items"]
                    .as_array()
                    .map_or(0, std::vec::Vec::len),
            ),
            // Cloudflare's own answer, by status or by code, never a bare number that
            // an id or a message could happen to contain.
            Err(said) if is_absent(&said) => ScriptState::Absent,
            Err(said) => ScriptState::Unknown(said.into()),
        }
    }
}

/// Whether a versions lookup's failure says the script does not exist: a 404, or Cloudflare's
/// code 10007 ("This Worker does not exist"). Anything else (auth, network) is not assumed.
fn is_absent(said: &crate::cloudflare::rest::RestError) -> bool {
    said.status() == Some(404) || said.has_code(10007)
}

/// Whether a failed first push was in fact the upload that created the Worker:
/// the rule in the module doc, pure so it is tested without wrangler or a network.
pub(crate) fn upload_survived(said: &str, before: &ScriptState, after: &ScriptState) -> bool {
    crate::cloudflare::push_lost_its_upload(said)
        && *before == ScriptState::Absent
        && *after == ScriptState::Present(1)
}

/// A first push being watched: the script's name, and that it did not exist
/// before.
pub(crate) struct Watch {
    script: String,
    scripts: Box<dyn ScriptVersions>,
    /// Whether a failure was forgiven, so the steps wrangler skipped can be run.
    forgave: Cell<bool>,
}

impl Watch {
    /// Start watching a first push of the Worker whose generated configuration is
    /// in `worker_dir`, in `environment`. `None` when the account cannot be asked,
    /// or already has a script of that name: then a failure is never forgiven.
    pub(crate) fn begin(worker_dir: &Path, environment: &str) -> Option<Self> {
        let scripts = RestVersions(
            crate::cloudflare::rest::for_purpose(
                "confirm a first upload",
                "an API token with \"Workers Scripts: Read\"",
            )
            .ok()?,
        );
        Self::with(worker_dir, environment, Box::new(scripts))
    }

    fn with(
        worker_dir: &Path,
        environment: &str,
        scripts: Box<dyn ScriptVersions>,
    ) -> Option<Self> {
        // The script's real name is the configuration's `name`, which a group's
        // `name` makes differ from its directory.
        let text = std::fs::read_to_string(worker_dir.join("wrangler.toml")).ok()?;
        let name = text
            .parse::<toml::Table>()
            .ok()?
            .get("name")?
            .as_str()?
            .to_string();
        let script = super::env_qualify(environment, &name);
        (scripts.state(&script) == ScriptState::Absent).then_some(Self {
            script,
            scripts,
            forgave: Cell::new(false),
        })
    }

    /// Whether a failed push, which said `said`, is forgiven.
    pub(crate) fn forgives(&self, said: &str) -> bool {
        upload_survived(
            said,
            &ScriptState::Absent,
            &self.scripts.state(&self.script),
        )
    }

    /// The status to act on: a success if `status` failed but the upload landed.
    pub(crate) fn settle(&self, status: Exit, said: &str) -> Exit {
        if status.success() || !self.forgives(said) {
            return status;
        }
        emsg!(
            "bynk: warning: wrangler could not look up `{}` after uploading it ([code: 10007], a race with the script's creation); the account shows the upload landed as the Worker's first version, so the deploy continues",
            self.script
        );
        self.forgave.set(true);
        Exit::Code(0)
    }

    /// Whether a failure was forgiven. wrangler's lookup that answers `10007` comes
    /// after the upload and **before** the triggers step (workers.dev, routes,
    /// custom domains, crons), which it therefore never ran.
    pub(crate) fn forgave(&self) -> bool {
        self.forgave.get()
    }
}

/// Run `wrangler triggers deploy` for a Worker whose `deploy` was forgiven: the
/// step wrangler never reached (workers.dev, routes, custom domains, crons). A
/// failure here is the deploy's failure, since the Worker would be live without
/// them. `Err(None)` when there is no wrangler to run.
pub(crate) fn deploy_triggers(
    api: &dyn CloudflareApi,
    worker_dir: &Path,
    environment: &str,
) -> Result<(), Option<String>> {
    api.triggers_deploy(worker_dir, environment)
        .map_err(|e| match e.failure() {
            Failure::Missing => None,
            Failure::Exit { status, .. } => {
                Some(format!("`wrangler triggers deploy` exited {status}"))
            }
            _ => Some(format!("could not run `wrangler triggers deploy`: {e}")),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAID: &str = "Uploaded revertprobe-core (0.86 sec)\n✘ [ERROR] A request to the Cloudflare API failed.\n  This Worker does not exist on your account. [code: 10007]";

    #[test]
    fn a_failure_is_forgiven_only_when_the_upload_created_the_script() {
        let one = ScriptState::Present(1);
        assert!(upload_survived(SAID, &ScriptState::Absent, &one));
        // The script was there before: it is not this upload's to claim.
        assert!(!upload_survived(SAID, &one, &one));
        // More than one version: a predecessor exists, so this is not the first.
        assert!(!upload_survived(
            SAID,
            &ScriptState::Absent,
            &ScriptState::Present(2)
        ));
        // Nothing was created: the upload did not land.
        assert!(!upload_survived(
            SAID,
            &ScriptState::Absent,
            &ScriptState::Absent
        ));
        // The account could not be asked: nothing is assumed.
        assert!(!upload_survived(
            SAID,
            &ScriptState::Absent,
            &ScriptState::Unknown("network".into())
        ));
        assert!(!upload_survived(
            SAID,
            &ScriptState::Unknown("network".into()),
            &one
        ));
        // Any other failure is not this one, whatever the account says.
        assert!(!upload_survived(
            "Service binding 'X' references Worker 'y' which was not found [code: 10143]",
            &ScriptState::Absent,
            &one
        ));
    }

    /// A script that answers as scripted: before, then after.
    struct Scripted(std::cell::RefCell<Vec<ScriptState>>);
    impl ScriptVersions for Scripted {
        fn state(&self, _: &str) -> ScriptState {
            self.0.borrow_mut().remove(0)
        }
    }

    fn dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("bynk-first-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("wrangler.toml"), "name = \"revertprobe-core\"\n").unwrap();
        dir
    }

    /// The watch asks about the script's configured name (a group's `name`, not its
    /// directory), qualified for the environment, and only starts if it is absent.
    #[test]
    fn a_watch_starts_only_for_a_script_the_account_does_not_have() {
        let dir = dir("watch");
        let absent_then_one = || {
            Scripted(std::cell::RefCell::new(vec![
                ScriptState::Absent,
                ScriptState::Present(1),
            ]))
        };
        let watch = Watch::with(&dir, "default", Box::new(absent_then_one())).unwrap();
        assert_eq!(watch.script, "revertprobe-core");
        assert!(watch.forgives(SAID));
        let staging = Watch::with(&dir, "staging", Box::new(absent_then_one())).unwrap();
        assert_eq!(staging.script, "revertprobe-core-staging");
        let present = Scripted(std::cell::RefCell::new(vec![ScriptState::Present(1)]));
        assert!(Watch::with(&dir, "default", Box::new(present)).is_none());
        let unknown = Scripted(std::cell::RefCell::new(vec![ScriptState::Unknown(
            "x".into(),
        )]));
        assert!(Watch::with(&dir, "default", Box::new(unknown)).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_forgiven_failure_settles_as_success_and_others_keep_their_status() {
        let dir = dir("settle");
        let watch = Watch::with(
            &dir,
            "default",
            Box::new(Scripted(std::cell::RefCell::new(vec![
                ScriptState::Absent,
                ScriptState::Present(1),
                ScriptState::Present(1),
            ]))),
        )
        .unwrap();
        let failed = Exit::Code(1);
        assert!(watch.settle(failed, SAID).success());
        assert!(!watch.settle(failed, "some other failure").success());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_cloudflares_own_not_found_reads_as_absent() {
        use crate::cloudflare::RestError;
        let absent = |e: RestError| is_absent(&e);
        assert!(absent(RestError::answered(
            "GET",
            "/workers/scripts/x/versions",
            404,
            "[{\"code\":10007,\"message\":\"x\"}]"
        )));
        assert!(absent(RestError::answered(
            "GET",
            "/y",
            200,
            "[{\"code\":10007}]"
        )));
        // A number that merely appears (an id, a Ray ID, another code) is not it.
        assert!(!absent(RestError::answered(
            "GET",
            "/y",
            500,
            "[{\"code\":10404}]"
        )));
        assert!(!absent(RestError::answered("GET", "/y", 403, "[]")));
    }

    /// `settle` of a failed push that said `10007`, on an account that shows the
    /// upload landed, records that triggers were skipped.
    #[test]
    fn a_forgiven_failure_records_that_triggers_were_skipped() {
        let dir = dir("forgave");
        let watch = Watch::with(
            &dir,
            "default",
            Box::new(Scripted(std::cell::RefCell::new(vec![
                ScriptState::Absent,
                ScriptState::Present(1),
            ]))),
        )
        .unwrap();
        assert!(!watch.forgave());
        assert!(watch.settle(Exit::Code(1), SAID).success());
        assert!(watch.forgave());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn triggers_deploy_reports_its_own_failure() {
        use crate::cloudflare::fake::{Recording, failed};
        let dir = std::env::temp_dir();
        let ok = Recording::new(|_| Ok(String::new()));
        assert_eq!(deploy_triggers(&ok, &dir, "staging"), Ok(()));
        assert_eq!(ok.calls(), ["triggers deploy --env staging"]);
        let bad = Recording::new(|op| Err(failed(op, "", "")));
        assert_eq!(
            deploy_triggers(&bad, &dir, "default"),
            Err(Some(
                "`wrangler triggers deploy` exited exit status: 1".into()
            ))
        );
        let missing = Recording::new(|op| Err(crate::cloudflare::WranglerError::missing(op)));
        assert_eq!(deploy_triggers(&missing, &dir, "default"), Err(None));
    }
}

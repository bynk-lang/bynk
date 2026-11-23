use crate::deploy::config::ForeignCaller;
use crate::deploy::config::Resources;
use crate::deploy::config::deploy_command;
use crate::deploy::config::deployed_worker_name;
use crate::deploy::containers::container_refusals;
use crate::doctor;
use crate::doctor::Report;
use crate::probe;
use crate::probe::Toolbox;
use crate::report;
use crate::report::Format;
use std::collections::BTreeMap;

/// The progress line of a multi-context run: `n` of `total`, the Worker by
/// its [`deployed_worker_name`] (#2047).
pub(crate) fn deploying_notice(environment: &str, worker: &str, n: usize, total: usize) -> String {
    format!(
        "bynk: deploying `{}` ({n}/{total})…\n",
        deployed_worker_name(environment, worker)
    )
}

/// A push that landed but could not be written to the ledger at `lock_path`.
/// The Worker by its [`deployed_worker_name`] (#2047).
pub(crate) fn unrecorded_deploy_failure(
    environment: &str,
    worker: &str,
    lock_path: &str,
    error: &str,
) -> String {
    format!(
        "bynk: error: deployed `{}` but could not record it in {lock_path}: {error}\n",
        deployed_worker_name(environment, worker)
    )
}

/// I3 (#1971): the run could not decide `caller`'s binding on `worker`
/// (`reason`, from `decide_foreign_key`). The Worker by its
/// [`deployed_worker_name`] (#2047).
pub(crate) fn foreign_binding_refusal(
    environment: &str,
    worker: &str,
    caller: &ForeignCaller,
    reason: &str,
) -> String {
    format!(
        "bynk: error: `{}` (the foreign caller `{}` of `{}`): {reason}\n",
        caller.binding,
        caller.actor,
        deployed_worker_name(environment, worker)
    )
}

/// I3 (#1971): a failed push may have set a secret minted for `caller`. The
/// Worker by its [`deployed_worker_name`], which the suggested `wrangler
/// secret list` names too, so it asks about the right Worker (#2047).
pub(crate) fn foreign_binding_maybe_set_notice(
    environment: &str,
    worker: &str,
    caller: &ForeignCaller,
) -> String {
    let worker = deployed_worker_name(environment, worker);
    format!(
        "bynk: note: `{worker}` may already hold the {} minted for its foreign caller `{}`. If `wrangler secret list --name {worker}` shows it, hand this off; if not, the next deploy mints another.\n",
        caller.binding, caller.actor
    )
}

/// Everything a push needs of the installed tools, refused before authenticating
/// so it costs no call: the wrangler floor for a Durable Object class
/// (a container is one; [`wrangler_floor_refusal`]), then a container's Docker
/// ([`container_refusals`]).
pub(crate) fn preflight_refusal(
    tb: &dyn Toolbox,
    probe: &probe::Probe,
    manager: bynk_driver::npm_install::PackageManager,
    order: &[String],
    resources: &BTreeMap<String, Resources>,
) -> Option<String> {
    wrangler_floor_refusal(probe, manager, order, resources)
        .or_else(|| container_refusals(tb, order, resources))
}

/// #1796: the refusal a deploy gets when a context it pushes declares a
/// Durable Object class and the wrangler that would push it is older than
/// [`bynk_emit::WRANGLER_MIN`].
///
/// Below that floor wrangler doesn't read the `exports` map the emitter
/// declares every class in, so the push would carry Durable Object bindings
/// with no namespace behind them and fail with Cloudflare's own error, which
/// says nothing about the version. `doctor`'s wrangler row only *warns* about
/// the floor, because a project with no agent still deploys on an older
/// wrangler. This is where the project is known, so this is where the floor
/// becomes a hard requirement. `None` when nothing pushed declares a class, or
/// when the version can't be judged without running npx
/// ([`doctor::wrangler_below_min`]'s own rule).
pub(crate) fn wrangler_floor_refusal(
    probe: &probe::Probe,
    manager: bynk_driver::npm_install::PackageManager,
    order: &[String],
    resources: &BTreeMap<String, Resources>,
) -> Option<String> {
    let version = probe
        .version
        .filter(|_| doctor::wrangler_below_min(probe))?;
    let worker = order
        .iter()
        .find(|worker| !resources[*worker].durable_objects.is_empty())?;
    Some(format!(
        "bynk: error: wrangler {version} is older than {}, the oldest this Bynk release supports \
         (`{worker}` declares Durable Object `exports`, which wrangler reads from 4.107.0, and an \
         older wrangler is untested against the generated configuration); the push is not \
         attempted. Upgrade with `{}`.",
        bynk_emit::WRANGLER_MIN,
        doctor::wrangler_upgrade_remedy(probe, manager)
    ))
}

pub fn preflight_failure_message(report: &Report) -> String {
    format!(
        "bynk: error: environment not ready for `deploy` — see below.\n\n{}",
        report::render(report, Format::Human)
    )
}

/// The D4 refusal: `--context` selected `worker`, which binds to `absent`
/// Workers the ledger has never recorded as deployed in `environment`.
///
/// Every Worker is named by its [`deployed_worker_name`], as the plan names it
/// (#2038): under `--env staging` the missing target is `payment-staging`, not
/// the logical `payment`. Pure, so the wording is goldened rather than
/// described.
pub(crate) fn absent_dependencies_refusal(
    environment: &str,
    worker: &str,
    absent: &[String],
) -> String {
    format!(
        "bynk: error: `{}` binds to {}, which {} never been deployed — a Service Binding to a Worker that does not exist fails at upload.\n  Deploy the whole project once (`{}`) to bring the topology up.\n",
        deployed_worker_name(environment, worker),
        absent
            .iter()
            .map(|a| format!("`{}`", deployed_worker_name(environment, a)))
            .collect::<Vec<_>>()
            .join(", "),
        if absent.len() == 1 { "has" } else { "have" },
        deploy_command(environment)
    )
}

/// What the run did **not** get to, once a context failed. `rest` is the order
/// *beyond* the failure, so the last context failing reports nothing — there was
/// nothing left to withhold, and the failure itself has already been named.
/// Each context is named by its [`deployed_worker_name`] in `environment`, as
/// the plan's `order` line names it (#2038).
///
/// Pure, so the wording — and the count's agreement with the list — is goldened
/// rather than described.
pub(crate) fn stopped_report(environment: &str, rest: &[String]) -> String {
    if rest.is_empty() {
        return String::new();
    }
    format!(
        "bynk: note: stopping — {} not deployed: {}. Re-run `{}` to resume; what already landed is kept.\n",
        if rest.len() == 1 {
            "1 more context was".to_string()
        } else {
            format!("{} further contexts were", rest.len())
        },
        rest.iter()
            .map(|worker| deployed_worker_name(environment, worker))
            .collect::<Vec<_>>()
            .join(", "),
        deploy_command(environment)
    )
}

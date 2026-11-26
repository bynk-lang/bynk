//! The `cloudflare api` row (#4056 Decision F): whether the two variables that `bynk deploy`
//! needs to manage the project's account resources through Cloudflare's REST API are set.
//!
//! - It shows only for a project that declares an account resource
//!   ([`declares_account_resources`]), as the Docker row shows only for a Container.
//! - It is `ok` or `warn`, never `fail`: `bynk dev` runs the Deploy gate too and needs no token,
//!   and a redeploy with everything recorded needs none either. The deploy's own pre-flight
//!   refuses exactly when one is needed. `--strict` escalates the warning, as for every row.
//! - It is offline: whether each variable is set, never its value and never a call to check the
//!   token. The deploy's own calls report a bad token with the permission it lacks.

use super::{Level, Row, any_bynk_line, capability_of};
use crate::probe::Toolbox;

/// The variables, in the order the row names them.
const VARIABLES: [&str; 2] = ["CLOUDFLARE_API_TOKEN", "CLOUDFLARE_ACCOUNT_ID"];

/// The capability kinds whose resources live on the account and are managed or looked up
/// through Cloudflare's API: KV, R2, D1, queues, Vectorize, Hyperdrive (`Postgres`, `MySql`),
/// and the linked kinds.
const ACCOUNT_KINDS: &[&str] = &[
    "AiSearch",
    "D1",
    "Kv",
    "MtlsCertificate",
    "MySql",
    "Postgres",
    "Queue",
    "R2",
    "StoredSecret",
    "Vectorize",
];

const REMEDY: &str = "set CLOUDFLARE_API_TOKEN and CLOUDFLARE_ACCOUNT_ID: `bynk deploy` creates, checks and deletes this project's account resources through Cloudflare's API; `wrangler login` covers the push only";

/// Whether the project declares an account resource: a `capability X = Kind …` of one of
/// [`ACCOUNT_KINDS`], a `queue(…)` source, or a use of the bare `Kv`. A text scan, as for the
/// Docker row: a comment that spells one counts.
pub(crate) fn declares_account_resources(root: &std::path::Path) -> bool {
    any_bynk_line(root, |line| {
        capability_of(line, ACCOUNT_KINDS) || has_queue_source(line) || uses_bare_kv(line)
    })
}

/// Whether `line` names a `queue(…)` source: `queue(` not preceded by an identifier character,
/// so `enqueue(` and `dequeue(` do not count.
fn has_queue_source(line: &str) -> bool {
    starts_a_word(line, "queue(")
}

/// Whether `line` calls the bare `Kv` (`Kv.get(…)`), which declares the Worker's own namespace.
fn uses_bare_kv(line: &str) -> bool {
    starts_a_word(line, "Kv.")
}

/// Whether `needle` occurs in `line` with no identifier character just before it.
fn starts_a_word(line: &str, needle: &str) -> bool {
    line.match_indices(needle).any(|(at, _)| {
        !line[..at]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
    })
}

/// The row: `ok` when both variables are set, else `warn` naming what is not.
pub(crate) fn cloudflare_api_row(tb: &dyn Toolbox) -> Row {
    let unset: Vec<&str> = VARIABLES
        .into_iter()
        .filter(|name| !tb.env_set(name))
        .collect();
    if unset.is_empty() {
        return Row {
            label: "cloudflare api".into(),
            level: Level::Ok,
            detail:
                "CLOUDFLARE_API_TOKEN and CLOUDFLARE_ACCOUNT_ID set (the token is not checked here)"
                    .into(),
            remedy: None,
        };
    }
    Row {
        label: "cloudflare api".into(),
        level: Level::Warn,
        detail: format!("{} not set", unset.join(" and ")),
        remedy: Some(REMEDY.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_declaration_is_found_by_its_kind_or_a_queue_source_or_the_bare_kv() {
        for line in [
            "capability Sessions = Kv",
            "  capability Files = R2 @jurisdiction(\"eu\")",
            "capability Orders = D1",
            "capability Docs = Vectorize(dimensions: 768)",
            "capability Db = Postgres",
            "capability Jobs = Queue(\"jobs\")",
            "service fulfil from queue(\"order-events\") {",
            "\tlet stored <- Kv.get(keyOf(name))",
        ] {
            assert!(
                capability_of(line, ACCOUNT_KINDS) || has_queue_source(line) || uses_bare_kv(line),
                "{line}"
            );
        }
        for line in [
            "capability Limit = RateLimit",
            "capability Box = Container",
            "capability Cache = EdgeCache",
            "capability Store = Kvv",
            "let x <- MyKv.get(k)",
            "do jobs.enqueue(item)",
            "let next <- dequeue(q)",
        ] {
            assert!(
                !(capability_of(line, ACCOUNT_KINDS)
                    || has_queue_source(line)
                    || uses_bare_kv(line)),
                "{line}"
            );
        }
    }
}

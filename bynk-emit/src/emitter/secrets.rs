//! `bynk-secrets.json` generation per Worker (v0.172, ADR 0195).
//!
//! The **declared** secret names a context's handlers will read from `env` at
//! runtime — an `actor`'s `auth = Bearer(secret = "…")` / `Signature(secret =
//! "…")`, including the members of a multi-actor sum. `deploy` reads this file
//! to know which secrets it must see set before it pushes.
//!
//! The file carries **two kinds of knowledge, which are not equally strong**
//! (ADR 0196):
//!
//! - `declared` — an `actor`'s auth secret. A literal fixed at parse time,
//!   required at compile time, and **fail-closed**: unset, the Worker answers
//!   401 to every request. `deploy` refuses to ship without a value.
//! - `read` — a literal `bynk.Secrets` name (`Secrets.get("X")`). `get` returns
//!   `Option`, so absence is a legitimate handled outcome — these are
//!   **advisory**, and `deploy` warns rather than failing.
//!
//! And `read_complete`, which is the honesty. `Secrets.get` takes an ordinary
//! `String` expression, so a computed name is invisible to any pass: where one
//! is seen, the context warns (`bynk.secrets.computed_name`) and this flag goes
//! false. So does composing an adapter provider `given` `bynk`'s `Secrets`
//! (#1976): its binding is TypeScript, and reads names no pass can see.
//! `declared` is a **floor, not a census** (ADR 0195 D2); `read` is a census
//! only while `read_complete` holds, and says so when it does not.
//!
//! Why a file rather than an API: the driver has two compile paths, and under a
//! `bynkc` override the compiler is a child process handing back an exit status
//! — there is no in-memory model to consult. A name the compiler knows must
//! reach the driver in the build output, or not at all (ADR 0195 D5).
//!
//! P5.5 (`design/tracks/semantics-in-the-checker.md` §6, §9): the checking
//! half — `SecretReads`, the `Secrets.get` AST walk, and the
//! `bynk.secrets.computed_name` diagnostic itself — moved to
//! `bynk_check::secrets`, a real gap this track's settling pass had not
//! scoped (see that module's own doc). What stays here is emission-only:
//! rendering the manifest `bynk deploy` reads, which is not this crate's
//! business to ask `bynk-check` to do.

use std::collections::BTreeSet;

use bynk_project::json_string;

use bynk_check::actors::SumMemberSeam;
use bynk_check::secrets::SecretReads;

use bynk_check::symbols::UnitTable;

/// The file the driver reads, beside each Worker's `wrangler.toml`.
pub const SECRETS_MANIFEST: &str = "bynk-secrets.json";

/// The manifest schema version. Bumped only by a breaking shape change; the
/// driver refuses a version it does not know rather than guessing, as the
/// deploy ledger does.
///
/// **2** (v0.173, ADR 0196) added `read` and `read_complete`. The bump is
/// deliberate rather than a default-on-absence read: a v1 manifest carries no
/// evidence either way about computed names, and defaulting `read_complete` to
/// `true` for it would be the manifest's one claim that could be silently wrong.
const MANIFEST_VERSION: u32 = 2;

/// Every secret name this context's handlers will read from `env`.
///
/// Enumerated over exactly the handlers the entry emitter lowers seams for
/// (`table.services`, which is where a `from websocket` service lives too), and
/// resolved with exactly the same `bynk_check::actors` functions — so the
/// manifest cannot describe a Worker other than the one emitted beside it. An
/// actor that is declared but named by no handler's `by` clause resolves no
/// seam and contributes nothing: the Worker never reads it.
///
/// `Oidc` names no secret — its trust root is the provider's published JWKS, not
/// a shared value — and a sum's `None` member (a catch-all such as `Visitor`)
/// verifies nothing. Both are skipped rather than defaulted: inventing a name
/// for them would ask the user to set a secret that nothing reads.
///
/// I3 (#1971): a `Foreign` actor's secret is **not** declared here, on purpose.
/// A declared secret is the user's to supply, and `deploy` prompts for one it
/// lacks; a foreign caller's is minted by `deploy` and handed to the foreign
/// Worker's operator ([`foreign_callers`]). None of the resolvers above match a
/// `Foreign` actor, and a test pins that.
pub(crate) fn declared_secrets(table: &UnitTable) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for handler in table.services.values().flat_map(|s| s.handlers.iter()) {
        if let Some(seam) = bynk_check::actors::bearer_seam_for(handler, &table.actors) {
            names.insert(seam.secret);
        }
        if let Some(seam) = bynk_check::actors::signature_seam_for(handler, &table.actors) {
            names.insert(seam.secret);
        }
        for member in bynk_check::actors::sum_members_for(handler, &table.actors)
            .into_iter()
            .flatten()
        {
            match member.seam {
                SumMemberSeam::Bearer { secret, .. } => {
                    names.insert(secret);
                }
                SumMemberSeam::Signature(seam) => {
                    names.insert(seam.secret);
                }
                SumMemberSeam::None => {}
            }
        }
    }
    names
}

/// #3386: the `SignedCookie` actors this context's handlers are guarded by or issue for, each
/// with the env binding of its signing key, by actor name. Unlike an `actor`'s auth secret, a
/// session key is **not** declared for the user to supply: `deploy` mints it when it is unset,
/// and `bynk dev` uses a per-run one. (The optional `previous` key is the operator's, so it is
/// not listed.) Every `SignedCookie` actor of the context counts, so a context that only issues
/// the cookie (a login service) still gets its key.
pub(crate) fn session_keys(table: &UnitTable) -> Vec<(String, String)> {
    let mut keys: Vec<(String, String)> = table
        .actors
        .values()
        .filter_map(|a| bynk_check::actors::signed_cookie_seam_of(a, None))
        .map(|s| (s.actor, s.secret))
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

/// I3 (#1971): the foreign callers this context's `on call` handlers admit, by
/// actor name, each with the secret its calls verify under and the foreign
/// Worker's name when declared. `deploy` mints each secret, sets it on this
/// Worker, and prints the command that hands it to the foreign Worker.
pub(crate) fn foreign_callers(table: &UnitTable) -> Vec<bynk_check::actors::ForeignCaller> {
    let mut callers: Vec<bynk_check::actors::ForeignCaller> = Vec::new();
    for handler in table.services.values().flat_map(|s| s.handlers.iter()) {
        for f in bynk_check::actors::call_admission_for(handler, &table.actors)
            .map(|a| a.foreign)
            .unwrap_or_default()
        {
            if !callers.iter().any(|g| g.actor == f.actor) {
                callers.push(f);
            }
        }
    }
    callers.sort_by(|a, b| a.actor.cmp(&b.actor));
    callers
}

/// The reads half of what a Worker knows about its own `bynk.Secrets` use —
/// `declared_secrets`'s counterpart, read from the AST rather than derived
/// from actor bindings. The walk itself, its `bynk.secrets.computed_name`
/// diagnostic, and the `reads_secrets_of_bynk` capability-resolution guard
/// live in `bynk_check::secrets` (P5.5); the Worker-wide closure — handlers
/// plus every provider the composition root builds, and the adapter-binding
/// case no pass can see (#1976) — is [`bynk_check::secrets::worker_secret_reads`].
/// `run_checks` raises the warnings, via
/// `bynk_check::project_model::phase_secrets_computed_name`, per file.
pub fn secret_reads(
    context: &str,
    unit_tables: &std::collections::HashMap<String, UnitTable>,
    unit_consumes: &std::collections::HashMap<String, Vec<String>>,
    unit_consumes_aliases: &std::collections::HashMap<
        String,
        std::collections::HashMap<String, String>,
    >,
    unit_flattened: &std::collections::HashMap<String, std::collections::HashMap<String, String>>,
) -> SecretReads {
    bynk_check::secrets::worker_secret_reads(
        context,
        unit_tables,
        unit_consumes,
        unit_consumes_aliases,
        unit_flattened,
    )
}

/// Render the manifest for a context, or `None` when there is nothing to say.
///
/// Emitted when **anything** is known ([DECISION E]): a declared secret, a read
/// name, or the fact that a name is computed. Slice 3's rule — emit only for a
/// non-empty `declared` — would have stayed silent for a context that reads
/// secrets but declares none, which is exactly the context this file now exists
/// to describe.
pub fn emit_secrets_manifest(table: &UnitTable, reads: &SecretReads) -> Option<String> {
    render(
        &declared_secrets(table),
        reads,
        &foreign_callers(table),
        &session_keys(table),
    )
}

/// Render a resolved name set. Split from the derivation so the file's shape is
/// tested without building a project model.
///
/// Absent rather than empty: a project with no declared secret must not grow a
/// file into every worker directory for a feature it does not use — and "no
/// file" is the same answer as "an empty list" to a driver that must tolerate a
/// build tree from a compiler predating this file anyway.
fn render(
    declared: &BTreeSet<String>,
    reads: &SecretReads,
    foreign: &[bynk_check::actors::ForeignCaller],
    session: &[(String, String)],
) -> Option<String> {
    // `complete` is the third thing worth saying: a context that reads one
    // computed name and nothing else knows something — that it does not know —
    // and the file has to carry it or the driver cannot.
    if declared.is_empty()
        && reads.names.is_empty()
        && reads.complete
        && foreign.is_empty()
        && session.is_empty()
    {
        return None;
    }
    // #3386: `session` likewise only when there is one, additive within version 2.
    let session = if session.is_empty() {
        String::new()
    } else {
        let rendered: Vec<String> = session
            .iter()
            .map(|(actor, secret)| {
                format!(
                    "    {{ \"actor\": {}, \"secret\": {} }}",
                    json_string(actor),
                    json_string(secret)
                )
            })
            .collect();
        format!(",\n  \"session\": [\n{}\n  ]", rendered.join(",\n"))
    };
    // I3 (#1971): `foreign` only when there is one. It is additive within
    // version 2 rather than a version 3: a manifest without it says, truly,
    // that the context admits no foreign caller, so an older file reads the
    // same as before and every existing manifest stays byte-identical.
    let foreign = if foreign.is_empty() {
        String::new()
    } else {
        let rendered: Vec<String> = foreign
            .iter()
            .map(|f| {
                let worker = f
                    .worker
                    .as_ref()
                    .map(|w| format!(", \"worker\": {}", json_string(w)))
                    .unwrap_or_default();
                format!(
                    "    {{ \"actor\": {}, \"secret\": {}{worker} }}",
                    json_string(&f.actor),
                    json_string(&f.secret)
                )
            })
            .collect();
        format!(",\n  \"foreign\": [\n{}\n  ]", rendered.join(",\n"))
    };
    // Hand-rendered rather than via serde: this crate does not depend on
    // serde_json, and the shape is a handful of fields.
    Some(format!(
        "{{\n  \"version\": {MANIFEST_VERSION},\n  \"declared\": {},\n  \"read\": {},\n  \"read_complete\": {}{foreign}{session}\n}}\n",
        json_array(declared),
        json_array(&reads.names),
        reads.complete,
    ))
}

/// A JSON array of names, one per line, or `[]`.
fn json_array(names: &BTreeSet<String>) -> String {
    if names.is_empty() {
        return "[]".to_string();
    }
    let rendered: Vec<String> = names
        .iter()
        .map(|n| format!("    {}", json_string(n)))
        .collect();
    format!("[\n{}\n  ]", rendered.join(",\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn a_session_key_is_listed_beside_the_declared_secrets_not_among_them() {
        let session = [("User".to_string(), "SESSION_KEY".to_string())];
        let text =
            render(&set(&[]), &reads(&[], true), &[], &session).expect("a session key renders");
        assert_eq!(
            text,
            "{\n  \"version\": 2,\n  \"declared\": [],\n  \"read\": [],\n  \"read_complete\": true,\n  \"session\": [\n    { \"actor\": \"User\", \"secret\": \"SESSION_KEY\" }\n  ]\n}\n"
        );
        // Declared secrets are the operator's to supply; the session key is not one of them.
        assert!(!text.contains("\"declared\": [\n"));
    }

    #[test]
    fn a_name_is_escaped_as_a_json_string() {
        assert_eq!(json_string("AUTH_JWT_SECRET"), "\"AUTH_JWT_SECRET\"");
        // A secret name is a Bynk string literal, so it is arbitrary text; the
        // manifest must stay parseable rather than usually-parseable.
        assert_eq!(json_string("a\"b"), "\"a\\\"b\"");
        assert_eq!(json_string("a\\b"), "\"a\\\\b\"");
        assert_eq!(json_string("a\nb"), "\"a\\nb\"");
        assert_eq!(json_string("a\u{1}b"), "\"a\\u0001b\"");
    }

    fn reads(names: &[&str], complete: bool) -> SecretReads {
        SecretReads {
            names: set(names),
            complete,
        }
    }

    /// [DECISION E]: the file appears when **anything** is known, and only then.
    #[test]
    fn a_manifest_appears_when_anything_is_known_and_not_otherwise() {
        // Nothing at all — no file. A project with no secrets must not grow one
        // into every worker directory.
        assert_eq!(render(&set(&[]), &reads(&[], true), &[], &[]), None);

        // A read with no declared secret is exactly the context slice 3's rule
        // would have stayed silent about — and exactly the one worth describing.
        assert!(render(&set(&[]), &reads(&["API_KEY"], true), &[], &[]).is_some());

        // Knowing that you *don't* know is knowledge too: a context whose only
        // secret is computed still emits, or the driver cannot say its list is
        // incomplete.
        assert!(render(&set(&[]), &reads(&[], false), &[], &[]).is_some());
    }

    /// The committed shape, byte for byte — it is a file a reviewer reads in a
    /// fixture diff and a driver parses. Asserted here rather than round-tripped
    /// through a parser because this crate deliberately carries three
    /// dependencies and `serde_json` is not among them; that the bytes *parse*
    /// is asserted driver-side, where the reader lives.
    #[test]
    fn the_manifest_is_sorted_and_pinned() {
        // A `BTreeSet` orders the names, so the file is byte-stable for a given
        // context rather than dependent on handler iteration order.
        assert_eq!(
            render(
                &set(&["B_SECRET", "A_SECRET"]),
                &reads(&["R"], true),
                &[],
                &[]
            )
            .expect("a non-empty set renders"),
            "{\n  \"version\": 2,\n  \"declared\": [\n    \"A_SECRET\",\n    \"B_SECRET\"\n  ],\n  \
             \"read\": [\n    \"R\"\n  ],\n  \"read_complete\": true\n}\n",
        );
        // The empty-list and false-flag shapes, which the fixtures also carry.
        assert_eq!(
            render(&set(&["ONLY"]), &reads(&[], false), &[], &[]).expect("renders"),
            "{\n  \"version\": 2,\n  \"declared\": [\n    \"ONLY\"\n  ],\n  \
             \"read\": [],\n  \"read_complete\": false\n}\n",
        );
    }

    /// I3 (#1971): foreign callers, and only foreign callers, make a file, with
    /// the worker named where it was declared.
    #[test]
    fn foreign_callers_are_listed_apart_from_declared_secrets() {
        let foreign = vec![
            bynk_check::actors::ForeignCaller {
                actor: "Ledger".into(),
                secret: "LEDGER_CALL_KEY".into(),
                worker: Some("ledger-svc".into()),
            },
            bynk_check::actors::ForeignCaller {
                actor: "Legacy".into(),
                secret: "LEGACY_CALL_KEY".into(),
                worker: None,
            },
        ];
        assert_eq!(
            render(&set(&[]), &reads(&[], true), &foreign, &[]).expect("a foreign caller renders"),
            "{\n  \"version\": 2,\n  \"declared\": [],\n  \"read\": [],\n  \"read_complete\": true,\n  \
             \"foreign\": [\n    { \"actor\": \"Ledger\", \"secret\": \"LEDGER_CALL_KEY\", \"worker\": \"ledger-svc\" },\n    \
             { \"actor\": \"Legacy\", \"secret\": \"LEGACY_CALL_KEY\" }\n  ]\n}\n",
        );
    }
}

//! #3938: the compiler-generated session agent of a `SignedCookie` actor with `sessions = true`.
//!
//! The agent is ordinary Bynk source, appended to the declaring file before it is parsed
//! again, so the checker, the schema lock, the emitter and the deploy migration treat it like any
//! agent the author wrote. Its shape is therefore the compiler's to evolve, and stable: it is
//! recorded in the schema lock. See `bynk_check::actors::sessions_agent_name` for its name.
//!
//! The agent holds one identity's live sessions as a `Cell` of a map from session id to expiry
//! (milliseconds). It never reads a clock: the seam and `issue` pass `now`, so a context needs no
//! `consumes Clock` for it. Expired sessions are pruned each time one is added.

use bynk_syntax::ast::{ActorDecl, CommonsForm, SourceUnit, TypeRef};

/// `source` with each `sessions = true` `SignedCookie` actor's session agent added to the context
/// that declares it, or `None` when the file declares none. A headerless (fragment) context runs to
/// the end of the file, so its agents go there; a brace-bodied one takes them just inside its closing
/// brace.
pub fn with_session_agents(source: &str, units: &[SourceUnit]) -> Option<String> {
    let mut inserts: Vec<(usize, String)> = Vec::new();
    for unit in units {
        let SourceUnit::Context(ctx) = unit else {
            continue;
        };
        let mut text = String::new();
        for actor in ctx.items.iter().filter_map(|i| i.as_actor()) {
            if let Some(identity) = sessions_identity(actor) {
                text.push_str(&agent_text(&actor.name.name, &identity));
            }
        }
        if text.is_empty() {
            continue;
        }
        let at = match ctx.form {
            CommonsForm::Fragment => source.len(),
            CommonsForm::Brace => source.get(..ctx.span.end)?.rfind('}')?,
        };
        inserts.push((at, text));
    }
    if inserts.is_empty() {
        return None;
    }
    // Later positions first, so an earlier insertion does not move them.
    inserts.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    let mut out = source.to_string();
    for (at, text) in inserts {
        out.insert_str(at, &text);
    }
    Some(out)
}

/// The identity type's name when `actor` is a `SignedCookie` with `sessions = true`.
fn sessions_identity(actor: &ActorDecl) -> Option<String> {
    if actor.auth.as_ref()?.name != "SignedCookie" {
        return None;
    }
    if actor.scheme_arg("sessions")?.value.as_ident() != Some("true") {
        return None;
    }
    match actor.identity.as_ref()? {
        TypeRef::Named(id) => Some(id.name.clone()),
        _ => None,
    }
}

fn agent_text(actor: &str, identity: &str) -> String {
    let agent = format!("{actor}Sessions");
    format!(
        "\n---\nThe live sessions of one `{actor}` identity, generated for `sessions = true`.\n---\n\
agent {agent} {{\n\
\tkey identity: {identity}\n\
\n\
\tstore live: Cell[Map[String, Int]] = Map.of([])\n\
\n\
\ton call add(sid: String, exp: Int, now: Int) -> Effect[()] {{\n\
\t\tdo live.update((m: Map[String, Int]) => m.filter((k: String, v: Int) => v > now).insert(sid, exp))\n\
\t}}\n\
\n\
\ton call remove(sid: String) -> Effect[()] {{\n\
\t\tdo live.update((m: Map[String, Int]) => m.remove(sid))\n\
\t}}\n\
\n\
\ton call revokeAll() -> Effect[()] {{\n\
\t\tdo live.update((m: Map[String, Int]) => m.filter((k: String, v: Int) => false))\n\
\t}}\n\
\n\
\ton call isLive(sid: String, now: Int) -> Effect[Bool] {{\n\
\t\tlet m = live\n\t\tmatch m.get(sid) {{\n\
\t\t\tSome(exp) => exp > now,\n\
\t\t\tNone => false,\n\
\t\t}}\n\
\t}}\n\
}}\n"
    )
}

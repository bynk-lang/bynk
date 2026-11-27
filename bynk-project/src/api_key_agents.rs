//! #3939 (the API keys track, D1): the compiler-generated agents of an `ApiKey` actor.
//!
//! Like a `sessions = true` cookie's agent (`crate::session_agents`, ADR 0998 B), they are
//! ordinary Bynk source appended to the declaring context before the file is parsed again, so the
//! checker, the schema lock, the emitter and the deploy migration treat them like any agent the
//! author wrote. Their shape is the compiler's to evolve.
//!
//! - `<Actor>Keys(handle)` holds one identity's keys, by key number: the peppered digest, the
//!   pepper's `kid`, the label, when it was created and an (empty, for now) scope set. It is the
//!   only agent the request path asks, and it compares digests only: the pepper never reaches it.
//! - `<Actor>KeyOwner(identity)` holds the identity's handle. Only the members use it.
//!
//! Neither reads a clock: the seam and the members pass `now`. The comparison is `ApiKey.equal`,
//! an internal kernel only these agents can name, which lowers to the runtime's one constant-time
//! comparison, the function `bynk.crypto`'s `Crypto.equal` calls: the same primitive as track §6's
//! "consumes `Crypto`", with no capability put in the author's scope. The store compares exactly
//! once per `verify` whatever it holds: against the record's digest when there is one under a
//! pepper the seam sent, else against a fixed dummy digest (D3 B). The text is appended after
//! everything the author wrote, so no author line or column moves.
//!
//! The names come from `bynk_check::api_key::{keys_agent_name, key_owner_agent_name}`, which
//! this crate cannot depend on; the two are pinned equal by a test there.

use bynk_syntax::ast::{ActorDecl, CommonsForm, CommonsItem, SourceUnit, TypeRef};

/// The marker the generated agents' documentation begins with (`bynk_check::api_key::GENERATED_MARKER`).
pub const GENERATED_MARKER: &str = "Generated for the `ApiKey` actor";

/// The most live keys one identity may hold (track §6).
pub const KEY_CAP: i64 = 100;

/// `source` with each `ApiKey` actor's agents added to the context that declares it, or `None`
/// when the file declares none. When the file already takes either name, neither is added, so the
/// checker reports the author's declaration (`bynk.actor.api_key_agent_name_reserved`).
pub fn with_api_key_agents(source: &str, units: &[SourceUnit]) -> Option<String> {
    let mut inserts: Vec<(usize, String)> = Vec::new();
    for unit in units {
        let SourceUnit::Context(ctx) = unit else {
            continue;
        };
        let mut text = String::new();
        for actor in ctx.items.iter().filter_map(|i| i.as_actor()) {
            let Some(identity) = api_key_identity(actor) else {
                continue;
            };
            let owner = &actor.name.name;
            let keys = format!("{owner}Keys");
            let key_owner = format!("{owner}KeyOwner");
            // The two are one store: when the author takes either name, neither is generated, so
            // the checker reports the declaration once rather than a cascade through the other.
            if is_taken(&ctx.items, &keys)
                || is_taken(&ctx.items, &key_owner)
                || is_taken(&ctx.items, &format!("{owner}KeyCheck"))
            {
                continue;
            }
            text.push_str(&keys_agent_text(
                owner,
                &keys,
                &identity,
                &format!("{owner}KeyCheck"),
            ));
            text.push_str(&key_owner_agent_text(owner, &key_owner, &keys, &identity));
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

/// Whether an item of the context already has `name`.
fn is_taken(items: &[CommonsItem], name: &str) -> bool {
    items
        .iter()
        .any(|i| i.name().is_some_and(|n| n.name == name))
}

/// The identity type's name when `actor` declares the `ApiKey` scheme with a named identity.
fn api_key_identity(actor: &ActorDecl) -> Option<String> {
    if actor.auth.as_ref()?.name != "ApiKey" {
        return None;
    }
    match actor.identity.as_ref()? {
        TypeRef::Named(id) => Some(id.name.clone()),
        _ => None,
    }
}

/// `<Actor>Keys`. `verify` only reads: it answers `Some(<Actor>KeyCheck { identity, touch })` for
/// an admitted key, else `None`, and the seam calls `touch` (last use, re-hash, live `kid`s) only on
/// success when `touch` is true. `insert` answers 0 (stored), 1 (the identity holds the most keys it
/// may) or 2 (the store is retired).
fn keys_agent_text(actor: &str, agent: &str, identity: &str, check: &str) -> String {
    format!(
        "\n---\n{GENERATED_MARKER} `{actor}`: what `{agent}.verify` answers for an admitted key, its identity and whether `touch` must record its use, re-hash it or note the `kid`s now live.\n---\n\
type {check} = {{ identity: {identity}, touch: Bool }}\n\
\n---\n{GENERATED_MARKER} `{actor}`: one identity's keys, addressed by the random handle in each key's id.\n---\n\
agent {agent} {{\n\
\tkey handle: String\n\
\n\
\tstore owner: Cell[Option[{identity}]] = None\n\
\tstore retired: Cell[Bool] = false\n\
\tstore digests: Map[String, Bytes]\n\
\tstore kids: Map[String, String]\n\
\tstore labels: Map[String, String]\n\
\tstore created: Map[String, Int]\n\
\tstore expires: Map[String, Int]\n\
\tstore used: Map[String, Int]\n\
\tstore scopes: Map[String, Set[String]]\n\
\tstore known: Cell[List[String]] = []\n\
\tstore knownAt: Cell[Int] = 0\n\
\n\
\ton call verify(keyNo: String, presented: Map[String, Bytes], first: Bytes, current: String, dummy: Bytes, now: Int) -> Effect[Option[{check}]] {{\n\
\t\tlet stored <- digests.get(keyNo)\n\
\t\tlet kid <- kids.get(keyNo)\n\
\t\tlet until <- expires.get(keyNo)\n\
\t\tlet seen <- used.get(keyNo)\n\
\t\tlet nothing: Option[Bytes] = None\n\
\t\tlet candidate = match kid {{\n\
\t\t\tSome(k) => presented.get(k),\n\
\t\t\tNone => nothing,\n\
\t\t}}\n\
\t\tlet expected = match stored {{\n\
\t\t\tSome(d) => match candidate {{\n\
\t\t\t\tSome(_) => d,\n\
\t\t\t\tNone => dummy,\n\
\t\t\t}},\n\
\t\t\tNone => dummy,\n\
\t\t}}\n\
\t\tlet actual = match candidate {{\n\
\t\t\tSome(c) => c,\n\
\t\t\tNone => first,\n\
\t\t}}\n\
\t\tlet real = match stored {{\n\
\t\t\tSome(_) => candidate.isSome(),\n\
\t\t\tNone => false,\n\
\t\t}}\n\
\t\tlet live = match until {{\n\
\t\t\tSome(t) => now < t,\n\
\t\t\tNone => true,\n\
\t\t}}\n\
\t\tlet same <- ApiKey.equal(expected, actual)\n\
\t\tlet rotated = match kid {{\n\
\t\t\tSome(k) => k != current,\n\
\t\t\tNone => false,\n\
\t\t}}\n\
\t\tlet stale = match seen {{\n\
\t\t\tSome(t) => now - t >= 3600000,\n\
\t\t\tNone => true,\n\
\t\t}}\n\
\t\tlet moved = presented.keys() != known\n\
\t\tlet admitted = same && real && live && !retired\n\
\t\tlet nobody: Option[{check}] = None\n\
\t\tmatch owner {{\n\
\t\t\tSome(who) => if admitted {{\n\
\t\t\t\tEffect.pure(Some({check} {{ identity: who, touch: rotated || stale || moved }}))\n\
\t\t\t}} else {{\n\
\t\t\t\tEffect.pure(nobody)\n\
\t\t\t}},\n\
\t\t\tNone => Effect.pure(nobody),\n\
\t\t}}\n\
\t}}\n\
\n\
\ton call touch(keyNo: String, current: String, first: Bytes, live: List[String], now: Int) -> Effect[Bool] {{\n\
\t\tlet held <- digests.contains(keyNo)\n\
\t\tif held && !retired {{\n\
\t\t\tlet kid <- kids.get(keyNo)\n\
\t\t\tlet rotated = match kid {{\n\
\t\t\t\tSome(k) => k != current,\n\
\t\t\t\tNone => false,\n\
\t\t\t}}\n\
\t\t\tlet _ <- if rotated {{\n\
\t\t\t\tlet _ <- digests.put(keyNo, first)\n\
\t\t\t\tkids.put(keyNo, current)\n\
\t\t\t}} else {{\n\
\t\t\t\tEffect.pure(())\n\
\t\t\t}}\n\
\t\t\tlet seen <- used.get(keyNo)\n\
\t\t\tlet stale = match seen {{\n\
\t\t\t\tSome(t) => now - t >= 3600000,\n\
\t\t\t\tNone => true,\n\
\t\t\t}}\n\
\t\t\tlet _ <- if stale {{\n\
\t\t\t\tused.put(keyNo, now)\n\
\t\t\t}} else {{\n\
\t\t\t\tEffect.pure(())\n\
\t\t\t}}\n\
\t\t\tif live != known {{\n\
\t\t\t\tknown := live\n\
\t\t\t\tknownAt := now\n\
\t\t\t\tEffect.pure(true)\n\
\t\t\t}} else {{\n\
\t\t\t\tEffect.pure(true)\n\
\t\t\t}}\n\
\t\t}} else {{\n\
\t\t\tEffect.pure(false)\n\
\t\t}}\n\
\t}}\n\
\n\
\ton call insert(identity: {identity}, keyNo: String, digest: Bytes, kid: String, label: String, now: Int, until: Option[Int], live: List[String], cap: Int) -> Effect[Int] {{\n\
\t\tif retired {{\n\
\t\t\tEffect.pure(2)\n\
\t\t}} else {{\n\
\t\t\tknown := live\n\
\t\t\tknownAt := now\n\
\t\t\tlet es <- expires.entries.collect()\n\
\t\t\tlet dead = es.filter((e) => e.value <= now).map((e) => e.key)\n\
\t\t\tlet _ <- dead.forEach((k: String) => digests.remove(k))\n\
\t\t\tlet _ <- dead.forEach((k: String) => kids.remove(k))\n\
\t\t\tlet _ <- dead.forEach((k: String) => labels.remove(k))\n\
\t\t\tlet _ <- dead.forEach((k: String) => created.remove(k))\n\
\t\t\tlet _ <- dead.forEach((k: String) => expires.remove(k))\n\
\t\t\tlet _ <- dead.forEach((k: String) => used.remove(k))\n\
\t\t\tlet _ <- dead.forEach((k: String) => scopes.remove(k))\n\
\t\t\tlet n <- digests.size()\n\
\t\t\tif n >= cap {{\n\
\t\t\t\tEffect.pure(1)\n\
\t\t\t}} else {{\n\
\t\t\t\towner := Some(identity)\n\
\t\t\t\tlet _ <- digests.put(keyNo, digest)\n\
\t\t\t\tlet _ <- kids.put(keyNo, kid)\n\
\t\t\t\tlet _ <- labels.put(keyNo, label)\n\
\t\t\t\tlet _ <- created.put(keyNo, now)\n\
\t\t\t\tlet _ <- match until {{\n\
\t\t\t\t\tSome(t) => expires.put(keyNo, t),\n\
\t\t\t\t\tNone => Effect.pure(()),\n\
\t\t\t\t}}\n\
\t\t\t\tlet _ <- scopes.put(keyNo, Set.of([]))\n\
\t\t\t\tEffect.pure(0)\n\
\t\t\t}}\n\
\t\t}}\n\
\t}}\n\
\n\
\ton call labelsOf() -> Effect[Map[String, String]] {{\n\
\t\tlet es <- labels.entries.collect()\n\
\t\tEffect.pure(Map.of(es))\n\
\t}}\n\
\n\
\ton call createdOf() -> Effect[Map[String, Int]] {{\n\
\t\tlet es <- created.entries.collect()\n\
\t\tEffect.pure(Map.of(es))\n\
\t}}\n\
\n\
\ton call expiresOf() -> Effect[Map[String, Int]] {{\n\
\t\tlet es <- expires.entries.collect()\n\
\t\tEffect.pure(Map.of(es))\n\
\t}}\n\
\n\
\ton call usedOf() -> Effect[Map[String, Int]] {{\n\
\t\tlet es <- used.entries.collect()\n\
\t\tEffect.pure(Map.of(es))\n\
\t}}\n\
\n\
\ton call kidsOf() -> Effect[Map[String, String]] {{\n\
\t\tlet es <- kids.entries.collect()\n\
\t\tEffect.pure(Map.of(es))\n\
\t}}\n\
\n\
\ton call expire(now: Int) -> Effect[Int] {{\n\
\t\tlet es <- expires.entries.collect()\n\
\t\tlet ended = es.filter((e) => e.value <= now).map((e) => e.key)\n\
\t\tlet _ <- ended.forEach((k: String) => digests.remove(k))\n\
\t\tlet _ <- ended.forEach((k: String) => kids.remove(k))\n\
\t\tlet _ <- ended.forEach((k: String) => labels.remove(k))\n\
\t\tlet _ <- ended.forEach((k: String) => created.remove(k))\n\
\t\tlet _ <- ended.forEach((k: String) => expires.remove(k))\n\
\t\tlet _ <- ended.forEach((k: String) => used.remove(k))\n\
\t\tlet _ <- ended.forEach((k: String) => scopes.remove(k))\n\
\t\tEffect.pure(ended.length())\n\
\t}}\n\
\n\
\ton call knownOf() -> Effect[List[String]] {{\n\
\t\tEffect.pure(known)\n\
\t}}\n\
\n\
\ton call knownAtOf() -> Effect[Int] {{\n\
\t\tEffect.pure(knownAt)\n\
\t}}\n\
\n\
\ton call sweep(now: Int, live: List[String], current: String) -> Effect[List[Int]] {{\n\
\t\tlet ks <- kids.entries.collect()\n\
\t\tlet es <- expires.entries.collect()\n\
\t\tlet ended = es.filter((e) => e.value <= now).map((e) => e.key)\n\
\t\tlet unkeyed = ks.filter((e) => !live.any((l: String) => l == e.value)).map((e) => e.key)\n\
\t\tlet orphaned = unkeyed.filter((k: String) => !ended.any((x: String) => x == k))\n\
\t\tlet dead = ended.concat(orphaned)\n\
\t\tlet _ <- dead.forEach((k: String) => digests.remove(k))\n\
\t\tlet _ <- dead.forEach((k: String) => kids.remove(k))\n\
\t\tlet _ <- dead.forEach((k: String) => labels.remove(k))\n\
\t\tlet _ <- dead.forEach((k: String) => created.remove(k))\n\
\t\tlet _ <- dead.forEach((k: String) => expires.remove(k))\n\
\t\tlet _ <- dead.forEach((k: String) => used.remove(k))\n\
\t\tlet _ <- dead.forEach((k: String) => scopes.remove(k))\n\
\t\tknown := live\n\
\t\tknownAt := now\n\
\t\tlet left <- kids.values.collect()\n\
\t\tEffect.pure([ended.length(), orphaned.length(), left.filter((k: String) => k != current).length()])\n\
\t}}\n\
\n\
\ton call revoke(keyNo: String) -> Effect[Bool] {{\n\
\t\tlet held <- digests.contains(keyNo)\n\
\t\tif held && !retired {{\n\
\t\t\tlet _ <- digests.remove(keyNo)\n\
\t\t\tlet _ <- kids.remove(keyNo)\n\
\t\t\tlet _ <- labels.remove(keyNo)\n\
\t\t\tlet _ <- created.remove(keyNo)\n\
\t\t\tlet _ <- expires.remove(keyNo)\n\
\t\t\tlet _ <- used.remove(keyNo)\n\
\t\t\tlet _ <- scopes.remove(keyNo)\n\
\t\t\tEffect.pure(true)\n\
\t\t}} else {{\n\
\t\t\tEffect.pure(false)\n\
\t\t}}\n\
\t}}\n\
\n\
\ton call clear() -> Effect[Int] {{\n\
\t\tlet ks <- digests.keys.collect()\n\
\t\tlet n <- digests.size()\n\
\t\tlet _ <- ks.forEach((k: String) => digests.remove(k))\n\
\t\tlet _ <- ks.forEach((k: String) => kids.remove(k))\n\
\t\tlet _ <- ks.forEach((k: String) => labels.remove(k))\n\
\t\tlet _ <- ks.forEach((k: String) => created.remove(k))\n\
\t\tlet _ <- ks.forEach((k: String) => expires.remove(k))\n\
\t\tlet _ <- ks.forEach((k: String) => used.remove(k))\n\
\t\tlet _ <- ks.forEach((k: String) => scopes.remove(k))\n\
\t\tretired := true\n\
\t\tEffect.pure(n)\n\
\t}}\n\
}}\n"
    )
}

/// `<Actor>KeyOwner`. `mint` stores a key under the identity's handle, taking `fresh` as the
/// handle when it has none or its store is retired, and answers the handle used or the store's
/// refusal (1, too many keys).
fn key_owner_agent_text(actor: &str, agent: &str, keys: &str, identity: &str) -> String {
    format!(
        "\n---\n{GENERATED_MARKER} `{actor}`: the handle of one identity's key store.\n---\n\
agent {agent} {{\n\
\tkey identity: {identity}\n\
\n\
\tstore handle: Cell[Option[String]] = None\n\
\n\
\ton call mint(fresh: String, keyNo: String, digest: Bytes, kid: String, label: String, now: Int, until: Option[Int], live: List[String], cap: Int) -> Effect[Result[String, Int]] {{\n\
\t\tlet current = match handle {{\n\
\t\t\tSome(h) => h,\n\
\t\t\tNone => fresh,\n\
\t\t}}\n\
\t\tlet r <- {keys}(current).insert(self.identity, keyNo, digest, kid, label, now, until, live, cap)\n\
\t\tif r == 0 {{\n\
\t\t\thandle := Some(current)\n\
\t\t\tEffect.pure(Ok(current))\n\
\t\t}} else if r == 2 {{\n\
\t\t\tlet again <- {keys}(fresh).insert(self.identity, keyNo, digest, kid, label, now, until, live, cap)\n\
\t\t\thandle := Some(fresh)\n\
\t\t\tif again == 0 {{\n\
\t\t\t\tEffect.pure(Ok(fresh))\n\
\t\t\t}} else {{\n\
\t\t\t\tEffect.pure(Err(again))\n\
\t\t\t}}\n\
\t\t}} else {{\n\
\t\t\tEffect.pure(Err(r))\n\
\t\t}}\n\
\t}}\n\
\n\
\ton call current() -> Effect[Option[String]] {{\n\
\t\tEffect.pure(handle)\n\
\t}}\n\
\n\
\ton call revoke(of: String, keyNo: String) -> Effect[Bool] {{\n\
\t\tlet mine = match handle {{\n\
\t\t\tSome(h) => h == of,\n\
\t\t\tNone => false,\n\
\t\t}}\n\
\t\tif mine {{\n\
\t\t\t{keys}(of).revoke(keyNo)\n\
\t\t}} else {{\n\
\t\t\tEffect.pure(false)\n\
\t\t}}\n\
\t}}\n\
\n\
\ton call revokeAll(fresh: String) -> Effect[Int] {{\n\
\t\tmatch handle {{\n\
\t\t\tSome(h) => {{\n\
\t\t\t\tlet n <- {keys}(h).clear()\n\
\t\t\t\thandle := Some(fresh)\n\
\t\t\t\tEffect.pure(n)\n\
\t\t\t}},\n\
\t\t\tNone => Effect.pure(0),\n\
\t\t}}\n\
\t}}\n\
}}\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> Vec<SourceUnit> {
        let tokens = bynk_syntax::lexer::tokenize(src).expect("lexes");
        bynk_syntax::parser::parse_units(&tokens, src).expect("parses")
    }

    #[test]
    fn a_fragment_context_gains_both_agents_after_the_authors_text() {
        let src = "context shop\n\ntype AccountId = String where NonEmpty\n\nactor Client { auth = ApiKey(secret = \"P\"), identity = AccountId }\n";
        let out = with_api_key_agents(src, &parse(src)).expect("generated");
        assert!(
            out.starts_with(src),
            "the author's text is unchanged, byte for byte"
        );
        assert!(out.contains("agent ClientKeys {"));
        assert!(out.contains("agent ClientKeyOwner {"));
        assert!(!out.contains("consumes"), "no capability is put in scope");
        assert!(
            !out.contains("Crypto"),
            "the agents use the internal comparison"
        );
        let _ = parse(&out);
    }

    /// D2 B: the pepper never enters an agent. The generated source names no pepper, no secret
    /// and no environment read: only digests, `kid`s and the identity cross into it.
    #[test]
    fn the_generated_agents_never_name_the_pepper() {
        let src = "context shop\n\ntype AccountId = String where NonEmpty\n\nactor Client { auth = ApiKey(secret = \"CLIENT_KEY_PEPPER\"), identity = AccountId }\n";
        let out = with_api_key_agents(src, &parse(src)).expect("generated");
        let agents = &out[src.len()..];
        for forbidden in ["CLIENT_KEY_PEPPER", "pepper", "Secrets", "secret", "env"] {
            assert!(
                !agents.contains(forbidden),
                "the agents mention `{forbidden}`"
            );
        }
    }

    #[test]
    fn a_name_the_author_takes_is_left_to_the_checker() {
        let src = "context shop\n\ntype AccountId = String where NonEmpty\n\nactor Client { auth = ApiKey(secret = \"P\"), identity = AccountId }\n\ntype ClientKeys = String\n";
        assert!(with_api_key_agents(src, &parse(src)).is_none());
    }

    #[test]
    fn a_file_without_an_api_key_actor_is_unchanged() {
        let src = "context shop\n\ntype UserId = String where NonEmpty\n\nactor User { auth = Bearer(secret = \"S\"), identity = UserId }\n";
        assert!(with_api_key_agents(src, &parse(src)).is_none());
    }
}

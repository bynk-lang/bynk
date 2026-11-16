//! A Worker group's `index.ts` (#3562, G2b step 2c-4b).
//!
//! A `[deploy.workers.<key>]` group is one Worker over several contexts. Each
//! member's entry is emitted as `<member>.index.ts` in *member mode*
//! (`workers_entry.rs`), which marks the router's own misses; the group's entry
//! is the Worker's `main` and dispatches to the members:
//!
//! - `fetch` tries the members in the order the manifest lists them. A marked
//!   response is a miss and the next member is tried; the first unmarked one is
//!   the answer (a handler's own `404` included). If every member misses, the
//!   answer is the first marked response that is not a `404` (the synthesised
//!   `405`/`204` with `Allow`), else the single-context `Not Found`.
//! - `scheduled` runs every member whose cron matches, `queue` goes to the member
//!   consuming `batch.queue`, `email` to the member claiming the recipient.
//! - Agent, events fan-out and container classes are re-exported by name (a
//!   member entry exports `isMiss`, so `export *` over members would collide).
//!
//! A request to the internal `/_bynk/call`, `/_bynk/event` or `/_bynk/migrate`
//! prefix that no member claims is answered as a standalone Worker answers it
//! unauthenticated: the same method gate, then the same `413` or `401` the
//! runtime's `verifyCall` gives. Answering `404` for an unknown service while a
//! known one answers `401` would let an unsigned caller list a group's services.

use bynk_check::wire_format as wf;
use bynk_ts::{TsProgram, TsStmt};

/// What the group's entry needs to know of one member.
pub struct GroupMember {
    /// The member's qualified context name.
    pub context: String,
    /// The import specifier of its `<member>.index.ts` (`./shop-orders.index.js`).
    pub specifier: String,
    /// Its cron expressions (its entry has `scheduled` iff there are any).
    pub crons: Vec<String>,
    /// The queues it consumes (its entry has `queue` iff there are any).
    pub queues: Vec<String>,
    /// The addresses its `from email` services claim (its entry has `email` iff
    /// there are any).
    pub addresses: Vec<String>,
    /// The Durable Object classes its entry exports: agents, and the events
    /// fan-out class when it publishes.
    pub classes: Vec<String>,
}

/// The group as a whole.
pub struct GroupEntry<'a> {
    pub key: &'a str,
    pub members: &'a [GroupMember],
    /// The container classes of `containers.ts`, exported once for the Worker.
    pub container_classes: &'a [String],
    /// Whether any member subscribes to events (answers `/_bynk/event/`).
    pub has_events: bool,
    /// Whether any member hosts a migratable agent (answers `/_bynk/migrate/`).
    pub has_migrate: bool,
}

/// `["a", "b"]`, as a TypeScript array literal of strings.
fn array(items: &[String]) -> String {
    let items: Vec<String> = items.iter().map(|i| format!("{i:?}")).collect();
    format!("[{}]", items.join(", "))
}

/// The unclaimed-request fallback: what a standalone Worker answers to an
/// unauthenticated request for an internal prefix (see the module doc).
fn unclaimed(entry: &GroupEntry<'_>) -> String {
    let mut text = String::from(
        "async function __unclaimed(request: globalThis.Request, env: unknown): Promise<globalThis.Response> {\n  const path = new globalThis.URL(request.url).pathname;\n",
    );
    // Every Worker answers `/_bynk/call/`.
    text.push_str(&format!(
        "  if (path.startsWith({:?})) {{\n    if (request.method !== {:?}) return new globalThis.Response(null, {{ status: 405, headers: {{ allow: {:?} }} }});\n    return __refused(await __verifyCall(request, env, \"\"));\n  }}\n",
        wf::CALL_PATH_PREFIX,
        wf::CALL_METHOD,
        wf::CALL_METHOD,
    ));
    if entry.has_events {
        text.push_str("  if (path.startsWith(\"/_bynk/event/\")) return __refused(await __verifyCall(request, env, \"\"));\n");
    }
    if entry.has_migrate {
        text.push_str(&format!(
            "  if (path.startsWith({prefix:?})) {{\n    if (typeof (env as unknown as globalThis.Record<string, unknown>)[{key:?}] !== \"string\") return new globalThis.Response(\"Not found\", {{ status: 404 }});\n    if (request.method !== {method:?}) return new globalThis.Response(null, {{ status: 405, headers: {{ allow: {method:?} }} }});\n    if (request.headers.get({caller_header:?}) !== {caller:?}) return new globalThis.Response(\"Unauthorized\", {{ status: 401 }});\n    return __refused(await __verifyCall(request, env, \"\"));\n  }}\n",
            prefix = wf::MIGRATE_PATH_PREFIX,
            key = wf::MIGRATE_KEY,
            method = wf::CALL_METHOD,
            caller_header = wf::CALLER_HEADER,
            caller = wf::MIGRATE_CALLER,
        ));
    }
    text.push_str("  return new globalThis.Response(\"Not Found\", { status: 404 });\n}\n");
    text
}

/// `fetch`: the members in order (see the module doc).
const FETCH: &str = "  async fetch(request: globalThis.Request, env: unknown, ctx: __Ctx): Promise<globalThis.Response> {
    let held: globalThis.Response | undefined;
    for (const member of __members) {
      const response = await member.entry.fetch(request, env as never, ctx);
      if (!member.isMiss(response)) return response;
      if (held === undefined && response.status !== 404) held = response;
    }
    return held ?? (await __unclaimed(request, env));
  },
";

fn scheduled(members: &[(usize, &GroupMember)]) -> String {
    let mut text = String::from(
        "  async scheduled(event: { readonly cron: string }, env: unknown, ctx: __Ctx): Promise<void> {\n    await globalThis.Promise.all([\n",
    );
    for (i, m) in members.iter().filter(|(_, m)| !m.crons.is_empty()) {
        text.push_str(&format!(
            "      ...({}.includes(event.cron) ? [__m{i}.scheduled(event as never, env as never, ctx)] : []),\n",
            array(&m.crons)
        ));
    }
    text.push_str("    ]);\n  },\n");
    text
}

fn queue(members: &[(usize, &GroupMember)]) -> String {
    let mut text = String::from(
        "  async queue(batch: { readonly queue: string }, env: unknown, ctx: __Ctx): Promise<void> {\n    switch (batch.queue) {\n",
    );
    for (i, m) in members.iter().filter(|(_, m)| !m.queues.is_empty()) {
        for q in &m.queues {
            text.push_str(&format!("      case {q:?}:\n"));
        }
        text.push_str(&format!(
            "        return __m{i}.queue(batch as never, env as never, ctx);\n"
        ));
    }
    text.push_str("      default:\n        return;\n    }\n  },\n");
    text
}

/// `email`: the member claiming the recipient, by the runtime's own rule (an
/// exact address, case-insensitively, before a `*@domain` catch-all). With no
/// claimant the first email member gets it, and its runtime rejects an unknown
/// recipient.
fn email(members: &[(usize, &GroupMember)]) -> String {
    let mut text = String::from(
        "  async email(message: { readonly to: string }, env: unknown, ctx: __Ctx): Promise<void> {\n    const to = message.to.toLowerCase();\n    const domain = to.slice(to.lastIndexOf(\"@\") + 1);\n    const routes: readonly { readonly address: string; readonly member: __Email }[] = [\n",
    );
    for (i, m) in members.iter().filter(|(_, m)| !m.addresses.is_empty()) {
        for a in &m.addresses {
            text.push_str(&format!(
                "      {{ address: {:?}, member: __m{i} }},\n",
                a.to_lowercase()
            ));
        }
    }
    let first = members
        .iter()
        .find(|(_, m)| !m.addresses.is_empty())
        .map_or(0, |(i, _)| *i);
    text.push_str(&format!(
        "    ];\n    const claimant = routes.find((r) => r.address === to) ?? routes.find((r) => r.address === `*@${{domain}}`);\n    await (claimant?.member ?? __m{first}).email(message as never, env as never, ctx);\n  }},\n"
    ));
    text
}

/// The group's `index.ts`.
pub fn emit_group_entry(entry: &GroupEntry<'_>) -> TsProgram {
    let mut program = TsProgram::new();
    program.push(TsStmt::comment(
        "Generated by bynk — do not edit by hand.",
        None,
    ));
    let names: Vec<&str> = entry.members.iter().map(|m| m.context.as_str()).collect();
    program.push(TsStmt::comment(
        format!(
            "Worker entry point for group `{}`: members {} (tried in this order).",
            entry.key,
            names.join(", ")
        ),
        None,
    ));
    let members: Vec<(usize, &GroupMember)> = entry.members.iter().enumerate().collect();
    let has = |f: &dyn Fn(&GroupMember) -> bool| members.iter().any(|(_, m)| f(m));
    let (has_cron, has_queue, has_email) = (
        has(&|m| !m.crons.is_empty()),
        has(&|m| !m.queues.is_empty()),
        has(&|m| !m.addresses.is_empty()),
    );
    // The runtime import is a tree node, so the build's runtime-module pass
    // finds it, as it does a member entry's.
    for stmt in crate::emitter::runtime_files::runtime_import_stmts(
        false,
        &["__verifyCall".to_string()],
        "../../runtime.js",
    ) {
        program.push(stmt);
    }
    let mut text = String::new();
    for (i, m) in &members {
        text.push_str(&format!(
            "import __entry{i}, {{ isMiss as __miss{i} }} from {:?};\n",
            m.specifier
        ));
    }
    for (_, m) in &members {
        if !m.classes.is_empty() {
            text.push_str(&format!(
                "export {{ {} }} from {:?};\n",
                m.classes.join(", "),
                m.specifier
            ));
        }
    }
    if !entry.container_classes.is_empty() {
        text.push_str(&format!(
            "export {{ {} }} from \"./containers.js\";\n",
            entry.container_classes.join(", ")
        ));
    }
    text.push_str("\ntype __Ctx = { waitUntil(promise: globalThis.Promise<unknown>): void };\n");
    text.push_str("type __Fetch = { fetch(request: globalThis.Request, env: never, ctx: __Ctx): Promise<globalThis.Response> };\n");
    if has_cron {
        text.push_str("type __Scheduled = { scheduled(event: never, env: never, ctx: __Ctx): Promise<void> };\n");
    }
    if has_queue {
        text.push_str(
            "type __Queue = { queue(batch: never, env: never, ctx: __Ctx): Promise<void> };\n",
        );
    }
    if has_email {
        text.push_str(
            "type __Email = { email(message: never, env: never, ctx: __Ctx): Promise<void> };\n",
        );
    }
    for (i, m) in &members {
        let mut ty = vec!["__Fetch"];
        if !m.crons.is_empty() {
            ty.push("__Scheduled");
        }
        if !m.queues.is_empty() {
            ty.push("__Queue");
        }
        if !m.addresses.is_empty() {
            ty.push("__Email");
        }
        text.push_str(&format!("const __m{i}: {} = __entry{i};\n", ty.join(" & ")));
    }
    text.push_str("\nconst __members: readonly { readonly entry: __Fetch; readonly isMiss: (response: globalThis.Response) => boolean }[] = [\n");
    for (i, _) in &members {
        text.push_str(&format!("  {{ entry: __m{i}, isMiss: __miss{i} }},\n"));
    }
    text.push_str("];\n\n");
    text.push_str(
        "function __refused(verified: string | globalThis.Response): globalThis.Response {\n  return typeof verified === \"string\" ? new globalThis.Response(\"Unauthorized\", { status: 401 }) : verified;\n}\n\n",
    );
    text.push_str(&unclaimed(entry));
    text.push_str("\nexport default {\n");
    text.push_str(FETCH);
    if has_cron {
        text.push_str(&scheduled(&members));
    }
    if has_queue {
        text.push_str(&queue(&members));
    }
    if has_email {
        text.push_str(&email(&members));
    }
    text.push_str("};\n");
    program.push(TsStmt::raw(text, None));
    program
}

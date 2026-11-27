//! `Container` instances (#3477, slice C1): the validation of each option of
//! `capability Renderer = Container @build("…") @port(8080) …`, and the name
//! of the Durable Object class generated for an instance.
//!
//! These are the pure rules. Whether `<build>/Dockerfile` exists is a question
//! about the project's files, which the driver answers
//! ([`dockerfile_problem`]); the checker has no filesystem.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use bynk_project::{ParsedFile, UnitKind};
use bynk_syntax::ast::{
    Block, CommonsItem, Expr, ExprKind, Handler, HandlerKind, MatchBody, ServiceProtocol, Statement,
};
use bynk_syntax::error::CompileError;
use bynk_syntax::span::Span;

use crate::firstparty;
use crate::project_model::ErrorSink;
use crate::symbols::UnitTable;

/// The prefix of the class generated for a `Container` instance, reserved
/// beside `__EventsFanout`: `capability Renderer = Container` is
/// `__Container_Renderer`. A user `agent` may not use it
/// (`bynk.container.class_collision`).
pub const CONTAINER_CLASS_PREFIX: &str = "__Container_";

/// The name of the Durable Object class generated for the `Container`
/// instance `capability`.
pub fn container_class_name(capability: &str) -> String {
    format!("{CONTAINER_CLASS_PREFIX}{capability}")
}

/// Why `@build`'s value is not a directory the project can name, or `None`
/// when it is: relative, inside the project (no `..`, no absolute or drive
/// path, no backslash) and not empty. A trailing `/` and `.` segments are
/// tolerated, and [`normalise_build`] removes them.
pub fn build_path_problem(path: &str) -> Option<&'static str> {
    if path.is_empty() {
        return Some("is empty");
    }
    if path.contains('\\') {
        return Some("uses `\\`; write a `/`-separated path");
    }
    if path.contains(['\0', '\n', '\r']) || path.chars().any(char::is_control) {
        return Some("holds a control character");
    }
    if path.starts_with('/')
        || matches!(path.as_bytes(), [d, b':'] | [d, b':', b'/', ..] if d.is_ascii_alphabetic())
    {
        return Some("is absolute; it must be relative to the project root");
    }
    if path.split('/').any(|seg| seg == "..") {
        return Some("leaves the project with `..`");
    }
    if normalise_build(path).is_empty() {
        return Some("names the project root, not a directory beneath it");
    }
    None
}

/// `@build`'s directory as a project-relative path without a trailing `/`,
/// empty segments or `.` segments (`./containers//renderer/` becomes
/// `containers/renderer`).
pub fn normalise_build(path: &str) -> String {
    path.split('/')
        .filter(|seg| !seg.is_empty() && *seg != ".")
        .collect::<Vec<_>>()
        .join("/")
}

/// The Dockerfile a `@build` directory names: `<dir>/Dockerfile`.
pub fn dockerfile_path(build: &str) -> String {
    format!("{}/Dockerfile", normalise_build(build))
}

/// Why `@port` is not a port, or `None` when it is (1 to 65535).
pub fn port_problem(value: i64) -> Option<&'static str> {
    (!(1..=65535).contains(&value)).then_some("is a TCP port, 1 to 65535")
}

/// `@sleepAfter` as whole seconds: more than zero and at most 6 hours, with no
/// part second. `None` otherwise.
pub fn sleep_after_seconds(millis: i64) -> Option<i64> {
    (millis > 0
        && millis % 1000 == 0
        && millis / 1000 <= firstparty::CONTAINER_MAX_SLEEP_AFTER_SECONDS)
        .then_some(millis / 1000)
}

/// Whether `name` is a Worker secret `@forward` may name: an identifier a
/// process reads from its environment (letters, digits and `_`, not starting
/// with a digit), at most 128 characters.
pub fn is_forwardable_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.len() <= 128
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Why a `@forward` list is not one, or `None` when it is: at most
/// [`firstparty::CONTAINER_MAX_FORWARD`] names, each a plain identifier, none
/// repeated.
pub fn forward_problem(items: &[String]) -> Option<String> {
    if items.len() > firstparty::CONTAINER_MAX_FORWARD {
        return Some(format!(
            "`@forward` lists {} names, and a container takes at most {}",
            items.len(),
            firstparty::CONTAINER_MAX_FORWARD
        ));
    }
    items.iter().enumerate().find_map(|(i, item)| {
        if !is_forwardable_name(item) {
            return Some(format!(
                "`{}` is not a name a container's environment can hold",
                item.escape_debug()
            ));
        }
        items[..i]
            .contains(item)
            .then(|| format!("`{item}` is listed twice"))
    })
}

/// The problem with the `Dockerfile` of a `@build` directory, looked up by
/// `has_file` (project-relative path to whether it is a file), or `None` when
/// it is there.
pub fn dockerfile_problem(build: &str, has_file: &dyn Fn(&str) -> bool) -> Option<String> {
    let file = dockerfile_path(build);
    (!has_file(&file)).then(|| format!("`{file}` does not exist"))
}

/// `bynk.container.build_missing`: every `Container` instance's `@build`
/// directory holds a `Dockerfile` under `project_root`. A `@build` that is not
/// a project path is already refused when the option is read
/// (`bynk.container.build_path`), so it is skipped here.
pub fn phase_container_builds(
    groups: &BTreeMap<String, Vec<usize>>,
    kinds: &BTreeMap<String, UnitKind>,
    parsed: &[ParsedFile],
    unit_tables: &HashMap<String, UnitTable>,
    project_root: &Path,
    errors: &mut ErrorSink,
) {
    let has_file = |rel: &str| project_root.join(rel).is_file();
    for (name, indices) in groups {
        if kinds.get(name) != Some(&UnitKind::Context) {
            continue;
        }
        let Some(table) = unit_tables.get(name) else {
            continue;
        };
        for &i in indices {
            if parsed[i].is_synthetic() {
                continue;
            }
            let path = parsed[i].identity_path();
            for item in parsed[i].items() {
                let CommonsItem::Capability(c) = item else {
                    continue;
                };
                let Some(build) = table
                    .derived
                    .get(&c.name.name)
                    .and_then(|d| d.container_build())
                else {
                    continue;
                };
                let Some(problem) = dockerfile_problem(build, &has_file) else {
                    continue;
                };
                errors.push_for(
                    Some(&path),
                    CompileError::new(
                        "bynk.container.build_missing",
                        c.span,
                        format!(
                            "the container `{}` has no Dockerfile: {problem}",
                            c.name.name
                        ),
                    )
                    .with_note(format!(
                        "`@build(\"{build}\")` names the directory that holds the Dockerfile, relative to the project root; create `{}`",
                        dockerfile_path(build)
                    )),
                );
            }
        }
    }
}

/// Why `name` cannot be a container's `@instances(by: …)` key, or `None` when it can: the type
/// must be declared in the context, and be a refined type with a refinement or an opaque type, over
/// `String`. The key's construction (`.of`, an opaque constructor) is where an author bounds the set
/// of names, so a plain alias of `String` that adds no check does not qualify.
pub fn instance_key_problem(
    name: &str,
    types: &HashMap<String, std::sync::Arc<bynk_syntax::ast::TypeDecl>>,
) -> Option<String> {
    use bynk_syntax::ast::{BaseType, TypeBody};
    let Some(decl) = types.get(name) else {
        return Some(format!("`{name}` is not a type declared in this context"));
    };
    match &decl.body {
        TypeBody::Refined {
            base: BaseType::String,
            refinement: Some(_),
            ..
        }
        | TypeBody::Opaque {
            base: BaseType::String,
            ..
        } => None,
        _ => Some(format!(
            "`{name}` is not a refined or opaque type over `String`, so it would not bound the names"
        )),
    }
}

/// Re-types the first parameter of the operations that name an instance, from `String` to the
/// instance's key type `key` (`@instances(by: key)`).
pub fn key_instance_operations(ops: &mut [bynk_syntax::ast::CapabilityOp], key: &str) {
    for op in ops
        .iter_mut()
        .filter(|op| INSTANCE_OPERATIONS.contains(&op.name.name.as_str()))
    {
        if let Some(first) = op.params.first_mut() {
            let span = first.type_ref.span();
            first.type_ref = bynk_syntax::ast::TypeRef::Named(bynk_syntax::ast::Ident {
                name: key.to_string(),
                span,
            });
        }
    }
}

/// The `Container` operations whose first argument names the instance.
const INSTANCE_OPERATIONS: &[&str] = &["send", "receiveBytes", "sendBytes"];

/// One call that names a container instance from request input.
#[derive(Debug, PartialEq, Eq)]
pub struct InstanceFromInput {
    /// The instance argument.
    pub span: Span,
    /// `Renderer.send`.
    pub call: String,
    /// The handler input the name depends on (`tenant`).
    pub input: String,
}

/// The names that depend on a handler input, each with the input it came from.
type Inputs = HashMap<String, String>;

/// `inputs` less the names a new scope binds, which shadow them.
fn without<'a>(inputs: &Inputs, names: impl IntoIterator<Item = &'a str>) -> Inputs {
    let mut scope = inputs.clone();
    for name in names {
        scope.remove(name);
    }
    scope
}

/// The first handler input `e` depends on, or `None`.
///
/// A value depends on an input when it mentions one, or a local bound from one: an interpolation,
/// a method call on it, an `if`. It does not follow a call to a function (`normalise(tenant)`): that
/// is where an author bounds the set, so a name laundered through one is not warned about, rather
/// than warned about wrongly (#4112). A lambda is not entered either.
///
/// It does not look at a value's type, so a `Bool`-valued input, or an enum interpolated directly,
/// still counts as an input although the name can take only a few values; mapping it through a
/// `match` onto its names is how a closed set is written (and is not warned about).
///
/// A binding that reuses an input's name (`let tenant = "shared"`, a match arm's pattern, a
/// block's own `let`) stops the name being an input in its scope.
fn depends_on_input(e: &Expr, inputs: &Inputs) -> Option<String> {
    match &e.kind {
        ExprKind::Ident(id) => inputs.get(&id.name).cloned(),
        ExprKind::Call { .. } | ExprKind::Lambda(_) => None,
        // The value of a `match` or an `if` is one of its branches, not what it tests: mapping a
        // value onto a closed set (`match plan { Free => "free", Pro => "pro" }`) is how a set is
        // bounded, so the scrutinee and the condition are not followed, only the branches.
        ExprKind::Match { arms, .. } => arms.iter().find_map(|arm| {
            let scope = without(
                inputs,
                arm.pattern
                    .bound_names()
                    .into_iter()
                    .map(|i| i.name.as_str()),
            );
            match &arm.body {
                MatchBody::Expr(body) => depends_on_input(body, &scope),
                MatchBody::Block(block) => block_depends_on_input(block, &scope),
            }
        }),
        ExprKind::If {
            then_block,
            else_block,
            ..
        } => block_depends_on_input(then_block, inputs)
            .or_else(|| block_depends_on_input(else_block, inputs)),
        ExprKind::Block(block) => block_depends_on_input(block, inputs),
        _ => bynk_syntax::ast::expr_children(e)
            .into_iter()
            .find_map(|c| depends_on_input(c, inputs)),
    }
}

/// [`depends_on_input`] for a block's value, its tail, after the block's own bindings.
fn block_depends_on_input(block: &Block, inputs: &Inputs) -> Option<String> {
    let mut scope = inputs.clone();
    for statement in &block.statements {
        bind_statement(&mut scope, statement);
    }
    depends_on_input(&block.tail, &scope)
}

/// What `statement` does to which names depend on an input once it has run. A pure `let x = …`
/// from an input makes `x` an input; any other binding of `x` (from something that does not
/// depend on an input, or from an effect's result, which is not followed) makes it not one, so a
/// parameter that is rebound stops being an input.
fn bind_statement(inputs: &mut Inputs, statement: &Statement) {
    match statement {
        Statement::Let(l) => match depends_on_input(&l.value, inputs) {
            Some(origin) => {
                inputs.insert(l.name.name.clone(), origin);
            }
            None => {
                inputs.remove(&l.name.name);
            }
        },
        Statement::EffectLet(l) => {
            inputs.remove(&l.name.name);
        }
        Statement::Par(p) => {
            for inner in &p.statements {
                if let Statement::Let(l) | Statement::EffectLet(l) = inner {
                    inputs.remove(&l.name.name);
                }
            }
        }
        _ => {}
    }
}

/// Every call in `e` of an instance operation on a container in `containers`.
fn collect_instance_calls(
    e: &Expr,
    containers: &HashSet<String>,
    inputs: &Inputs,
    out: &mut Vec<InstanceFromInput>,
) {
    if let ExprKind::MethodCall {
        receiver,
        method,
        args,
        ..
    } = &e.kind
        && INSTANCE_OPERATIONS.contains(&method.name.as_str())
        && let ExprKind::Ident(cap) = &receiver.kind
        && containers.contains(&cap.name)
        && let Some(instance) = args.first()
        && let Some(input) = depends_on_input(instance, inputs)
    {
        out.push(InstanceFromInput {
            span: instance.span,
            call: format!("{}.{}", cap.name, method.name),
            input,
        });
    }
    match &e.kind {
        ExprKind::Block(block) => collect_in_block(block, containers, inputs, out),
        ExprKind::If {
            cond,
            then_block,
            else_block,
        } => {
            collect_instance_calls(cond, containers, inputs, out);
            collect_in_block(then_block, containers, inputs, out);
            collect_in_block(else_block, containers, inputs, out);
        }
        ExprKind::Match { discriminant, arms } => {
            collect_instance_calls(discriminant, containers, inputs, out);
            for arm in arms {
                let scope = without(
                    inputs,
                    arm.pattern
                        .bound_names()
                        .into_iter()
                        .map(|i| i.name.as_str()),
                );
                if let Some(guard) = &arm.guard {
                    collect_instance_calls(guard, containers, &scope, out);
                }
                match &arm.body {
                    MatchBody::Expr(body) => collect_instance_calls(body, containers, &scope, out),
                    MatchBody::Block(block) => collect_in_block(block, containers, &scope, out),
                }
            }
        }
        ExprKind::Lambda(l) => {
            let scope = without(inputs, l.params.iter().map(|p| p.name.name.as_str()));
            collect_instance_calls(&l.body, containers, &scope, out);
        }
        _ => {
            for child in bynk_syntax::ast::expr_children(e) {
                collect_instance_calls(child, containers, inputs, out);
            }
        }
    }
}

/// [`collect_instance_calls`] through a block's statements in order, each seeing what the ones
/// before it bound, then its tail.
fn collect_in_block(
    block: &Block,
    containers: &HashSet<String>,
    inputs: &Inputs,
    out: &mut Vec<InstanceFromInput>,
) {
    let mut scope = inputs.clone();
    for statement in &block.statements {
        let mut exprs: Vec<&Expr> = Vec::new();
        bynk_syntax::ast::statement_exprs(statement, &mut exprs);
        for e in exprs {
            collect_instance_calls(e, containers, &scope, out);
        }
        bind_statement(&mut scope, statement);
    }
    collect_instance_calls(&block.tail, containers, &scope, out);
}

/// The calls in `handler` that name a container instance (one of `containers`) from the handler's
/// own input: a path, query, header, cookie or body parameter, or a local bound from one, in
/// statement order.
///
/// The check follows values through pure `let`s, in the handler body and in the blocks inside it,
/// and stops at what it cannot follow: a binding from the result of an effect, a function's result,
/// a lambda. So it can miss a name; a name that is rebound (to anything but a value that depends on
/// an input) is no longer one, so it does not report one wrongly for that.
pub fn instances_from_input(
    handler: &Handler,
    containers: &HashSet<String>,
) -> Vec<InstanceFromInput> {
    // Each name that depends on an input, with the input it came from (a parameter, or the
    // parameter a local was bound from).
    let inputs: Inputs = handler
        .params
        .iter()
        .map(|p| (p.name.name.clone(), p.name.name.clone()))
        .collect();
    let mut found = Vec::new();
    collect_in_block(&handler.body, containers, &inputs, &mut found);
    found
}

/// `bynk.container.unbounded_instances` (a warning, #4112): a `Container` instance name built from
/// a request's own input on a route whose caller is not authenticated. Every distinct name starts
/// its own container, and a running container costs money, so anyone who can reach the route can
/// start as many as they have values to send.
pub fn phase_container_instance_names(
    groups: &BTreeMap<String, Vec<usize>>,
    parsed: &[ParsedFile],
    unit_tables: &HashMap<String, UnitTable>,
    errors: &mut ErrorSink,
) {
    for (name, indices) in groups {
        let Some(table) = unit_tables.get(name) else {
            continue;
        };
        let containers: HashSet<String> = table
            .derived
            .iter()
            // A keyed instance (`@instances(by: K)`, #4112) is checked like any other: a key type
            // bounds a name's shape, not how many there are, so it does not make an open route safe
            // (#4584). A route served by an authenticated actor is never checked.
            .filter(|(_, d)| d.is_container())
            .map(|(cap, _)| cap.clone())
            .collect();
        if containers.is_empty() {
            continue;
        }
        for &i in indices {
            if parsed[i].is_synthetic() {
                continue;
            }
            let path = parsed[i].identity_path();
            for item in parsed[i].items() {
                let CommonsItem::Service(svc) = item else {
                    continue;
                };
                if !matches!(svc.protocol, ServiceProtocol::Http) {
                    continue;
                }
                for h in &svc.handlers {
                    let HandlerKind::Http {
                        method,
                        path: route,
                    } = &h.kind
                    else {
                        continue;
                    };
                    let Some(by) = h.by_clause.as_ref().or(svc.default_by.as_ref()) else {
                        continue;
                    };
                    if !crate::actors::sum_has_open_member(by, &table.actors) {
                        continue;
                    }
                    for call in instances_from_input(h, &containers) {
                        errors.push_for(
                            Some(&path),
                            CompileError::new(
                                "bynk.container.unbounded_instances",
                                call.span,
                                format!(
                                    "`{}` names a container instance from `{}`, which any caller of `{} {route}` chooses, and each distinct name starts its own container",
                                    call.call,
                                    call.input,
                                    method.as_str()
                                ),
                            )
                            .with_note(
                                "a running container costs money, and nothing here limits how many names a caller can send; serve the route `by` an authenticated actor, or choose the name from a fixed set (an enum's names, a constant, the authenticated identity)",
                            ),
                        );
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The calls `instances_from_input` finds in the first service handler of `src`, as
    /// `(call, input)`.
    fn found(src: &str) -> Vec<(String, String)> {
        let tokens = bynk_syntax::lexer::tokenize(src).expect("lex");
        let unit = bynk_syntax::parser::parse_unit(&tokens, src).expect("parse");
        let bynk_syntax::ast::SourceUnit::Context(ctx) = unit else {
            panic!("expected a context unit");
        };
        let containers: HashSet<String> = ["Renderer".to_string()].into();
        ctx.items
            .iter()
            .filter_map(|i| match i {
                CommonsItem::Service(s) => s.handlers.first(),
                _ => None,
            })
            .flat_map(|h| instances_from_input(h, &containers))
            .map(|c| (c.call, c.input))
            .collect()
    }

    fn handler(body: &str) -> String {
        format!(
            "context t\n\nservice api from http {{\n  on GET(\"/x/:tenant\") (tenant: String) -> Effect[HttpResult[String]] by Visitor {{\n{body}\n  }}\n}}\n"
        )
    }

    #[test]
    fn an_instance_name_from_a_parameter_is_found_through_interpolation_and_locals() {
        let direct = handler("    let r <- Renderer.send(tenant, req)\n    Ok(\"\")");
        assert_eq!(found(&direct), [("Renderer.send".into(), "tenant".into())]);
        let interpolated =
            handler("    let r <- Renderer.send(\"t-\\(tenant)\", req)\n    Ok(\"\")");
        assert_eq!(found(&interpolated).len(), 1);
        // A local keeps the input it came from, through a chain of locals.
        let chained = handler(
            "    let a = tenant.toLower()\n    let b = a.concat(\"-x\")\n    let r <- Renderer.receiveBytes(b, req)\n    Ok(\"\")",
        );
        assert_eq!(
            found(&chained),
            [("Renderer.receiveBytes".into(), "tenant".into())]
        );
    }

    #[test]
    fn a_bounded_name_is_not_found() {
        for body in [
            "    let r <- Renderer.send(\"shared\", req)\n    Ok(\"\")",
            // Through a function: where a set is bounded, so not followed.
            "    let r <- Renderer.send(shard(tenant), req)\n    Ok(\"\")",
            // What a `match` tests is not what it yields.
            "    let n = match tenant {\n      \"a\" => \"one\"\n      _ => \"two\"\n    }\n    let r <- Renderer.send(n, req)\n    Ok(\"\")",
            // Another capability's `send`.
            "    let r <- Other.send(tenant, req)\n    Ok(\"\")",
            // An effect result is not followed.
            "    let n <- Lookup.get(tenant)\n    let r <- Renderer.send(n, req)\n    Ok(\"\")",
        ] {
            assert_eq!(found(&handler(body)), [], "{body}");
        }
    }

    #[test]
    fn a_rebound_parameter_is_no_longer_an_input() {
        for body in [
            // A pure `let` of something that does not depend on an input.
            "    let tenant = \"shared\"\n    let r <- Renderer.send(tenant, req)\n    Ok(\"\")",
            // An effect bind: its result is not followed.
            "    let tenant <- Tenants.canonical(tenant)\n    let r <- Renderer.send(tenant, req)\n    Ok(\"\")",
            // A block's own `let`.
            "    let n = if true {\n      let tenant = \"shared\"\n      tenant\n    } else {\n      \"x\"\n    }\n    let r <- Renderer.send(n, req)\n    Ok(\"\")",
            // A match arm's pattern reusing the name.
            "    let n = match Plans.pick() {\n      tenant => tenant\n    }\n    let r <- Renderer.send(n, req)\n    Ok(\"\")",
            // The same, with the call inside the arm.
            "    match Plans.pick() {\n      tenant => Renderer.send(tenant, req)\n    }",
            // A lambda parameter reusing the name, with the call inside the lambda.
            "    let f = (tenant) => Renderer.send(tenant, req)\n    Ok(\"\")",
        ] {
            assert_eq!(found(&handler(body)), [], "{body}");
        }
    }

    #[test]
    fn a_name_still_depends_on_an_input_inside_a_nested_scope() {
        for body in [
            // A block that uses the parameter, not a rebound name.
            "    let n = if true {\n      let other = \"x\"\n      tenant\n    } else {\n      \"x\"\n    }\n    let r <- Renderer.send(n, req)\n    Ok(\"\")",
            // The call inside an `if` branch.
            "    if true {\n      Renderer.send(tenant, req)\n    } else {\n      Ok(\"\")\n    }",
            // A match arm that binds another name.
            "    match Plans.pick() {\n      other => Renderer.send(tenant, req)\n    }",
            // Rebinding to a value that depends on the input keeps it one.
            "    let tenant = tenant.toLower()\n    let r <- Renderer.send(tenant, req)\n    Ok(\"\")",
        ] {
            assert_eq!(
                found(&handler(body)),
                [("Renderer.send".into(), "tenant".into())],
                "{body}"
            );
        }
    }

    #[test]
    fn a_build_path_stays_inside_the_project() {
        assert_eq!(build_path_problem("containers/renderer"), None);
        assert_eq!(build_path_problem("./containers/renderer/"), None);
        assert!(build_path_problem("").is_some());
        assert!(build_path_problem("../x").is_some());
        assert!(build_path_problem("a/../../x").is_some());
        assert!(build_path_problem("/abs").is_some());
        assert!(build_path_problem("C:/abs").is_some());
        assert_eq!(build_path_problem("a:b/c"), None);
        assert!(build_path_problem("a\\b").is_some());
        assert!(build_path_problem(".").is_some());
        assert!(build_path_problem("a\nb").is_some());
    }

    #[test]
    fn a_build_path_is_normalised() {
        assert_eq!(
            normalise_build("./containers//renderer/"),
            "containers/renderer"
        );
        assert_eq!(
            dockerfile_path("containers/renderer/"),
            "containers/renderer/Dockerfile"
        );
    }

    #[test]
    fn sleep_after_is_whole_seconds_up_to_six_hours() {
        assert_eq!(sleep_after_seconds(300_000), Some(300));
        assert_eq!(sleep_after_seconds(21_600_000), Some(21_600));
        assert_eq!(sleep_after_seconds(21_601_000), None);
        assert_eq!(sleep_after_seconds(0), None);
        assert_eq!(sleep_after_seconds(1_500), None);
    }

    #[test]
    fn forwarded_names_are_plain_identifiers() {
        assert!(forward_problem(&["API_KEY".into(), "_x1".into()]).is_none());
        assert!(forward_problem(&["1X".into()]).is_some());
        assert!(forward_problem(&["A-B".into()]).is_some());
        assert!(forward_problem(&["A".into(), "A".into()]).is_some());
        assert!(port_problem(0).is_some() && port_problem(65536).is_some());
        assert!(port_problem(8080).is_none());
    }
}

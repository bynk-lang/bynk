//! A producer's message type, checked against the queue's consumer (#3879).
//!
//! Both sides of a queue name the same string, so the compiler is the only
//! party that sees both. When some context consumes the queue a
//! `Queue @name("q")` instance sends to, a `sendValue`, `sendValueWith` or
//! `sendValueBatch` call must send a type the consumer's `on message` accepts,
//! decided by the structural comparison a cross-context call gets (ADR 0714:
//! [`crate::contract_compat`]), so a stronger refinement is admitted where the
//! consumer expects a weaker one. The untyped `send`, `sendWith` and
//! `sendBatch` take JSON text the compiler cannot check; they warn when the
//! project consumes the queue. A queue nobody in the project consumes has no
//! type to compare against, and nothing is reported.
//!
//! Two reports: `bynk.queue.message_type_mismatch` (an error, labelled at the
//! call and at the consumer's parameter) and `bynk.queue.untyped_send` (a
//! warning).

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;

use bynk_project::{ParsedFile, UnitKind};
use bynk_syntax::ast::{
    Block, CommonsItem, Expr, HandlerKind, Param, ServiceProtocol, SourceUnit, TypeRef,
    expr_children, statement_exprs,
};
use bynk_syntax::error::CompileError;
use bynk_syntax::span::Span;

use crate::checker::{Callee, Types};
use crate::context_checks::ts_type_ref_display;
use crate::contract_compat::{Compatibility, ServiceShape, Verdict, check_call, shape_of};
use crate::project_model::{ErrorSink, TypeDecl, UnitCallees};
use crate::symbols::{UnitTable, contract_types_for};
use crate::ty_ref::ty_to_type_ref;

type TypeTable = HashMap<String, Arc<TypeDecl>>;

/// The operations that send a typed value, and the untyped ones.
const TYPED_SENDS: [&str; 3] = ["sendValue", "sendValueWith", "sendValueBatch"];
const UNTYPED_SENDS: [&str; 3] = ["send", "sendWith", "sendBatch"];

/// Whether `callee` is a send operation of some capability. The driver keeps
/// just these (and the other callees its readers need) from each unit's
/// checked program; the phase decides whether the capability is a queue.
pub fn is_queue_send(callee: &Callee) -> bool {
    matches!(callee, Callee::Capability { op, .. } if is_send_op(op))
}

/// Whether `op` names a send operation of the `Queue` capability.
pub fn is_send_op(op: &str) -> bool {
    TYPED_SENDS.contains(&op) || UNTYPED_SENDS.contains(&op)
}

/// A queue's consumer: where it is and the message type it takes.
pub struct Consumer<'a> {
    pub context: &'a str,
    pub file: usize,
    pub param: &'a Param,
}

/// The consumer of each queue the project reads, by queue name. A queue with
/// two consumers is reported elsewhere (`bynk.queue.duplicate_consumer`); the
/// first, in source order, is the one a producer is compared with.
pub fn consumers_of<'a>(
    parsed: &'a [ParsedFile],
    groups: &'a BTreeMap<String, Vec<usize>>,
    kinds: &BTreeMap<String, UnitKind>,
) -> HashMap<&'a str, Consumer<'a>> {
    let mut out: HashMap<&str, Consumer> = HashMap::new();
    for (ctx_name, indices) in groups {
        if kinds.get(ctx_name) != Some(&UnitKind::Context) {
            continue;
        }
        for &i in indices {
            let SourceUnit::Context(ctx) = parsed[i].unit() else {
                continue;
            };
            for item in &ctx.items {
                let CommonsItem::Service(s) = item else {
                    continue;
                };
                let ServiceProtocol::Queue { name } = &s.protocol else {
                    continue;
                };
                let Some(param) = s
                    .handlers
                    .iter()
                    .filter(|h| matches!(h.kind, HandlerKind::Message))
                    .find_map(|h| {
                        h.params
                            .iter()
                            .find(|p| !bynk_syntax::queue::is_queue_delivery(&p.type_ref))
                    })
                else {
                    continue;
                };
                out.entry(name.as_str()).or_insert(Consumer {
                    context: ctx_name,
                    file: i,
                    param,
                });
            }
        }
    }
    out
}

/// The queue each `Queue` instance of `table` sends to, by instance name.
pub fn queue_instances(table: &UnitTable) -> HashMap<&str, &str> {
    table
        .derived
        .iter()
        .filter_map(|(name, d)| d.queue().map(|q| (name.as_str(), q)))
        .collect()
}

/// Can the consumer decode every value of `sent`? `sent` is the sender's type
/// in the producer's namespace, `taken` the consumer's in its own. The same
/// comparison a cross-context call's single parameter gets.
pub fn message_compat(
    sent: &TypeRef,
    sent_types: &TypeTable,
    taken: &TypeRef,
    taken_types: &TypeTable,
) -> Compatibility {
    let shape = |t: &TypeRef, types: &TypeTable| ServiceShape {
        name: "message".to_string(),
        params: vec![(
            "message".to_string(),
            shape_of(t, types, &mut Vec::new(), &HashMap::new()),
        )],
        ret: crate::contract_compat::Shape::Unit,
    };
    check_call(&shape(sent, sent_types), &shape(taken, taken_types))
}

/// What one send site sends, as the checker instantiated it: the type of
/// `message`, or of each element for `sendValueBatch`. `None` where the type
/// is still abstract (a type parameter), which the check skips.
fn sent_type(op: &str, encoded: Option<crate::checker::TyId>, tys: &Arc<Types>) -> Option<TypeRef> {
    let t = ty_to_type_ref(encoded?, tys)?;
    match (op, t) {
        ("sendValueBatch", TypeRef::List(inner, _)) => Some(*inner),
        ("sendValueBatch", _) => None,
        (_, t) => Some(t),
    }
}

/// The bodies of a context file that can call a capability.
fn bodies(items: &[CommonsItem]) -> Vec<&Block> {
    let mut out = Vec::new();
    for item in items {
        match item {
            CommonsItem::Fn(f) => out.push(&f.body),
            CommonsItem::Service(s) => out.extend(s.handlers.iter().map(|h| &h.body)),
            CommonsItem::Agent(a) => out.extend(a.handlers.iter().map(|h| &h.body)),
            CommonsItem::Provider(p) => out.extend(p.ops.iter().map(|op| &op.body)),
            _ => {}
        }
    }
    out
}

/// Check every send to a queue the project consumes.
#[allow(clippy::too_many_arguments)]
pub fn phase_queue_producers(
    parsed: &[ParsedFile],
    groups: &BTreeMap<String, Vec<usize>>,
    kinds: &BTreeMap<String, UnitKind>,
    unit_tables: &HashMap<String, UnitTable>,
    unit_uses: &HashMap<String, Vec<String>>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_callees: &UnitCallees,
    tys: &Arc<Types>,
    errors: &mut ErrorSink,
) {
    let consumers = consumers_of(parsed, groups, kinds);
    if consumers.is_empty() {
        return;
    }
    for (ctx_name, indices) in groups {
        if kinds.get(ctx_name) != Some(&UnitKind::Context) {
            continue;
        }
        let (Some(table), Some(callees)) = (unit_tables.get(ctx_name), unit_callees.get(ctx_name))
        else {
            continue;
        };
        let instances = queue_instances(table);
        if instances.is_empty() {
            continue;
        }
        let own_types = contract_types_for(ctx_name, unit_tables, unit_uses, unit_consumes).0;
        for &i in indices {
            let SourceUnit::Context(ctx) = parsed[i].unit() else {
                continue;
            };
            let file = parsed[i].identity_path();
            let mut stack: Vec<&Expr> = Vec::new();
            for body in bodies(&ctx.items) {
                for s in &body.statements {
                    statement_exprs(s, &mut stack);
                }
                stack.push(&body.tail);
            }
            // Source order, so the diagnostics read the same on every run.
            let mut sites: Vec<(Span, &str, &str, Option<crate::checker::TyId>)> = Vec::new();
            while let Some(e) = stack.pop() {
                if let Some(Callee::Capability { cap, op, wire, .. }) = callees.get(&e.id)
                    && let Some(queue) = instances.get(cap.as_str())
                    && is_send_op(op)
                {
                    let encoded = wire
                        .as_ref()
                        .and_then(|w| w.encode.first().copied().flatten());
                    sites.push((e.span, queue, op.as_str(), encoded));
                }
                stack.extend(expr_children(e));
            }
            sites.sort_by_key(|(span, ..)| *span);
            for (span, queue, op, encoded) in sites {
                let Some(consumer) = consumers.get(queue) else {
                    continue;
                };
                check_site(
                    (span, queue, op, encoded),
                    consumer,
                    &own_types,
                    &Project {
                        tables: unit_tables,
                        uses: unit_uses,
                        consumes: unit_consumes,
                    },
                    tys,
                    &file,
                    errors,
                );
            }
        }
    }
}

/// The project tables a type namespace is built from.
struct Project<'a> {
    tables: &'a HashMap<String, UnitTable>,
    uses: &'a HashMap<String, Vec<String>>,
    consumes: &'a HashMap<String, Vec<String>>,
}

fn check_site(
    (span, queue, op, encoded): (Span, &str, &str, Option<crate::checker::TyId>),
    consumer: &Consumer,
    own_types: &TypeTable,
    project: &Project,
    tys: &Arc<Types>,
    file: &Path,
    errors: &mut ErrorSink,
) {
    if UNTYPED_SENDS.contains(&op) {
        errors.push_for(Some(file), untyped_send(span, queue, op, consumer));
        return;
    }
    let Some(sent) = sent_type(op, encoded, tys) else {
        return;
    };
    let taken = &consumer.param.type_ref;
    let taken_types = contract_types_for(
        consumer.context,
        project.tables,
        project.uses,
        project.consumes,
    )
    .0;
    let verdict = message_compat(&sent, own_types, taken, &taken_types);
    if verdict.verdict != Verdict::Breaking {
        return;
    }
    errors.push_for(
        Some(file),
        mismatch(span, queue, op, &sent, consumer, &verdict),
    );
}

fn mismatch(
    span: Span,
    queue: &str,
    op: &str,
    sent: &TypeRef,
    consumer: &Consumer,
    verdict: &Compatibility,
) -> CompileError {
    let taken = &consumer.param.type_ref;
    let mut err = CompileError::new(
        "bynk.queue.message_type_mismatch",
        span,
        format!(
            "`{op}` sends `{}` to queue `{queue}`, but its consumer in `{}` takes `{}`",
            ts_type_ref_display(sent),
            consumer.context,
            ts_type_ref_display(taken),
        ),
    )
    .with_label(
        consumer.param.span,
        format!(
            "`{queue}` is consumed here, in context `{}`",
            consumer.context
        ),
    );
    for f in verdict.findings.iter().filter(|f| f.breaking) {
        err = err.with_note(format!("{}: {}", f.path, f.message));
    }
    err.with_note(
        "the consumer decodes each message as its own type, so a message it cannot decode would be retried, then dead-lettered or dropped",
    )
}

fn untyped_send(span: Span, queue: &str, op: &str, consumer: &Consumer) -> CompileError {
    let typed = match op {
        "send" => "sendValue",
        "sendWith" => "sendValueWith",
        _ => "sendValueBatch",
    };
    CompileError::new(
        "bynk.queue.untyped_send",
        span,
        format!(
            "`{op}` sends JSON text the compiler cannot check against the consumer of queue `{queue}`"
        ),
    )
    .with_label(
        consumer.param.span,
        format!(
            "consumed here, in context `{}`, as `{}`",
            consumer.context,
            ts_type_ref_display(&consumer.param.type_ref)
        ),
    )
    .with_note(format!(
        "`{typed}` encodes a value and has its type checked against the consumer; keep `{op}` to forward a body that is already encoded"
    ))
}

#[cfg(test)]
mod tests {
    use crate::stability::StabilityPolicy;

    fn analyse(files: &[(&str, &str)]) -> crate::testkit::Analysed {
        crate::testkit::analyse_with(files, Some(StabilityPolicy::permissive()))
    }

    const TYPES: &str = "type Order = { id: String, qty: Int }\ntype Refund = { id: String }\ntype Qty = Int where InRange(1, 100)\ntype Line = { qty: Qty }\ntype Loose = { qty: Int }\n";

    const CONSUMER: &str = "context shop.fulfil\n\nservice fulfil from queue(\"order-events\") {\n  delivery { deadLetter: None }\n  on message(order: Order) -> Effect[QueueResult] { Ack }\n}\n";

    fn producer(call: &str) -> String {
        format!(
            "context shop.orders\n\nconsumes bynk.cloudflare {{ Queue }}\n\ncapability OrderEvents = Queue @name(\"order-events\")\n\n{TYPES}\nservice go {{\n  on call(o: Order) -> Effect[()] by Caller given OrderEvents {{\n    {call}\n  }}\n}}\n"
        )
    }

    fn categories(call: &str, consumer: &str) -> Vec<&'static str> {
        let consumer = format!("{}{TYPES}", consumer);
        analyse(&[
            ("shop/orders.bynk", &producer(call)),
            ("shop/fulfil.bynk", &consumer),
        ])
        .categories()
    }

    #[test]
    fn a_matching_value_is_clean() {
        assert!(categories("OrderEvents.sendValue(o)", CONSUMER).is_empty());
    }

    #[test]
    fn a_different_type_is_refused() {
        assert_eq!(
            categories("OrderEvents.sendValue(Refund { id: o.id })", CONSUMER),
            ["bynk.queue.message_type_mismatch"]
        );
    }

    #[test]
    fn a_batch_checks_its_element_type() {
        assert!(categories("OrderEvents.sendValueBatch([o])", CONSUMER).is_empty());
        assert_eq!(
            categories(
                "OrderEvents.sendValueBatch([Refund { id: o.id }])",
                CONSUMER
            ),
            ["bynk.queue.message_type_mismatch"]
        );
    }

    #[test]
    fn untyped_sends_warn_only_when_a_consumer_exists() {
        assert_eq!(
            categories("OrderEvents.send(\"{}\")", CONSUMER),
            ["bynk.queue.untyped_send"]
        );
        let none = analyse(&[("shop/orders.bynk", &producer("OrderEvents.send(\"{}\")"))]);
        assert!(none.categories().is_empty());
    }

    fn consumer_of(ty: &str) -> String {
        format!(
            "context shop.fulfil\n\nservice fulfil from queue(\"order-events\") {{\n  delivery {{ deadLetter: None }}\n  on message(m: {ty}) -> Effect[QueueResult] {{ Ack }}\n}}\n"
        )
    }

    #[test]
    fn a_stronger_refinement_is_admitted_where_a_weaker_one_is_expected() {
        let line = "OrderEvents.sendValue(Line { qty: 1 })";
        assert!(categories(line, &consumer_of("Loose")).is_empty());
        assert_eq!(
            categories(
                "OrderEvents.sendValue(Loose { qty: 1 })",
                &consumer_of("Line")
            ),
            ["bynk.queue.message_type_mismatch"]
        );
    }

    #[test]
    fn a_producer_and_consumer_in_one_context_are_compared() {
        let source = |call: &str| {
            format!(
                "context shop.orders\n\nconsumes bynk.cloudflare {{ Queue }}\n\ncapability OrderEvents = Queue @name(\"order-events\")\n\n{TYPES}\nservice go {{\n  on call(o: Order) -> Effect[()] by Caller given OrderEvents {{\n    {call}\n  }}\n}}\n\nservice fulfil from queue(\"order-events\") {{\n  delivery {{ deadLetter: None }}\n  on message(order: Order) -> Effect[QueueResult] {{ Ack }}\n}}\n"
            )
        };
        assert!(
            analyse(&[("shop/orders.bynk", &source("OrderEvents.sendValue(o)"))])
                .categories()
                .is_empty()
        );
        assert_eq!(
            analyse(&[(
                "shop/orders.bynk",
                &source("OrderEvents.sendValue(Refund { id: o.id })")
            )])
            .categories(),
            ["bynk.queue.message_type_mismatch"]
        );
    }

    #[test]
    fn a_send_of_a_type_parameter_is_not_checked() {
        let source = "context shop.orders\n\nconsumes bynk.cloudflare { Queue }\n\ncapability OrderEvents = Queue @name(\"order-events\")\n\nfn forward[T: Codable](x: T) -> Effect[()] given OrderEvents {\n  OrderEvents.sendValue(x)\n}\n";
        let consumer = format!("{}{TYPES}", consumer_of("Order"));
        let a = analyse(&[
            ("shop/orders.bynk", source),
            ("shop/fulfil.bynk", &consumer),
        ]);
        assert!(a.categories().is_empty(), "{:?}", a.categories());
    }
}

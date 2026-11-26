//! #3583: the project-wide facts behind `bynk/architectureModel`'s `events`
//! and `uses` sections — who emits and who subscribes to each event, which
//! capabilities each handler requires through `given`, and which suites
//! `stub` a capability.
//!
//! Until this module, event subscriptions were computed only to be validated
//! ([`crate::project_model::phase_event_subscriptions`]) and a handler's
//! resolved `given` was read only for the sequence diagram. A client could
//! approximate either by text search, which breaks on aliases and `consumes`
//! qualification. These facts are resolved the way the checker resolves them:
//!
//! - an event's *owner* is the context whose `event` table declares it, found
//!   locally first and then through the consuming context's `consumes`, as
//!   [`crate::symbols::discover_event_subscribers`] does;
//! - an *emitter* is a call the checker resolved to `Events.emit` (its own
//!   [`crate::checker::Callee`], not the receiver's spelling), with the event
//!   type the checker recorded on the call's `Callee` (its type argument, written or inferred);
//! - a handler's `given` set is read after [`crate::project_model::normalize_service_defaults`],
//!   so a service's `default_given` is already part of each handler that
//!   declares none of its own; a use is marked `inherited` when its span lies
//!   outside the handler (it is written on the service header);
//! - a capability reference resolves to its providing unit through the unit's
//!   `consumes` aliases and flattened selections, as
//!   [`crate::project_model::handler_cross_caps`] does.
//!
//! Spans are bare byte offsets against the project-relative `file` they are
//! paired with. Nothing here carries source text.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use bynk_project::{ParsedFile, UnitKind};
use bynk_syntax::ast::{
    CapRef, CommonsItem, Handler, HandlerKind, ServiceDecl, ServiceProtocol, SourceUnit, TypeRef,
    expr_children, statement_exprs,
};
use bynk_syntax::span::Span;

use crate::project_model::{UnitCallees, is_events_emit, resolve_consume_prefix};
use crate::symbols::UnitTable;

/// A project-relative file and a span within it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FactSite {
    pub file: PathBuf,
    pub span: Span,
}

/// What declares the member that emits, subscribes or uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MemberKind {
    Service,
    Agent,
    /// A `provides Cap = Name { op … }` operation body (it can emit, and
    /// carries its provider's `given`).
    Provider,
}

/// One handler (or provider operation) of one unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberFact {
    /// The declaring unit's qualified name.
    pub unit: String,
    pub kind: MemberKind,
    /// The service, agent or provider name (`provider_name` for a provider).
    pub container: String,
    /// The handler's request descriptor (`call`, `call addItem`,
    /// `http GET /x`, `event`, …), or a provider operation's name.
    pub handler: String,
    /// The handler's own span.
    pub site: FactSite,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmitterFact {
    pub member: MemberFact,
    /// The `Events.emit[E](…)` call.
    pub site: FactSite,
}

/// How a subscription selects what it is delivered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryKind {
    /// `from Events(E)`: every emission.
    All,
    /// `from Events(E { f: v, .. })`: emissions whose payload matches.
    Pattern,
    /// `from Events(E) via schema(N)`: emissions of that schema version.
    Schema,
    /// Both a payload pattern and a `via schema(…)` clause.
    PatternSchema,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscriberFact {
    /// The subscribing service's `on event` handler.
    pub member: MemberFact,
    pub delivery: DeliveryKind,
    /// The `from Events(…)` header.
    pub site: FactSite,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventFact {
    /// The context that declares the event.
    pub owner: String,
    pub name: String,
    /// The `event` declaration.
    pub site: FactSite,
    pub emitters: Vec<EmitterFact>,
    pub subscribers: Vec<SubscriberFact>,
}

/// One capability a handler requires through `given`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UseFact {
    /// The capability's simple name.
    pub capability: String,
    /// The unit that declares it: this unit, a consumed one, or `bynk`.
    pub provider_unit: Option<String>,
    /// Written on the service header (`default_given`), not on the handler.
    /// Always `false` for a provider operation, whose `given` is its provider's.
    pub inherited: bool,
    /// The `given` entry.
    pub site: FactSite,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandlerUsesFact {
    pub member: MemberFact,
    pub uses: Vec<UseFact>,
    /// Model-diagrams slice 5: the services and agents the handler calls, as
    /// the checker resolved them. One entry per target, at its first call.
    pub calls: Vec<CallFact>,
}

/// What a handler calls (model-diagrams slice 5).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum CallTarget {
    /// A consumed context's service (`B.service(…)`, `Alias.service(…)`).
    Service { unit: String, service: String },
    /// One of this unit's own agents, and the handler dispatched to.
    Agent { agent: String, handler: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallFact {
    pub target: CallTarget,
    /// The first call to `target` in the handler.
    pub site: FactSite,
}

/// A `stub` of one capability in one suite (suite-level or in a case).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StubFact {
    /// The unit the suite tests.
    pub target: String,
    pub capability: String,
    /// The `stub` clause.
    pub site: FactSite,
}

/// Everything `bynk/architectureModel` needs beyond a unit's own declarations.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArchitectureFacts {
    pub events: Vec<EventFact>,
    pub uses: Vec<HandlerUsesFact>,
    pub stubs: Vec<StubFact>,
}

/// The per-unit resolution tables the facts read.
pub struct FactInputs<'a> {
    pub parsed: &'a [ParsedFile],
    pub groups: &'a BTreeMap<String, Vec<usize>>,
    pub test_groups: &'a BTreeMap<String, Vec<usize>>,
    pub kinds: &'a BTreeMap<String, UnitKind>,
    pub unit_tables: &'a HashMap<String, UnitTable>,
    pub unit_consumes: &'a HashMap<String, Vec<String>>,
    pub unit_consumes_aliases: &'a HashMap<String, HashMap<String, String>>,
    pub unit_flattened: &'a HashMap<String, HashMap<String, String>>,
    pub callees: &'a UnitCallees,
}

/// Compute the facts. Deterministic: units, events, members and uses are
/// sorted, so the wire form does not churn between rounds.
pub fn architecture_facts(inp: &FactInputs<'_>) -> ArchitectureFacts {
    let mut events: BTreeMap<(String, String), EventFact> = BTreeMap::new();
    let mut uses = Vec::new();

    // Declared events first: an emitter or subscriber in one unit attaches to
    // an event another unit declares, whichever is visited first.
    for (unit, indices) in inp.groups {
        if !matches!(
            inp.kinds.get(unit),
            Some(UnitKind::Context | UnitKind::Adapter)
        ) {
            continue;
        }
        for &i in indices {
            let pf = &inp.parsed[i];
            for item in pf.items() {
                if let CommonsItem::Event(e) = item {
                    events.insert(
                        (unit.clone(), e.name.name.clone()),
                        EventFact {
                            owner: unit.clone(),
                            name: e.name.name.clone(),
                            site: FactSite {
                                file: pf.identity_path(),
                                span: e.name.span,
                            },
                            emitters: Vec::new(),
                            subscribers: Vec::new(),
                        },
                    );
                }
            }
        }
    }

    for (unit, indices) in inp.groups {
        if !matches!(
            inp.kinds.get(unit),
            Some(UnitKind::Context | UnitKind::Adapter)
        ) {
            continue;
        }
        let mut cx = UnitCx::new(inp, unit);
        for &i in indices {
            cx.walk_file(&inp.parsed[i], &mut events, &mut uses);
        }
    }

    let mut events: Vec<EventFact> = events.into_values().collect();
    for e in &mut events {
        let key = |m: &MemberFact| (m.unit.clone(), m.container.clone(), m.handler.clone());
        e.emitters.sort_by(|a, b| {
            (key(&a.member), &a.site.file, a.site.span.start).cmp(&(
                key(&b.member),
                &b.site.file,
                b.site.span.start,
            ))
        });
        e.subscribers.sort_by_key(|a| key(&a.member));
    }
    uses.sort_by_key(|a: &HandlerUsesFact| {
        (
            a.member.unit.clone(),
            a.member.container.clone(),
            a.member.handler.clone(),
        )
    });
    ArchitectureFacts {
        events,
        uses,
        stubs: stub_facts(inp),
    }
}

struct UnitCx<'a> {
    inp: &'a FactInputs<'a>,
    unit: &'a str,
    consumed: &'a [String],
    aliases: Option<&'a HashMap<String, String>>,
    flattened: Option<&'a HashMap<String, String>>,
    callees: Option<&'a HashMap<bynk_syntax::ast::ExprId, crate::checker::Callee>>,
}

impl<'a> UnitCx<'a> {
    fn new(inp: &'a FactInputs<'a>, unit: &'a str) -> Self {
        UnitCx {
            inp,
            unit,
            consumed: inp.unit_consumes.get(unit).map_or(&[], Vec::as_slice),
            aliases: inp.unit_consumes_aliases.get(unit),
            flattened: inp.unit_flattened.get(unit),
            callees: inp.callees.get(unit),
        }
    }

    /// The context that declares event `name`, as seen from this unit.
    fn event_owner(&self, name: &str) -> Option<String> {
        let declares = |u: &str| {
            self.inp
                .unit_tables
                .get(u)
                .is_some_and(|t| t.events.contains_key(name))
        };
        if declares(self.unit) {
            return Some(self.unit.to_string());
        }
        self.consumed.iter().find(|c| declares(c)).cloned()
    }

    fn resolve_cap(&self, cap: &CapRef) -> Option<String> {
        match cap.prefix() {
            Some(p) => {
                resolve_consume_prefix(&p, self.consumed, self.aliases.unwrap_or(&HashMap::new()))
            }
            None => match self.flattened.and_then(|f| f.get(cap.key())) {
                Some(u) => Some(u.clone()),
                None => Some(self.unit.to_string()),
            },
        }
    }

    fn walk_file(
        &mut self,
        pf: &ParsedFile,
        events: &mut BTreeMap<(String, String), EventFact>,
        uses: &mut Vec<HandlerUsesFact>,
    ) {
        let file = pf.identity_path();
        let site = |span: Span| FactSite {
            file: file.clone(),
            span,
        };
        for item in pf.items() {
            match item {
                CommonsItem::Service(s) => {
                    for h in &s.handlers {
                        let member = self.member(MemberKind::Service, &s.name.name, h, &site);
                        self.handler(member, h, Some(s), &site, events, uses);
                    }
                }
                CommonsItem::Agent(a) => {
                    for h in &a.handlers {
                        let member = self.member(MemberKind::Agent, &a.name.name, h, &site);
                        self.handler(member, h, None, &site, events, uses);
                    }
                }
                CommonsItem::Provider(p) => {
                    for op in &p.ops {
                        let member = MemberFact {
                            unit: self.unit.to_string(),
                            kind: MemberKind::Provider,
                            container: p.provider_name.name.clone(),
                            handler: op.name.name.clone(),
                            site: site(op.span),
                        };
                        self.emits(&member, &op.body, &site, events);
                        uses.push(HandlerUsesFact {
                            uses: self.use_facts(&p.given, None, &site),
                            calls: self.calls(&op.body, &site),
                            member,
                        });
                    }
                }
                _ => {}
            }
        }
    }

    fn member(
        &self,
        kind: MemberKind,
        container: &str,
        h: &Handler,
        site: &dyn Fn(Span) -> FactSite,
    ) -> MemberFact {
        MemberFact {
            unit: self.unit.to_string(),
            kind,
            container: container.to_string(),
            handler: handler_label(h),
            site: site(h.span),
        }
    }

    fn handler(
        &self,
        member: MemberFact,
        h: &Handler,
        service: Option<&ServiceDecl>,
        site: &dyn Fn(Span) -> FactSite,
        events: &mut BTreeMap<(String, String), EventFact>,
        uses: &mut Vec<HandlerUsesFact>,
    ) {
        if let Some(ServiceProtocol::Events {
            event_type: TypeRef::Named(id),
            pattern,
            schema_dispatch,
        }) = service.map(|s| &s.protocol)
            && h.kind == HandlerKind::Event
            && let Some(owner) = self.event_owner(&id.name)
            && let Some(fact) = events.get_mut(&(owner, id.name.clone()))
        {
            let delivery = match (pattern.is_some(), schema_dispatch.is_some()) {
                (false, false) => DeliveryKind::All,
                (true, false) => DeliveryKind::Pattern,
                (false, true) => DeliveryKind::Schema,
                (true, true) => DeliveryKind::PatternSchema,
            };
            fact.subscribers.push(SubscriberFact {
                member: member.clone(),
                delivery,
                site: site(id.span),
            });
        }
        self.emits(&member, &h.body, site, events);
        uses.push(HandlerUsesFact {
            uses: self.use_facts(&h.given, Some(h.span), site),
            calls: self.calls(&h.body, site),
            member,
        });
    }

    fn use_facts(
        &self,
        given: &[CapRef],
        handler_span: Option<Span>,
        site: &dyn Fn(Span) -> FactSite,
    ) -> Vec<UseFact> {
        given
            .iter()
            .map(|c| UseFact {
                capability: c.key().to_string(),
                provider_unit: self.resolve_cap(c),
                inherited: handler_span
                    .is_some_and(|h| c.span.start < h.start || c.span.end > h.end),
                site: site(c.span),
            })
            .collect()
    }

    /// The services and agents a body calls, in source order, one entry per
    /// target (its first call).
    fn calls(
        &self,
        body: &bynk_syntax::ast::Block,
        site: &dyn Fn(Span) -> FactSite,
    ) -> Vec<CallFact> {
        let Some(callees) = self.callees else {
            return Vec::new();
        };
        let mut found: Vec<(CallTarget, Span)> = Vec::new();
        let mut stack = Vec::new();
        for s in &body.statements {
            statement_exprs(s, &mut stack);
        }
        stack.push(&body.tail);
        while let Some(e) = stack.pop() {
            let target = match callees.get(&e.id) {
                Some(crate::checker::Callee::Cross { unit, service }) => {
                    Some(CallTarget::Service {
                        unit: unit.clone(),
                        service: service.clone(),
                    })
                }
                Some(crate::checker::Callee::Agent { agent, handler }) => Some(CallTarget::Agent {
                    agent: agent.clone(),
                    handler: handler.clone(),
                }),
                _ => None,
            };
            if let Some(target) = target {
                match found.iter_mut().find(|(t, _)| *t == target) {
                    Some((_, at)) if e.span.start < at.start => *at = e.span,
                    Some(_) => {}
                    None => found.push((target, e.span)),
                }
            }
            stack.extend(expr_children(e));
        }
        found.sort_by_key(|(_, span)| span.start);
        found
            .into_iter()
            .map(|(target, span)| CallFact {
                target,
                site: site(span),
            })
            .collect()
    }

    fn emits(
        &self,
        member: &MemberFact,
        body: &bynk_syntax::ast::Block,
        site: &dyn Fn(Span) -> FactSite,
        events: &mut BTreeMap<(String, String), EventFact>,
    ) {
        let Some(callees) = self.callees else { return };
        let mut stack = Vec::new();
        for s in &body.statements {
            statement_exprs(s, &mut stack);
        }
        stack.push(&body.tail);
        while let Some(e) = stack.pop() {
            if let Some(crate::checker::Callee::Capability {
                event: Some(name), ..
            }) = callees.get(&e.id)
                && callees.get(&e.id).is_some_and(is_events_emit)
                && let Some(owner) = self.event_owner(name)
                && let Some(fact) = events.get_mut(&(owner, name.clone()))
            {
                fact.emitters.push(EmitterFact {
                    member: member.clone(),
                    site: site(e.span),
                });
            }
            stack.extend(expr_children(e));
        }
    }
}

/// The request descriptor of a handler, as the sequence diagram labels it.
fn handler_label(h: &Handler) -> String {
    match &h.kind {
        HandlerKind::Call => h
            .method_name
            .as_ref()
            .map_or_else(|| "call".to_string(), |m| m.name.clone()),
        HandlerKind::Http { method, path } => format!("{} {}", method.as_str(), path),
        HandlerKind::Cron { expr } => format!("cron \"{expr}\""),
        HandlerKind::Message => "message".to_string(),
        HandlerKind::Open => "open".to_string(),
        HandlerKind::Close => "close".to_string(),
        HandlerKind::Event => "event".to_string(),
        HandlerKind::Rejected => "rejected".to_string(),
    }
}

fn stub_facts(inp: &FactInputs<'_>) -> Vec<StubFact> {
    let mut out = Vec::new();
    for (target, indices) in inp.test_groups {
        for &i in indices {
            let pf = &inp.parsed[i];
            let SourceUnit::Suite(suite) = pf.unit() else {
                continue;
            };
            let file = pf.identity_path();
            let stubs = suite
                .stubs
                .iter()
                .chain(suite.cases.iter().flat_map(|c| c.stubs.iter()));
            for s in stubs {
                out.push(StubFact {
                    target: target.clone(),
                    capability: s.capability.name.clone(),
                    site: FactSite {
                        file: file.clone(),
                        span: s.span,
                    },
                });
            }
        }
    }
    out.sort_by(|a, b| {
        (&a.target, &a.capability, &a.site.file, a.site.span.start).cmp(&(
            &b.target,
            &b.capability,
            &b.site.file,
            b.site.span.start,
        ))
    });
    out
}

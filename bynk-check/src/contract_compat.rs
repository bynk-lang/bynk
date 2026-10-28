//! #2485: is a service-contract change compatible, or breaking?
//!
//! [`crate::contract`] reduces a service to a canonical text and an FNV-1a hash,
//! and the deploy gate (ADR 0491) counts any difference as skew. A hash says
//! *whether* a contract moved, never *how*. This module keeps the structure the
//! hash throws away, so the toolchain can say whether a caller compiled against
//! one contract still works against a callee providing another, and why.
//!
//! Three pieces, all here so the deploy gate and the editor share one answer:
//!
//! 1. [`ServiceShape`]: the contract as a tree, built by the same walk as
//!    [`crate::contract::service_normal_form`] and rendering back to exactly
//!    that text ([`ServiceShape::normal_form`]), so a stored shape can be
//!    checked against the hash it was recorded with.
//! 2. [`ServiceShape::to_text`] / [`ServiceShape::from_text`]: a compact,
//!    unambiguous text for the ledger and the build manifest. The normal form
//!    itself cannot be parsed back (`AgeInt where Positive` does not say where
//!    the type's name stops and its base begins), so the ledger keeps this
//!    beside the hash instead.
//! 3. [`check_call`]: the classifier.
//!
//! ## The rule
//!
//! Compatibility is about **values on the wire**, so the rules are the wire
//! format's own (`reference/wire-format.md`): a record is an object keyed by
//! field name, an unknown key is ignored, a missing `Option` field is `None`,
//! a bare value is accepted where an `Option` is expected, and a multi-argument
//! call is an object keyed by parameter name. A request flows caller to callee
//! and a response callee to caller; in both, the receiver must be able to
//! decode everything the sender can produce. That one question,
//! `flows(sender, receiver)`, is asked of the request with the caller as sender
//! and of the response with the callee as sender, which is how "add an optional
//! request field" is compatible and "add a response variant" is breaking from a
//! single definition.
//!
//! The classifier errs towards **breaking**. It cannot see a field's default,
//! so an added field is compatible only when it is an `Option`. A wrong
//! "compatible" lets a breaking deploy through; a wrong "breaking" only names a
//! skew the gate already refuses.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use bynk_syntax::ast::{TypeBody, TypeDecl, TypeRef};

use crate::contract::canon_pred_set;
use crate::resolver::CrossContextService;

/// One type as it appears on the wire. Mirrors the `Contract` rendering of
/// [`crate::contract`]: named types expand structurally, record fields and sum
/// variants are sorted, predicates are a sorted set, and an opaque type's
/// predicate is left out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shape {
    Base(String),
    Unit,
    List(Box<Shape>),
    Option(Box<Shape>),
    Result(Box<Shape>, Box<Shape>),
    Map(Box<Shape>, Box<Shape>),
    HttpResult(Box<Shape>),
    /// A name with nothing to expand: a runtime-known type, or one outside the
    /// callee's table.
    Named(String),
    /// A back-reference to an enclosing named type, closing a recursive shape.
    Ref(String),
    Record {
        name: String,
        fields: Vec<Field>,
    },
    Sum {
        name: String,
        variants: Vec<Variant>,
    },
    Refined {
        name: String,
        base: String,
        preds: Vec<String>,
    },
    Opaque {
        name: String,
        base: String,
    },
    /// A generic instantiation: `head` is the declaration expanded over the
    /// arguments, which are kept so the form renders as the normal form does.
    App {
        head: Box<Shape>,
        args: Vec<Shape>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    pub ty: Shape,
    /// The field's own inline refinement (#1969), as a sorted predicate set.
    pub preds: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variant {
    pub name: String,
    /// The variant's payload fields, keyed by name on the wire and sorted like
    /// a record's. Inline refinements are not part of a payload's form.
    pub payload: Vec<Field>,
}

/// One `on call` service contract as a tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceShape {
    pub name: String,
    pub params: Vec<(String, Shape)>,
    pub ret: Shape,
}

impl ServiceShape {
    /// Build the shape of `svc` in the callee's own namespace (`types`), as
    /// [`crate::contract::service_normal_form`] does.
    pub fn of(svc: &CrossContextService, types: &HashMap<String, Arc<TypeDecl>>) -> Self {
        let build = |t: &TypeRef| shape_of(t, types, &mut Vec::new(), &HashMap::new());
        ServiceShape {
            name: svc.name.clone(),
            params: svc
                .params
                .iter()
                .map(|(n, t)| (n.clone(), build(t)))
                .collect(),
            ret: build(&svc.return_type),
        }
    }

    /// The canonical normal form, byte-for-byte what
    /// [`crate::contract::service_normal_form`] renders for the same service.
    pub fn normal_form(&self) -> String {
        let mut out = format!("{}(", self.name);
        for (i, (name, ty)) in self.params.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            let _ = write!(out, "{name}: {}", render(ty));
        }
        let _ = write!(out, ") -> {}", render(&self.ret));
        out
    }

    /// The hash of [`ServiceShape::normal_form`], i.e. the service's contract
    /// hash.
    pub fn hash(&self) -> String {
        crate::contract::contract_hash(&self.normal_form())
    }

    /// The ledger and manifest text. Round-trips through
    /// [`ServiceShape::from_text`].
    pub fn to_text(&self) -> String {
        let mut args = vec![Node::Str(self.name.clone())];
        for (name, ty) in &self.params {
            args.push(Node::call(
                "P",
                vec![Node::Str(name.clone()), shape_node(ty)],
            ));
        }
        args.push(Node::call("Ret", vec![shape_node(&self.ret)]));
        let mut out = String::new();
        Node::call("Svc", args).write(&mut out);
        out
    }

    /// Read [`ServiceShape::to_text`] back. `None` for anything else: a stored
    /// shape that does not parse is treated as absent, never guessed at.
    pub fn from_text(text: &str) -> Option<Self> {
        let node = Node::parse(text)?;
        let Node::Call(head, mut args) = node else {
            return None;
        };
        if head != "Svc" || args.len() < 2 {
            return None;
        }
        let Node::Str(name) = args.remove(0) else {
            return None;
        };
        let Node::Call(ret_head, mut ret_args) = args.pop()? else {
            return None;
        };
        if ret_head != "Ret" || ret_args.len() != 1 {
            return None;
        }
        let ret = node_shape(&ret_args.remove(0))?;
        let mut params = Vec::new();
        for p in args {
            let Node::Call(h, mut a) = p else {
                return None;
            };
            if h != "P" || a.len() != 2 {
                return None;
            }
            let ty = node_shape(&a.pop()?)?;
            let Node::Str(pname) = a.pop()? else {
                return None;
            };
            params.push((pname, ty));
        }
        Some(ServiceShape { name, params, ret })
    }

    /// A stored shape is trusted only if it renders to the form that hashes to
    /// the hash the ledger recorded beside it. A stale, hand-edited or
    /// differently-versioned entry then reads as no shape at all.
    pub fn from_text_checked(text: &str, hash: &str) -> Option<Self> {
        Self::from_text(text).filter(|s| s.hash() == hash)
    }
}

/// A context's own `on call` shapes as ledger text, keyed by service name —
/// the structured twin of [`crate::contract::own_contract_hashes`], built from
/// the same projection and the same table so each text hashes to the hash
/// recorded beside it.
pub fn own_contract_shapes(
    table: &crate::symbols::UnitTable,
    own_types: &HashMap<String, Arc<TypeDecl>>,
) -> std::collections::BTreeMap<String, String> {
    table
        .services
        .iter()
        .filter_map(|(name, decl)| {
            let svc = crate::resolver::cross_context_service_for(name, decl)?;
            Some((name.clone(), ServiceShape::of(&svc, own_types).to_text()))
        })
        .collect()
}

/// #2554: what a callee accepts of the older contracts a lock lists for one
/// service.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Accepted {
    /// The hashes the callee accepts besides its own, in lock order, each the
    /// hash of a kept shape.
    pub hashes: Vec<String>,
    /// The shape texts kept, parallel to `hashes`.
    pub kept: Vec<String>,
    /// The entries left out, each with the reason.
    pub rejected: Vec<(String, String)>,
}

/// Decide which of `older` (shape texts from the contracts lock) the callee
/// whose service has `current` accepts.
///
/// Each entry is checked against `current` itself with [`check_call`]: it is
/// kept only if a caller compiled against it still works against `current`.
/// Compatibility is never carried through intermediate versions, so an entry
/// that was compatible with the contract before this one is dropped if it is
/// not compatible with this one. An entry for another service, one that does
/// not parse, or one equal to `current` is left out (the last silently: the
/// callee already accepts its own hash).
pub fn accepted_for(current: &ServiceShape, older: &[String]) -> Accepted {
    let mut out = Accepted::default();
    for text in older {
        let Some(old) = ServiceShape::from_text(text) else {
            out.rejected.push((
                text.clone(),
                "it is not a contract shape this version can read".into(),
            ));
            continue;
        };
        if old.name != current.name {
            out.rejected.push((
                text.clone(),
                format!(
                    "it is the contract of `{}`, not `{}`",
                    old.name, current.name
                ),
            ));
            continue;
        }
        let hash = old.hash();
        if hash == current.hash() || out.hashes.contains(&hash) {
            continue;
        }
        let verdict = check_call(&old, current);
        if verdict.verdict == Verdict::Breaking {
            let why = verdict
                .breaking()
                .next()
                .map(|f| format!("{}: {}", f.path, f.message))
                .unwrap_or_default();
            out.rejected
                .push((text.clone(), format!("callers on it would break ({why})")));
            continue;
        }
        out.hashes.push(hash);
        out.kept.push(text.clone());
    }
    out
}

/// #2554: the older contracts one context's services accept, from the contracts
/// lock, with what was left out.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ContextAccepted {
    /// Service to the hashes it accepts besides its own. A service accepting
    /// none is absent.
    pub hashes: std::collections::BTreeMap<String, Vec<String>>,
    /// `(service, why)` for each lock entry left out.
    pub stale: Vec<(String, String)>,
    /// `(service, count)` for each service accepting more than `cap`.
    pub oversized: Vec<(String, usize)>,
}

/// Which older contracts each of a context's `on call` services accepts.
/// `older(service)` is the lock's shape texts for it. `cap` is how many a
/// service may keep before it is reported.
pub fn own_accepted<'a>(
    table: &crate::symbols::UnitTable,
    own_types: &HashMap<String, Arc<TypeDecl>>,
    older: impl Fn(&str) -> &'a [String],
    cap: usize,
) -> ContextAccepted {
    let mut out = ContextAccepted::default();
    for (name, decl) in &table.services {
        let texts = older(name);
        if texts.is_empty() {
            continue;
        }
        let Some(svc) = crate::resolver::cross_context_service_for(name, decl) else {
            for _ in texts {
                out.stale.push((
                    name.clone(),
                    "the service is no longer an `on call` service".into(),
                ));
            }
            continue;
        };
        let current = ServiceShape::of(&svc, own_types);
        let accepted = accepted_for(&current, texts);
        for (_, why) in accepted.rejected {
            out.stale.push((name.clone(), why));
        }
        if accepted.hashes.len() > cap {
            out.oversized.push((name.clone(), accepted.hashes.len()));
        }
        if !accepted.hashes.is_empty() {
            out.hashes.insert(name.clone(), accepted.hashes);
        }
    }
    out
}

// ---- building ---------------------------------------------------------------

/// The tree twin of `contract::canon_type_in` in `Mode::Contract`. Keep the two
/// in step: [`ServiceShape::normal_form`] is tested against the real normal
/// form over every type former, which fails if they drift.
fn shape_of(
    t: &TypeRef,
    types: &HashMap<String, Arc<TypeDecl>>,
    seen: &mut Vec<String>,
    subst: &HashMap<String, Shape>,
) -> Shape {
    let mut go = |t: &TypeRef| shape_of(t, types, seen, subst);
    match t {
        TypeRef::Base(b, _) => Shape::Base(b.name().to_string()),
        TypeRef::Unit(_) => Shape::Unit,
        TypeRef::Effect(inner, _) => go(inner),
        TypeRef::List(a, _) => Shape::List(Box::new(go(a))),
        TypeRef::Option(a, _) => Shape::Option(Box::new(go(a))),
        TypeRef::Result(a, b, _) => Shape::Result(Box::new(go(a)), Box::new(go(b))),
        TypeRef::Map(k, v, _) => Shape::Map(Box::new(go(k)), Box::new(go(v))),
        TypeRef::App { name, args, .. } => {
            let inner: Vec<Shape> = args.iter().map(&mut go).collect();
            let bound: HashMap<String, Shape> = types
                .get(&name.name)
                .map(|d| {
                    d.type_params
                        .iter()
                        .zip(&inner)
                        .map(|(p, a)| (p.name.name.clone(), a.clone()))
                        .collect()
                })
                .unwrap_or_default();
            let head = named_shape(&name.name, types, seen, &bound);
            Shape::App {
                head: Box::new(head),
                args: inner,
            }
        }
        TypeRef::Named(id) => match subst.get(&id.name) {
            Some(bound) => bound.clone(),
            None => named_shape(&id.name, types, seen, subst),
        },
        TypeRef::HttpResult(a, _) => Shape::HttpResult(Box::new(go(a))),
        TypeRef::ValidationError(_) => Shape::Named("ValidationError".into()),
        TypeRef::JsonError(_) => Shape::Named("JsonError".into()),
        TypeRef::QueueResult(_) => Shape::Named("QueueResult".into()),
        TypeRef::Asset(_) => Shape::Named("Asset".into()),
        TypeRef::RequestHeaders(_) => Shape::Named("RequestHeaders".into()),
        TypeRef::TokenClaims(_) => Shape::Named("TokenClaims".into()),
        TypeRef::BoundaryRejection(_) => Shape::Named("BoundaryRejection".into()),
        TypeRef::Fn(..)
        | TypeRef::Query(..)
        | TypeRef::Stream(..)
        | TypeRef::Connection(..)
        | TypeRef::History(..) => Shape::Named("<non-boundary>".into()),
    }
}

fn named_shape(
    name: &str,
    types: &HashMap<String, Arc<TypeDecl>>,
    seen: &mut Vec<String>,
    subst: &HashMap<String, Shape>,
) -> Shape {
    if seen.iter().any(|n| n == name) {
        return Shape::Ref(name.to_string());
    }
    let Some(decl) = types.get(name) else {
        return Shape::Named(name.to_string());
    };
    seen.push(name.to_string());
    let shape = match &decl.body {
        TypeBody::Record(r) => {
            let mut fields: Vec<Field> = r
                .fields
                .iter()
                .map(|f| Field {
                    name: f.name.name.clone(),
                    ty: shape_of(&f.type_ref, types, seen, subst),
                    preds: f
                        .refinement
                        .as_ref()
                        .map(canon_pred_set)
                        .unwrap_or_default(),
                })
                .collect();
            fields.sort_by_cached_key(render_field);
            Shape::Record {
                name: name.to_string(),
                fields,
            }
        }
        TypeBody::Sum(s) => {
            let mut variants: Vec<Variant> = s
                .variants
                .iter()
                .map(|v| Variant {
                    name: v.name.name.clone(),
                    payload: v
                        .payload
                        .iter()
                        .map(|p| Field {
                            name: p.name.name.clone(),
                            ty: shape_of(&p.type_ref, types, seen, subst),
                            preds: Vec::new(),
                        })
                        .collect(),
                })
                .collect();
            for v in &mut variants {
                v.payload.sort_by_cached_key(render_field);
            }
            variants.sort_by_cached_key(render_variant);
            Shape::Sum {
                name: name.to_string(),
                variants,
            }
        }
        TypeBody::Refined {
            base, refinement, ..
        } => Shape::Refined {
            name: name.to_string(),
            base: base.name().to_string(),
            preds: refinement.as_ref().map(canon_pred_set).unwrap_or_default(),
        },
        TypeBody::Opaque { base, .. } => Shape::Opaque {
            name: name.to_string(),
            base: base.name().to_string(),
        },
    };
    seen.pop();
    shape
}

// ---- rendering the normal form ------------------------------------------------

fn render_field(f: &Field) -> String {
    let ty = render(&f.ty);
    if f.preds.is_empty() {
        format!("{}: {ty}", f.name)
    } else {
        format!("{}: {ty} where {}", f.name, f.preds.join(", "))
    }
}

fn render_variant(v: &Variant) -> String {
    if v.payload.is_empty() {
        v.name.clone()
    } else {
        let mut payload: Vec<String> = v.payload.iter().map(render_field).collect();
        payload.sort();
        format!("{}({})", v.name, payload.join(", "))
    }
}

fn render(s: &Shape) -> String {
    match s {
        Shape::Base(b) => b.clone(),
        Shape::Unit => "()".to_string(),
        Shape::List(a) => format!("List[{}]", render(a)),
        Shape::Option(a) => format!("Option[{}]", render(a)),
        Shape::Result(a, b) => format!("Result[{}, {}]", render(a), render(b)),
        Shape::Map(k, v) => format!("Map[{}, {}]", render(k), render(v)),
        Shape::HttpResult(a) => format!("HttpResult[{}]", render(a)),
        Shape::Named(n) => n.clone(),
        Shape::Ref(n) => format!("@{n}"),
        Shape::Record { name, fields } => {
            let mut fs: Vec<String> = fields.iter().map(render_field).collect();
            fs.sort();
            format!("{name}{{{}}}", fs.join(", "))
        }
        Shape::Sum { name, variants } => {
            let mut vs: Vec<String> = variants.iter().map(render_variant).collect();
            vs.sort();
            format!("{name}|{}", vs.join("|"))
        }
        Shape::Refined { name, base, preds } => {
            if preds.is_empty() {
                format!("{name}{base} ")
            } else {
                format!("{name}{base} where {}", preds.join(", "))
            }
        }
        Shape::Opaque { name, base } => format!("{name}{base} opaque"),
        Shape::App { head, args } => {
            let a: Vec<String> = args.iter().map(render).collect();
            format!("{}[{}]", render(head), a.join(", "))
        }
    }
}

// ---- the text form ------------------------------------------------------------

/// `Head(arg, …)` where an arg is a quoted string or another call. Small enough
/// to hand-write, and unlike the normal form it has no ambiguous joins.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Node {
    Str(String),
    Call(String, Vec<Node>),
}

impl Node {
    fn call(head: &str, args: Vec<Node>) -> Node {
        Node::Call(head.to_string(), args)
    }

    fn write(&self, out: &mut String) {
        match self {
            Node::Str(s) => {
                out.push('"');
                for c in s.chars() {
                    if c == '"' || c == '\\' {
                        out.push('\\');
                    }
                    out.push(c);
                }
                out.push('"');
            }
            Node::Call(head, args) => {
                out.push_str(head);
                out.push('(');
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    a.write(out);
                }
                out.push(')');
            }
        }
    }

    fn parse(text: &str) -> Option<Node> {
        let chars: Vec<char> = text.chars().collect();
        let mut at = 0;
        let node = Self::parse_at(&chars, &mut at)?;
        (at == chars.len()).then_some(node)
    }

    fn parse_at(c: &[char], at: &mut usize) -> Option<Node> {
        if *c.get(*at)? == '"' {
            *at += 1;
            let mut s = String::new();
            loop {
                match *c.get(*at)? {
                    '"' => {
                        *at += 1;
                        return Some(Node::Str(s));
                    }
                    '\\' => {
                        *at += 1;
                        s.push(*c.get(*at)?);
                    }
                    ch => s.push(ch),
                }
                *at += 1;
            }
        }
        let start = *at;
        while c.get(*at).is_some_and(|ch| ch.is_ascii_alphanumeric()) {
            *at += 1;
        }
        if start == *at || *c.get(*at)? != '(' {
            return None;
        }
        let head: String = c[start..*at].iter().collect();
        *at += 1;
        let mut args = Vec::new();
        if *c.get(*at)? == ')' {
            *at += 1;
            return Some(Node::Call(head, args));
        }
        loop {
            args.push(Self::parse_at(c, at)?);
            match *c.get(*at)? {
                ',' => *at += 1,
                ')' => {
                    *at += 1;
                    return Some(Node::Call(head, args));
                }
                _ => return None,
            }
        }
    }
}

fn strs(items: &[String]) -> impl Iterator<Item = Node> + '_ {
    items.iter().map(|s| Node::Str(s.clone()))
}

fn field_node(f: &Field) -> Node {
    let mut fa = vec![Node::Str(f.name.clone()), shape_node(&f.ty)];
    fa.extend(strs(&f.preds));
    Node::call("F", fa)
}

fn node_field(n: &Node) -> Option<Field> {
    let Node::Call(fh, fa) = n else {
        return None;
    };
    if fh != "F" || fa.len() < 2 {
        return None;
    }
    let string = |n: &Node| match n {
        Node::Str(s) => Some(s.clone()),
        Node::Call(..) => None,
    };
    Some(Field {
        name: string(&fa[0])?,
        ty: node_shape(&fa[1])?,
        preds: fa[2..].iter().map(string).collect::<Option<_>>()?,
    })
}

fn shape_node(s: &Shape) -> Node {
    let one = |h: &str, a: &Shape| Node::call(h, vec![shape_node(a)]);
    match s {
        Shape::Base(b) => Node::call("B", vec![Node::Str(b.clone())]),
        Shape::Unit => Node::call("U", vec![]),
        Shape::List(a) => one("L", a),
        Shape::Option(a) => one("O", a),
        Shape::HttpResult(a) => one("H", a),
        Shape::Result(a, b) => Node::call("R", vec![shape_node(a), shape_node(b)]),
        Shape::Map(k, v) => Node::call("M", vec![shape_node(k), shape_node(v)]),
        Shape::Named(n) => Node::call("N", vec![Node::Str(n.clone())]),
        Shape::Ref(n) => Node::call("Ref", vec![Node::Str(n.clone())]),
        Shape::Record { name, fields } => {
            let mut args = vec![Node::Str(name.clone())];
            args.extend(fields.iter().map(field_node));
            Node::call("Rec", args)
        }
        Shape::Sum { name, variants } => {
            let mut args = vec![Node::Str(name.clone())];
            for v in variants {
                let mut va = vec![Node::Str(v.name.clone())];
                va.extend(v.payload.iter().map(field_node));
                args.push(Node::call("V", va));
            }
            Node::call("Sum", args)
        }
        Shape::Refined { name, base, preds } => {
            let mut args = vec![Node::Str(name.clone()), Node::Str(base.clone())];
            args.extend(strs(preds));
            Node::call("Rf", args)
        }
        Shape::Opaque { name, base } => {
            Node::call("Op", vec![Node::Str(name.clone()), Node::Str(base.clone())])
        }
        Shape::App { head, args } => {
            let mut a = vec![shape_node(head)];
            a.extend(args.iter().map(shape_node));
            Node::call("A", a)
        }
    }
}

fn node_shape(n: &Node) -> Option<Shape> {
    let Node::Call(head, args) = n else {
        return None;
    };
    let string = |n: &Node| match n {
        Node::Str(s) => Some(s.clone()),
        Node::Call(..) => None,
    };
    let sub = |i: usize| node_shape(args.get(i)?).map(Box::new);
    let arity = |k: usize| (args.len() == k).then_some(());
    Some(match head.as_str() {
        "B" => {
            arity(1)?;
            Shape::Base(string(&args[0])?)
        }
        "U" => {
            arity(0)?;
            Shape::Unit
        }
        "L" => {
            arity(1)?;
            Shape::List(sub(0)?)
        }
        "O" => {
            arity(1)?;
            Shape::Option(sub(0)?)
        }
        "H" => {
            arity(1)?;
            Shape::HttpResult(sub(0)?)
        }
        "R" => {
            arity(2)?;
            Shape::Result(sub(0)?, sub(1)?)
        }
        "M" => {
            arity(2)?;
            Shape::Map(sub(0)?, sub(1)?)
        }
        "N" => {
            arity(1)?;
            Shape::Named(string(&args[0])?)
        }
        "Ref" => {
            arity(1)?;
            Shape::Ref(string(&args[0])?)
        }
        "Rec" => {
            let name = string(args.first()?)?;
            let fields = args[1..].iter().map(node_field).collect::<Option<_>>()?;
            Shape::Record { name, fields }
        }
        "Sum" => {
            let name = string(args.first()?)?;
            let mut variants = Vec::new();
            for v in &args[1..] {
                let Node::Call(vh, va) = v else {
                    return None;
                };
                if vh != "V" || va.is_empty() {
                    return None;
                }
                variants.push(Variant {
                    name: string(&va[0])?,
                    payload: va[1..].iter().map(node_field).collect::<Option<_>>()?,
                });
            }
            Shape::Sum { name, variants }
        }
        "Rf" => {
            if args.len() < 2 {
                return None;
            }
            Shape::Refined {
                name: string(&args[0])?,
                base: string(&args[1])?,
                preds: args[2..].iter().map(string).collect::<Option<_>>()?,
            }
        }
        "Op" => {
            arity(2)?;
            Shape::Opaque {
                name: string(&args[0])?,
                base: string(&args[1])?,
            }
        }
        "A" => {
            if args.is_empty() {
                return None;
            }
            Shape::App {
                head: sub(0)?,
                args: args[1..].iter().map(node_shape).collect::<Option<_>>()?,
            }
        }
        _ => return None,
    })
}

// ---- the classifier -------------------------------------------------------------

/// Which half of the call a finding is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Arguments, caller to callee.
    Request,
    /// The result, callee to caller.
    Response,
}

impl Direction {
    pub fn name(self) -> &'static str {
        match self {
            Direction::Request => "request",
            Direction::Response => "response",
        }
    }
}

/// One difference between the two contracts, and whether the call survives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub direction: Direction,
    /// Where in the request or response: `request.order.qty`, `response`.
    pub path: String,
    pub breaking: bool,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The two contracts are the same.
    Unchanged,
    /// They differ, but the caller still works against the callee.
    Compatible,
    /// Some call the caller can make is refused, or some answer it cannot read.
    Breaking,
}

impl Verdict {
    pub fn name(self) -> &'static str {
        match self {
            Verdict::Unchanged => "unchanged",
            Verdict::Compatible => "compatible",
            Verdict::Breaking => "breaking",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compatibility {
    pub verdict: Verdict,
    pub findings: Vec<Finding>,
}

impl Compatibility {
    /// One line per finding, breaking ones first, for a terminal or a panel:
    /// `breaking request.note: the callee requires …`.
    pub fn lines(&self) -> Vec<String> {
        let mut findings: Vec<&Finding> = self.findings.iter().collect();
        findings.sort_by_key(|f| !f.breaking);
        findings
            .into_iter()
            .map(|f| {
                format!(
                    "{} {}: {}",
                    if f.breaking { "breaking" } else { "compatible" },
                    f.path,
                    f.message
                )
            })
            .collect()
    }

    /// The findings that make the verdict `Breaking`.
    pub fn breaking(&self) -> impl Iterator<Item = &Finding> {
        self.findings.iter().filter(|f| f.breaking)
    }
}

/// Can a caller compiled against `caller` safely call a callee providing
/// `callee`?
///
/// Both deploy skews are this one question. Pushing a callee over live callers
/// asks it of `(what the callers recorded, what the new build provides)`;
/// pushing a caller over a live callee asks it of `(what the new build expects,
/// what the callee records)`. The editor's "old contract to new contract" is
/// the first.
pub fn check_call(caller: &ServiceShape, callee: &ServiceShape) -> Compatibility {
    let mut cx = Cx::default();
    if caller == callee {
        return Compatibility {
            verdict: Verdict::Unchanged,
            findings: Vec::new(),
        };
    }
    if caller.name != callee.name {
        cx.push(
            Direction::Request,
            &caller.name,
            true,
            format!("the service is `{}`, not `{}`", callee.name, caller.name),
        );
    }
    cx.dir = Direction::Request;
    cx.params(&caller.params, &callee.params);
    cx.dir = Direction::Response;
    cx.flows(&callee.ret, &[], &caller.ret, &[], "response");
    let breaking = cx.findings.iter().any(|f| f.breaking);
    Compatibility {
        verdict: if breaking {
            Verdict::Breaking
        } else {
            Verdict::Compatible
        },
        findings: cx.findings,
    }
}

struct Cx {
    dir: Direction,
    findings: Vec<Finding>,
}

impl Default for Cx {
    fn default() -> Self {
        Cx {
            dir: Direction::Request,
            findings: Vec::new(),
        }
    }
}

impl Cx {
    fn push(&mut self, direction: Direction, path: &str, breaking: bool, message: String) {
        self.findings.push(Finding {
            direction,
            path: path.to_string(),
            breaking,
            message,
        });
    }

    fn note(&mut self, path: &str, breaking: bool, message: String) {
        let dir = self.dir;
        self.push(dir, path, breaking, message);
    }

    /// Who sends and who receives, in the words of a finding.
    fn roles(&self) -> (&'static str, &'static str) {
        match self.dir {
            Direction::Request => ("caller", "callee"),
            Direction::Response => ("callee", "caller"),
        }
    }

    /// The parameter list is the request body: one parameter travels bare, two
    /// or more as an object keyed by parameter name.
    fn params(&mut self, caller: &[(String, Shape)], callee: &[(String, Shape)]) {
        match (caller, callee) {
            ([], []) => {}
            ([(cn, c)], [(en, e)]) => {
                let path = "request".to_string();
                if cn != en {
                    self.note(
                        &path,
                        false,
                        format!("the parameter is renamed `{cn}` to `{en}`; one parameter travels as a bare value, so the name is not on the wire"),
                    );
                }
                self.flows(c, &[], e, &[], &path);
            }
            (c, e) if c.len() >= 2 && e.len() >= 2 => {
                for (en, eshape) in e {
                    let path = format!("request.{en}");
                    match c.iter().find(|(cn, _)| cn == en) {
                        Some((_, cshape)) => self.flows(cshape, &[], eshape, &[], &path),
                        None if matches!(eshape, Shape::Option(_)) => self.note(
                            &path,
                            false,
                            format!("the callee adds the optional parameter `{en}`; callers that omit it send `None`"),
                        ),
                        None => self.note(
                            &path,
                            true,
                            format!("the callee requires the parameter `{en}`, which the caller does not send"),
                        ),
                    }
                }
                for (cn, _) in c {
                    if !e.iter().any(|(en, _)| en == cn) {
                        self.note(
                            &format!("request.{cn}"),
                            false,
                            format!("the callee drops the parameter `{cn}`; unknown keys are ignored"),
                        );
                    }
                }
            }
            (c, e) => self.note(
                "request",
                true,
                format!(
                    "the parameter list changes shape ({} to {} parameters); one parameter is sent bare and several as an object",
                    c.len(),
                    e.len()
                ),
            ),
        }
    }

    /// Can the receiver decode every value of `sender`? `sp`/`rp` are inline
    /// field refinements that sit beside the type.
    fn flows(&mut self, s: &Shape, sp: &[String], r: &Shape, rp: &[String], path: &str) {
        let (sender, receiver) = self.roles();
        // A generic instantiation is its expanded head; the arguments are
        // already substituted into it.
        if let Shape::App { head, .. } = s {
            return self.flows(head, sp, r, rp, path);
        }
        if let Shape::App { head, .. } = r {
            return self.flows(s, sp, head, rp, path);
        }
        // A bare value is accepted where an `Option` is expected.
        if let (false, Shape::Option(ri)) = (matches!(s, Shape::Option(_)), r) {
            self.note(
                path,
                false,
                format!("made optional: the {receiver} accepts a bare value as `Some`"),
            );
            return self.flows(s, sp, ri, rp, path);
        }
        if let (Some((sb, sset)), Some((rb, rset))) = (leaf(s, sp), leaf(r, rp)) {
            if sb != rb {
                return self.note(
                    path,
                    true,
                    format!("the type changes from `{sb}` to `{rb}`"),
                );
            }
            return self.refinements(&sb, &sset, &rset, path);
        }
        match (s, r) {
            (Shape::Unit, Shape::Unit) => {}
            (Shape::Named(a), Shape::Named(b)) if a == b => {}
            (Shape::Ref(a), Shape::Ref(b)) if a == b => {}
            (Shape::Opaque { base: a, .. }, Shape::Opaque { base: b, .. }) if a == b => {}
            (Shape::List(a), Shape::List(b)) => self.flows(a, &[], b, &[], &format!("{path}[]")),
            (Shape::HttpResult(a), Shape::HttpResult(b)) => {
                self.flows(a, &[], b, &[], &format!("{path}.value"))
            }
            (Shape::Option(a), Shape::Option(b)) => self.flows(a, &[], b, &[], &format!("{path}?")),
            (Shape::Result(ao, ae), Shape::Result(bo, be)) => {
                self.flows(ao, &[], bo, &[], &format!("{path}.ok"));
                self.flows(ae, &[], be, &[], &format!("{path}.err"));
            }
            (Shape::Map(ak, av), Shape::Map(bk, bv)) => {
                self.flows(ak, &[], bk, &[], &format!("{path}.key"));
                self.flows(av, &[], bv, &[], &format!("{path}[]"));
            }
            (Shape::Record { fields: sf, .. }, Shape::Record { fields: rf, .. }) => {
                for rfield in rf {
                    let fpath = format!("{path}.{}", rfield.name);
                    match sf.iter().find(|f| f.name == rfield.name) {
                        Some(sfield) => {
                            self.flows(&sfield.ty, &sfield.preds, &rfield.ty, &rfield.preds, &fpath)
                        }
                        None if matches!(rfield.ty, Shape::Option(_)) => self.note(
                            &fpath,
                            false,
                            format!("the {sender} no longer sends the optional field `{}`; the {receiver} reads `None`", rfield.name),
                        ),
                        None => self.note(
                            &fpath,
                            true,
                            format!("the {receiver} requires the field `{}`, which the {sender} does not send{}", rfield.name, DEFAULT_HINT),
                        ),
                    }
                }
                for sfield in sf {
                    if !rf.iter().any(|f| f.name == sfield.name) {
                        self.note(
                            &format!("{path}.{}", sfield.name),
                            false,
                            format!(
                                "the {sender} sends the field `{}`, which the {receiver} ignores",
                                sfield.name
                            ),
                        );
                    }
                }
            }
            (Shape::Sum { variants: sv, .. }, Shape::Sum { variants: rv, .. }) => {
                for svar in sv {
                    let vpath = format!("{path}.{}", svar.name);
                    match rv.iter().find(|v| v.name == svar.name) {
                        None => self.note(
                            &vpath,
                            true,
                            format!("the {sender} can send the variant `{}`, which the {receiver} does not handle", svar.name),
                        ),
                        Some(rvar) => {
                            // Payload fields are keyed by name on the wire, so
                            // the classifier errs towards breaking on any
                            // difference in the set of names.
                            if payload_names(svar) != payload_names(rvar) {
                                self.note(
                                    &vpath,
                                    true,
                                    format!(
                                        "the payload fields of the variant `{}` change from ({}) to ({}); payload fields are keyed by name",
                                        svar.name,
                                        payload_names(svar).join(", "),
                                        payload_names(rvar).join(", ")
                                    ),
                                );
                            } else {
                                for sf in &svar.payload {
                                    if let Some(rf) = rvar.payload.iter().find(|f| f.name == sf.name) {
                                        self.flows(&sf.ty, &[], &rf.ty, &[], &format!("{vpath}.{}", sf.name));
                                    }
                                }
                            }
                        }
                    }
                }
                for rvar in rv {
                    if !sv.iter().any(|v| v.name == rvar.name) {
                        self.note(
                            &format!("{path}.{}", rvar.name),
                            false,
                            format!("the {receiver} also handles the variant `{}`, which the {sender} never sends", rvar.name),
                        );
                    }
                }
            }
            (a, b) => self.note(
                path,
                true,
                format!("the type changes from `{}` to `{}`", render(a), render(b)),
            ),
        }
    }

    /// A numeric or string base on both sides: the receiver's predicates must
    /// each follow from the sender's.
    fn refinements(&mut self, base: &str, sender: &[String], receiver: &[String], path: &str) {
        if sender == receiver {
            return;
        }
        let (s, r) = self.roles();
        let unmet: Vec<&String> = receiver
            .iter()
            .filter(|q| !entailed(base, sender, q))
            .collect();
        if unmet.is_empty() {
            self.note(
                path,
                false,
                format!(
                    "the refinement changes, and the {r} still accepts everything the {s} sends"
                ),
            );
        } else {
            let list: Vec<&str> = unmet.iter().map(|q| q.as_str()).collect();
            self.note(
                path,
                true,
                format!(
                    "the {r} requires {}, which the {s} does not guarantee",
                    list.join(", ")
                ),
            );
        }
    }
}

/// A variant's payload field names, sorted.
fn payload_names(v: &Variant) -> Vec<&str> {
    let mut n: Vec<&str> = v.payload.iter().map(|f| f.name.as_str()).collect();
    n.sort_unstable();
    n
}

/// Appended to a missing-field finding: the contract does not carry defaults, so
/// the classifier cannot credit one.
const DEFAULT_HINT: &str =
    " (a field's default is not part of the contract, so only an `Option` may be absent)";

/// A base type with its predicates, whether it is written as a base, an inline
/// refinement on a base, or a named refined type. `None` for anything with
/// structure.
fn leaf(s: &Shape, field_preds: &[String]) -> Option<(String, Vec<String>)> {
    let (base, mut preds) = match s {
        Shape::Base(b) => (b.clone(), Vec::new()),
        Shape::Refined { base, preds, .. } => (base.clone(), preds.clone()),
        _ => return None,
    };
    preds.extend(field_preds.iter().cloned());
    preds.sort();
    preds.dedup();
    Some((base, preds))
}

// ---- predicate entailment --------------------------------------------------------

#[derive(Clone, Copy)]
struct Interval<T> {
    lo: Option<(T, bool)>,
    hi: Option<(T, bool)>,
}

impl<T: PartialOrd + Copy> Interval<T> {
    const ALL: Self = Interval { lo: None, hi: None };

    fn intersect(self, o: Self) -> Self {
        Interval {
            lo: tighter(self.lo, o.lo, Ordering::Greater),
            hi: tighter(self.hi, o.hi, Ordering::Less),
        }
    }

    /// Every value of `self` lies in `outer`.
    fn within(self, outer: Self) -> bool {
        let lo_ok = match (self.lo, outer.lo) {
            (_, None) => true,
            (None, Some(_)) => false,
            (Some((s, so)), Some((o, oo))) => match s.partial_cmp(&o) {
                Some(Ordering::Greater) => true,
                Some(Ordering::Equal) => so || !oo,
                _ => false,
            },
        };
        let hi_ok = match (self.hi, outer.hi) {
            (_, None) => true,
            (None, Some(_)) => false,
            (Some((s, so)), Some((o, oo))) => match s.partial_cmp(&o) {
                Some(Ordering::Less) => true,
                Some(Ordering::Equal) => so || !oo,
                _ => false,
            },
        };
        lo_ok && hi_ok
    }
}

/// The tighter of two bounds; `want` is `Greater` for lower bounds.
fn tighter<T: PartialOrd + Copy>(
    a: Option<(T, bool)>,
    b: Option<(T, bool)>,
    want: Ordering,
) -> Option<(T, bool)> {
    match (a, b) {
        (None, x) | (x, None) => x,
        (Some((av, ao)), Some((bv, bo))) => match av.partial_cmp(&bv) {
            Some(o) if o == want => Some((av, ao)),
            Some(Ordering::Equal) => Some((av, ao || bo)),
            _ => Some((bv, bo)),
        },
    }
}

/// `args` of `Name(a, b)`, split on the top-level comma.
fn pred_args<'a>(p: &'a str, name: &str) -> Option<Vec<&'a str>> {
    let inner = p.strip_prefix(name)?.strip_prefix('(')?.strip_suffix(')')?;
    Some(inner.split(", ").collect())
}

fn closed<T>(lo: T, hi: T) -> Interval<T> {
    Interval {
        lo: Some((lo, false)),
        hi: Some((hi, false)),
    }
}

/// A predicate as a bound on the numeric domain (`Int` or `Float`) or the
/// length domain.
enum Domain {
    Int(Interval<i128>),
    Float(Interval<f64>),
    Length(Interval<i128>),
}

fn domain(base: &str, p: &str) -> Option<Domain> {
    let int_pred = || -> Option<Interval<i128>> {
        Some(match p {
            "Positive" => Interval {
                lo: Some((1, false)),
                hi: None,
            },
            "NonNegative" => Interval {
                lo: Some((0, false)),
                hi: None,
            },
            _ => {
                let a = pred_args(p, "InRange")?;
                closed(a.first()?.parse().ok()?, a.get(1)?.parse().ok()?)
            }
        })
    };
    let float_pred = || -> Option<Interval<f64>> {
        Some(match p {
            "Positive" => Interval {
                lo: Some((0.0, true)),
                hi: None,
            },
            "NonNegative" => Interval {
                lo: Some((0.0, false)),
                hi: None,
            },
            _ => {
                let a = pred_args(p, "InRangeF")?;
                closed(a.first()?.parse().ok()?, a.get(1)?.parse().ok()?)
            }
        })
    };
    if let Some(a) = pred_args(p, "MinLength") {
        return Some(Domain::Length(Interval {
            lo: Some((a.first()?.parse().ok()?, false)),
            hi: None,
        }));
    }
    if let Some(a) = pred_args(p, "MaxLength") {
        return Some(Domain::Length(Interval {
            lo: None,
            hi: Some((a.first()?.parse().ok()?, false)),
        }));
    }
    if let Some(a) = pred_args(p, "Length") {
        let n = a.first()?.parse().ok()?;
        return Some(Domain::Length(closed(n, n)));
    }
    match base {
        "Int" => int_pred().map(Domain::Int),
        "Float" => float_pred().map(Domain::Float),
        _ => None,
    }
}

/// Does the conjunction `sender` guarantee the predicate `q`?
fn entailed(base: &str, sender: &[String], q: &str) -> bool {
    if sender.iter().any(|p| p == q) {
        return true;
    }
    let Some(want) = domain(base, q) else {
        // `Matches`, or a predicate this version cannot reason about: only an
        // identical predicate satisfies it.
        return false;
    };
    let given = sender.iter().filter_map(|p| domain(base, p));
    match want {
        Domain::Int(w) => {
            let mut have = Interval::ALL;
            for d in given {
                if let Domain::Int(i) = d {
                    have = have.intersect(i);
                }
            }
            have.within(w)
        }
        Domain::Float(w) => {
            let mut have = Interval::ALL;
            for d in given {
                if let Domain::Float(i) = d {
                    have = have.intersect(i);
                }
            }
            have.within(w)
        }
        Domain::Length(w) => {
            let mut have = Interval::ALL;
            for d in given {
                if let Domain::Length(i) = d {
                    have = have.intersect(i);
                }
            }
            have.within(w)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::service_normal_form;

    fn types_of(src: &str) -> HashMap<String, Arc<TypeDecl>> {
        let tokens = bynk_syntax::lexer::tokenize(src).expect("lex");
        let commons = bynk_syntax::parser::parse(&tokens, src).expect("parse");
        commons
            .items
            .iter()
            .filter_map(|i| match i {
                bynk_syntax::ast::CommonsItem::Type(t) => {
                    Some((t.name.name.clone(), Arc::new(t.clone())))
                }
                _ => None,
            })
            .collect()
    }

    fn sp() -> bynk_syntax::span::Span {
        bynk_syntax::span::Span::new(0, 0)
    }

    fn named(n: &str) -> TypeRef {
        TypeRef::Named(bynk_syntax::ast::Ident {
            name: n.to_string(),
            span: sp(),
        })
    }

    fn service(params: Vec<(&str, TypeRef)>, ret: TypeRef) -> CrossContextService {
        CrossContextService {
            name: "probe".to_string(),
            params: params
                .into_iter()
                .map(|(n, t)| (n.to_string(), t))
                .collect(),
            return_type: ret,
            span: sp(),
            foreign_callers: Vec::new(),
        }
    }

    /// `probe(req: Req) -> Resp` over the declarations in `src`.
    fn shape(src: &str) -> ServiceShape {
        let types = types_of(&format!("commons x\n\n{src}\n"));
        ServiceShape::of(&service(vec![("req", named("Req"))], named("Resp")), &types)
    }

    fn verdict(old: &str, new: &str) -> Compatibility {
        // Old callers against the new callee.
        check_call(&shape(old), &shape(new))
    }

    fn breaking(old: &str, new: &str) -> Compatibility {
        let c = verdict(old, new);
        assert_eq!(c.verdict, Verdict::Breaking, "{c:#?}");
        assert!(c.breaking().count() > 0);
        c
    }

    fn compatible(old: &str, new: &str) -> Compatibility {
        let c = verdict(old, new);
        assert_eq!(c.verdict, Verdict::Compatible, "{c:#?}");
        assert!(!c.findings.is_empty(), "a change has a reason");
        c
    }

    fn only(c: &Compatibility, dir: Direction, path: &str) {
        assert!(
            c.findings
                .iter()
                .any(|f| f.direction == dir && f.path == path),
            "no {dir:?} finding at `{path}`: {c:#?}"
        );
    }

    const REQ: &str = "type Resp = { ok: Bool }\n";
    const RESP: &str = "type Req = { id: Int }\n";

    // ---- the shape is the normal form ---------------------------------------

    #[test]
    fn a_shape_renders_the_exact_normal_form_and_hash() {
        let types = types_of(
            "commons x\n\n\
             type Age = Int where InRange(0, 150)\n\
             type Tag = String where NonEmpty && MaxLength(8)\n\
             type Token = opaque String\n\
             type Shape = | Circle(radius: Float) | Dot\n\
             type Node = { next: Option[Node], v: Int where Positive }\n\
             type Page[T] = { items: List[T], tag: Tag }\n\
             type Bag = { age: Age, shape: Shape, tok: Token, m: Map[String, Page[Age]], r: Result[Node, String], u: () }\n",
        );
        let svc = service(
            vec![("a", named("Bag")), ("b", TypeRef::Unit(sp()))],
            named("Node"),
        );
        let s = ServiceShape::of(&svc, &types);
        assert_eq!(s.normal_form(), service_normal_form(&svc, &types));
        assert_eq!(
            s.hash(),
            crate::contract::service_contract_hash(&svc, &types)
        );
    }

    #[test]
    fn a_shape_round_trips_through_its_text() {
        let types = types_of(
            "commons x\n\n\
             type Age = Int where InRange(0, 150)\n\
             type Token = opaque String\n\
             type Shape = | Circle(radius: Float) | Dot\n\
             type Node = { next: Option[Node], v: Int where Positive }\n\
             type Page[T] = { items: List[T] }\n\
             type Bag = { age: Age, shape: Shape, tok: Token, p: Page[Age] }\n",
        );
        let svc = service(vec![("a", named("Bag"))], named("Node"));
        let s = ServiceShape::of(&svc, &types);
        let text = s.to_text();
        assert_eq!(ServiceShape::from_text(&text), Some(s.clone()), "{text}");
        assert_eq!(
            ServiceShape::from_text_checked(&text, &s.hash()),
            Some(s.clone())
        );
        assert_eq!(
            ServiceShape::from_text_checked(&text, "0000000000000000"),
            None,
            "a shape that does not hash to its recorded hash is not trusted"
        );
    }

    #[test]
    fn text_that_is_not_a_shape_reads_as_none() {
        for bad in [
            "",
            "Svc(",
            "Svc(\"a\")",
            "X()",
            "Svc(\"a\",Ret(Q()))",
            "junk",
        ] {
            assert_eq!(ServiceShape::from_text(bad), None, "{bad:?}");
        }
    }

    // ---- unchanged ------------------------------------------------------------

    #[test]
    fn identical_contracts_are_unchanged() {
        let s = shape(&format!("{REQ}{RESP}"));
        let c = check_call(&s, &s);
        assert_eq!((c.verdict, c.findings.len()), (Verdict::Unchanged, 0));
    }

    // ---- record fields: requests --------------------------------------------------

    #[test]
    fn request_optional_field_added_is_compatible() {
        let c = compatible(
            &format!("{REQ}type Req = {{ id: Int }}\n"),
            &format!("{REQ}type Req = {{ id: Int, note: Option[String] }}\n"),
        );
        only(&c, Direction::Request, "request.note");
    }

    #[test]
    fn request_required_field_added_is_breaking() {
        let c = breaking(
            &format!("{REQ}type Req = {{ id: Int }}\n"),
            &format!("{REQ}type Req = {{ id: Int, note: String }}\n"),
        );
        only(&c, Direction::Request, "request.note");
    }

    #[test]
    fn request_field_removed_is_compatible_because_the_callee_ignores_it() {
        let c = compatible(
            &format!("{REQ}type Req = {{ id: Int, note: String }}\n"),
            &format!("{REQ}type Req = {{ id: Int }}\n"),
        );
        only(&c, Direction::Request, "request.note");
    }

    #[test]
    fn request_field_made_optional_is_compatible() {
        compatible(
            &format!("{REQ}type Req = {{ id: Int, note: String }}\n"),
            &format!("{REQ}type Req = {{ id: Int, note: Option[String] }}\n"),
        );
    }

    #[test]
    fn request_field_made_required_is_breaking() {
        let c = breaking(
            &format!("{REQ}type Req = {{ id: Int, note: Option[String] }}\n"),
            &format!("{REQ}type Req = {{ id: Int, note: String }}\n"),
        );
        only(&c, Direction::Request, "request.note");
    }

    #[test]
    fn request_field_retyped_is_breaking() {
        breaking(
            &format!("{REQ}type Req = {{ id: Int }}\n"),
            &format!("{REQ}type Req = {{ id: String }}\n"),
        );
    }

    // ---- record fields: responses -------------------------------------------------

    #[test]
    fn response_field_added_is_compatible() {
        let c = compatible(
            &format!("{RESP}type Resp = {{ ok: Bool }}\n"),
            &format!("{RESP}type Resp = {{ ok: Bool, at: Int }}\n"),
        );
        only(&c, Direction::Response, "response.at");
    }

    #[test]
    fn response_required_field_removed_is_breaking() {
        let c = breaking(
            &format!("{RESP}type Resp = {{ ok: Bool, at: Int }}\n"),
            &format!("{RESP}type Resp = {{ ok: Bool }}\n"),
        );
        only(&c, Direction::Response, "response.at");
    }

    #[test]
    fn response_optional_field_removed_is_compatible() {
        compatible(
            &format!("{RESP}type Resp = {{ ok: Bool, at: Option[Int] }}\n"),
            &format!("{RESP}type Resp = {{ ok: Bool }}\n"),
        );
    }

    #[test]
    fn response_field_made_optional_is_breaking() {
        let c = breaking(
            &format!("{RESP}type Resp = {{ ok: Bool, at: Int }}\n"),
            &format!("{RESP}type Resp = {{ ok: Bool, at: Option[Int] }}\n"),
        );
        only(&c, Direction::Response, "response.at");
    }

    #[test]
    fn response_field_made_required_is_compatible() {
        compatible(
            &format!("{RESP}type Resp = {{ ok: Bool, at: Option[Int] }}\n"),
            &format!("{RESP}type Resp = {{ ok: Bool, at: Int }}\n"),
        );
    }

    #[test]
    fn response_field_retyped_is_breaking() {
        breaking(
            &format!("{RESP}type Resp = {{ ok: Bool }}\n"),
            &format!("{RESP}type Resp = {{ ok: String }}\n"),
        );
    }

    #[test]
    fn field_order_is_not_a_change() {
        let c = check_call(
            &shape(&format!("{RESP}type Resp = {{ a: Int, b: Int }}\n")),
            &shape(&format!("{RESP}type Resp = {{ b: Int, a: Int }}\n")),
        );
        assert_eq!(c.verdict, Verdict::Unchanged);
    }

    // ---- refinements ----------------------------------------------------------------

    #[test]
    fn request_refinement_widened_is_compatible() {
        let c = compatible(
            &format!("{REQ}type Req = {{ qty: Int where InRange(1, 10) }}\n"),
            &format!("{REQ}type Req = {{ qty: Int where InRange(0, 100) }}\n"),
        );
        only(&c, Direction::Request, "request.qty");
    }

    #[test]
    fn request_refinement_narrowed_is_breaking() {
        let c = breaking(
            &format!("{REQ}type Req = {{ qty: Int where InRange(0, 100) }}\n"),
            &format!("{REQ}type Req = {{ qty: Int where InRange(1, 10) }}\n"),
        );
        only(&c, Direction::Request, "request.qty");
    }

    #[test]
    fn request_refinement_added_is_breaking_and_removed_is_compatible() {
        breaking(
            &format!("{REQ}type Req = {{ qty: Int }}\n"),
            &format!("{REQ}type Req = {{ qty: Int where Positive }}\n"),
        );
        compatible(
            &format!("{REQ}type Req = {{ qty: Int where Positive }}\n"),
            &format!("{REQ}type Req = {{ qty: Int }}\n"),
        );
    }

    #[test]
    fn response_refinement_narrowed_is_compatible_and_widened_is_breaking() {
        compatible(
            &format!("{RESP}type Resp = {{ qty: Int where InRange(0, 100) }}\n"),
            &format!("{RESP}type Resp = {{ qty: Int where InRange(1, 10) }}\n"),
        );
        let c = breaking(
            &format!("{RESP}type Resp = {{ qty: Int where InRange(1, 10) }}\n"),
            &format!("{RESP}type Resp = {{ qty: Int where InRange(0, 100) }}\n"),
        );
        only(&c, Direction::Response, "response.qty");
    }

    #[test]
    fn named_refined_types_follow_the_same_rules() {
        compatible(
            &format!("{REQ}type Age = Int where InRange(18, 65)\ntype Req = {{ age: Age }}\n"),
            &format!("{REQ}type Age = Int where InRange(0, 150)\ntype Req = {{ age: Age }}\n"),
        );
        breaking(
            &format!("{REQ}type Age = Int where InRange(0, 150)\ntype Req = {{ age: Age }}\n"),
            &format!("{REQ}type Age = Int where InRange(18, 65)\ntype Req = {{ age: Age }}\n"),
        );
    }

    #[test]
    fn length_and_sign_predicates_entail_each_other() {
        // `Positive` implies `NonNegative`; `Length(3)` implies both bounds.
        compatible(
            &format!("{RESP}type Resp = {{ n: Int where NonNegative }}\n"),
            &format!("{RESP}type Resp = {{ n: Int where Positive }}\n"),
        );
        compatible(
            &format!("{REQ}type Req = {{ s: String where Length(3) }}\n"),
            &format!("{REQ}type Req = {{ s: String where MinLength(1) && MaxLength(8) }}\n"),
        );
        breaking(
            &format!("{REQ}type Req = {{ s: String where MaxLength(8) }}\n"),
            &format!("{REQ}type Req = {{ s: String where Length(3) }}\n"),
        );
    }

    #[test]
    fn a_changed_pattern_is_breaking_both_ways() {
        breaking(
            &format!("{REQ}type Req = {{ s: String where Matches(\"[a-z]+\") }}\n"),
            &format!("{REQ}type Req = {{ s: String where Matches(\"[a-z0-9]+\") }}\n"),
        );
    }

    #[test]
    fn float_ranges_compare_by_value() {
        compatible(
            &format!("{REQ}type Req = {{ r: Float where InRange(0.5, 1.5) }}\n"),
            &format!("{REQ}type Req = {{ r: Float where InRange(0.0, 2.0) }}\n"),
        );
        breaking(
            &format!("{REQ}type Req = {{ r: Float where NonNegative }}\n"),
            &format!("{REQ}type Req = {{ r: Float where Positive }}\n"),
        );
        compatible(
            &format!("{RESP}type Resp = {{ r: Float where NonNegative }}\n"),
            &format!("{RESP}type Resp = {{ r: Float where Positive }}\n"),
        );
    }

    // ---- sum types --------------------------------------------------------------------

    #[test]
    fn request_variant_added_is_compatible_and_removed_is_breaking() {
        let one = format!("{REQ}type Kind = | A | B\ntype Req = {{ k: Kind }}\n");
        let two = format!("{REQ}type Kind = | A | B | C\ntype Req = {{ k: Kind }}\n");
        let c = compatible(&one, &two);
        only(&c, Direction::Request, "request.k.C");
        let c = breaking(&two, &one);
        only(&c, Direction::Request, "request.k.C");
    }

    #[test]
    fn response_variant_added_is_breaking_and_removed_is_compatible() {
        let one = format!("{RESP}type Kind = | A | B\ntype Resp = {{ k: Kind }}\n");
        let two = format!("{RESP}type Kind = | A | B | C\ntype Resp = {{ k: Kind }}\n");
        let c = breaking(&one, &two);
        only(&c, Direction::Response, "response.k.C");
        compatible(&two, &one);
    }

    #[test]
    fn a_variant_payload_change_is_breaking() {
        breaking(
            &format!("{RESP}type Resp = | A(n: Int) | B\n"),
            &format!("{RESP}type Resp = | A(n: String) | B\n"),
        );
        breaking(
            &format!("{RESP}type Resp = | A(n: Int) | B\n"),
            &format!("{RESP}type Resp = | A(n: Int, m: Int) | B\n"),
        );
    }

    #[test]
    fn a_variant_payload_field_is_keyed_by_name() {
        // A rename is a wire break in either direction; a reorder is invisible.
        let c = breaking(
            &format!("{RESP}type Resp = | A(x: Int) | B\n"),
            &format!("{RESP}type Resp = | A(y: Int) | B\n"),
        );
        only(&c, Direction::Response, "response.A");
        breaking(
            &format!("{REQ}type Req = | A(x: Int) | B\n"),
            &format!("{REQ}type Req = | A(y: Int) | B\n"),
        );
        let reordered = check_call(
            &shape(&format!("{RESP}type Resp = | A(x: Int, y: String) | B\n")),
            &shape(&format!("{RESP}type Resp = | A(y: String, x: Int) | B\n")),
        );
        assert_eq!(reordered.verdict, Verdict::Unchanged);
        let (a, b) = (
            shape(&format!("{RESP}type Resp = | A(x: Int) | B\n")),
            shape(&format!("{RESP}type Resp = | A(y: Int) | B\n")),
        );
        assert_ne!(a.normal_form(), b.normal_form());
        for s in [&a, &b] {
            assert_eq!(ServiceShape::from_text(&s.to_text()).as_ref(), Some(s));
        }
    }

    // ---- containers and the response itself ----------------------------------------------

    #[test]
    fn element_types_are_checked_through_containers() {
        let c = breaking(
            &format!("{RESP}type Resp = {{ xs: List[Int] }}\n"),
            &format!("{RESP}type Resp = {{ xs: List[String] }}\n"),
        );
        only(&c, Direction::Response, "response.xs[]");
        compatible(
            &format!("{RESP}type Row = {{ a: Int }}\ntype Resp = {{ xs: List[Row] }}\n"),
            &format!("{RESP}type Row = {{ a: Int, b: Int }}\ntype Resp = {{ xs: List[Row] }}\n"),
        );
    }

    #[test]
    fn a_response_made_optional_is_breaking_and_a_bare_request_made_optional_is_compatible() {
        let types = types_of("commons x\n");
        let svc = |param: TypeRef, ret: TypeRef| {
            ServiceShape::of(&service(vec![("p", param)], ret), &types)
        };
        let int = || TypeRef::Base(bynk_syntax::ast::BaseType::Int, sp());
        let opt = || TypeRef::Option(Box::new(int()), sp());
        let bare = svc(int(), int());
        let optional_param = svc(opt(), int());
        let optional_ret = svc(int(), opt());
        assert_eq!(
            check_call(&bare, &optional_param).verdict,
            Verdict::Compatible
        );
        assert_eq!(
            check_call(&optional_param, &bare).verdict,
            Verdict::Breaking
        );
        assert_eq!(check_call(&bare, &optional_ret).verdict, Verdict::Breaking);
        assert_eq!(
            check_call(&optional_ret, &bare).verdict,
            Verdict::Compatible
        );
    }

    #[test]
    fn a_recursive_type_closes_on_itself() {
        let a = format!("{REQ}type Req = {{ next: Option[Req], v: Int }}\n");
        let b = format!("{REQ}type Req = {{ next: Option[Req], v: Int, tag: Option[String] }}\n");
        compatible(&a, &b);
        breaking(&a, &b.replace("Option[String]", "String"));
    }

    // ---- parameters ----------------------------------------------------------------------------

    fn params(old: &[(&str, &str)], new: &[(&str, &str)]) -> Compatibility {
        let types = types_of("commons x\n");
        let make = |ps: &[(&str, &str)]| {
            let ps = ps
                .iter()
                .map(|(n, t)| {
                    let ty = match *t {
                        "Int" => TypeRef::Base(bynk_syntax::ast::BaseType::Int, sp()),
                        "String" => TypeRef::Base(bynk_syntax::ast::BaseType::String, sp()),
                        "Option[String]" => TypeRef::Option(
                            Box::new(TypeRef::Base(bynk_syntax::ast::BaseType::String, sp())),
                            sp(),
                        ),
                        other => panic!("{other}"),
                    };
                    (*n, ty)
                })
                .collect();
            ServiceShape::of(
                &service(ps, TypeRef::Base(bynk_syntax::ast::BaseType::Int, sp())),
                &types,
            )
        };
        check_call(&make(old), &make(new))
    }

    #[test]
    fn parameters_follow_the_keyed_object_rules() {
        let c = params(
            &[("a", "Int"), ("b", "Int")],
            &[("a", "Int"), ("b", "Int"), ("c", "Option[String]")],
        );
        assert_eq!(c.verdict, Verdict::Compatible, "{c:#?}");
        let c = params(
            &[("a", "Int"), ("b", "Int")],
            &[("a", "Int"), ("b", "Int"), ("c", "String")],
        );
        assert_eq!(c.verdict, Verdict::Breaking, "{c:#?}");
        only(&c, Direction::Request, "request.c");
        let c = params(&[("a", "Int"), ("b", "Int")], &[("a", "Int")]);
        assert_eq!(
            c.verdict,
            Verdict::Breaking,
            "one parameter is sent bare: {c:#?}"
        );
        let c = params(
            &[("a", "Int"), ("b", "Int"), ("c", "Int")],
            &[("b", "Int"), ("a", "Int")],
        );
        assert_eq!(c.verdict, Verdict::Compatible, "{c:#?}");
        let c = params(
            &[("a", "Int"), ("b", "Int")],
            &[("a", "Int"), ("b", "String")],
        );
        assert_eq!(c.verdict, Verdict::Breaking, "{c:#?}");
    }

    #[test]
    fn a_single_parameter_is_bare_so_its_name_is_not_a_break() {
        let c = params(&[("a", "Int")], &[("n", "Int")]);
        assert_eq!(c.verdict, Verdict::Compatible, "{c:#?}");
        let c = params(&[("a", "Int")], &[("a", "String")]);
        assert_eq!(c.verdict, Verdict::Breaking, "{c:#?}");
        let c = params(&[], &[("a", "Int")]);
        assert_eq!(c.verdict, Verdict::Breaking, "{c:#?}");
    }

    #[test]
    fn a_different_service_is_breaking() {
        let a = shape(&format!("{REQ}{RESP}"));
        let mut b = a.clone();
        b.name = "other".into();
        assert_eq!(check_call(&a, &b).verdict, Verdict::Breaking);
    }

    // ---- the classifier against a model of the decoder ---------------------
    //
    // `check_call` says a change is compatible. This holds it to that: for
    // every pair of shapes in a pool, if the verdict is not `Breaking`, every
    // value the sender can produce must decode at the receiver, under the
    // decode rules of `reference/wire-format.md`. The model below is written
    // from that reference, not from the classifier.
    //
    // The converse is not asserted: the classifier is allowed to be
    // conservative. A sum's payload fields are keyed by name on the wire, as a
    // record's are, and the model keys them by name on both sides.

    #[derive(Clone, Debug, PartialEq)]
    enum J {
        Bool(bool),
        Int(i64),
        Float(f64),
        Str(String),
        Arr(Vec<J>),
        Obj(Vec<(String, J)>),
    }

    fn obj(fields: Vec<(&str, J)>) -> J {
        J::Obj(
            fields
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
        )
    }

    fn get<'a>(fields: &'a [(String, J)], key: &str) -> Option<&'a J> {
        fields.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// The predicate, from its canonical text, held to a value.
    fn holds(pred: &str, v: &J) -> bool {
        let int = |j: &J| match j {
            J::Int(i) => Some(*i as f64),
            J::Float(f) => Some(*f),
            _ => None,
        };
        let len = |j: &J| match j {
            J::Str(s) => Some(s.chars().count() as i64),
            _ => None,
        };
        let args = |name: &str| -> Vec<f64> {
            pred.strip_prefix(name)
                .and_then(|r| r.strip_prefix('('))
                .and_then(|r| r.strip_suffix(')'))
                .map(|r| r.split(", ").map(|n| n.parse().unwrap()).collect())
                .unwrap_or_default()
        };
        match pred {
            "Positive" => int(v).is_some_and(|n| n > 0.0),
            "NonNegative" => int(v).is_some_and(|n| n >= 0.0),
            p if p.starts_with("InRangeF(") => {
                let a = args("InRangeF");
                int(v).is_some_and(|n| a[0] <= n && n <= a[1])
            }
            p if p.starts_with("InRange(") => {
                let a = args("InRange");
                int(v).is_some_and(|n| a[0] <= n && n <= a[1])
            }
            p if p.starts_with("MinLength(") => {
                len(v).is_some_and(|n| n as f64 >= args("MinLength")[0])
            }
            p if p.starts_with("MaxLength(") => {
                len(v).is_some_and(|n| n as f64 <= args("MaxLength")[0])
            }
            p if p.starts_with("Length(") => len(v).is_some_and(|n| n as f64 == args("Length")[0]),
            other => panic!("the model does not know {other}"),
        }
    }

    /// Can `shape` (with the field's own `preds`) decode `v`?
    fn decodes(shape: &Shape, preds: &[String], v: &J) -> bool {
        let base = |b: &str, v: &J| {
            matches!(
                (b, v),
                ("Int", J::Int(_))
                    | ("String", J::Str(_))
                    | ("Bool", J::Bool(_))
                    | ("Float", J::Int(_) | J::Float(_))
            )
        };
        let checked =
            |b: &str, ps: &[String]| base(b, v) && ps.iter().chain(preds).all(|p| holds(p, v));
        match shape {
            Shape::Base(b) => checked(b, &[]),
            Shape::Refined {
                base: b, preds: ps, ..
            } => checked(b, ps),
            Shape::Option(inner) => match v {
                J::Obj(f) if get(f, "kind") == Some(&J::Str("None".into())) => true,
                J::Obj(f) if get(f, "kind") == Some(&J::Str("Some".into())) => {
                    get(f, "value").is_some_and(|x| decodes(inner, &[], x))
                }
                // A bare value is `Some`.
                other => decodes(inner, &[], other),
            },
            Shape::List(inner) => {
                matches!(v, J::Arr(xs) if xs.iter().all(|x| decodes(inner, &[], x)))
            }
            Shape::Map(k, val) => matches!(v, J::Arr(xs) if xs.iter().all(|pair| matches!(
                pair, J::Arr(kv) if kv.len() == 2 && decodes(k, &[], &kv[0]) && decodes(val, &[], &kv[1])
            ))),
            Shape::Result(ok, err) => match v {
                J::Obj(f) => match get(f, "kind") {
                    Some(J::Str(k)) if k == "Ok" => {
                        get(f, "value").is_some_and(|x| decodes(ok, &[], x))
                    }
                    Some(J::Str(k)) if k == "Err" => {
                        get(f, "error").is_some_and(|x| decodes(err, &[], x))
                    }
                    _ => false,
                },
                _ => false,
            },
            Shape::Record { fields, .. } => match v {
                J::Obj(given) => fields.iter().all(|f| match get(given, &f.name) {
                    Some(x) => decodes(&f.ty, &f.preds, x),
                    None => matches!(f.ty, Shape::Option(_)),
                }),
                _ => false,
            },
            Shape::Sum { variants, .. } => match v {
                J::Obj(given) => match get(given, "kind") {
                    Some(J::Str(k)) => variants.iter().find(|x| &x.name == k).is_some_and(|var| {
                        var.payload
                            .iter()
                            .all(|p| get(given, &p.name).is_some_and(|x| decodes(&p.ty, &[], x)))
                    }),
                    _ => false,
                },
                _ => false,
            },
            other => panic!("the model does not cover {other:?}"),
        }
    }

    /// A small, fixed sample of what a sender of `shape` can produce.
    fn values(shape: &Shape, preds: &[String]) -> Vec<J> {
        let cap = |mut v: Vec<J>| {
            while v.len() > 40 {
                v.remove(v.len() / 2);
            }
            v
        };
        let keep = |vs: Vec<J>| -> Vec<J> {
            vs.into_iter()
                .filter(|v| preds.iter().all(|p| holds(p, v)))
                .collect()
        };
        match shape {
            Shape::Base(b) => keep(match b.as_str() {
                "Int" => [-1, 0, 1, 5, 200].map(J::Int).to_vec(),
                "Float" => [0.0, 0.5, 1.5].map(J::Float).to_vec(),
                "String" => ["", "a", "abc", "abcdefghij"]
                    .map(|s| J::Str(s.into()))
                    .to_vec(),
                "Bool" => vec![J::Bool(true), J::Bool(false)],
                other => panic!("{other}"),
            }),
            Shape::Refined {
                base, preds: ps, ..
            } => {
                let all: Vec<String> = ps.iter().chain(preds).cloned().collect();
                values(&Shape::Base(base.clone()), &all)
            }
            Shape::Option(inner) => {
                let mut out = vec![obj(vec![("kind", J::Str("None".into()))])];
                for v in values(inner, &[]).into_iter().take(3) {
                    out.push(obj(vec![("kind", J::Str("Some".into())), ("value", v)]));
                }
                out
            }
            Shape::List(inner) => {
                let vs = values(inner, &[]);
                let mut out = vec![J::Arr(vec![])];
                out.extend(vs.iter().take(2).map(|v| J::Arr(vec![v.clone()])));
                out.push(J::Arr(vs.into_iter().take(2).collect()));
                out
            }
            Shape::Map(k, v) => {
                let (ks, vs) = (values(k, &[]), values(v, &[]));
                vec![
                    J::Arr(vec![]),
                    J::Arr(vec![J::Arr(vec![ks[0].clone(), vs[0].clone()])]),
                ]
            }
            Shape::Result(ok, err) => {
                let mut out = Vec::new();
                for v in values(ok, &[]).into_iter().take(3) {
                    out.push(obj(vec![("kind", J::Str("Ok".into())), ("value", v)]));
                }
                for v in values(err, &[]).into_iter().take(3) {
                    out.push(obj(vec![("kind", J::Str("Err".into())), ("error", v)]));
                }
                out
            }
            Shape::Record { fields, .. } => {
                // Each field takes each of a few values, and an `Option` field
                // may also be left out.
                let mut rows: Vec<Vec<(String, J)>> = vec![vec![]];
                for f in fields {
                    let mut vs: Vec<Option<J>> = values(&f.ty, &f.preds)
                        .into_iter()
                        .take(3)
                        .map(Some)
                        .collect();
                    if matches!(f.ty, Shape::Option(_)) {
                        vs.push(None);
                    }
                    rows = rows
                        .into_iter()
                        .flat_map(|row| {
                            vs.iter().map(move |v| {
                                let mut r = row.clone();
                                if let Some(v) = v {
                                    r.push((f.name.clone(), v.clone()));
                                }
                                r
                            })
                        })
                        .collect();
                }
                cap(rows.into_iter().map(J::Obj).collect())
            }
            Shape::Sum { variants, .. } => {
                let mut out = Vec::new();
                for var in variants {
                    let mut rows: Vec<Vec<(String, J)>> =
                        vec![vec![("kind".into(), J::Str(var.name.clone()))]];
                    for p in &var.payload {
                        let vs: Vec<J> = values(&p.ty, &[]).into_iter().take(2).collect();
                        rows = rows
                            .into_iter()
                            .flat_map(|row| {
                                vs.iter().map(move |v| {
                                    let mut r = row.clone();
                                    r.push((p.name.clone(), v.clone()));
                                    r
                                })
                            })
                            .collect();
                    }
                    out.extend(rows.into_iter().map(J::Obj));
                }
                out
            }
            other => panic!("{other:?}"),
        }
    }

    fn int() -> Shape {
        Shape::Base("Int".into())
    }
    fn string() -> Shape {
        Shape::Base("String".into())
    }
    fn refined(base: &str, preds: &[&str]) -> Shape {
        Shape::Refined {
            name: "T".into(),
            base: base.into(),
            preds: preds.iter().map(|p| p.to_string()).collect(),
        }
    }
    fn record(fields: Vec<(&str, Shape)>) -> Shape {
        Shape::Record {
            name: "R".into(),
            fields: fields
                .into_iter()
                .map(|(n, ty)| Field {
                    name: n.into(),
                    ty,
                    preds: Vec::new(),
                })
                .collect(),
        }
    }
    fn sum(variants: Vec<(&str, Vec<(&str, Shape)>)>) -> Shape {
        Shape::Sum {
            name: "S".into(),
            variants: variants
                .into_iter()
                .map(|(n, payload)| Variant {
                    name: n.into(),
                    payload: payload
                        .into_iter()
                        .map(|(f, ty)| Field {
                            name: f.into(),
                            ty,
                            preds: Vec::new(),
                        })
                        .collect(),
                })
                .collect(),
        }
    }
    fn opt(s: Shape) -> Shape {
        Shape::Option(Box::new(s))
    }

    fn pool() -> Vec<Shape> {
        vec![
            int(),
            string(),
            Shape::Base("Bool".into()),
            Shape::Base("Float".into()),
            refined("Int", &["Positive"]),
            refined("Int", &["NonNegative"]),
            refined("Int", &["InRange(0, 10)"]),
            refined("Int", &["InRange(1, 5)"]),
            refined("Int", &["InRange(1, 5)", "Positive"]),
            refined("Float", &["Positive"]),
            refined("Float", &["InRangeF(0, 2)"]),
            refined("String", &["MinLength(1)"]),
            refined("String", &["MaxLength(5)"]),
            refined("String", &["Length(3)"]),
            refined("String", &["MinLength(1)", "MaxLength(5)"]),
            opt(int()),
            opt(string()),
            opt(refined("Int", &["Positive"])),
            Shape::List(Box::new(int())),
            Shape::List(Box::new(opt(int()))),
            Shape::Map(Box::new(string()), Box::new(int())),
            Shape::Result(Box::new(int()), Box::new(string())),
            Shape::Result(Box::new(int()), Box::new(int())),
            record(vec![]),
            record(vec![("a", int())]),
            record(vec![("b", int())]),
            record(vec![("a", opt(int()))]),
            record(vec![("a", int()), ("b", opt(int()))]),
            record(vec![("a", int()), ("b", int())]),
            record(vec![("a", int()), ("b", opt(string()))]),
            record(vec![("a", refined("Int", &["Positive"]))]),
            record(vec![("a", record(vec![("x", int())]))]),
            record(vec![("a", record(vec![("x", int()), ("y", opt(int()))]))]),
            sum(vec![("A", vec![]), ("B", vec![])]),
            sum(vec![("A", vec![])]),
            sum(vec![("A", vec![]), ("B", vec![]), ("C", vec![])]),
            sum(vec![("A", vec![("x", int())])]),
            sum(vec![("A", vec![("y", int())])]),
            sum(vec![("A", vec![("x", string())])]),
            sum(vec![("A", vec![("x", int()), ("y", string())])]),
            sum(vec![("A", vec![("y", string()), ("x", int())])]),
            sum(vec![("A", vec![("x", int())]), ("B", vec![])]),
        ]
    }

    fn response(ret: Shape) -> ServiceShape {
        ServiceShape {
            name: "probe".into(),
            params: Vec::new(),
            ret,
        }
    }

    fn request(param: Shape) -> ServiceShape {
        ServiceShape {
            name: "probe".into(),
            params: vec![("p".into(), param)],
            ret: Shape::Base("Bool".into()),
        }
    }

    /// The property, once per direction. Returns how many pairs were not
    /// breaking, so the test can tell it was not vacuous.
    fn sound(wrap: fn(Shape) -> ServiceShape, sender_is_callee: bool) -> usize {
        let pool = pool();
        let mut not_breaking = 0;
        for sender in &pool {
            for receiver in &pool {
                // A response flows callee to caller; a request, caller to callee.
                let (caller, callee) = if sender_is_callee {
                    (wrap(receiver.clone()), wrap(sender.clone()))
                } else {
                    (wrap(sender.clone()), wrap(receiver.clone()))
                };
                let verdict = check_call(&caller, &callee);
                if verdict.verdict == Verdict::Breaking {
                    continue;
                }
                not_breaking += 1;
                for v in values(sender, &[]) {
                    assert!(
                        decodes(receiver, &[], &v),
                        "the classifier called this compatible ({:?}), but the receiver refuses a value the sender can send:\n\
                         sender   {}\nreceiver {}\nvalue    {v:?}\n{:#?}",
                        verdict.verdict,
                        render(sender),
                        render(receiver),
                        verdict.findings,
                    );
                }
            }
        }
        not_breaking
    }

    #[test]
    fn every_response_the_classifier_calls_compatible_decodes_at_the_caller() {
        let n = sound(response, true);
        assert!(
            n > 100,
            "only {n} compatible pairs: the property is close to vacuous"
        );
    }

    #[test]
    fn every_request_the_classifier_calls_compatible_decodes_at_the_callee() {
        let n = sound(request, false);
        assert!(
            n > 100,
            "only {n} compatible pairs: the property is close to vacuous"
        );
    }
}

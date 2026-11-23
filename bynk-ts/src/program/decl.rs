use super::{TsExpr, TsStmt, TsType, TsTypeMember};
use bynk_syntax::span::Span;

/// One function/method/constructor parameter.
#[derive(Debug, Clone, PartialEq)]
pub struct TsParam {
    pub name: String,
    pub ty: Option<TsType>,
    /// `name?: ty` — `events_fanout.rs`'s own `env?: unknown` constructor
    /// parameter needs this. `optional` and a [`TsParam::default`] do not combine
    /// (`x?: T = v` is a TypeScript error); the printer asserts it.
    pub optional: bool,
    /// `...name: T` — a rest parameter (1A.32b: the history driver's console mute,
    /// `(...__a: unknown[]) => …`). Prints before the name; `optional` and `rest`
    /// do not combine.
    pub rest: bool,
    /// `[a, b]` (1A.35a part B): the parameter destructures an array, and these
    /// are its element names (an empty name is a hole). It prints in place of
    /// `name`, which is then unused. Build it with [`TsParam::array_pattern`].
    pub array_pattern: Option<Vec<String>>,
    /// `name: ty = default` (1A.38b): the value the parameter takes when its
    /// argument is left out (`query: Q = {}`). Build one with
    /// [`TsParam::with_default`]; `optional` and a default do not combine. The default
    /// is printed without a source-map segment of its own.
    pub default: Option<TsExpr>,
}

impl TsParam {
    /// A required parameter, with its type if it has one.
    pub fn new(name: impl Into<String>, ty: Option<TsType>) -> Self {
        Self {
            name: name.into(),
            ty,
            optional: false,
            rest: false,
            array_pattern: None,
            default: None,
        }
    }

    /// This parameter with a default value, `name: T = default`.
    #[must_use]
    pub fn with_default(mut self, default: TsExpr) -> Self {
        debug_assert!(
            !self.optional && !self.rest,
            "with_default: an optional or rest parameter takes no default"
        );
        self.default = Some(default);
        self
    }

    /// A parameter that destructures an array: `([a, b]) => …`.
    pub fn array_pattern(names: Vec<String>, ty: Option<TsType>) -> Self {
        Self {
            array_pattern: Some(names),
            ..Self::new(String::new(), ty)
        }
    }

    /// An optional parameter (`name?: T`).
    pub fn optional(name: impl Into<String>, ty: Option<TsType>) -> Self {
        Self {
            optional: true,
            ..Self::new(name, ty)
        }
    }

    /// This parameter as a rest parameter (`...name: T`).
    #[must_use]
    pub fn rest(mut self) -> Self {
        self.rest = true;
        self
    }
}

/// One `class` field.
#[derive(Debug, Clone, PartialEq)]
pub struct TsClassField {
    pub name: String,
    /// The field's type. `None` for a field that has only an initialiser, whose type TypeScript
    /// infers (`__seq_0 = 0;`); build one with [`TsClassField::inferred`].
    pub ty: Option<TsType>,
    /// The field's initialiser (`name: T = init;`). `None` for an ordinary field, which the
    /// constructor or a factory assigns.
    pub init: Option<TsExpr>,
    /// `private` — the one visibility modifier `events_fanout.rs` uses
    /// (`private env: ...`). Not a decorator or a constructor parameter
    /// property (R7.1 forbids both categorically — there is no variant
    /// shape here that could construct either).
    pub private: bool,
    /// `name!: T;` — a definite-assignment assertion, for a field the
    /// constructor does not set but whoever builds the object does (1A.32b: an
    /// agent's `__key`, assigned by its factory or its dispatch before any
    /// handler runs). Printed after the name, before the `:`.
    pub definite: bool,
}

impl TsClassField {
    /// A public, ordinarily-assigned field. Mark it with [`TsClassField::private`]
    /// or [`TsClassField::definite`].
    pub fn new(name: impl Into<String>, ty: TsType) -> Self {
        Self {
            name: name.into(),
            ty: Some(ty),
            init: None,
            private: false,
            definite: false,
        }
    }

    /// A public field with an initialiser and no written type (`name = init;`), whose type
    /// TypeScript infers from it (1B.5c: a stub class's `__seq_0 = 0;`).
    pub fn inferred(name: impl Into<String>, init: TsExpr) -> Self {
        Self {
            name: name.into(),
            ty: None,
            init: Some(init),
            private: false,
            definite: false,
        }
    }

    /// This field with the `private` modifier.
    #[must_use]
    pub fn private(mut self) -> Self {
        self.private = true;
        self
    }

    /// This field with a definite-assignment assertion (`name!: T;`).
    #[must_use]
    pub fn definite(mut self) -> Self {
        self.definite = true;
        self
    }
}

/// A class's own constructor.
#[derive(Debug, Clone, PartialEq)]
pub struct TsClassCtor {
    pub params: Vec<TsParam>,
    pub body: Vec<TsStmt>,
}

/// One class method.
#[derive(Debug, Clone, PartialEq)]
pub struct TsClassMethod {
    pub name: String,
    /// `private {name}(...)`, e.g. `loadState`/`commitState` — the
    /// grounding pass's own predicted gap (#1366), closed by Arc C's own
    /// `emit_agent` class-scaffold slice: every real `TsClassMethod` site
    /// before this one (`emit_provider` #1359's own op methods) was
    /// public-only. Rendered before `async`, matching the one real site's
    /// own modifier order (`private async loadState()`, not `async private
    /// loadState()`).
    pub private: bool,
    pub is_async: bool,
    pub params: Vec<TsParam>,
    pub return_type: Option<TsType>,
    /// A JSDoc block immediately preceding the method — the grounding
    /// pass's own second predicted gap (#1366), closed alongside `private`'s
    /// own sibling site: `emit_agent`'s own per-handler methods each carry
    /// a doc comment (`emit_doc_block`'s pre-conversion standalone call),
    /// the same need `TsObjectEntry::Method.doc` (#1337) and
    /// `TsTypeMember::Method.doc` (#1357) already solved for their own node
    /// kinds. Rendered the identical way those two already are — a
    /// `TsStmtKind::DocComment`-shaped block at the method's own indent,
    /// immediately before its header line.
    pub doc: Option<String>,
    pub body: Vec<TsStmt>,
    /// The source span the method's first printed line maps to (its doc comment,
    /// if it has one), so the header and prologue anchor to the method's own
    /// declaration rather than to the previous member's last body line (#2280,
    /// 1A.32a). `None` records nothing.
    pub span: Option<Span>,
}

impl TsClassMethod {
    /// A public, undocumented method with no span. Set `private`, `doc` or `span`
    /// with struct update (`TsClassMethod { private: true, ..TsClassMethod::new(..) }`)
    /// when one is needed.
    pub fn new(
        name: impl Into<String>,
        is_async: bool,
        params: Vec<TsParam>,
        return_type: Option<TsType>,
        body: Vec<TsStmt>,
    ) -> Self {
        Self {
            name: name.into(),
            private: false,
            is_async,
            params,
            return_type,
            doc: None,
            body,
            span: None,
        }
    }

    /// This method with the `private` modifier.
    #[must_use]
    pub fn private(mut self) -> Self {
        self.private = true;
        self
    }
}

/// A top-level declaration. `Import`, `Export`, `Interface`, `ConstDecl`,
/// and `Class` were `events_fanout.rs`'s own grounding (P7.8's own note:
/// "not the sketch's `Function`/`TypeAlias` — unused in the grounding
/// file"). #1321 (`workers.rs`) needed both after all — a real gap the
/// accepted proposal's own Framing didn't name (its own "no other gap
/// surfaced" checked `TsStmt`/`TsExpr`/`TsType` shapes, not `TsDecl` ones):
/// `compose.ts`'s own `export function compose(env: Env, …) { … }` is a
/// top-level function declaration, and (when the Worker has agents or
/// publishes events) a `type DurableObjectNamespace = { … };` fallback
/// alias sits alongside it. Named explicitly as a deviation from the
/// accepted proposal's own catalogue, not silently added — both are
/// mechanical, direct `TsDecl` siblings of `ConstDecl`/`Class` already
/// here, the same "small, grounded, same class of gap" this track's own
/// history repeatedly found and closed (P7.8's `Assign`/`Continue`/
/// `TryCatch`, Arc C slice 1's `Comment`).
#[derive(Debug, Clone, PartialEq)]
pub enum TsDecl {
    /// `import { a, b } from "spec";` (`type_only: false`) or
    /// `import type { a, b } from "spec";` (`type_only: true`). Only the
    /// named-imports form — `events_fanout.rs` never uses a default
    /// import, so none is represented. `names` entries are pushed verbatim
    /// (`printer.rs`'s own `names.join(", ")`), so a renamed import
    /// (`__messagesLocales as __locale_declaredLocales`, `workers.rs`'s own
    /// locale-negotiation import) is one opaque `String` entry, not a
    /// structured rename — the same "the field is already a raw-text slot"
    /// reasoning that also covers a `type`-prefixed single specifier
    /// (`"type KVNamespace"`) inside an otherwise non-`type_only` import.
    Import {
        type_only: bool,
        names: Vec<String>,
        from: String,
    },
    /// `import * as alias from "spec";` — a namespace import, structurally
    /// different from [`TsDecl::Import`]'s braced named-imports form (no
    /// `{ }`, one bound name for the whole module). #1321's own real gap:
    /// `workers.rs`'s `compose.ts` imports `handlers.js` and each
    /// referenced unit's binding module this way; `events_fanout.rs` never
    /// used one. `type_only` (Arc C, step (10), #1392) mirrors
    /// [`TsDecl::Import`]'s own identical field, a parallel gap by
    /// omission, not deliberate design — `emit_cross_context_namespace_
    /// imports`'s own real `import type * as ns from "...";` form (a
    /// Workers-mode consumed-context import reaching the callee's types
    /// only, #661) had no way to represent the `type` keyword until now.
    ImportNamespace {
        type_only: bool,
        alias: String,
        from: String,
    },
    /// `import name from "spec";` — a default import, structurally
    /// different from both [`TsDecl::Import`] (braced named-imports form)
    /// and [`TsDecl::ImportNamespace`] (`* as` form) — no braces, no `* as`,
    /// one bound local name for the module's own default export. No
    /// `type_only` form — nothing in the grounding file default-imports a
    /// type. Arc C, slice 37 (#1409, `tests_emit.rs`'s own slice G): a
    /// per-participant `import worker_{ns} from "../workers/{dir}/
    /// index.js";` (the participant's own Worker entry module's default
    /// export) is the first real default import anywhere in `bynk-emit`'s
    /// own converted content — every prior import site is either named
    /// ([`TsDecl::Import`]) or namespace ([`TsDecl::ImportNamespace`]).
    ///
    /// 1A.38a: `named` carries the braced names that ride on the same statement
    /// (`import __entry0, { isMiss as __miss0 } from "…";`, a Worker group's
    /// member import), in [`TsDecl::Import`]'s own spelling (`a as b`,
    /// `type T`). Empty is the plain default import every earlier site builds.
    ImportDefault {
        alias: String,
        named: Vec<String>,
        from: String,
    },
    /// `export { a, b } from "spec";` — a re-export, structurally distinct
    /// from both [`TsDecl::Import`] (which binds locally, carries no
    /// `export` keyword) and [`TsDecl::Export`] (which wraps a whole
    /// declaration, not a braced name list re-pointed at another module).
    /// #1323's own real gap: `workers_entry.rs` re-exports each agent's
    /// Durable Object class from `./handlers.js`, and (when the context
    /// publishes events) the fan-out DO's class from `./events_fanout.js`.
    /// No `type_only` form — nothing in the grounding file re-exports a
    /// type-only name.
    ReExport { names: Vec<String>, from: String },
    /// `export { a, b };` — exports names already bound in this module (an
    /// import's, typically), with no `from` clause, which [`TsDecl::ReExport`]
    /// always carries. A Workers context re-exports the boundary codecs it
    /// imports from a `uses`d commons this way. #1820: a structured variant,
    /// not opaque text, so [`TsStmt::exported_names`] reports what the module
    /// exports to a barrel that must find the names two modules both export.
    ExportNames { names: Vec<String> },
    /// `export * from "spec";` — a wildcard re-export, structurally distinct
    /// from [`TsDecl::ReExport`] (which always carries a braced name list —
    /// an empty `names` there would render the ill-formed `export {  }
    /// from "spec";`, not this). Mirrors [`TsDecl::ImportNamespace`]'s own
    /// "no braces, no name list, one bound target" shape, on the export
    /// side. #1329's own real gap: `emit_commons_barrel`
    /// (`bynk-emit/src/project/tests_emit.rs`) builds a multi-file
    /// `commons` unit's barrel module as one `export *` line per
    /// constituent source file.
    ReExportAll { from: String },
    /// Marks the wrapped declaration `export`ed — `export class Foo { .. }`
    /// is `Export(Box::new(Class { .. }))`. A wrapper, not a per-variant
    /// `exported: bool` field, matching the reference sketch's own naming
    /// (`TsDecl::Export` is a peer variant, not a modifier on each other
    /// one).
    Export(Box<TsDecl>),
    /// `type_params`/per-member `readonly` (#1339's own real gap):
    /// `emit_record_type`'s own `export interface {name}{params} {
    /// readonly {field}: {ty}; ... }` — bare generic names (matching
    /// `ts_type_params`'s/`TsObjectEntry::Method.generics`'s own
    /// convention) and a `readonly` modifier every real field here
    /// carries. `members` reuses [`TsTypeMember`] (the exact same shape
    /// [`TsType::Object`]'s own structural members already use) rather
    /// than a bespoke tuple, since `readonly` is already that type's own
    /// field — this interface's real content has never needed
    /// `Method`/`Index`, but nothing about reusing the shared type
    /// forecloses a future slice that does.
    Interface {
        name: String,
        type_params: Vec<String>,
        members: Vec<TsTypeMember>,
    },
    /// A top-level `const` — distinct from `TsStmtKind::Const` (private —
    /// reachable through [`TsStmt::const_stmt`]), the local
    /// form.
    ConstDecl {
        name: String,
        ty: Option<TsType>,
        init: TsExpr,
    },
    /// `class Name implements A, B { ... }`. `implements` is empty for a class that
    /// names no interface, and prints as before. `end_span` is the source span
    /// the closing `}` line maps to (1A.32a), so what follows the last member
    /// belongs to the declaration rather than to that member's final body line;
    /// `None` records nothing there. Build one with [`TsDecl::class`].
    Class {
        name: String,
        implements: Vec<TsType>,
        fields: Vec<TsClassField>,
        constructor: Option<TsClassCtor>,
        methods: Vec<TsClassMethod>,
        end_span: Option<Span>,
    },
    /// `function name(params): ret { body }` (`is_async: false`) or `async
    /// function name(params): ret { body }` (`is_async: true`) — a top-level
    /// function declaration. `is_async` added by #1325: `workers.rs`'s own
    /// one real site (`compose`) is never async, so the field didn't exist
    /// until `emit_test_main`'s own top-level `async function main() {...}`
    /// needed it — the exact "extend narrowly, add it when a future slice's
    /// own grounding needs it" deferral this variant's own history already
    /// named. `generics` added by #1351: `emit_free_fn`'s own v0.20a erased
    /// generics (`export function foo<T>(...)`) — bare names only, empty
    /// for every non-generic site (`compose`/`main`), matching every other
    /// real generics-list precedent in this crate
    /// (`TsObjectEntry::Method.generics` #1337, `TsExpr::Arrow.generics`
    /// #1339). `inline` added by #1369 (Arc C, slice 20, step (9)'s own
    /// second sub-slice): `emit_agent`'s own zero-factory function
    /// (`function __zeroOf{Name}State(): {Name}State { return {...}; }`) is
    /// a genuinely single-physical-line declaration — braces and body share
    /// the header's own line, not `render_block_stmts`'s always-multi-line
    /// shape. `false` (every site landed before #1369) renders the ordinary
    /// multi-line body; `true` reuses `render_inline_block`'s own compact
    /// `{ stmt; stmt; }` shape directly at the declaration's own header
    /// line, the same "one more bool, mirroring an existing precedent"
    /// scope `TsObjectEntry::Method.inline` (#1337) already set for the
    /// identical single-line-vs-multi-line tension at a different node kind.
    Function {
        name: String,
        generics: Vec<String>,
        params: Vec<TsParam>,
        return_type: Option<TsType>,
        body: Vec<TsStmt>,
        is_async: bool,
        inline: bool,
    },
    /// `type name = ty;` (non-generic) or `type name<T, U> = ty;` (via
    /// `type_params`, #1339's own real gap: `emit_sum_type`'s own generic
    /// sum types erase to `export type Foo<T> = ...`) — a top-level type
    /// alias. `workers.rs`'s own real site: the `DurableObjectNamespace`
    /// local fallback type (emitted only when the Worker has agents or
    /// publishes events), never generic — `type_params` is empty there.
    TypeAlias {
        name: String,
        type_params: Vec<String>,
        ty: TsType,
    },
    /// `export default <expr>;` — a default export of an *expression*, not
    /// a declaration. #1323's own real gap (`workers_entry.rs`'s own
    /// top-level shape, `export default { fetch, scheduled?, queue? }`):
    /// [`TsDecl::Export`] wraps a [`TsDecl`] (a `Class`/`ConstDecl`/etc.,
    /// something with its own name), which cannot represent exporting a
    /// bare, unnamed object-literal expression — so this is its own
    /// variant, not `Export(Box::new(ConstDecl { .. }))` with a synthetic
    /// name that would print wrong.
    ExportDefault(TsExpr),
    /// `declare const name: ty;` — an ambient binding with no initialiser,
    /// narrowing a global TypeScript otherwise doesn't know about. #1325's
    /// own real, narrow site: `emit_test_main`'s own `declare const process:
    /// { exit(code: number): never; env: { [k: string]: string | undefined
    /// } };`, narrowing Node's `process` global without a `@types/node`
    /// dependency. Distinct from [`TsDecl::ConstDecl`] (always has a real
    /// `init` expression) — a `declare const` has none, by definition; a
    /// `TsDecl::ConstDecl` with a synthetic placeholder `init` would print
    /// wrong (`= <something>;` where nothing should appear at all).
    DeclareConst { name: String, ty: TsType },
}

impl TsDecl {
    /// `import <alias> from "<from>";`: a default import with no braced names.
    pub fn import_default(alias: impl Into<String>, from: impl Into<String>) -> Self {
        TsDecl::ImportDefault {
            alias: alias.into(),
            named: Vec::new(),
            from: from.into(),
        }
    }

    /// A class with no `implements` clause and no closing-line span: the shape
    /// every class had before 1A.32a. Name the extras with [`TsDecl::implementing`]
    /// and [`TsDecl::ending_at`].
    pub fn class(
        name: impl Into<String>,
        fields: Vec<TsClassField>,
        constructor: Option<TsClassCtor>,
        methods: Vec<TsClassMethod>,
    ) -> Self {
        TsDecl::Class {
            name: name.into(),
            implements: Vec::new(),
            fields,
            constructor,
            methods,
            end_span: None,
        }
    }

    /// This class with `implements` as its `implements` clause. A declaration that
    /// is not a class is returned unchanged.
    #[must_use]
    pub fn implementing(mut self, implements: Vec<TsType>) -> Self {
        if let TsDecl::Class {
            implements: slot, ..
        } = &mut self
        {
            *slot = implements;
        }
        self
    }

    /// This class with its closing `}` line mapped to `span`. A declaration that
    /// is not a class is returned unchanged.
    #[must_use]
    pub fn ending_at(mut self, span: Span) -> Self {
        if let TsDecl::Class { end_span, .. } = &mut self {
            *end_span = Some(span);
        }
        self
    }
}

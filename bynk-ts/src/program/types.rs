#[cfg(doc)]
use super::{TsDecl, TsLit, TsObjectEntry};
use super::{TsExpr, TsParam};

/// A type-position node. `Named` (extended with type arguments — a real gap
/// the reference sketch left unaddressed: `Record<string,
/// Array<{...}>>`/`Promise<Response>` both need one, and a bare `Named`
/// with no type-argument slot cannot represent either), `Array` (extended
/// with a `readonly` modifier — P7.9's own real gap: every `List`/`Query`
/// element type `bynk-emit`'s `ts_type_ref*`/`ts_ty` families build is
/// `readonly T[]`, not plain `T[]`), `Object`, and `Fn` (P7.9's own second
/// real gap — the query-thunk wrapper `(() => readonly T[])` and a real
/// parametered function type `(a0: T0, …) => Ret` both need one) — not the
/// sketch's `Union`/`Intersection`/`Literal`/`TypeParam`/`Readonly` (still
/// unused; `readonly` here is a modifier on `Array`, not the sketch's own
/// separate `Readonly` wrapper variant).
#[derive(Debug, Clone, PartialEq)]
pub enum TsType {
    /// A named type, optionally generic — `string`/`unknown` (no type
    /// arguments) or `Record<string, T>`/`Promise<Response>` (some).
    Named {
        name: String,
        type_args: Vec<TsType>,
    },
    /// `T[]` (`readonly: false`) or `readonly T[]` (`readonly: true`) —
    /// TypeScript's own postfix array-type syntax (not the equivalent
    /// `Array<T>`/`ReadonlyArray<T>` generic spelling either family uses).
    ///
    /// The printer parenthesises `element` wherever a bare postfix `[]`
    /// would bind to the wrong thing (#1898): a readonly array (`readonly
    /// T[][]` is TS1354), a [`TsType::Fn`] (whose `[]` would bind to its
    /// return type), a [`TsType::Union`] and a [`TsType::Intersection`]
    /// (`A | B[]` reads as `A | (B[])`). So `Array` over any element prints
    /// correctly as built — a caller must not wrap the element itself, or
    /// it double-parenthesises. The rule is `needs_parens_as_array_element`
    /// in `printer.rs`. This closes the hazard the review of #1315/#1316
    /// recorded here.
    Array {
        element: Box<TsType>,
        readonly: bool,
    },
    /// A type-position object shape, e.g. `{ type: string; payload: unknown }`
    /// — semicolon-separated, always printed on one line (see
    /// [`TsExpr::Object`]'s own doc for the value-position contrast). An
    /// `interface`'s own *members* are each printed on their own line by
    /// [`TsDecl::Interface`]'s printer, independent of this variant's own
    /// (always-inline) rendering — the two only look different because
    /// `TsDecl::Interface` is the thing choosing to put each member on its
    /// own line, not because `Object` has two rendering modes.
    ///
    /// #1323 replaced the plain `Vec<(String, TsType, bool)>` (`optional`
    /// only, #1321) with [`TsTypeMember`] — `workers_entry.rs`'s own real
    /// gap needed both a `readonly` modifier (every structural type this
    /// file builds is `readonly`-qualified, e.g. `{ readonly cron: string;
    /// readonly scheduledTime: number }`) and a method-signature member
    /// (`ack(): void`, `retry(): void`, `waitUntil(promise: Promise<
    /// unknown>): void`), neither representable by one more anonymous
    /// tuple `bool` — a fourth positional `bool` next to `optional` would
    /// be genuinely ambiguous at call sites (`readonly` vs. `optional`, ordered
    /// how?), the same reasoning [`TsObjectEntry`] already used over a raw
    /// tuple for the value-position sibling.
    Object(Vec<TsTypeMember>),
    /// `(a0: T0, a1: T1, …) => Ret` — a function type. Parameters carry no
    /// name of their own (just their type); the printer numbers them
    /// positionally (`a0`, `a1`, …), matching the exact convention
    /// `bynk-emit`'s own pre-P7.9 `ts_type_ref_with`/`ts_ty` already used
    /// (TypeScript requires *some* name in function-type syntax, and
    /// nothing about a `TypeRef::Fn`/`Ty::Fn` parameter carries a real one
    /// to use instead). A zero-`params` `Fn` is the query-thunk wrapper
    /// shape, `() => Ret`. Inside a [`TsType::Array`] the printer
    /// parenthesises it (`readonly ((a0: T) => U)[]`); see that variant's
    /// own doc.
    ///
    /// `names` (1A.18b, #3719): optional parameter names, for a function type
    /// whose names are part of its spelling (`(acc: A, x: T) => A`, the list
    /// kernel's `fold`). Empty is the positional `a0, a1, …` form above, which
    /// every other site builds; otherwise one name per entry of `params`, in
    /// order.
    ///
    /// `rest` (1A.40c, #3996): the last parameter is a rest parameter, printed `...name: T`, so
    /// the type spells a variadic function (`(...bynkArgs: unknown[]) => unknown`). Its name is
    /// taken from `names`, so a rest `Fn` carries names; `false` is every other function type.
    Fn {
        params: Vec<TsType>,
        names: Vec<String>,
        rest: bool,
        ret: Box<TsType>,
    },
    /// `A | B | C` — a type-position union. Added in review of #1315:
    /// `bynk-emit`'s `ts_ty` builds a real union type for a resolved
    /// multi-actor sum (`Ty::ActorSum`, discriminated-union members tagged
    /// by actor name), a shape none of `Named`/`Array`/`Object`/`Fn` can
    /// represent — a real, grounded gap the same way `readonly`/`Fn`
    /// themselves were (P7.9's own accepted proposal), not a speculative
    /// addition. Each member prints through the ordinary `render_type`
    /// recursion; a member that is itself a `Union` is legal to construct
    /// but nothing in `bynk-emit` builds one today. Inside a
    /// [`TsType::Array`] the printer parenthesises it (`(A | B)[]`); see that
    /// variant's own doc.
    ///
    /// `multiline` (#1339): `false` (via [`TsType::union`]) is this variant's
    /// original single-line `A | B | C` form, unchanged. `true` (via
    /// [`TsType::multiline_union`]) is `emit_sum_type`'s own real
    /// discriminated-union shape — one variant per line, a leading `|` on
    /// every line *except* the first (which gets equivalent spacing
    /// instead), the closing `;` appended directly to the last variant's own
    /// line. Mirrors [`TsExpr::Object`]'s own `multiline` field precedent
    /// (#1317), but — unlike that one — needs no depth-aware wrapper: the
    /// pre-conversion `writeln!` code this reproduces always used a fixed
    /// two-space indent regardless of nesting (this shape is only ever a
    /// top-level `export type` alias body, never nested inside another
    /// type), so `render_type` stays entirely depth-unaware here too. Only
    /// rendered correctly from [`TsDecl::TypeAlias`]'s own top-level
    /// render — not reachable, and not given a defined rendering, from any
    /// nested position (an array element, a type argument, …), the same
    /// named boundary [`TsExpr::Object`]'s own `multiline` doc already
    /// draws for its own nested case.
    Union {
        members: Vec<TsType>,
        multiline: bool,
    },
    /// `A & B` — a type-position intersection. #1339's own real gap:
    /// `emit_refined_type`'s own branded-type alias,
    /// `{base} & { readonly __brand: "..." }`, has no representation among
    /// `Named`/`Array`/`Object`/`Fn`/`Union` — mirrors `Union`'s own
    /// shape/precedent exactly (a flat `Vec`, each member printed through
    /// the ordinary `render_type` recursion, joined by ` & `), single-line
    /// only (nothing in `bynk-emit` builds a multi-line intersection).
    Intersection(Vec<TsType>),
    /// A mapped type, `{ -readonly [K in keyof T]: V }`: one property per key of
    /// `keys_of`, typed `value` (which may mention `key`).
    ///
    /// `readonly` is the modifier on the generated properties: `None` leaves them as
    /// they are, `Some(Add)` prints `+readonly` and `Some(Remove)` prints `-readonly`.
    /// `keys_of` is the operand of `keyof`, so the constraint is always
    /// `key in keyof keys_of`. `key` is a name the type binds for `value`. Always
    /// printed on one line, as a [`TsType::Object`] is.
    Mapped {
        readonly: Option<TsMappedModifier>,
        key: String,
        keys_of: Box<TsType>,
        value: Box<TsType>,
    },
    /// `typeof x`: the type of a value. The operand is an identifier or a chain of
    /// member accesses (`typeof a.b`), the only expressions TypeScript allows here;
    /// it is a use of that value for `name_closure`.
    TypeOf(Box<TsExpr>),
    /// `T[K]`: an indexed access type, `object` indexed by `index` (a key type such
    /// as [`TsType::StringLiteral`] or a mapped type's own key).
    Indexed {
        object: Box<TsType>,
        index: Box<TsType>,
    },
    /// A string literal type, `"k"`, escaped as a [`TsLit::Str`] is.
    StringLiteral(String),
    /// A non-negative integer literal type, `0`, as a tuple index (`T[0]`).
    NumberLiteral(u32),
    /// `(T)`: a type in parentheses, always. The printer already parenthesises where a
    /// postfix operator would misread (an array element, an indexed object); this is for a
    /// position the printer does not know about, such as the thunk type a `Query` annotation
    /// is written as, `(() => T)`. It prints as the parentheses around the inner type's own
    /// spelling, and `name_closure` looks through it.
    Paren(Box<TsType>),
}

/// The modifier on a [`TsType::Mapped`] type's `readonly`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TsMappedModifier {
    /// `+readonly`.
    Add,
    /// `-readonly`.
    Remove,
}

impl TsType {
    /// A plain named type with no type arguments — `string`, `unknown`,
    /// `Request`, …
    pub fn named(name: impl Into<String>) -> Self {
        TsType::Named {
            name: name.into(),
            type_args: Vec::new(),
        }
    }

    /// A generic named type — `Record<K, V>`, `Promise<T>`, …
    pub fn named_with_args(name: impl Into<String>, type_args: Vec<TsType>) -> Self {
        TsType::Named {
            name: name.into(),
            type_args,
        }
    }

    /// `T[]` — the non-`readonly` array shape.
    pub fn array(element: TsType) -> Self {
        TsType::Array {
            element: Box::new(element),
            readonly: false,
        }
    }

    /// `readonly T[]`.
    pub fn readonly_array(element: TsType) -> Self {
        TsType::Array {
            element: Box::new(element),
            readonly: true,
        }
    }

    /// `A | B | C` — the original single-line union form, unchanged.
    pub fn union(members: Vec<TsType>) -> Self {
        TsType::Union {
            members,
            multiline: false,
        }
    }

    /// `emit_sum_type`'s own real multi-line discriminated-union shape — see
    /// [`TsType::Union`]'s own doc for the exact rendering rules and why
    /// this needs no depth parameter.
    pub fn multiline_union(members: Vec<TsType>) -> Self {
        TsType::Union {
            members,
            multiline: true,
        }
    }

    /// `A & B` — an intersection type.
    pub fn intersection(members: Vec<TsType>) -> Self {
        TsType::Intersection(members)
    }

    /// `{ [key in keyof keys_of]: value }`, with no `readonly` modifier.
    pub fn mapped(key: impl Into<String>, keys_of: TsType, value: TsType) -> Self {
        TsType::Mapped {
            readonly: None,
            key: key.into(),
            keys_of: Box::new(keys_of),
            value: Box::new(value),
        }
    }

    /// The same mapped type with its properties' `readonly` modifier set.
    pub fn with_mapped_readonly(mut self, modifier: TsMappedModifier) -> Self {
        if let TsType::Mapped { readonly, .. } = &mut self {
            *readonly = Some(modifier);
        } else {
            debug_assert!(false, "with_mapped_readonly: not a mapped type");
        }
        self
    }

    /// `typeof name`.
    pub fn type_of(operand: TsExpr) -> Self {
        TsType::TypeOf(Box::new(operand))
    }

    /// `object[index]`.
    pub fn indexed(object: TsType, index: TsType) -> Self {
        TsType::Indexed {
            object: Box::new(object),
            index: Box::new(index),
        }
    }

    /// `(inner)`: the type in parentheses.
    pub fn paren(inner: TsType) -> Self {
        TsType::Paren(Box::new(inner))
    }

    /// The string literal type `"text"`.
    pub fn string_literal(text: impl Into<String>) -> Self {
        TsType::StringLiteral(text.into())
    }

    /// The integer literal type `n` (a tuple index: `Parameters<F>[0]`).
    pub fn number_literal(n: u32) -> Self {
        TsType::NumberLiteral(n)
    }
}

/// One member of a [`TsType::Object`] structural type — a property (`Prop`)
/// or a method signature (`Method`, #1323's own real gap: `ack(): void`,
/// `retry(): void`, `waitUntil(promise: Promise<unknown>): void` — no body,
/// a type-position sibling to [`TsObjectEntry::Method`], which does carry
/// one). `Method`'s own parameters reuse [`TsParam`] directly (not a bare
/// `Vec<TsType>` the way [`TsType::Fn`]'s anonymous, positionally-numbered
/// parameters do) — `waitUntil`'s own real parameter has a real name
/// (`promise`) the printed text must show, unlike `Fn`'s callers, none of
/// which have one to show.
#[derive(Debug, Clone, PartialEq)]
pub enum TsTypeMember {
    Prop {
        name: String,
        ty: TsType,
        optional: bool,
        readonly: bool,
    },
    /// `generics`/`doc` added by #1357: `emit_capability`'s own interface
    /// methods are genuinely generic (`op<T>(...): ret;`, no
    /// monomorphisation) and doc-commented — bare generic names, matching
    /// every other real generics-list precedent in this crate; `doc`
    /// mirrors `TsObjectEntry::Method.doc`'s own identical field (#1337).
    /// Both default empty/`None` via [`TsTypeMember::method`]'s own
    /// existing constructor — every one of its 6 real pre-#1357 callers is
    /// unaffected.
    ///
    /// `doc` renders from exactly one of this variant's two reachable
    /// positions: `TsDecl::Interface`'s own render arm (a real, multi-line
    /// declaration body with `depth` available, so it calls
    /// `render_doc_comment` before a documented member's own line — the
    /// only place `doc` is honoured). A `Method` reached through
    /// `TsType::Object`'s own inline, single-line shape (a type-position
    /// object literal, e.g. `{ a: X; b(): Y }`) has no line budget for a
    /// JSDoc block at all — `doc: Some(_)` there is a real, `debug_assert`-
    /// guarded misuse (`render_type`'s own `TsType::Object` arm), the same
    /// "loud, not silently dropped" precedent review of #1338 already
    /// established for `render_object_entry_inline`'s identical
    /// `TsObjectEntry::Method.doc` case.
    Method {
        name: String,
        generics: Vec<String>,
        params: Vec<TsParam>,
        ret: TsType,
        doc: Option<String>,
    },
    /// `[key_name: key_ty]: value_ty` — a TypeScript index signature.
    /// #1323's own real gap: `workers_entry.rs`'s multi-param `on call`
    /// dispatch casts its raw JSON args object through `{ [k: string]:
    /// JsonValue }` before indexing it by field name — textually distinct
    /// from the semantically-equivalent `Record<string, JsonValue>` (a
    /// `Named` type with type arguments), which `bynk-emit`'s own
    /// pre-conversion `writeln!` never wrote here. The one real site.
    Index {
        key_name: String,
        key_ty: TsType,
        value_ty: TsType,
    },
}

impl TsTypeMember {
    /// `name: ty` — the ordinary, non-`optional`, non-`readonly` case.
    pub fn prop(name: impl Into<String>, ty: TsType) -> Self {
        TsTypeMember::Prop {
            name: name.into(),
            ty,
            optional: false,
            readonly: false,
        }
    }

    /// `name?: ty`.
    pub fn optional_prop(name: impl Into<String>, ty: TsType) -> Self {
        TsTypeMember::Prop {
            name: name.into(),
            ty,
            optional: true,
            readonly: false,
        }
    }

    /// `readonly name: ty`.
    pub fn readonly_prop(name: impl Into<String>, ty: TsType) -> Self {
        TsTypeMember::Prop {
            name: name.into(),
            ty,
            optional: false,
            readonly: true,
        }
    }

    /// `name(params): ret` — no body, no generics, no doc comment.
    pub fn method(name: impl Into<String>, params: Vec<TsParam>, ret: TsType) -> Self {
        TsTypeMember::Method {
            name: name.into(),
            generics: Vec::new(),
            params,
            ret,
            doc: None,
        }
    }

    /// `[key_name: key_ty]: value_ty`.
    pub fn index(key_name: impl Into<String>, key_ty: TsType, value_ty: TsType) -> Self {
        TsTypeMember::Index {
            key_name: key_name.into(),
            key_ty,
            value_ty,
        }
    }
}

//! #4190: the wire a stubbed cross-context call crosses at `integration`.
//!
//! A real call encodes its arguments and decodes its result through the consumed
//! context's codecs. At `integration` the stub's double is wrapped in
//! `__bynkWireStub` with those same codecs, imported from the context's emitted
//! `workers` handlers module (one codec per type), so a value the wire would
//! not carry fails the call.

use super::*;
use crate::emitter::RuntimeUse;
use crate::emitter::serialisation::{deserialise_ref_via, serialise_ref_via};
use bynk_check::test_suites::ResolvedStub;
use bynk_syntax::ast::TypeRef;
use bynk_ts::{TsDecl, TsExpr, TsStmt};

/// What a test module's printed text holds when it imports a context's codecs: the driver reads
/// it to build the `workers` output those imports resolve against.
pub const WIRE_IMPORT_MARK: &str = "__wire from \"../workers/";

/// The namespace the consumed context's codecs are imported under.
pub(super) fn wire_ns(ctx: &str) -> String {
    [&ctx.replace('.', "_"), "__wire"].concat()
}

/// `import * as <ns> from "../workers/<ctx>/handlers.js"`.
pub(super) fn wire_import(ctx: &str, ext: &str) -> TsStmt {
    let module = ["workers/", &bynk_project::worker_dir_name(ctx), "/handlers"].concat();
    TsStmt::decl(
        TsDecl::ImportNamespace {
            type_only: false,
            alias: wire_ns(ctx),
            from: parent_specifier(&module, ext),
        },
        None,
    )
}

/// `[serialise, deserialise]` for a type: the codecs a real call's wire applies, a base type's
/// included (a non-finite `Float` and an out-of-range `Int` are refused there).
fn codec_pair(t: &TypeRef, ns: &str, ru: &RuntimeUse) -> TsExpr {
    TsExpr::Array {
        items: vec![serialise_ref_via(t, ns, ru), deserialise_ref_via(t, ns, ru)],
        multiline: false,
    }
}

/// `__bynkWireStub(<double>, { <service>: { args: [...], ret: [...] }, ... })`.
pub(super) fn wire_wrap(rp: &ResolvedStub, ctx: &str, double: TsExpr, ru: &RuntimeUse) -> TsExpr {
    let ns = [&wire_ns(ctx), "."].concat();
    let ops: Vec<(String, TsExpr)> = rp
        .cap_decl
        .ops
        .iter()
        .map(|op| {
            let args = op
                .params
                .iter()
                .map(|p| codec_pair(&p.type_ref, &ns, ru))
                .collect();
            (
                op.name.name.clone(),
                TsExpr::object(vec![
                    (
                        "args".to_string(),
                        TsExpr::Array {
                            items: args,
                            multiline: false,
                        },
                    ),
                    ("ret".to_string(), codec_pair(&op.return_type, &ns, ru)),
                ]),
            )
        })
        .collect();
    call(ident("__bynkWireStub"), vec![double, TsExpr::object(ops)])
}

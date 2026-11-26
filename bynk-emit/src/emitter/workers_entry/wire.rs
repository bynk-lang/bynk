//! The Worker entry's codec glue to `serialisation`: the wire identifier of a type, and the
//! encode and decode calls the dispatch families share.

use crate::emitter::{RuntimeUse, ts_ident};
use bynk_syntax::ast::TypeRef;
use bynk_ts::TsExpr;

/// #2648: the TypeScript binder for a user parameter in a Worker wrapper or
/// entry dispatch.
///
/// On top of [`ts_ident`]'s reserved-word renames, a parameter whose name is one
/// the wrapper itself declares or reads after binding the parameters (`request`,
/// `surface`, `handlers`, `result`, `args`, ...) goes into the generated-name
/// namespace (`__id_<name>`), which the parser keeps free of user identifiers.
/// Otherwise such a parameter redeclares or shadows the wrapper's own local and
/// the Worker fails `tsc`.
///
/// `body` is *not* in this set: on an `http` route a parameter named `body` is
/// the request body the wrapper binds under that very name. The `call` dispatch,
/// where it is an ordinary parameter, renames it through [`call_wire_ident`].
pub(crate) fn wire_ident(name: &str) -> String {
    // Deliberately absent, because no user parameter is bound in their scope:
    // - `event` / `batch`: the scheduled / queue entries' parameters; those
    //   handlers are fed `event.scheduledTime` / the message directly.
    // - `servicePath`: read only by the `switch`, before any binding.
    // - `why`: the parameter of the `on rejected` wrapper, which has no user
    //   parameters.
    // `url` / `path` are listed preventively: today's dispatch reads them only in
    // the route conditions, before any parameter is bound, but a later read in
    // the route block would otherwise hit the user's string.
    const WRAPPER_NAMES: &[&str] = &[
        // The fetch/scheduled/queue entry's own parameters and locals.
        "request",
        "env",
        "ctx",
        "method",
        "surface",
        "url",
        "path",
        // `deps` is not listed: `ts_ident` already renames it.
        // The dispatch's decoded-arguments and result locals.
        "args",
        "argsObj",
        "result",
        // The compose-surface namespace and runtime constructors the wrappers
        // and the deserialising IIFEs reference by name.
        "handlers",
        "HttpResult",
        "Result",
        "Option",
        "Ok",
        "Err",
        "Some",
        "None",
    ];
    let t = ts_ident(name);
    if WRAPPER_NAMES.contains(&t.as_str()) {
        format!("__id_{t}")
    } else {
        t
    }
}

/// Strip the `Effect[HttpResult[_]]` wrapper to expose the inner payload type
/// `T`. Used to choose the right serialiser when emitting the HttpResult
/// body.
pub(super) fn http_result_inner(t: &TypeRef) -> TypeRef {
    let inner = if let TypeRef::Effect(t, _) = t {
        t.as_ref()
    } else {
        t
    };
    if let TypeRef::HttpResult(payload, _, _) = inner {
        (**payload).clone()
    } else {
        inner.clone()
    }
}

/// v0.176 (#642): the callee side of the workers cross-context boundary. Both
/// of these used to be a *parallel* dispatch that shadowed
/// `serialisation.rs`'s — and drifted from it. `serialise_call` cast a `Bytes`
/// to `JsonValue` while `deserialise_call` base64-decoded it, the asymmetry that
/// made `bynk.types.bytes_at_workers_boundary` necessary. They are now one line
/// each over the shared dispatch, reaching this context's helpers through the
/// `handlers` namespace the entry point imports.
pub(crate) fn deserialise_call(t: &TypeRef, json: &TsExpr, path: &str, ru: &RuntimeUse) -> TsExpr {
    crate::emitter::serialisation::deserialise_expr_via(t, json, path, "handlers.", ru)
}

/// [`deserialise_call`]'s encoding twin: `t`'s value, read from `value`, as JSON.
pub(super) fn serialise_call(t: &TypeRef, value: &TsExpr, ru: &RuntimeUse) -> TsExpr {
    crate::emitter::serialisation::serialise_expr_via(t, value, "handlers.", ru)
}

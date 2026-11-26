//! #3752 (pass-through HTTP, slice 3; D7–D11): the `on ANY(…)` pass-through
//! route. Its rules are checked here, beside the ordinary route's in
//! [`crate::context_checks`].

use crate::stability::UnitStability;
use crate::symbols::UnitTable;
use bynk_syntax::CompileError;
use bynk_syntax::ast::{HandlerKind, HttpMethod};

/// The `[unstable] allow` entry an `ANY` route needs while it is experimental.
pub const ANY_ITEM: &str = "bynk.http.ANY";

/// The pass-through checks, for every `on ANY(…)` handler of the unit's services.
pub(crate) fn check_pass_through_routes(
    table: &UnitTable,
    stability: &UnitStability,
    errors: &mut Vec<CompileError>,
) {
    for service in table.services.values() {
        for handler in &service.handlers {
            let HandlerKind::Http {
                method: HttpMethod::Any,
                ..
            } = &handler.kind
            else {
                continue;
            };
            if let Some(err) = stability.policy.check(ANY_ITEM, handler.span) {
                errors.push(err);
            }
        }
    }
}

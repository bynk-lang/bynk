//! #3752 (pass-through HTTP, slice 3a): an `on ANY(…)` route type-checks, but its
//! router suspensions and the `Upstream` exchange arrive in slice 3b. Until then a
//! build that would emit one is refused, never built without its routes: a Worker
//! deployed without them would answer `404` where the program says it forwards.

use bynk_project::{AttributedError, ParsedFile};
use bynk_syntax::ast::{CommonsItem, HandlerKind, HttpMethod};

/// One error per `on ANY(…)` route in `parsed`, attributed to its file.
pub(super) fn not_emitted_errors(parsed: &[ParsedFile]) -> Vec<AttributedError> {
    let mut out = Vec::new();
    for file in parsed {
        for item in file.items() {
            let CommonsItem::Service(service) = item else {
                continue;
            };
            for h in &service.handlers {
                if let HandlerKind::Http {
                    method: HttpMethod::Any,
                    path,
                } = &h.kind
                {
                    let error = bynk_check::http_pass_through::not_emitted_error(path, h.span);
                    out.push(AttributedError::new(Some(file.identity_path()), error));
                }
            }
        }
    }
    out
}

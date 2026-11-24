//! A queue consumer's `on message` parameters (#3880).
//!
//! The message is every parameter that is not the delivery metadata. The
//! delivery parameter is identified by its type, as a cron handler's is
//! ([`crate::cron::cron_param`]): exactly the prelude's `QueueDelivery`.

use crate::ast::TypeRef;

/// Whether `ty` is exactly the prelude's `QueueDelivery`.
pub fn is_queue_delivery(ty: &TypeRef) -> bool {
    matches!(ty, TypeRef::Named(id) if id.name == "QueueDelivery")
}

/// The positions of the delivery parameters among `types`, in order.
pub fn delivery_positions<'a>(types: impl IntoIterator<Item = &'a TypeRef>) -> Vec<usize> {
    types
        .into_iter()
        .enumerate()
        .filter(|(_, t)| is_queue_delivery(t))
        .map(|(i, _)| i)
        .collect()
}

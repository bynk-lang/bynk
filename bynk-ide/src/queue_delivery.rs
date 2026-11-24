//! The editor surfaces of a queue consumer's delivery metadata (#3880).

use std::collections::HashMap;
use std::path::PathBuf;

use bynk_syntax::ast::{CommonsItem, HandlerKind, ServiceProtocol, SourceUnit};

use crate::completion::{for_each_unit, innermost_open_paren, word_before_brace};
use crate::symbols::type_ref_str;

/// The signature label of a queue service's test address, `svc.message(…)`: the
/// message, and, when the handler declares a `QueueDelivery`, the optional
/// delivery that follows it. `None` when `service` is not a queue consumer.
pub fn message_address_label(
    service: &str,
    doc_text: &str,
    files: Option<&HashMap<PathBuf, String>>,
) -> Option<String> {
    let mut label = None;
    for_each_unit(doc_text, files, |unit| {
        let items = match unit {
            SourceUnit::Context(c) => &c.items,
            SourceUnit::Commons(c) => &c.items,
            _ => return,
        };
        for item in items {
            let CommonsItem::Service(s) = item else {
                continue;
            };
            if s.name.name != service || !matches!(s.protocol, ServiceProtocol::Queue { .. }) {
                continue;
            }
            let Some(h) = s
                .handlers
                .iter()
                .find(|h| matches!(h.kind, HandlerKind::Message))
            else {
                continue;
            };
            let message = h
                .params
                .iter()
                .find(|p| !bynk_syntax::queue::is_queue_delivery(&p.type_ref));
            let has_delivery = h
                .params
                .iter()
                .any(|p| bynk_syntax::queue::is_queue_delivery(&p.type_ref));
            if let Some(m) = message {
                let mut text = format!(
                    "{service}.message({}: {}",
                    m.name.name,
                    type_ref_str(&m.type_ref)
                );
                if has_delivery {
                    text.push_str(", delivery: QueueDelivery?");
                }
                text.push(')');
                label = Some(text);
            }
        }
    });
    label
}

/// The cursor is at a parameter-*name* position in the parameter list of an
/// `on message( … )` handler header: the innermost open paren follows `on message`,
/// and the current parameter has no `:` yet.
pub fn in_message_param_name_position(text: &str, offset: usize) -> bool {
    let Some(open) = innermost_open_paren(text, offset) else {
        return false;
    };
    let head = text[..open].trim_end();
    if word_before_brace(head, head.len()) != "message" {
        return false;
    }
    let before = head[..head.len() - "message".len()].trim_end();
    if !before.ends_with("on") {
        return false;
    }
    let Some(seg) = text.get(open + 1..offset.min(text.len())) else {
        return false;
    };
    !seg.rsplit(['\n', ',']).next().unwrap_or("").contains(':')
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "context c\n\nconsumes bynk.cloudflare { QueueDelivery }\n\ntype Job = { id: String }\n\nservice w from queue(\"q\") {\n  on message(job: Job, delivery: QueueDelivery) -> Effect[QueueResult] { Ack }\n}\n\nservice p from queue(\"p\") {\n  on message(job: Job) -> Effect[QueueResult] { Ack }\n}\n";

    #[test]
    fn the_message_address_shows_the_optional_delivery_only_when_the_handler_takes_it() {
        assert_eq!(
            message_address_label("w", SRC, None).as_deref(),
            Some("w.message(job: Job, delivery: QueueDelivery?)")
        );
        assert_eq!(
            message_address_label("p", SRC, None).as_deref(),
            Some("p.message(job: Job)")
        );
        assert_eq!(message_address_label("nope", SRC, None), None);
    }

    #[test]
    fn a_message_handlers_parameter_name_position_is_found() {
        let t = "service w from queue(\"q\") {\n  on message(";
        assert!(in_message_param_name_position(t, t.len()));
        let t2 = "service w from queue(\"q\") {\n  on message(job: Job, ";
        assert!(in_message_param_name_position(t2, t2.len()));
        let t3 = "service w from queue(\"q\") {\n  on message(job: ";
        assert!(!in_message_param_name_position(t3, t3.len()));
    }
}

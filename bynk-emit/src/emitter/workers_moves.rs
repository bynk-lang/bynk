//! #3565: the phase variants of a supported agent move
//! (`design/tracks/agent-context-moves.md` §3.2).
//!
//! A context that hosts agents moves to another Worker script in three pushes. The first and
//! last push the receiving Worker, the second the source; phase 3 is the receiver's ordinary
//! build. This module writes the other two beside the ordinary files of the Worker they are
//! for, so `bynk deploy` can push them by config path:
//!
//! - **receive** (`wrangler.receive.toml`, `index.receive.ts`): the receiver with each moving
//!   context *forwarded*. Every request no remaining member claims goes, unchanged, to the
//!   context on the script it is moving from, so phase 1 opens no error window. The moving
//!   classes are exported in code and declared `expecting-transfer` with `transfer_from`, and
//!   bound to nothing (Cloudflare refuses a binding to a class still expecting a transfer). The
//!   moving context's crons, queue consumers and email addresses are left out: one queue has
//!   one consumer and an address one Worker, so they stay on the source until it drops them.
//! - **commit** (`wrangler.commit.toml`): the source's own build with a `transferred` tombstone
//!   (`transferred_to`) for each class it gives up, which reassigns the namespace. A source the
//!   build no longer has gets a stub Worker (`index.commit.ts`, which answers `503`) carrying
//!   only the tombstones. The class is not exported in code: the transfer commits without it
//!   (#3562's V0, and live check S4 found both forms commit). That is a choice, not the
//!   documented form (track doc §3.2, D14); if Cloudflare comes to refuse it, the phase 2 push
//!   is refused before anything commits, and an export-only member would be added here.
//!
//! Script names are the unqualified ones the ordinary `wrangler.toml` uses; `bynk deploy`
//! environment-qualifies them as it does every other name. With no move, nothing here runs and
//! a build is byte for byte what it was.

use super::toml_doc::{TomlBlock, TomlDocument, TomlEntry, TomlValue};
use super::wrangler::COMPATIBILITY_DATE;
use crate::ts_build::{ident, method_call, new, num_lit, ret, str_lit};
use bynk_project::wrangler_keys::WranglerKey;
use bynk_ts::{TsDecl, TsExpr, TsObjectEntry, TsParam, TsProgram, TsStmt, TsType, TsTypeMember};

/// The receiving Worker's config in the receive phase.
pub const RECEIVE_CONFIG: &str = "wrangler.receive.toml";
/// The receiving Worker's entry in the receive phase.
pub const RECEIVE_ENTRY: &str = "index.receive.ts";
/// The source Worker's config in the commit phase.
pub const COMMIT_CONFIG: &str = "wrangler.commit.toml";
/// A dissolved source's stub entry in the commit phase.
pub const STUB_ENTRY: &str = "index.commit.ts";

pub use bynk_project::worker_moves::{Incoming, MoveVariants, Outgoing};

/// The Service Binding a receiving Worker forwards `context`'s requests over:
/// `BYNK_FORWARD_SHOP_ORDERS` for `shop.orders`.
pub fn forward_binding(context: &str) -> String {
    let name: String = context
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    format!("BYNK_FORWARD_{name}")
}

/// Whether `block` binds or plainly declares one of `classes`.
fn names_class(block: &TomlBlock, classes: &[&str]) -> bool {
    classes.iter().any(|class| {
        block.is_array_entry(WranglerKey::DurableObjectBindings, "class_name", class)
            || block.is_keyed(WranglerKey::Exports, class)
    })
}

/// The receive phase's config, from the receiver's ordinary one built without the incoming
/// contexts' crons, queues and addresses: each incoming class unbound and `expecting-transfer`,
/// a forwarding Service Binding per incoming context, and the receive entry as `main`.
pub(crate) fn receive_config(doc: &mut TomlDocument, incoming: &[Incoming]) {
    let classes: Vec<&str> = incoming
        .iter()
        .flat_map(|i| i.classes.iter().map(String::as_str))
        .collect();
    doc.retain_blocks(|block| !names_class(block, &classes));
    let _ = doc.set_main(RECEIVE_ENTRY);
    for i in incoming {
        doc.push_block(TomlBlock::array_table(
            WranglerKey::Services,
            vec![
                TomlEntry::kv_owned(
                    "binding".into(),
                    TomlValue::str(forward_binding(&i.context)),
                ),
                TomlEntry::kv("service", TomlValue::str(i.from_script.clone())),
            ],
        ));
    }
    for i in incoming {
        for class in &i.classes {
            doc.push_block(TomlBlock::keyed_table(
                WranglerKey::Exports,
                class.clone(),
                vec![
                    TomlEntry::kv("type", TomlValue::str("durable-object")),
                    TomlEntry::kv("state", TomlValue::str("expecting-transfer")),
                    TomlEntry::kv("storage", TomlValue::str("sqlite")),
                    TomlEntry::kv("transfer_from", TomlValue::str(i.from_script.clone())),
                ],
            ));
        }
    }
}

/// A `transferred` tombstone for each class given up.
fn push_transferred(doc: &mut TomlDocument, outgoing: &[Outgoing]) {
    for o in outgoing {
        doc.push_block(TomlBlock::keyed_table(
            WranglerKey::Exports,
            o.class.clone(),
            vec![
                TomlEntry::kv("type", TomlValue::str("durable-object")),
                TomlEntry::kv("state", TomlValue::str("transferred")),
                TomlEntry::kv("transferred_to", TomlValue::str(o.to_script.clone())),
            ],
        ));
    }
}

/// The commit phase's config, from the source's ordinary one: each class it gives up unbound
/// (its ordinary build no longer has the context) and tombstoned `transferred`.
pub(crate) fn commit_config(doc: &mut TomlDocument, outgoing: &[Outgoing]) {
    let classes: Vec<&str> = outgoing.iter().map(|o| o.class.as_str()).collect();
    doc.retain_blocks(|block| !names_class(block, &classes));
    push_transferred(doc, outgoing);
}

/// A dissolved source's commit-phase config: the stub entry and the tombstones only.
pub(crate) fn stub_config(script: &str, flags: &[String], outgoing: &[Outgoing]) -> TomlDocument {
    let mut doc = TomlDocument::new(
        "Generated by bynk — do not edit by hand.",
        vec![
            TomlEntry::kv(WranglerKey::Name.header(), TomlValue::str(script)),
            TomlEntry::kv(WranglerKey::Main.header(), TomlValue::str(STUB_ENTRY)),
            TomlEntry::kv(
                WranglerKey::CompatibilityDate.header(),
                TomlValue::str(COMPATIBILITY_DATE),
            ),
            TomlEntry::kv(
                WranglerKey::CompatibilityFlags.header(),
                TomlValue::Array(flags.iter().map(TomlValue::str).collect()),
            ),
        ],
    );
    push_transferred(&mut doc, outgoing);
    doc
}

fn response_ty() -> TsType {
    TsType::named("globalThis.Response")
}

/// `export default { <method> }`.
fn export_default(method: TsObjectEntry) -> TsStmt {
    TsStmt::decl(
        TsDecl::ExportDefault(TsExpr::object_entries(vec![method])),
        None,
    )
}

fn header(program: &mut TsProgram, what: String) {
    program.push(TsStmt::comment(
        "Generated by bynk — do not edit by hand.",
        None,
    ));
    program.push(TsStmt::comment(what, None));
}

/// A context's own Worker in the receive phase: every request forwarded to the script it is
/// moving from, and its classes exported from its ordinary entry without being run.
pub(crate) fn standalone_receive_entry(incoming: &Incoming) -> TsProgram {
    let mut program = TsProgram::new();
    header(
        &mut program,
        format!(
            "Agent move, receive phase: `{}` forwarded to `{}`, the script it is moving from.",
            incoming.context, incoming.from_script
        ),
    );
    if !incoming.classes.is_empty() {
        program.push(TsStmt::decl(
            TsDecl::ReExport {
                names: incoming.classes.clone(),
                from: "./index.js".to_string(),
            },
            None,
        ));
    }
    let fetcher = TsType::Object(vec![TsTypeMember::method(
        "fetch",
        vec![TsParam::new(
            "request",
            Some(TsType::named("globalThis.Request")),
        )],
        TsType::named_with_args("Promise", vec![response_ty()]),
    )]);
    let env_ty =
        TsType::named_with_args("globalThis.Record", vec![TsType::named("string"), fetcher]);
    let forward = method_call(
        TsExpr::Index {
            object: Box::new(ident("env")),
            index: Box::new(str_lit(forward_binding(&incoming.context))),
        },
        "fetch",
        vec![ident("request")],
    );
    program.push(export_default(TsObjectEntry::method(
        "fetch",
        false,
        vec![
            TsParam::new("request", Some(TsType::named("globalThis.Request"))),
            TsParam::new("env", Some(env_ty)),
        ],
        Some(TsType::named_with_args("Promise", vec![response_ty()])),
        vec![ret(forward)],
    )));
    program
}

/// A dissolved source's stub in the commit phase: it carries the tombstones, and answers any
/// request that still reaches it (a caller not yet rebound) with `503`.
pub(crate) fn stub_entry(script: &str) -> TsProgram {
    let mut program = TsProgram::new();
    header(
        &mut program,
        format!(
            "Agent move, commit phase: `{script}` gives up its agent classes and serves nothing."
        ),
    );
    let answer = new(
        TsExpr::Member {
            object: Box::new(ident("globalThis")),
            property: "Response".to_string(),
        },
        vec![
            str_lit("Moved"),
            TsExpr::object(vec![("status".to_string(), num_lit(503))]),
        ],
    );
    program.push(export_default(TsObjectEntry::method(
        "fetch",
        false,
        Vec::new(),
        Some(response_ty()),
        vec![ret(answer)],
    )));
    program
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emitter::toml_doc::print_toml_document;

    fn printed(program: &TsProgram) -> String {
        bynk_ts::print(program, "x.bynk", "", "index.ts").text
    }

    fn ordinary() -> TomlDocument {
        let mut doc = TomlDocument::new(
            "Generated by bynk — do not edit by hand.",
            vec![
                TomlEntry::kv(WranglerKey::Name.header(), TomlValue::str("core")),
                TomlEntry::kv(WranglerKey::Main.header(), TomlValue::str("index.ts")),
            ],
        );
        for (binding, class) in [("CART", "Cart"), ("PAYMENT", "Payment")] {
            doc.push_block(TomlBlock::array_table(
                WranglerKey::DurableObjectBindings,
                vec![
                    TomlEntry::kv("name", TomlValue::str(binding)),
                    TomlEntry::kv("class_name", TomlValue::str(class)),
                ],
            ));
        }
        for class in ["Cart", "Payment"] {
            doc.push_block(TomlBlock::keyed_table(
                WranglerKey::Exports,
                class,
                vec![
                    TomlEntry::kv("type", TomlValue::str("durable-object")),
                    TomlEntry::kv("storage", TomlValue::str("sqlite")),
                ],
            ));
        }
        doc
    }

    fn incoming() -> Incoming {
        Incoming {
            context: "shop.orders".into(),
            classes: vec!["Cart".into()],
            from_script: "shop-orders".into(),
        }
    }

    #[test]
    fn a_forward_binding_is_named_for_its_context() {
        assert_eq!(forward_binding("shop.orders"), "BYNK_FORWARD_SHOP_ORDERS");
        assert_eq!(forward_binding("a-b.c"), "BYNK_FORWARD_A_B_C");
    }

    /// The incoming class loses its binding and its plain entry, and is declared expecting a
    /// transfer from the old script; the members' own classes are untouched.
    #[test]
    fn the_receive_config_expects_the_class_unbound_and_forwards_the_context() {
        let mut doc = ordinary();
        receive_config(&mut doc, &[incoming()]);
        let text = print_toml_document(&doc);
        assert_eq!(
            text,
            "# Generated by bynk — do not edit by hand.\n\
             name = \"core\"\n\
             main = \"index.receive.ts\"\n\
             \n\
             [[durable_objects.bindings]]\n\
             name = \"PAYMENT\"\n\
             class_name = \"Payment\"\n\
             \n\
             [exports.Payment]\n\
             type = \"durable-object\"\n\
             storage = \"sqlite\"\n\
             \n\
             [[services]]\n\
             binding = \"BYNK_FORWARD_SHOP_ORDERS\"\n\
             service = \"shop-orders\"\n\
             \n\
             [exports.Cart]\n\
             type = \"durable-object\"\n\
             state = \"expecting-transfer\"\n\
             storage = \"sqlite\"\n\
             transfer_from = \"shop-orders\"\n\
             \n"
        );
    }

    #[test]
    fn the_commit_config_tombstones_each_class_given_up() {
        let mut doc = ordinary();
        commit_config(
            &mut doc,
            &[Outgoing {
                class: "Cart".into(),
                to_script: "core".into(),
            }],
        );
        let text = print_toml_document(&doc);
        assert!(!text.contains("class_name = \"Cart\""), "{text}");
        assert!(text.contains("main = \"index.ts\""), "{text}");
        assert!(
            text.ends_with(
                "[exports.Cart]\ntype = \"durable-object\"\nstate = \"transferred\"\ntransferred_to = \"core\"\n\n"
            ),
            "{text}"
        );
        assert!(text.contains("[exports.Payment]"), "{text}");
    }

    /// Fan-in: two contexts from two scripts into one receiver, each with its own forward and
    /// its own `transfer_from`.
    #[test]
    fn a_receiver_of_two_contexts_forwards_and_expects_each_from_its_own_script() {
        let mut doc = ordinary();
        let second = Incoming {
            context: "shop.billing".into(),
            classes: vec!["Payment".into()],
            from_script: "shop-billing".into(),
        };
        receive_config(&mut doc, &[incoming(), second]);
        let text = print_toml_document(&doc);
        assert!(!text.contains("[[durable_objects.bindings]]"), "{text}");
        for (binding, script) in [
            ("BYNK_FORWARD_SHOP_ORDERS", "shop-orders"),
            ("BYNK_FORWARD_SHOP_BILLING", "shop-billing"),
        ] {
            assert!(
                text.contains(&format!(
                    "binding = \"{binding}\"\nservice = \"{script}\"\n"
                )),
                "{text}"
            );
        }
        for (class, script) in [("Cart", "shop-orders"), ("Payment", "shop-billing")] {
            assert!(
                text.contains(&format!(
                    "[exports.{class}]\ntype = \"durable-object\"\nstate = \"expecting-transfer\"\nstorage = \"sqlite\"\ntransfer_from = \"{script}\"\n"
                )),
                "{text}"
            );
        }
        assert_eq!(text.matches("[exports.").count(), 2, "{text}");
    }

    /// Fan-out: one source giving up two classes to two receivers, each tombstone naming its
    /// own receiver.
    #[test]
    fn a_source_giving_up_two_classes_tombstones_each_to_its_receiver() {
        let mut doc = ordinary();
        commit_config(
            &mut doc,
            &[
                Outgoing {
                    class: "Cart".into(),
                    to_script: "shop-orders".into(),
                },
                Outgoing {
                    class: "Payment".into(),
                    to_script: "shop-billing".into(),
                },
            ],
        );
        let text = print_toml_document(&doc);
        assert!(!text.contains("[[durable_objects.bindings]]"), "{text}");
        assert!(!text.contains("storage = \"sqlite\""), "{text}");
        for (class, script) in [("Cart", "shop-orders"), ("Payment", "shop-billing")] {
            assert!(
                text.contains(&format!(
                    "[exports.{class}]\ntype = \"durable-object\"\nstate = \"transferred\"\ntransferred_to = \"{script}\"\n"
                )),
                "{text}"
            );
        }
    }

    #[test]
    fn a_dissolved_source_is_a_stub_with_only_its_tombstones() {
        let doc = stub_config(
            "shop-orders",
            &["nodejs_compat".to_string()],
            &[Outgoing {
                class: "Cart".into(),
                to_script: "core".into(),
            }],
        );
        let text = print_toml_document(&doc);
        assert!(
            text.starts_with(
                "# Generated by bynk — do not edit by hand.\nname = \"shop-orders\"\nmain = \"index.commit.ts\"\n"
            ),
            "{text}"
        );
        assert!(text.contains("state = \"transferred\""), "{text}");
        assert!(!text.contains("durable_objects"), "{text}");
        let entry = printed(&stub_entry("shop-orders"));
        assert!(entry.contains("status: 503"), "{entry}");
    }

    #[test]
    fn a_standalone_receiver_forwards_everything_and_exports_the_classes() {
        let entry = printed(&standalone_receive_entry(&incoming()));
        assert!(
            entry.contains("export { Cart } from \"./index.js\";"),
            "{entry}"
        );
        assert!(
            entry.contains("return env[\"BYNK_FORWARD_SHOP_ORDERS\"].fetch(request);"),
            "{entry}"
        );
    }
}

//! #4393: hover on a refinement of the prelude `Sender` or `Producer` actor says what it admits.
//!
//! `actor Authenticated = Sender where Dmarc(DmarcPass)` is a declaration whose predicate is not
//! just a type: it decides which mail reaches a handler, and the Worker enforces it before the
//! handler runs. Hover rendered only the declaration line, so the verdicts it admits, and that the
//! rest is refused, were readable only from the predicate and the language reference. This adds the
//! note after the declaration, on the declaration itself and wherever hover resolves to it (a
//! handler's `by s: Authenticated`), because both reach [`describe_actor_in`].
//!
//! Every claim is worded against what the emitter and the runtime do, not the issue text:
//! - `Sender`: `bynk-emit` routes the verdicts `SenderPredicate::admitted` returns to the email
//!   runtime (`workers_entry/email.rs`), which rejects other mail before `route.handle` runs
//!   (`runtime/src/email.ts`, `setReject`), logged by outcome only.
//! - `Producer`: a `from queue` consumer whose `by` names only signed actors admits only signed
//!   messages (`workers_entry/queue.rs`, `runtime/src/queue_admit.ts`); a refused message is
//!   acknowledged and reported and never reaches the handler.
//!
//! A user-declared actor of the same name shadows the prelude one, and then the predicate means
//! whatever that actor makes of it, so the note would be a false security statement. The check is
//! syntactic and cheap: no note when the unit declares an actor named `Sender` / `Producer`.

use bynk_check::actors::{
    CallerPredicate, DMARC_VERDICTS, parse_caller_predicate, parse_sender_predicate,
};
use bynk_syntax::ast::{ActorDecl, CommonsItem};

/// `a` as hover renders an actor, plus what a `Sender` or `Producer` refinement admits.
/// `items` are the declaring unit's items, to see whether a local actor shadows the prelude base.
pub(crate) fn describe_actor_in(a: &ActorDecl, items: &[CommonsItem]) -> String {
    let mut out = crate::symbols::describe_actor(a);
    if let Some(note) = refinement_note(a, items) {
        out.push('\n');
        out.push_str(&note);
        out.push('\n');
    }
    out
}

/// The note for a refinement of the prelude `Sender` or `Producer`; `None` for any other actor,
/// for a base the unit shadows, and for a predicate the checker rejects (it reports that itself).
fn refinement_note(a: &ActorDecl, items: &[CommonsItem]) -> Option<String> {
    let r = a.refinement.as_ref()?;
    let base = r.base.name.as_str();
    if !matches!(base, "Sender" | "Producer") || declares_actor(items, base) {
        return None;
    }
    match base {
        "Sender" => sender_note(&r.predicate),
        _ => producer_note(&r.predicate),
    }
}

fn declares_actor(items: &[CommonsItem], name: &str) -> bool {
    items
        .iter()
        .any(|i| matches!(i, CommonsItem::Actor(x) if x.name.name == name))
}

/// `` `A` ``, `` `A` or `B` ``, `` `A`, `B` or `C` ``.
fn list(names: &[String]) -> String {
    let quoted: Vec<String> = names.iter().map(|n| format!("`{n}`")).collect();
    match quoted.split_last() {
        None => String::new(),
        Some((only, [])) => only.clone(),
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
    }
}

fn sender_note(predicate: &bynk_syntax::ast::Expr) -> Option<String> {
    let admitted = parse_sender_predicate(predicate).ok()?.admitted();
    let head = "**Refines `Sender`**, the sender of inbound mail with its DMARC verdict. On a \
                `from email` service that names it in `by`, ";
    let tail = match admitted.len() {
        0 => "it admits no mail: the Worker refuses every message before the handler runs."
            .to_string(),
        n if n == DMARC_VERDICTS.len() => {
            "it admits mail with any DMARC verdict, so the Worker refuses none.".to_string()
        }
        _ => {
            let verdicts: Vec<String> = admitted.iter().map(|v| v.to_string()).collect();
            format!(
                "it admits mail whose DMARC verdict is {}. The Worker refuses any other mail \
                 before the handler runs (rejected as unauthenticated), so the handler never sees it.",
                list(&verdicts)
            )
        }
    };
    Some(format!("{head}{tail}"))
}

fn producer_note(predicate: &bynk_syntax::ast::Expr) -> Option<String> {
    let parsed = parse_caller_predicate(predicate).ok()?;
    let who = match named_contexts(&parsed) {
        Some(names) => format!("by {}", list(&names)),
        // `Not` and `And` do not name a finite set, so say the predicate rather than guess one.
        None => format!(
            "by a context for which `{}` holds",
            bynk_fmt::expr_to_string(predicate)
        ),
    };
    Some(format!(
        "**Refines `Producer`**, the sender of a queue message. On a `from queue` consumer that \
         names it in `by`, the consumer requires signatures and admits only messages signed {who}. \
         A message that is unsigned, signed by another context, or whose signature does not \
         verify is acknowledged and reported, and never reaches the handler."
    ))
}

/// The contexts an `IsContext(…)` predicate, or an `or` of them, names; `None` when the
/// predicate also negates or intersects, where no finite list is the whole story.
fn named_contexts(p: &CallerPredicate) -> Option<Vec<String>> {
    match p {
        CallerPredicate::IsContext(names) => Some(names.clone()),
        CallerPredicate::Or(l, r) => {
            let mut names = named_contexts(l)?;
            names.extend(named_contexts(r)?);
            Some(names)
        }
        CallerPredicate::And(..) | CallerPredicate::Not(..) => None,
    }
}

#[cfg(test)]
mod tests {
    use crate::symbols::describe_symbol;

    fn mail(actor: &str) -> String {
        format!("context demo.mail\n\n{actor}\n")
    }

    #[test]
    fn a_sender_refinement_names_the_verdicts_it_admits_and_the_refusal() {
        let hover = describe_symbol(
            &mail("actor Authenticated = Sender where Dmarc(DmarcPass)"),
            "Authenticated",
        )
        .expect("hover on the actor");
        assert!(
            hover.contains("actor Authenticated = Sender where Dmarc(DmarcPass)"),
            "{hover}"
        );
        assert!(hover.contains("Refines `Sender`"), "{hover}");
        assert!(
            hover.contains("it admits mail whose DMARC verdict is `DmarcPass`"),
            "{hover}"
        );
        assert!(
            hover.contains("refuses any other mail before the handler runs"),
            "{hover}"
        );
    }

    /// The admitted set is evaluated, not read off the text: `Not` and `Or` are the checker's own.
    #[test]
    fn a_sender_predicate_is_evaluated_not_transcribed() {
        let not_fail =
            describe_symbol(&mail("actor A = Sender where !Dmarc(DmarcFail)"), "A").unwrap();
        assert!(
            not_fail.contains("is `DmarcPass` or `DmarcNone`"),
            "{not_fail}"
        );
        let either = describe_symbol(
            &mail("actor A = Sender where Dmarc(DmarcPass) || Dmarc(DmarcNone)"),
            "A",
        )
        .unwrap();
        assert!(either.contains("is `DmarcPass` or `DmarcNone`"), "{either}");
        let nothing = describe_symbol(
            &mail("actor A = Sender where Dmarc(DmarcPass) && Dmarc(DmarcFail)"),
            "A",
        )
        .unwrap();
        assert!(nothing.contains("it admits no mail"), "{nothing}");
        let all = describe_symbol(
            &mail("actor A = Sender where Dmarc(DmarcPass, DmarcFail, DmarcNone)"),
            "A",
        )
        .unwrap();
        assert!(all.contains("with any DMARC verdict"), "{all}");
    }

    #[test]
    fn a_producer_refinement_names_the_contexts_and_requires_signatures() {
        let hover = describe_symbol(
            &mail("actor Billing = Producer where IsContext(shop.billing, shop.orders)"),
            "Billing",
        )
        .expect("hover on the actor");
        assert!(hover.contains("Refines `Producer`"), "{hover}");
        assert!(
            hover.contains("requires signatures and admits only messages signed by `shop.billing` or `shop.orders`"),
            "{hover}"
        );
        assert!(hover.contains("never reaches the handler"), "{hover}");
        // A negation names no finite set: the predicate is shown, not a guessed list.
        let not = describe_symbol(
            &mail("actor Other = Producer where !IsContext(shop.billing)"),
            "Other",
        )
        .unwrap();
        assert!(
            not.contains("by a context for which `!IsContext(shop.billing)` holds"),
            "{not}"
        );
    }

    /// A local actor named `Sender` shadows the prelude one, so the predicate is not the
    /// prelude's and the note would be a false statement about what is refused.
    #[test]
    fn a_shadowed_base_gets_no_note() {
        let src = mail(
            "actor Sender { auth = None }\n\nactor Authenticated = Sender where Dmarc(DmarcPass)",
        );
        let hover = describe_symbol(&src, "Authenticated").unwrap();
        assert!(!hover.contains("Refines"), "{hover}");
        let src = mail(
            "actor Producer { auth = None }\n\nactor P = Producer where IsContext(shop.billing)",
        );
        let hover = describe_symbol(&src, "P").unwrap();
        assert!(!hover.contains("Refines"), "{hover}");
    }

    /// Everything else keeps today's hover exactly: a Bearer refinement, a plain actor, and a
    /// predicate the checker rejects (it reports that; hover does not guess a meaning).
    #[test]
    fn other_actors_and_malformed_predicates_are_unchanged() {
        let bearer = describe_symbol(
            &mail("actor User { auth = Bearer(secret = \"S\") }\n\nactor Admin = User where hasClaim(\"admin\")"),
            "Admin",
        )
        .unwrap();
        assert!(!bearer.contains("Refines"), "{bearer}");
        let plain = describe_symbol(&mail("actor Public { auth = None }"), "Public").unwrap();
        assert!(!plain.contains("Refines"), "{plain}");
        let malformed =
            describe_symbol(&mail("actor A = Sender where Dmarc(DmarcMaybe)"), "A").unwrap();
        assert!(!malformed.contains("Refines"), "{malformed}");
    }
}

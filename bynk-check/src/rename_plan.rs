//! #3793: how a field's `replaces old` clause reads against the schema lock.
//!
//! The lock records an event's fields ([`FieldShape`]) and an agent's stored fields
//! ([`StoreShape`]) by name. A field renamed in source is a different name to the lock, so a
//! reconcile that compared by name alone would see a removal plus an addition. [`plan`] pairs
//! each live field that declares `replaces old` with the recorded field of that name, [`apply`]
//! renames the recorded shape to match, and the ordinary additive/non-additive comparison then
//! runs on what is left. A rename that changes nothing else is additive, because the new build
//! reads the old key.
//!
//! `renamed_from` is the history: the old name, then the names the old field was itself renamed
//! from, so a record stored two renames ago is still read. It is kept on the recorded shape even
//! after the clause leaves the source ([`inherit`]), and the clause is judged against it
//! ([`crate::schema_registry`]'s `replaces_needed` and `replaces_settled`).

use bynk_project::schema_registry::{AgentEntry, EventEntry, FieldShape, StoreShape};
use bynk_syntax::ast::{AgentDecl, EventDecl};
use bynk_syntax::error::CompileError;

/// A recorded field the plan can rename.
pub(crate) trait Renamable: Clone {
    fn name(&self) -> &str;
    fn renamed_from(&self) -> &[String];
    fn set_renamed_from(&mut self, names: Vec<String>);
    /// Rename this recorded field to `new`, rewriting anything inside it that carries the name.
    fn rename_to(&mut self, new: &str);
}

impl Renamable for FieldShape {
    fn name(&self) -> &str {
        &self.name
    }
    fn renamed_from(&self) -> &[String] {
        &self.renamed_from
    }
    fn set_renamed_from(&mut self, names: Vec<String>) {
        self.renamed_from = names;
    }
    fn rename_to(&mut self, new: &str) {
        let old = std::mem::replace(&mut self.name, new.to_string());
        // #4523: the paths inside a record field are rooted at the field's name.
        for p in &mut self.paths {
            if let Some(rest) = p.path.strip_prefix(&old)
                && rest.chars().next().is_none_or(|c| !is_name_char(c))
            {
                p.path = format!("{new}{rest}");
            }
        }
    }
}

impl Renamable for StoreShape {
    fn name(&self) -> &str {
        &self.name
    }
    fn renamed_from(&self) -> &[String] {
        &self.renamed_from
    }
    fn set_renamed_from(&mut self, names: Vec<String>) {
        self.renamed_from = names;
    }
    fn rename_to(&mut self, new: &str) {
        let old = std::mem::replace(&mut self.name, new.to_string());
        // A stored value's paths are rooted at the field's name (`field`, `field.x`,
        // `field|Variant`, `field[]`, `field?`, `field{}`).
        for p in &mut self.paths {
            if let Some(rest) = p.path.strip_prefix(&old)
                && rest.chars().next().is_none_or(|c| !is_name_char(c))
            {
                p.path = format!("{new}{rest}");
            }
        }
    }
}

fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// What the live fields' clauses ask of the recorded ones.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct RenamePlan {
    /// `(old, new)`: the recorded field `old` becomes `new` in this build.
    pub applied: Vec<(String, String)>,
    /// `(new, old)`: the lock records no field `old` for this to have been renamed from.
    pub unknown: Vec<(String, String)>,
    /// `(new, old)`: the lock records both names as separate fields.
    pub onto_recorded: Vec<(String, String)>,
}

/// Pair each live field's `replaces` with the recorded field it renames. A rename an earlier
/// build already applied (the recorded `new` lists `old` in its history) needs nothing.
pub(crate) fn plan<T: Renamable>(current: &[T], stored: &[T]) -> RenamePlan {
    let mut plan = RenamePlan::default();
    for c in current {
        let Some(old) = c.renamed_from().first() else {
            continue;
        };
        let recorded_new = stored.iter().find(|s| s.name() == c.name());
        let recorded_old = stored.iter().any(|s| s.name() == old);
        match (recorded_new, recorded_old) {
            (Some(n), _) if n.renamed_from().contains(old) => {}
            (Some(_), true) => plan.onto_recorded.push((c.name().to_string(), old.clone())),
            (None, true) => plan.applied.push((old.clone(), c.name().to_string())),
            (_, false) => plan.unknown.push((c.name().to_string(), old.clone())),
        }
    }
    plan
}

/// The recorded fields with each planned rename made, and the old name put in front of the
/// history the renamed field already had.
pub(crate) fn apply<T: Renamable>(stored: &[T], plan: &RenamePlan) -> Vec<T> {
    let mut out = stored.to_vec();
    for (old, new) in &plan.applied {
        if let Some(f) = out.iter_mut().find(|f| f.name() == old) {
            let mut history = vec![old.clone()];
            history.extend(f.renamed_from().iter().cloned());
            f.rename_to(new);
            f.set_renamed_from(history);
        }
    }
    out
}

/// Bring the live fields' history in line with the recorded one: a field renamed by this build
/// takes the old field's whole history, and a field with no clause keeps the history it was
/// recorded with, so deleting a clause never forgets a rename.
pub(crate) fn inherit<T: Renamable>(current: &mut [T], stored: &[T], plan: &RenamePlan) {
    for c in current.iter_mut() {
        if let Some((old, _)) = plan.applied.iter().find(|(_, n)| n == c.name()) {
            let mut history = vec![old.clone()];
            if let Some(s) = stored.iter().find(|s| s.name() == old) {
                history.extend(s.renamed_from().iter().cloned());
            }
            c.set_renamed_from(history);
        } else if let Some(s) = stored.iter().find(|s| s.name() == c.name())
            && (c.renamed_from().is_empty() || c.renamed_from().first() == s.renamed_from().first())
        {
            // No clause (the history stays), or a rename an earlier build applied (the recorded
            // history is the longer one).
            c.set_renamed_from(s.renamed_from().to_vec());
        }
    }
}

/// An event's recorded entry with the live fields' renames applied, and the live fields' history
/// brought in line with it. The plan comes back for [`event_errors`].
pub(crate) fn event_view(
    current: &mut [FieldShape],
    entry: &EventEntry,
) -> (EventEntry, RenamePlan) {
    let plan = plan(current, &entry.fields);
    inherit(current, &entry.fields, &plan);
    let view = EventEntry {
        schema: entry.schema,
        fields: apply(&entry.fields, &plan),
    };
    (view, plan)
}

/// An agent's recorded stored fields with the live fields' renames applied, and the live fields'
/// history brought in line with them.
pub(crate) fn store_view(
    current: &mut [StoreShape],
    stored: &[StoreShape],
) -> (Vec<StoreShape>, RenamePlan) {
    let plan = plan(current, stored);
    inherit(current, stored, &plan);
    (apply(stored, &plan), plan)
}

fn problems(
    plan: &RenamePlan,
    owner: &str,
    key: &str,
    span_of: impl Fn(&str) -> Option<bynk_syntax::span::Span>,
) -> Vec<CompileError> {
    let at = |new: &str| span_of(new).unwrap_or_default();
    let mut errors = Vec::new();
    for (new, old) in &plan.unknown {
        errors.push(
            CompileError::new(
                "bynk.field.replaces_unknown",
                at(new),
                format!(
                    "field `{new}` replaces `{old}`, but `bynk.schema.lock` records no field \
                     `{old}` on {owner} `{key}`"
                ),
            )
            .with_note(
                "a rename carries a recorded field's data; check the old name, and that it was \
                 built with this project",
            ),
        );
    }
    for (new, old) in &plan.onto_recorded {
        errors.push(
            CompileError::new(
                crate::field_renames::REPLACES_CONFLICT,
                at(new),
                format!(
                    "field `{new}` replaces `{old}`, but `bynk.schema.lock` records both as \
                     separate fields of {owner} `{key}`"
                ),
            )
            .with_note(
                "renaming onto a recorded field would merge two histories; keep both fields, or \
                 drop `replaces`",
            ),
        );
    }
    errors
}

/// The errors a plan raises against an event's recorded shape.
pub(crate) fn event_errors(plan: &RenamePlan, event: &EventDecl, key: &str) -> Vec<CompileError> {
    problems(plan, "event", key, |new| {
        event
            .body
            .fields
            .iter()
            .find(|f| f.wire_name().unwrap_or_else(|| f.name.name.clone()) == new)
            .and_then(|f| f.replaces.as_ref())
            .map(|r| r.span)
    })
}

/// The errors a plan raises against an agent's recorded stored shape.
pub(crate) fn store_errors(plan: &RenamePlan, agent: &AgentDecl, key: &str) -> Vec<CompileError> {
    problems(plan, "agent", key, |new| {
        agent
            .store_fields
            .iter()
            .find(|f| f.name.name == new)
            .and_then(|f| f.replaces.as_ref())
            .map(|r| r.span)
    })
}

/// The result of renaming an agent's recorded stored fields to match its live clauses.
pub(crate) struct StoreRename {
    /// The recorded fields with each planned rename made, to compare the live ones against.
    pub view: Vec<StoreShape>,
    /// Whether the build renames a recorded field, which is a change even if every shape is equal.
    pub renamed: bool,
    /// The live fields whose clause names nothing the lock records, which the clause check skips.
    pub unrecorded: Vec<String>,
}

/// [`store_view`] with its errors reported, for one agent: what the reconcile needs of a class
/// with a recorded history.
pub(crate) fn store_rename(
    current: &mut [StoreShape],
    recorded: &[StoreShape],
    agent: &AgentDecl,
    key: &str,
    errors: &mut Vec<CompileError>,
) -> StoreRename {
    let (view, plan) = store_view(current, recorded);
    errors.extend(store_errors(&plan, agent, key));
    StoreRename {
        view,
        renamed: !plan.applied.is_empty(),
        unrecorded: plan.unknown.into_iter().map(|(new, _)| new).collect(),
    }
}

/// What an agent's renamed `store` fields ask of loading a stored record, and what the lock says
/// about their clauses. `reads` is every `(old, new)` pair a record at a retained version may still
/// need (an older name the lock's retained shapes still record), which the emitter turns into the
/// read fallback. A clause no retained version needs is `bynk.field.replaces_settled`; a field
/// renamed whose clause is gone while a retained version needs it is
/// `bynk.field.replaces_needed`.
pub(crate) fn store_renames(
    agent: &AgentDecl,
    entry: &AgentEntry,
    (previous, current): (&[StoreShape], &[StoreShape]),
    unrecorded: Option<&[String]>,
    errors: &mut Vec<CompileError>,
) -> Vec<(String, String)> {
    let retained = |name: &str| {
        entry
            .shapes
            .iter()
            .any(|s| s.fields.iter().any(|f| f.name == name))
    };
    let mut reads = Vec::new();
    let mut verdicts = Vec::new();
    for c in current
        .iter()
        .filter(|c| !c.renamed_from.is_empty() && !unrecorded.is_some_and(|u| u.contains(&c.name)))
    {
        let needed: Vec<&String> = c.renamed_from.iter().filter(|n| retained(n)).collect();
        let decl = agent.store_fields.iter().find(|f| f.name.name == c.name);
        match (decl.and_then(|f| f.replaces.as_ref()), needed.is_empty()) {
            (None, false) => {
                if let Some(f) = decl {
                    verdicts.push(
                        CompileError::new(
                            "bynk.field.replaces_needed",
                            f.name.span,
                            format!(
                                "`{}.{}` was renamed from `{}`, and a stored version the lock \
                                 still holds keeps its value under that name",
                                agent.name.name,
                                c.name,
                                needed.first().map_or("", |n| n.as_str())
                            ),
                        )
                        .with_note(format!(
                            "keep `replaces {}` until `bynk migrate` has raised the class's \
                             floor past those versions",
                            needed.first().map_or("", |n| n.as_str())
                        )),
                    );
                }
            }
            (Some(clause), true) => verdicts.push(
                CompileError::new(
                    "bynk.field.replaces_settled",
                    clause.span,
                    format!(
                        "no stored version the lock holds still keeps `{}.{}` under `{}`",
                        agent.name.name, c.name, clause.name
                    ),
                )
                .with_note("the clause is no longer read; delete `replaces`"),
            ),
            _ => {}
        }
        reads.extend(needed.into_iter().map(|n| (n.clone(), c.name.clone())));
    }
    // A class the lock has no history for (`unrecorded` is `None`) is a baseline, which no clause is
    // judged against.
    if unrecorded.is_some() {
        errors.extend(verdicts);
        // #4483: and the clauses on record fields inside its stored values.
        nested_renames(agent, entry, previous, current, errors);
    }
    reads
}

/// #4483: what the `replaces` clauses on record fields *inside* an agent's stored values ask of
/// the lock, judged as a top-level `store` rename is. `previous` is the class's recorded fields
/// before this build, `current` the live ones, `entry` the reconciled entry whose retained shapes
/// say which stored versions may still hold an old name. A clause deleted while a retained
/// version keeps the value under the old name is `bynk.field.replaces_needed`. There is no
/// `replaces_settled` for these: a record field's clause also lets its decoders accept the old key
/// from a wire client, which the lock cannot see, so it is never called unneeded.
pub(crate) fn nested_renames(
    agent: &AgentDecl,
    entry: &AgentEntry,
    previous: &[StoreShape],
    current: &[StoreShape],
    errors: &mut Vec<CompileError>,
) {
    let old_path = |path: &str, old: &str| {
        path.rsplit_once('.')
            .map(|(parent, _)| format!("{parent}.{old}"))
    };
    // Whether a retained shape holds `path` of the store field `live`, under any name `live` has
    // had: an older shape records a field renamed by a top-level `replaces` under its old name,
    // and its paths start with that name.
    let retained = |live: &StoreShape, path: &str| {
        let rest = path.strip_prefix(live.name.as_str()).unwrap_or(path);
        std::iter::once(&live.name)
            .chain(&live.renamed_from)
            .any(|name| {
                let at = format!("{name}{rest}");
                entry.shapes.iter().any(|s| {
                    s.fields
                        .iter()
                        .any(|f| &f.name == name && f.paths.iter().any(|p| p.path == at))
                })
            })
    };
    let span_of = |field: &str| {
        agent
            .store_fields
            .iter()
            .find(|f| f.name.name == field)
            .map(|f| (f.name.span, f.replaces.is_some()))
    };
    for prev in previous {
        let Some(live) = current.iter().find(|c| c.name == prev.name) else {
            continue;
        };
        for p in prev.paths.iter().filter(|p| p.renamed_from.is_some()) {
            let old = p.renamed_from.as_deref().unwrap_or_default();
            let clause_gone = live
                .paths
                .iter()
                .any(|l| l.path == p.path && l.renamed_from.is_none());
            let Some(held) = old_path(&p.path, old).filter(|o| retained(live, o)) else {
                continue;
            };
            if !clause_gone {
                continue;
            }
            let at = span_of(&prev.name).map(|(s, _)| s).unwrap_or_default();
            errors.push(
                CompileError::new(
                    "bynk.field.replaces_needed",
                    at,
                    format!(
                        "`{}.{}` was renamed from `{old}`, and a stored version the lock still \
                         holds keeps its value under that name",
                        agent.name.name, p.path
                    ),
                )
                .with_note(format!(
                    "keep `replaces {old}` on the record field until `bynk migrate` has raised the \
                     class's floor past those versions ({held} is what they hold)"
                )),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(name: &str, from: &[&str]) -> FieldShape {
        FieldShape {
            name: name.to_string(),
            ty: "String".to_string(),
            default: false,
            renamed_from: from.iter().map(|s| s.to_string()).collect(),
            paths: Vec::new(),
        }
    }

    #[test]
    fn a_clause_pairs_with_the_recorded_old_name() {
        let plan = plan(&[field("b", &["a"])], &[field("a", &[])]);
        assert_eq!(plan.applied, [("a".to_string(), "b".to_string())]);
        let renamed = apply(&[field("a", &[])], &plan);
        assert_eq!(renamed, [field("b", &["a"])]);
    }

    #[test]
    fn an_applied_rename_needs_nothing() {
        let plan = plan(&[field("b", &["a"])], &[field("b", &["a"])]);
        assert_eq!(plan, RenamePlan::default());
    }

    #[test]
    fn an_unrecorded_old_name_is_unknown() {
        let plan = plan(&[field("b", &["a"])], &[field("c", &[])]);
        assert_eq!(plan.unknown, [("b".to_string(), "a".to_string())]);
    }

    #[test]
    fn both_names_recorded_is_a_conflict() {
        let plan = plan(&[field("b", &["a"])], &[field("a", &[]), field("b", &[])]);
        assert_eq!(plan.onto_recorded, [("b".to_string(), "a".to_string())]);
    }

    #[test]
    fn a_second_rename_keeps_the_first_in_its_history() {
        let stored = [field("b", &["a"])];
        let mut current = [field("c", &["b"])];
        let plan = plan(&current, &stored);
        inherit(&mut current, &stored, &plan);
        assert_eq!(current[0].renamed_from, ["b", "a"]);
        assert_eq!(apply(&stored, &plan), [field("c", &["b", "a"])]);
    }

    #[test]
    fn deleting_a_clause_keeps_the_recorded_history() {
        let stored = [field("b", &["a"])];
        let mut current = [field("b", &[])];
        let plan = plan(&current, &stored);
        inherit(&mut current, &stored, &plan);
        assert_eq!(current[0].renamed_from, ["a"]);
    }

    #[test]
    fn a_stored_value_is_renamed_through_its_paths() {
        let mut shape = StoreShape {
            name: "a".to_string(),
            kind: "Cell".to_string(),
            ty: "{}".to_string(),
            refinement: String::new(),
            paths: vec![
                bynk_project::schema_registry::PathShape {
                    path: "a".to_string(),
                    ty: "{}".to_string(),
                    default: false,
                    refinement: String::new(),
                    renamed_from: None,
                },
                bynk_project::schema_registry::PathShape {
                    path: "a.x".to_string(),
                    ty: "Int".to_string(),
                    default: false,
                    refinement: String::new(),
                    renamed_from: None,
                },
                bynk_project::schema_registry::PathShape {
                    path: "ab.x".to_string(),
                    ty: "Int".to_string(),
                    default: false,
                    refinement: String::new(),
                    renamed_from: None,
                },
            ],
            renamed_from: Vec::new(),
        };
        shape.rename_to("n");
        let paths: Vec<&str> = shape.paths.iter().map(|p| p.path.as_str()).collect();
        assert_eq!(paths, ["n", "n.x", "ab.x"]);
    }
}

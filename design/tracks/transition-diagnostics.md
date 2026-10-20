# Transition diagnostics — report a `transition` a handler would violate, before it runs

- **Status:** Draft, settling. Spine issue
  [#2215](https://github.com/bynk-lang/bynk/issues/2215). This doc lands via
  its settling PR, which references the spine (*"Part of #2215"*, never
  `Closes`). Every question in §3 carries a **draft leaning** for review; none
  is settled yet.
- **Follows:** the agent-state-views track's §3.5 follow-on (retired by
  [#2214](https://github.com/bynk-lang/bynk/pull/2214); summary in
  [`../archive/retired-tracks.md`](../archive/retired-tracks.md)) and
  [ADR 0539](../decisions/0539-transition-overlay-is-a-view-annotation.md),
  which named the likely code `bynk.transition.may_violate`.
- **Posture:** feature track per
  [ADR 0076](../decisions/0076-feature-track-posture.md), run GitHub-native per
  [ADR 0167](../decisions/0167-feature-tracks-run-github-native.md). §2 makes
  the case and notes the risk that it is one increment.
- **What it does not do:** change run-time behaviour. The commit-time check
  stays the authority; a warning changes no emitted code. It also adds no
  language surface unless §3.6 decides a suppression form is needed, which
  would be its own proposal.
- **Governing records:**
  - [0107](../decisions/0107-agent-invariant-predicate-surface.md) and [0144](../decisions/0144-one-predicate-surface.md):
    the `transition` surface, checked at commit from the second commit onward.
  - [0535](../decisions/0535-agent-state-model-data-source.md): the state
    model is a re-parse in `bynk-ide`, shaped (track §3.7) so a checker rule
    can reuse it.
  - [0539](../decisions/0539-transition-overlay-is-a-view-annotation.md): the
    overlay's fragment, certainty and commit condition.

## 1. The theme

A `transition` step invariant constrains how an agent's state may move:

```bynk
transition paid_is_terminal: old.status is Paid implies new.status is Paid
```

It is checked only when the handler commits. A handler that can make a
refused move first shows it as an `InvariantViolation` at run time, in a test
if one happens to reach that state, or in production if none does.

The agent-state-views track built the analysis that finds these moves. The
state view's overlay (ADR 0539) evaluates each `transition` against the
derived state model and marks a violating move red (known) or amber
(possible). Fixture 245's unguarded `place()` from `Paid` is red as you edit.

But the overlay is only seen by someone who opens the state view. It does not
reach the Problems panel, `bynkc check`, a pull request's CI, or anyone who
uses another editor. This track asks whether, and how, the same finding
becomes a checker diagnostic.

### 1.1 What is already true

- **The evaluator exists and is tested.** `bynk_ide::agent_state` derives,
  per state field, edges (`from` set × handler × guards → target, outcome),
  reachability from the initialiser, and per-edge `Violation`s with a
  per-start `Certainty` (`Known` / `Possible`). Predicates outside the fragment
  are listed as `NotEvaluated`.
- **It lives in the wrong crate for a diagnostic.** `bynk-ide` depends on
  `bynk-check`, not the other way round. The model uses `bynk_fmt` only to
  turn guard expressions into labels; `bynk-fmt` depends on `bynk-syntax` and
  `bynk-render` only, so `bynk-check` could depend on it without a cycle.
- **The checker already visits every `transition`.**
  `checker::check_transitions` (called from `context_checks.rs`) types each
  predicate and emits the existing `bynk.transition.*` codes. A new rule has
  an obvious home beside it.
- **The commit condition is mirrored, not shared.** The overlay recomputes
  syntactically whether a handler commits; the emitter decides with
  `bynk_lower::body_writes_state` over resolved callees. Both read the
  store-op registry's `mutates` flag, but nothing checks that they agree.
- **The corpus is small.** Running the evaluator over every `.bynk` file in
  `examples/`, `tests/`, `bynkc/tests` and `site/` (types resolved per
  directory) finds:
  - 242 agents, of which 18 have a state field;
  - **one** violation: fixture 245's deliberate `Paid → Placed`, `Known`;
  - no `Possible` violations and no violating unchanged paths;
  - one `transition` not evaluated (fixture 1374's hoisting test).

  So the repository cannot measure a false-positive rate: it has almost no
  state-field `transition`s to be wrong about.
- **The genesis commit is not checked.** The Book's
  [agent invariants reference](../../site/src/content/docs/book/reference/agent-invariants.md)
  says a `transition` "is checked from the **second commit onward**: the
  genesis commit (an agent's first) has no `old` and is skipped". The overlay
  does not model this. An edge from the initial state is flagged even when the
  only way to take it is the agent's first call, which never faults. Fixture
  245 is unaffected (`Paid` is not the initial state), but the overlay
  over-claims in general, and a diagnostic would inherit the error.

## 2. Why a track (the ADR 0076 trigger)

- **Multi-increment.** Moving the model below `bynk-ide` is a change of its own
  with no change in behaviour. The genesis fix corrects the shipped overlay
  whether or not a diagnostic follows. The diagnostic itself, with its
  fixtures, ADR and reference row, is a third.
- **Surface not yet settled.** The code or codes, the severity, which
  certainty fires, what the message promises, and whether a warning can be
  silenced (§3.4, §3.6) are all open.
- **Not a security boundary.**

The risk is that, once §3 settles, the work is small enough to be one
increment with two prerequisites. If review agrees, this doc can be cut to a
single proposal and the spine closed without a track.

## 3. Design questions

### 3.1 — Placement — where the model lives

**Question.** The checker can't call into `bynk-ide`. Where do the extractor
and the evaluator move, and does the move change what they read?

**Draft leaning.**

- **Move `agent_state` into `bynk-check`**, as `bynk_check::agent_state`,
  with what it needs: `store_write`, and the three helpers it borrows from
  the sequence diagram (`branch_outcome`, `pattern_summary`, `peel_paren`),
  which `sequence.rs` then imports back. `bynk-ide` re-exports or calls
  it; the state view and the LSP request are unchanged. `agent_effects` can
  stay in `bynk-ide`: no diagnostic needs it.
- **Add the `bynk-fmt` dependency to `bynk-check`** for the guard labels,
  rather than splitting labelling out. Guards are merged by their text
  (`combine_guards` cancels `c` against `!(c)`), so the labels are part of the
  model, not decoration.
- **Keep the syntactic store-write recogniser.** The view runs on a re-parse,
  not a checked program, and one recogniser must serve both. Resolved callees
  would make the checker's answer differ from the view's, which is worse than
  either alone.
- **Pin the commit condition instead of sharing it.** The emitter's
  `body_writes_state` stays authoritative over resolved callees. Add a corpus
  test asserting that the syntactic commit test and `body_writes_state` agree
  on every handler in the fixtures, so a change to either fails loudly. This
  closes ADR 0539's "a change to `body_writes_state` needs the same change
  here" by test rather than by rewriting the emitter.

### 3.2 — What fires

**Question.** The overlay distinguishes known violations, possible ones (a
computed write that may land badly) and violating unchanged paths. Which
become warnings?

**Draft leaning.**

- **Known violations only**: a known target `S → T` the predicate refuses, or
  an unchanged path `S → S` it refuses in a handler that commits.
- **The unchanged path warns.** `transition must_advance: old.status is
  Pending implies new.status is Placed` makes a refused call from `Pending`
  fail at run time, and that is exactly what the author needs to know.
- **`Possible` stays view-only.** A computed write's target is unknown by
  construction; warning on every `status := next(x)` that could land badly
  would train authors to ignore the code. The amber overlay remains.
- **Truncated handlers fire nothing.** A handler past the path budget has no
  trustworthy edges.

### 3.3 — Opaque guards

**Question.** A known violation can sit on a path whose guards the model
doesn't evaluate (`if amount > 0 { status := Placed }`). If the guard is
correlated with the state, the path may be infeasible from the flagged start.
Does that weaken the warning?

**Draft leaning.**

- **Still warn, and say which path.** The model already narrows on every guard
  that tests the state field itself (`match status`, `status is Paid`), which
  is where correlation with the state normally lives. A guard over another
  value rarely implies the state, and the corpus has no example either way.
- **The message carries the path's guards**: "`place()` can move `status` from
  `Paid` to `Placed` (when `amount > 0`), which `paid_is_terminal` refuses at
  commit". An author who knows the guard excludes `Paid` can see why the
  checker disagrees and restructure (§3.6).

### 3.4 — Code, severity and message

**Question.** One code or several? Which severity?

**Draft leaning.**

- **One code, `bynk.transition.may_violate`, always a warning.** It is the
  name ADR 0539 recorded. "May" is honest even for a known violation: opaque
  guards (§3.3) and the start state's reachability are over-approximations.
- **Never an error.** The run-time check is unchanged and authoritative; an
  error would make a sound but imprecise analysis block builds.
- **No quick fix in this track.** The right fix (a guard, a refusal arm, a
  different target) depends on intent.

### 3.5 — Locality and spans

**Question.** Reachability is computed over all of an agent's handlers, so an
edit to one handler can add or remove a warning in another. Where does the
warning point, and is that non-locality acceptable?

**Draft leaning.**

- **Primary span: the write** (for a known target) **or the path's tail**
  (for an unchanged path), as in the overlay's click-to-code.
- **Related information:** the `transition` declaration, and the start state's
  witness, meaning one handler edge that reaches it from the initialiser, so the
  author can see why `Paid` is considered reachable.
- **Accept the non-locality.** It is inherent: whether a move is a problem
  depends on whether its start state is reachable, which other handlers
  decide. The witness makes it explainable.

### 3.6 — Silencing

**Question.** How does an author resolve a warning they think is wrong?

**Draft leaning.**

- **By code, not annotation.** A guard the model understands removes the edge:
  adding a `Paid => Err(AlreadyPaid)` arm to a `match status`, or an
  `if status is Paid { … }` refusal, narrows the path's `from` set and the
  warning goes. The diagnostics reference documents this recipe.
- **No suppression surface in this track.** The language has no per-site
  `allow` today. Adding one is cross-cutting (it would apply to every warning)
  and belongs in its own proposal if §3.2's conservative scope still proves
  noisy downstream.

### 3.7 — Not-evaluated `transition`s

**Question.** A `transition` outside the fragment is listed in the view as
"not evaluated". Does the checker say anything?

**Draft leaning.** **Silent.** A hint on every `transition` that mentions a
payload or another field would be noise about the analysis, not about the
program. The view keeps its list.

### 3.8 — Measuring

**Question.** §3.5 of the retired track said the diagnostic should wait "once
the overlay has shown how often it would fire falsely". How is that measured?

**Draft leaning.**

- **The repository can't measure it** (§1.1: one violation in 242 agents, and
  it is deliberate). That is a reason for §3.2's conservative scope, not for
  waiting indefinitely.
- **Keep the probe as a test.** A corpus test in `bynk-check` pins the
  evaluator's findings over the fixtures and examples, so a change to the
  model that adds findings shows up in review.
- **Ask for one downstream run before the warning slice merges**: the maintainer
  runs the probe over a real project (for example `bynk-lang/shop`) and the
  results are recorded on the slice's proposal.

### 3.9 — The genesis commit

**Question.** A `transition` is not checked on an agent's first commit
(§1.1). Which edges does that exempt?

**Draft leaning.**

- **An edge from state `S` can fault only when the agent can be in `S` after a
  commit.** For any state other than the initial one this is plain
  reachability: getting there took a commit. For the initial state it holds
  only if some committing path leaves the agent in it, either an unchanged
  path from the initial state in a handler that commits, or an edge back into
  it.
- **Compute "reachable after a commit" in the model**, and use it for both the
  overlay and the warning. A violating edge from an initial state that is
  never re-entered after a commit is not flagged: it can only run as the
  genesis commit, which is skipped.
- **Fix the overlay first, on its own.** It is a correctness bug in shipped
  behaviour, independent of whether the diagnostic lands.

## 4. Candidate slice decomposition

Each slice becomes an increment proposal, a sub-issue of the spine.

0. **Genesis-aware reachability** (§3.9). In the model, with the overlay
   consuming it. A fixture where the only violating edge leaves an initial
   state that is never re-entered, flagged today and clear after.
1. **Move the model into `bynk-check`** (§3.1). No behaviour change. Add the
   commit-condition agreement test and the corpus probe test (§3.8).
2. **The `bynk.transition.may_violate` warning** (§3.2–§3.7): emitted beside
   `check_transitions`, with:
   - fixtures for a known target, an unchanged path, an opaque guard, a
     `Possible`-only agent (no warning), a truncated handler (no warning) and a
     genesis-only edge (no warning);
   - fixture 245's expectations updated to carry the warning;
   - a row in the diagnostics reference with the §3.6 recipe;
   - the ADR, and the downstream measurement recorded on the proposal.

Slices 0 and 1 are independent; 2 needs both.

## 5. Front-loaded ADR candidates

- **A statically violable `transition` is a warning, for known violations
  only** (§3.2, §3.4). It amends ADR 0539's "there is no diagnostic".
- **The state model lives in `bynk-check`, shared by the view and the
  checker** (§3.1). It amends ADR 0535's placement.

## 6. Threat model

None. The checker reads source it already reads. A warning changes no emitted
code and no run-time behaviour.

## 7. Slice status

- [ ] Slice 0: genesis-aware reachability
- [ ] Slice 1: the model in `bynk-check`
- [ ] Slice 2: the `bynk.transition.may_violate` warning

## 8. Done when

- `bynkc check` warns at the write when a handler can make a move a
  `transition` refuses from a state the agent can be in after a commit, with
  the `transition` and a reachability witness as related information.
- The state view and the checker read one model, and no longer disagree with
  each other or with the run-time check about the genesis commit.
- A computed write, a truncated handler or a not-evaluated `transition` raises
  no warning.
- The diagnostics reference shows how to resolve the warning in code.

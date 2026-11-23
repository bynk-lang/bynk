# Bynk — Positioning & Identity

*Decision record for [#540](https://github.com/accuser/bynk/issues/540) §7(1).
Settles the "pick one identity" question raised by the Bynk Language Design
Review (2026-07-05). This is a strategy record, not a language-defining call, so
it lives here rather than in [`decisions/`](decisions/README.md).*

---

## The problem this settles

The repo had drifted into carrying three identities at once, and they pulled the
roadmap in different directions:

1. **A production service-tier language** — the pitch in the root `README.md` and
   in [*What is Bynk?* › Why Bynk exists](../site/src/content/docs/book/introduction/what-is-bynk.md):
   a statically typed, architecture-first language for building services that
   compiles to typed TypeScript on Cloudflare Workers.
2. **A pedagogy-first principle** — the design-notes framing of Bynk as a
   *taught* language: the compiler as "the more knowledgeable other", the surface
   organised for novices, students and course design.
3. **A research instrument** — [`bynk-phd-exploratory-memo.md`](archive/bynk-phd-exploratory-memo.md),
   which reframed the artefact as the apparatus for an empirical
   computing-education doctorate, with the type theory demoted to "machinery, not
   contribution".

A design review can hold all three as live options; a roadmap cannot. Each
implies a different next six months — ship deploy and migrations, or write a
curriculum and progressive-disclosure tutor, or recruit novices and clear an
ethics board — and they do not compose.

## The decision

**Bynk is a production service-tier language.** That is the single identity. The
work that moves the project is the work that makes real services shippable and
maintainable: deploy, state migrations, and ecosystem posture, in that order
(#540 §7(2)).

The other two framings do not survive as *goals*. One is retired outright; one is
kept, but demoted to a *constraint* — something true of the production language,
not a second mission competing with it.

### The PhD framing is dropped

The research-instrument identity is retired. The exploratory memo moves to
[`archive/`](archive/bynk-phd-exploratory-memo.md) for the record. Nothing in it
governs the roadmap: Bynk is not being built to make a computing-education
question askable, and the type theory — refinement types, the effect/capability
system, compilation correctness — is core contribution, not "machinery". A
doctorate may one day *use* Bynk; the language is not being shaped to serve one.

### Pedagogy is kept as a constraint, not a goal — the language teaches *as it is used*

Bynk is **not a teaching language**. It is not designed around novices, it does
not assume a course or an instructor, and "would this help a beginner learn to
program" is not a design test it must pass.

What is kept — and it is load-bearing — is a narrower, sharper claim: **a good
production language teaches the practitioner in the act of using it.** The
teaching is not a curriculum bolted on; it is an emergent property of two things
the production language already does:

- **The expressible program tends to be the correct one.** Because good
  architecture is inexpressible to violate, the path of least resistance is the
  sound shape. The practitioner absorbs the architecture by writing programs that
  compile, not by being lectured about it first.
- **The diagnostic is the lesson.** When a program *is* refused, Bynk does not
  merely fail — it names the invariant that was broken and points at the
  sanctioned way to express the intent. The refusal arrives at the one moment the
  lesson is most useful: while you are trying to write the thing.

This is pedagogy for the working developer, delivered at the point of use, and it
serves the production identity rather than competing with it: error messages that
explain, refusals that teach, and a surface where the correct shape is the easy
shape are all straightforwardly *good language design*. The audience is the
person shipping the service, not a student in week three of a module.

The public positioning already states this correctly and is the canonical
phrasing to match:

> Bynk is a service-tier language, not a teaching tool — but a language that
> explains every refusal is one you come to understand *as you use it*.
> — [*What is Bynk?* › Why Bynk exists](../site/src/content/docs/book/introduction/what-is-bynk.md)

## What this changes

- **Design notes §2** are reconciled to this record: the "taught language"
  framing (the compiler as "more knowledgeable other"; the layered surface
  justified by novices, students and course design) is rephrased as
  teaches-as-you-use-it for the practitioner. The *substance* of progressive
  disclosure — foundations / coordination / advanced layers that compose without
  entangling — is retained; only the audience framing changes. Progressive
  disclosure is good ergonomics for any developer meeting a large surface, not a
  concession to a classroom.
- **The PhD memo is archived** and dropped from the live design index.
- **What does not change:** the public *What is Bynk?* page (whose "Why Bynk exists" section absorbed the old `about/why-bynk-exists` page in web presence slice 13) and the root
  `README.md`, which already pitch the production identity with pedagogy as the
  "quieter principle" underneath it. This record brings the internal notes into
  line with them, not the other way round.

## What stays out of scope here

This record settles only the identity question — #540 §7(1). The review's other
strategic calls (blocker sequencing, freezing tooling depth, monthly milestone
cadence, the 1.0 definition, drift guards for the README/about pages, the honest
comparison page, the two-production-deployment bar) are separate decisions on the
same tracking issue and are not resolved here. They now inherit one useful
constraint: they are decisions for a *production language*, and can be weighed on
that basis alone.

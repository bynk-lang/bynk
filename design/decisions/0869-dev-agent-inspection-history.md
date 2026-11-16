# 0869 — A dev build keeps each agent instance's recent transitions in a bounded ring buffer outside layout 2

- **Status:** Accepted (v0.633.2)

**Context.** The list-and-read slice of the dev agent-inspection track shows what
an instance holds now, not how it got there. Transitions exist only inside
`commitState`, so only the emitter can record them. The track is
`design/archive/retired-tracks.md` (`dev-agent-inspection.md`); its spine is #3581.

**Decision.**

- **Appended in `commitState`, in a dev build only.** With `inspect_agents` on,
  `commitState` takes the handler's name as an optional trailing parameter,
  snapshots what is stored just before its write, and after the write appends
  `{handler, old, new, at}`. Every other build's `commitState` is unchanged, and a
  test pins that.
- **Bounded, and outside layout 2.** The buffer is the last 50 entries (and under 96 KiB, since it is one stored value and storage caps a value at 128 KiB), under the
  reserved key `__bynk_dev/history`. It lies outside every layout-2 namespace
  (`state`, `m/`, `s/`, `l/`, `i/`, `x/`, `c/`), so no read, write or sweep of
  layout 2 sees it, and it is storage, so it survives eviction.
- **What `old` and `new` hold.** The `Cell` fields in wire form and each
  collection's size. A collection's entries are not copied, so one large
  collection cannot fill the buffer. `old` is `null` for an instance's first commit.
- **Recording never changes a commit.** The snapshot and the append catch their
  own faults, and neither throws. A commit that throws before its write, an
  invariant or `transition` refusal, never reaches the append, so a refused
  transition has no entry and leaves the stored state as it was.
- **Read with the state.** `bynk dev inspect agent` returns the buffer as
  `history`, oldest first.

**Consequences.** A dev build's `commitState` differs from a deploy build's by two
awaited calls and a parameter, and the behavioural fixture shows outcomes are
unchanged. The entry shape is the decision comment's `{handler, old, new, at}`;
a refusal is not an entry. This slice completes the track.

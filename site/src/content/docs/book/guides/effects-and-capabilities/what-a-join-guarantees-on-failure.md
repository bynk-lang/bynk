---
title: What a join guarantees on failure
---
A [`par` block](/book/guides/effects-and-capabilities/run-independent-calls-at-the-same-time/)
starts several effects and waits for all of them. This page is about what it does when
one of them fails, which is the part that differs from the JavaScript you may be
expecting.

## The block always waits for every effect

In JavaScript, `Promise.all` rejects as soon as the first promise does, and the others
keep running with nobody waiting for them. A `par` block does not do that. If one
effect fails, the block still waits for the rest to settle, and only then raises the
failure. Nothing the block started is still running when it ends.

That has a cost you should expect: a call that fails fast waits for a slower sibling
before the handler learns of it. It buys two things.

- **Work stays inside the handler.** An effect that outlived the handler could finish,
  or fail, after the response was sent, with nobody to see it.
- **The same failure every time.** If two effects fail, the block raises the **first in
  source order**, not whichever happened to fail first in time. Reordering the calls in
  your source reorders which failure wins, and nothing else does. The failure that loses
  is attached to the one raised, so a log shows both, but it cannot be caught on its own.

## What stands after a failure

A block does not undo its effects, any more than a sequence of `<-`s does. Calls that
finished before the failure keep their effect, in the same way as in sequential code
([agents and state](/book/reference/agents/)):

- a call to another agent or context that returned has run, and stays run;
- writes to the agent's own `store` are staged and discarded with the failed handler;
- a `~>` send that was already sent stands.

## In an agent handler

A block is allowed in an agent handler. A Durable Object lets another request run while
the handler waits on a subrequest, whether that wait is one `<-` or a block, so a block
adds no interleaving that two `<-`s do not already allow. The one thing it removes is
the instant between two adjacent lines, and nothing in a block can depend on that,
because no call in it can read another's result.

## In a test

A stub resolves at once, so a test sees the same outcome every run. `expect A.op before
B.op` compares the order calls were *issued* in; see
[observation](/book/reference/testing/#observation).

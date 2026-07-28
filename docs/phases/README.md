# Why phase gates

0xin's roadmap isn't a backlog or a progress bar — it's a series of stages,
each defined by one concrete **deliverable**: a thing you can point at, run,
and see working. "A nested window shows a solid color." "A terminal appears." "I
can type into it." That's a deliberate constraint, not an accident of how
the notes got organized.

## The alternative, and why it was rejected

An installer-shaped project plan says "when everything is done, you get a
working system" — you can't meaningfully check progress until the very end,
because the pieces don't do anything in isolation. That shape is fine for
software you're assembling from parts you already trust. It's the wrong
shape for software you're building specifically to *understand*, because it
lets you accumulate code you don't actually understand yet, as long as it
compiles — the gap only shows up later, at the worst possible time, when
three unverified layers are stacked on top of each other.

A **phase gate** inverts that: you don't move to the next stage until the
current one has a working, demonstrable deliverable *and* you understand why
it works. Concretely, that's the learning-first workflow this project runs
on (from `KICKOFF.md`):

1. Explain the concept first — what's being built, why it's needed, which
   Wayland/wlroots/Linux concept it touches, what's unsafe or ABI-specific.
2. Make the smallest useful change — no large generated drops.
3. Show and explain every file and function touched.
4. Say how to test it, then actually run it and show the real output — never
   claim something works without verifying it.
5. Don't advance to the next stage until the current one is understood.
6. Every commit should be understandable on its own.

## What a stage actually is

Each stage below has the same shape: what it is, why it matters, its stated
deliverable (verbatim from `KICKOFF.md`), how it actually went, and its
current status. "How it actually went" is the part a plan can't predict in
advance — VT-switch black-screen bugs, opaque-struct FFI surprises, a
reversibility bug in directional window navigation that only appears at four
or more windows. The gate isn't the plan; it's the verified result.

Stages 0–5 are done and described in full; 6, 8, and 9 are substantially
working. The rest are open — those chapters are short and will grow as the
work happens, the same way the rest of this book grows: after the fact,
from what was actually built, not written speculatively in advance.

The roadmap itself also grows. Stages 0–8 were the bootstrap era — from
"a window shows a solid color" to a compositor that runs real hardware and
real apps. Stages 9–11 are the daily-driver era: floating windows, a
split-tree layout, runtime control. New stages get added when new work
earns a gate of its own; what never changes is the rule that each stage has
exactly one concrete, testable deliverable.

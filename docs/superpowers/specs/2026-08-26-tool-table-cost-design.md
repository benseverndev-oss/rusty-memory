# The tool table costs 2,600 tokens a turn, and two thirds of it is schema prose

**Status:** proposed
**Date:** 2026-08-26

## The problem

The tool table is sent on every turn of every session with this server
configured, used or not. It is now **nine tools, ~2,600 tokens**, up from
~2,060 at eight. A thirty-decision log costs about 1,850 tokens to read in
full, so a session that only ever consults decisions pays more than a whole log
per turn to advertise tools it will never call.

`RMEM_TOOLS` already exists and answers this for anyone who sets it. The
question is what the table costs for everyone who does not.

## Where the bytes actually are

Measured per tool, serialised:

| tool | description | whole | description share |
|---|---|---|---|
| `note` | 677 | 2,164 | 31% |
| `decide` | 723 | 2,089 | 35% |
| `decision` | 375 | 1,194 | 31% |
| `decisions` | 370 | 1,188 | 31% |
| `recall` | 647 | 1,174 | 55% |
| `about` | 316 | 889 | 36% |
| `remember` | 258 | 788 | 33% |
| `reviews` | 259 | 380 | 68% |
| `resolve_review` | 139 | 492 | 28% |
| | | **10,358** | |

The top-level `description` is a **third** of a tool's bytes. The other two
thirds are `inputSchema` — overwhelmingly the per-property `description`
strings inside it, since the structural JSON of a property is only a few dozen
characters.

This is the finding that shapes the work. "Tighten the descriptions" aimed at
the top-level field would address a third of the cost while removing the part
that does the most good.

## What is worth keeping

Not all of this prose is fat. Some of it is the only thing standing between a
model and a wrong answer, and this store's wrong answers are unusually
expensive:

- The sentence distinguishing `note` from `remember`. Choosing wrong costs a
  completion call or a fact recorded as prose.
- The `absent` explanation. `absent` and `unknown` being different is the
  store's central claim, and a model that conflates them will state that
  someone has no employer because nobody was asked.
- The warning on `scope`, which is the field most often wrong and the one a
  session cannot infer from where it stands.
- The `fields` note distinguishing identifying fields from attributes. These
  reach the resolver; getting it wrong is silent.

What is worth cutting is prose that **restates the schema**: telling a model a
property is a string when `"type": "string"` is on the same line, explaining a
default that `default` could carry, or re-describing in a property what the
tool description already said.

## The change

1. **Rewrite property descriptions to carry only what the schema cannot say.**
   Type, requiredness and enumeration are structural — express them
   structurally. Prose is for consequence: what happens if you get this wrong,
   and when to reach for this tool instead of its neighbour.

2. **Measure before and after, per the README's own table**, and update both
   the table and `definitions()`' doc comment in the same change. Those two
   disagreed for a day in August 2026 because one moved and the other did not.

3. **No tool is removed and no default changes.** Every existing configuration
   keeps working and nobody's session silently loses a capability.

The target is a meaningful reduction with the four load-bearing sentences
intact. A number is not set here on purpose: a budget invites cutting the
sentence that is hardest to justify per byte, which is usually the one about
`absent`.

## What it does not do

**It does not narrow the default.** Defaulting to the decision tools would save
the most and was considered. It silently removes `remember`, `note` and `about`
from every session that has not set `RMEM_TOOLS`, with no error — the model
simply stops having the capability, which is the hardest kind of change to
diagnose from the far side.

**It does not compress by abbreviating.** Terse prose that a model reads wrong
costs more than the tokens it saved. The measure is bytes removed *without*
losing a distinction, not bytes removed.

**It does not restructure the table.** Grouping, aliasing, or a two-tier
"summary then detail" protocol are larger changes with a protocol-compatibility
argument attached, and this is a prose edit.

## Testing

The existing table tests already do most of the work and were the ones that
caught the ninth tool: the example-per-schema test, the fixed-order test, and
the protocol handshake's count.

What to add:

- A byte-count assertion per tool, so a description growing back is visible in
  a diff rather than discovered a year later. This is the tier-two "pin" from
  the executable-claims spec, and the two changes should agree on where the
  chars-per-token ratio is stated so it lives in exactly one place.
- The example-per-schema test must still pass, which it will, since property
  descriptions are not read by `Call::read`. Worth stating because it means
  this change cannot break argument parsing — the risk is entirely that a model
  understands the tool less well, and no test in this repository can see that.

That last point is the honest limit of this work: the cost is measurable and
the benefit of the prose is not. Which is the argument for cutting only what
demonstrably restates the schema, and leaving anything whose value is arguable.

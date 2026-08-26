# The absence benchmark

**Measured 2026-08-26.** Harness: `crates/rm-engine/tests/absence.rs`.
Corpus: `crates/rm-engine/tests/absence/cases.json`.

## The question

"They have no employer" and "nobody has ever said" are different answers. Does
the store actually keep them apart, on data where the correct answer is written
down in advance?

## The result

Eight cases, three attributes each, twenty-four questions.

|  | answered value | answered absent | answered unknown |
|---|---|---|---|
| **truth: value** | **8** | 0 | 0 |
| **truth: absent** | 0 | **8** | 0 |
| **truth: never mentioned** | 0 | **0** | **8** |

The cell that matters is the bottom middle. Answering *absent* where nothing
was ever said is stating as fact that someone has no NPI because nobody
supplied one, or no beneficiary because nobody was asked. It is a confident
wrong answer somebody acts on, and it is **invisible in any two-state
system** — not because such a system scores badly on it, but because it has no
way to represent the difference.

## Why a matrix and not a percentage

Two of the nine cells are fabrications and they are asserted separately. A
single accuracy figure lets one grow while another shrinks and reports no
change, which is a shape of number this project has had to catch more than
once.

## Why the diagonal is not the whole story

A clean diagonal is also what a vacuous test looks like. Three things guard
against that, and they are part of the result:

1. **Every case must label all three outcomes.** A corpus of values and
   silences is one any two-state system scores perfectly on — without a stated
   absence there is nothing to confuse an asserted "there is none" with.
2. **The labels are checked for honesty.** A case that both states an attribute
   and calls it `unknown` would make the store look wrong for answering
   correctly. That error lives in the fixture, which is the hardest place to
   find it.
3. **The matrix was seen red before it was committed.** Writing one
   never-stated attribute as an asserted absence moved `unknown->absent` from 0
   to 8, and the assertion named the cell rather than reporting a moved number.

Both failures on the first run were real, and worth recording. The shape guard
caught a case that carried no stated absence at all. And the resolver refused
to seed subjects named `Case A` and `Case B`: they share a three-character
prefix, scored inside the review band, and were filed as an open question — so
the corpus was measuring identity rather than the three-way answer. The
subjects now have deliberately dissimilar names.

## What this measures, and what it does not

**It measures** whether the store returns the right one of three answers on a
corpus where all three are labelled, through `Engine::about` and nothing else.
No `recall`, no vector threshold, no `weak_below` — the claim is structural, an
assertion exists or it does not, and measuring it through a probabilistic path
would reintroduce the mechanism `benches/locomo` already tried and rejected.

**It does not measure** retrieval, recall, or whether this memory is better
than another. The corpus is synthetic, purpose-built, and eight cases deep.
That a system handles a known-hard distinction is a smaller claim than that it
works well.

## The benchmark is designed around a distinction only this system makes

Said plainly, because a reader who works it out unaided will discount
everything else here.

Competitors do not score badly on this axis. **They cannot be scored on it**,
having no third state to return. A benchmark whose metric only one system can
express is not a fair comparison and is not offered as one.

Two things keep it honest:

- **Recall is reported on the axis where comparison *is* fair.** `benches/locomo`
  scores retrieval against evidence turn ids on the corpus others benchmark on.
  A store that cannot retrieve is not saved by knowing what it does not know,
  and that number is published whether or not it flatters.
- **The prior negative result is cited rather than buried.** A score-based
  refusal was tried here first: six candidate signals scored by Youden's J
  against LoCoMo's 382 answerable and 112 unanswerable questions, best J =
  0.494 on the raw top score. The trade was bad — keeping 90% of answerable
  questions refuses only 36.6% of unanswerable ones — and the cutoff did not
  transfer to another corpus, so `weak_below` ships off by default. That
  failure is the argument for a structural distinction, and it is stronger
  evidence than any assertion about one.

## The competitive comparison has not been run

The plan calls for a two-scenario demonstration against other memory systems:

- **A:** the conversation states "I'm single."
- **B:** partners are never mentioned.
- Both asked: does this person have a partner?

**This has not been done, and nothing here should be read as though it had.**
It needs each system installed and credentialed, and it is deliberately manual
and small rather than a harness.

Until it is run, one claim in `docs/positioning.md` remains **an inference from
how those systems are built, not a measurement**: that a summarise-and-dedupe
architecture answers both scenarios the same way. If a system distinguishes
them, the positioning is wrong and this document should say so first.

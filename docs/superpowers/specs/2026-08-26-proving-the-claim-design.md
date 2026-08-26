# Proving the claim: an absence corpus, and what LoCoMo can and cannot show

**Status:** proposed
**Date:** 2026-08-26

**Positioning:** `docs/positioning.md` — this is the proof obligation that
document sets, revised after reading what `benches/locomo` already measured.

## The claim, stated precisely

> "They have no employer" and "nobody has ever said" are different answers.

Three states, not two. `Believed::Value`, `Believed::Absent`, `Believed::Unknown`.
Observable today, end to end, at no cost:

```
spouse    Alex
employer  no value — asserted to have none
pets      nothing known — this was never discussed
```

That distinction is **structural**: either an assertion exists for the
attribute or it does not. There is no threshold and no score.

## Why LoCoMo cannot prove it

The positioning document proposed a two-axis LoCoMo benchmark — recall on the
382 answerable, correct refusal on the 112 unanswerable. Reading what the
harness already does makes that the wrong instrument, for three reasons.

**1. That measurement largely exists, and it is unflattering.**
`benches/locomo/README.md` already scores six refusal signals by Youden's J
against the 382/112 label. Best is the raw top score at J = 0.494, and the
trade is stated plainly: keeping 90% of answerable questions refuses only 36.6%
of unanswerable ones. `weak_below` ships at `0.0`, off, because of it, and
because the cutoff did not transfer to another corpus.

**2. It measures a different mechanism.** That analysis asks whether a
*similarity score* can separate answerable from unanswerable. It is a good
question and the answer is "not well". It is not this project's claim. The
claim is that an assertion either exists or does not, which is exact.

**3. LoCoMo has no category for the distinction.** Its ground truth is
two-way — answerable, or adversarial-and-unanswerable. There is no
*asserted absent*: no question whose correct answer is "the conversation
established there is none." The three-way distinction has no counterpart in
the corpus, and none in any competitor either, which is precisely why it is
the differentiator and precisely why no existing benchmark exercises it.

There is also a plumbing mismatch: LoCoMo questions are conversational and
route through `recall`, while the structural refusal lives on `about`.

## What to build instead

**An absence corpus**: turns with three-way ground truth per attribute, asked
through `about`.

```
value          the conversation states one
asserted-absent  the conversation states there is none
never-mentioned  the conversation is silent
```

The metric is a 3×3 confusion matrix, and one cell is the product:

| | answered value | answered absent | answered unknown |
|---|---|---|---|
| truth: value | correct | wrong | miss |
| truth: absent | **fabrication** | correct | conservative |
| truth: never mentioned | **fabrication** | **the failure this exists to prevent** | correct |

The bottom-right-but-one cell is the one to name in the README: answering
*absent* when nothing was ever said is stating as fact that someone has no
employer because nobody mentioned their job. It is a confident wrong answer a
user would act on, and it is invisible in any two-state system because such a
system has no way to represent the difference.

Ground truth is hand-written per turn, never derived from a run.

## The competitive demonstration, and its honesty problem

The comparison is not a leaderboard, and pretending otherwise would be rigging.
Competitors do not score badly on this axis; **they cannot be scored on it**,
because they have no third state to return.

So the demonstration is a pair of scenarios, run against each system, shown
side by side:

- Scenario A: the conversation says "I'm single."
- Scenario B: partners are never mentioned.
- Ask both: "does this person have a partner?"

If a system answers identically in both, that is the finding, and it is a
finding about the *shape* of the system rather than the quality of its
retrieval. If it distinguishes them, the positioning is wrong and better to
know cheaply.

**The rigging risk is real and must be handled in how it is reported**, on two
conditions:

1. **Report recall honestly on the axis where comparison is fair.** LoCoMo
   keeps a role here: it is the corpus others benchmark on, and a store that
   cannot retrieve is not saved by knowing what it does not know. Report it
   even where it is not best in class.
2. **State that the benchmark was designed around a distinction only this
   system makes.** A reader who works that out for themselves will discount
   everything; a reader told it up front can weigh it.

## What LoCoMo is still for

Comparability on recall, and nothing about the claim. The existing harness
already does the right things — retrieval scored against evidence turn ids with
no LLM judge between the measurement and the thing measured, and category 5
reported separately and never counted as a retrieval failure. None of that
changes.

Its refusal analysis also stands as a genuine negative result and should be
cited, not buried: a score-based refusal was tried, measured across six
signals, and rejected on the evidence. That is a stronger argument for a
structural distinction than any assertion about it.

## What this does not do

**It does not add a confidence score to `about`.** The whole point is that this
answer is exact.

**It does not claim the corpus is representative.** It is purpose-built,
synthetic, and measures whether a system handles a known-hard distinction.
That is a different and smaller claim than "this memory is better".

**It does not run competitors' pipelines end to end.** The scenario pair is
small and manual by design. A full harness against three other systems is a
project, and it is not needed to answer the question.

## Risks

**The distinction may not matter to anyone.** The strongest version of this
critique: adopters have never asked for it, and a benchmark demonstrating a
capability nobody wants proves the capability, not the demand. The mitigation
is in what the corpus is built from — the absences should be ones with obvious
consequence, drawn from the domains named in the positioning document (people,
money, records, compliance), not from toy examples about pets.

**Synthetic and self-designed.** Both true, both stated up front, and neither
is fixable by pretending otherwise. What makes it honest is reporting the fair
axis alongside, and saying who designed the unfair one.

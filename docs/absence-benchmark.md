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

## The two-scenario comparison, run

**Measured 2026-08-26** against mem0 2.0.19, open source, `gpt-4o-mini` and
`text-embedding-3-small` — the same model this project's own template names,
so neither side gets a better one. Harness and verbatim output in
`docs/comparison/`.

Two conversations identical but for one sentence. **A**: the speaker says they
have no partner. **B**: partners never come up. Both asked the same question.

| | mem0 | rusty-memory |
|---|---|---|
| **A** | `"User is not married and does not have a partner."` @ **0.6356** | `Absent` — "no value, asserted to have none" |
| **B** | three unrelated memories @ 0.1952, 0.1725, 0.1117 | `Unknown` — "nothing known, this was never discussed" |

### This falsified what was written here before

An earlier version of this document, and the README's first paragraph, said
mem0 could not represent a stated absence — reasoning from its documented
`ADD`/`UPDATE`/`DELETE` vocabulary that a negation resolves to a delete and
leaves the store as if nothing had been said.

**That was wrong.** mem0 stores the negation and retrieves it. The inference
was from documentation, and running it took twenty minutes.

### What the difference actually is

Both systems keep the distinction. They differ in what the caller receives.

mem0 returns content and a relevance score, and the caller decides what 0.19
means. Note that scenario B is not empty — it returns three memories, all
about employment and cycling. Nothing in the response says "this was never
discussed"; that has to be inferred from the numbers.

That inference is the thing this project has already measured and rejected.
On LoCoMo's 382 answerable against 112 unanswerable questions, the best
score-based signal reached Youden's J = 0.494, keeping 90% of answerable
questions meant refusing only 36.6% of unanswerable ones, and a cutoff taken
from that table marked a question with a perfect answer as a miss on a
different corpus. `weak_below` ships at `0.0` because of it.

So the claim that survives measurement is narrower than the one that preceded
it, and more specific: **not that others lose the information, but that they
return a score where this returns an answer.**

### What was not run

Graphiti/Zep and Letta were not measured — Graphiti needs a graph database
standing up, and neither was necessary to correct the claim that was wrong.
The documentation review below still stands for those two, with its own limits.

### What the other systems document

A documentation review, 2026-08-26. **Not a measurement**, and the mem0 row is
the cautionary case: its documentation supported an inference that running it
disproved.

| system | how it settles conflicts | three-valued answer documented? |
|---|---|---|
| mem0 | a per-fact decision to `ADD`, `UPDATE`, `DELETE` or leave alone | no — but it does retain a negation, see above |
| Graphiti / Zep | temporal edge invalidation, with lifecycle metadata on edges | no |
| Letta / MemGPT | free-text memory blocks an agent rewrites | no |

Documentation omitting a capability is not proof the capability is absent.
That is not a caveat added for balance; it is what happened.
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

## What the other systems document

A documentation review, done 2026-08-26. **Not a measurement** — see the
section below, which still stands.

| system | how it settles conflicts | asserted-absent documented? |
|---|---|---|
| mem0 | a per-fact decision to `ADD`, `UPDATE`, `DELETE` or leave alone | no |
| Graphiti / Zep | temporal edge invalidation, with lifecycle metadata on edges | no |
| Letta / MemGPT | free-text memory blocks an agent rewrites | no |

None of the three documents a way to record that something was asserted *not*
to be, distinct from never having come up, or any three-valued answer. mem0's
vocabulary is the clearest case: a negation resolves to `DELETE`, which leaves
the store in the same state as never having been told.

**This corrected an overstatement on the README's front page.** It had said
these systems "settle conflicts by asking a model to re-summarise". That is
fair to mem0 and Letta and wrong about Graphiti, whose own documentation
contrasts its temporal edge invalidation with GraphRAG's "LLM-driven
summarization judgments" — an approach genuinely adjacent to the bi-temporality
here. The claim now names what each does and limits itself to what none of them
documents.

Two things this review cannot establish. Documentation omitting a capability is
not proof the capability is absent, and none of these systems was run. The
claim is therefore about what they *document*, and the README says so in those
words.

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

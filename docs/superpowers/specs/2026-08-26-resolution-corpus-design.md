# A ground-truthed resolution corpus, and a baseline that fails when it moves

**Status:** proposed
**Date:** 2026-08-26

## The problem

Every resolution decision this project has made was made by argument.

The thresholds are `review_at = 5.2439` and `match_at = 7.2439`, and the config
explains at length where they came from: `u = 0.38` for the `kind` field is
"the rate at which two entities that share a name prefix happen to share a
kind, measured across four stores from a real corpus". That corpus is not in
this repository. Nobody can re-measure it, check whether it still holds, or
tell what a change to it would do.

On 2026-08-26 this cost a full session. The question was whether to add `email`
as a resolution field — an obvious improvement, since the field with the
strongest identifying power was the one field resolution never consulted.
Answering it required hand-building a 40-write corpus with ground truth, and
the answer reversed the recommendation twice:

| config | blocking | email comparator | questions | silent misses | wrong merges |
|---|---|---|---|---|---|
| A (as shipped) | prefix | — | 4 | 1 | 0 |
| B | prefix | `normalized` | 1 | 2 | 0 |
| C | prefix | `jaro_winkler` | 1 | 1 | **3** |
| D | token | `normalized` | — | — | **≥1** |

C silently absorbed three strangers into real people. D showed that B's clean
record was luck: prefix blocking happened never to compare the one pair whose
addresses collided, and switching the blocking key merged them.

None of that is visible from reading the code, and none of it would have been
caught by any test in the repository. The corpus was thrown away at the end of
the session; the next person to ask this question starts over.

## What this builds

A synthetic, ground-truthed corpus and a test that scores the current
configuration against it, compared to a committed baseline.

```
crates/rm-resolve/tests/corpus.rs        the test
crates/rm-resolve/tests/corpus/people.json     the corpus, with ground truth
crates/rm-resolve/tests/corpus/baseline.json   what the current config scores
```

A configuration change that turns a caught match into a silent miss, or a
stranger into a merge, fails `cargo test`. Updating `baseline.json` is how you
say a change was intended — a deliberate act with a diff a reviewer can read,
rather than a number nobody notices moving.

## The corpus is synthetic, and that is a requirement

This repository is public. The corpus that produced the table above was 27 real
colleagues with their real addresses, and it must not be committed.

Synthetic does not mean easy. The corpus must reproduce the *shapes* that
decided the outcome, because the shapes are the whole content of the test:

| shape | example | truth |
|---|---|---|
| exact repeat | `Sarah Viruet` twice | same |
| nickname | `Jonathan Severn` / `Jon Severn` | same |
| first name alone | `Himanshu` / `Himanshu Nagpal` | same |
| surname alone | `Severn` / `Jonathan Severn` | same |
| changed surname, stable address | `Andrea Johnsen` / `Andrea Herdman` | same |
| shared surname, different people | `Michael Johnsen` / `Andrea Johnsen` | **different** |
| shared given name + shared domain | `Robert Gomez` / `Robert Garavente` | **different** |
| colliding local part | `Adam Immordino` / `Amber Immordino` | **different** |

The last three are the ones worth the effort. Every configuration that looked
good failed on one of them, and each was found only because somebody
deliberately wrote a case designed to break the thing they were about to
recommend. A corpus of true matches measures nothing: any configuration that
merges everything scores perfectly on it.

## What is measured

Three counts, and the pairs behind each.

- **questions** — pairs landing in the review band. The cheap outcome. A person
  answers, and either answer is recoverable.
- **silent misses** — true matches that scored below `review_at`. A duplicate
  entity nobody is told about.
- **wrong merges** — different people merged. Silent and permanent, and the
  outcome the whole design exists to avoid.

They are not summed into a score. A single wrong merge is worse than any number
of questions, and a weighted total would let one hide behind the other. The
baseline records all three separately, with the specific pairs, so a diff says
*which* pair changed rather than that a number moved.

## What it does not do

**It does not propose thresholds.** The harness reports what a configuration
does. Choosing `review_at` remains a judgement about what a question costs
versus what a silent duplicate costs, and that judgement belongs to a person.

**It does not replace `rm-conform`.** That checks the engine against an
independently written reference model — whether the implementation is what was
specified. This checks whether what was specified produces good answers on data
with known truth. A configuration can be perfectly conformant and badly
calibrated.

**It does not sweep configurations automatically.** Scoring the shipped config
is the deliverable. A search over m/u values is a different piece of work and
would need an argument about what it is optimising, which is exactly the
judgement the previous paragraph reserves for a person.

## Testing

The test is the deliverable, so what needs stating is how it avoids being
vacuous — a live concern here, since one of this project's own recorded lessons
is that a test which supplies the thing it checks cannot fail.

- The corpus is loaded from JSON, not constructed by the code under test.
- Ground truth is written by hand in the corpus file and never derived from a
  resolver result.
- A guard test asserts the corpus contains at least one of each shape in the
  table above, so the file cannot be trimmed down to only the easy cases while
  still passing.
- A guard test asserts the baseline is not all zeros — a corpus that produces
  no questions, no misses and no merges is a corpus that is not exercising the
  resolver, and would otherwise look like a clean pass.

## Risks

**The corpus becomes the target.** Tuning until the baseline reads
`0/0/0` would be optimising for the fixture. This is why the corpus contains
cases that are *correctly* unresolvable — `Severn` scores 4.80 against a true
match while a true non-match scores 4.73, 0.07 bits apart, and no threshold
separates them. That row should stay a silent miss in the baseline, recorded
with a comment saying why it is not a bug.

**Synthetic data drifts from real data.** The shapes came from one real
register of 27 people at one company. Other corpora will have shapes this one
does not — patronymics, transliteration, initials-only. The corpus should be
appended to whenever real data teaches something new, and the commit that adds
a shape should say where the shape was seen.

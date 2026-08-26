# An extractor that declines

**Status:** proposed
**Date:** 2026-08-26

**Positioning:** `docs/positioning.md` — the input boundary, one of the two
where the store's principle has not yet been carried.

## The problem

`rm-extract` reads prose and produces facts. Every fact it produces is asserted
with the same confidence as every other, and the store has no way to tell an
unambiguous reading from a guess.

That matters more here than anywhere else in the system, because **the input
boundary is where fabrication enters**. Every other boundary can only be as
honest as what it was given. A store that refuses to invent a survivor, refuses
to invent an identity, and distinguishes `Absent` from `Unknown` will still
state a fabricated employer with total confidence if extraction invented it
from "I think he might have moved."

## What already refuses here

More than the crate's dormancy suggests, and worth stating so the change is
scoped honestly:

- **`Fact.value: Option<String>`** — `None` asserts the attribute has no value.
  The three-way answer already reaches the input boundary for *stated*
  absences.
- **`Dropped`** — what the response carried that the crate would not keep, and
  why, in the same voice as a refusal. Salvage stopped being a silent loss and
  became a reported one.
- **`Supersession::Unstated`** — the crate declines to infer whether a fact
  replaces or joins, rather than guessing from arrival order.

What is missing is narrow and specific: **a reading the model was unsure of
becomes a confident fact, or nothing at all.** There is no third outcome.

## The failure, concretely

| turn | what is produced now | what it should produce |
|---|---|---|
| "I think he might have left Acme" | `employer` absent, asserted | a question — the text hedges |
| "Priya told Rosa she'd been promoted" | one reading of the pronoun, picked | a question naming both readings |
| "he's at Globex now, or maybe Initech" | one value, or the fact dropped | a question naming both values |
| "she runs circulation" | `role` or `team`, picked | a question, if the attribute is genuinely unclear |

Each of these is the same shape the resolver already handles at the identity
boundary: evidence that supports more than one answer, and no basis to choose.
The resolver files a question. Extraction picks.

## The design

**An ambiguous reading produces the alternatives, never a probability.**

This is the constraint that keeps the change inside the project's idiom. A
confidence score would be a number nobody can calibrate, and this repository
has two measurements arguing against exactly that: `weak_below` ships off by
default because no cutoff transferred between corpora, and a fuzzy email
comparator that produced plausible partial scores silently merged three
strangers into real people. Partial credit is how fabrication gets in wearing a
number.

So the new outcome names readings, the way a review names a pair:

```rust
/// A reading the turn supports but does not settle.
///
/// Not a confidence score. The alternatives are named, and something that can
/// see more than this turn -- a later turn, or a person -- decides. A number
/// here would be a number nobody could calibrate, which is the mistake
/// `weak_below` is off by default to avoid.
pub struct Unsure {
    /// Which list it would have joined: "fact" or "relation".
    pub what: &'static str,
    /// The readings, in the order the model gave them. Always two or more --
    /// one reading is not an ambiguity, it is a fact.
    pub readings: Vec<Reading>,
    /// What in the turn is unsettled, in the same voice as `Dropped::why`:
    /// what is unclear, not merely that something is.
    pub why: String,
}
```

`Extraction` gains `pub unsure: Vec<Unsure>`, beside `dropped`. Empty on an
unambiguous turn, which is the common case and the one to check against.

**An unsure reading is not written.** It is returned to the caller, which is
what `dropped` already established: the crate reports rather than decides, and
the host chooses. A host that wants to record the ambiguity can; a host that
wants to ask can; a host that ignores the field is exactly as correct as it is
today, which keeps this additive.

## The constraint that shapes how it is detected

**Asking the model one more question per fact has been tried, and measured.**

`prompt`'s own documentation records it: a `"replaces"` boolean per fact cost
**19% of the facts** (735 and 763 across two samplings of the unchanged prompt,
616 with the rule), against noise bracketed at about ±4%. The shape of the loss
is legible — mentions went *up* while facts went down, so a model given one
more question per fact answers it by emitting fewer of them.

The conclusion drawn there is the one to reuse: the question was moved *away*
from extraction, to be asked once per attribute name and cached, because that
"cannot cost an extraction anything, because it does not touch one."

A per-fact "are you sure?" is the same rule in a new coat and should be
expected to cost the same 19%. Two routes avoid it, and **both are to be
measured against the same baseline before either ships**:

**Route A — structural, no model call.** Some ambiguity is a property of the
turn rather than a judgement about it, and is detectable without asking:

- A fact whose subject is a pronoun while the turn holds two mentions of a
  compatible kind. This is the "Priya told Rosa" case, and it is decidable from
  the parsed turn alone.
- Hedging in the span the fact was drawn from. Cheap and deterministic, in the
  idiom of `possessive_aware` and `normalize` — hand-written rules whose
  behaviour can be read.

Honest about its limit: a hedge list is brittle, misses paraphrase, and will
have false positives on quoted speech. It is worth building first because it
costs no facts and no tokens, and its false-positive rate is measurable.

**Route B — a second pass that cannot cost facts.** Adjudicate the already
extracted facts in a separate call. By construction it cannot suppress a fact,
because extraction has already finished. It costs a second completion, which is
the trade to measure: what fraction of turns need it, and does gating it on
Route A's signal make it affordable.

## The proof obligation

The positioning document sets it: a labelled set where the right answer is
**"this text does not say."**

It mirrors the resolution corpus, and for the same reason — a set of turns that
all say something clearly measures nothing, because an extractor that asserts
everything scores perfectly on it. The set needs turns whose correct outcome is
a question, and the metric is the pair:

- **facts retained** against the unchanged-prompt baseline — this is where the
  19% would show up
- **ambiguities caught** on turns labelled ambiguous, and **false questions**
  raised on turns labelled clear

A change that catches every ambiguity by declaring everything uncertain is a
regression, and only the second number makes that visible.

## What it does not do

**It does not add a confidence score.** See above; this is the load-bearing
constraint, not a stylistic preference.

**It does not resolve the ambiguity.** Naming two readings and picking one is
the behaviour being removed. Something with more context decides — a later
turn, or a person.

**It does not change the wire format the model answers in beyond what a route
requires.** Route A changes nothing the model sees. Route B is a separate
prompt and leaves `prompt` alone.

**It does not wake extraction as a product surface.** `note` and `decide`
remain the deliberate input paths. This makes the harvested path honest for
when it is wanted; the positioning document's trigger for that still stands.

## Risks

**The 19% repeats anyway.** Route A is structural, but if it proves too weak
and Route B becomes the answer, the cost moves from facts to calls rather than
disappearing. The measurement is what decides whether that trade is worth
making — and the honest outcome may be that it is not, in which case this ships
as Route A alone or does not ship.

**A question nobody answers is a fact nobody has.** Every ambiguity raised is a
fact not written. If nothing consumes `unsure`, the net effect is a store that
knows less and feels no better. Whichever host lands first has to do something
visible with the field, or this is a loss dressed as rigour.

**Brittleness read as principle.** A hedge-word list that misses "he may well
have" while catching "I think" is not refusing to fabricate, it is refusing
unevenly. The false-positive and false-negative rates belong in the README
beside the feature, not in a commit message.

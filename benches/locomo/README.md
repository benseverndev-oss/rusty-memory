# LoCoMo run

Runs the real pipeline — real extraction, real embeddings — over a real
multi-session conversation corpus, and reports what it does.

Everything in the workspace's own tests is synthetic: fixtures written to state
a property, and stub providers that return what the test wants. That is the
right way to pin behaviour and it cannot tell you whether the behaviour is any
good. The `m`/`u` probabilities in `rmem.toml` are numbers somebody chose;
whether the review band catches real ambiguity or fires on every pair of names
in a conversation is not a question a fixture can answer.

Not in the workspace and not run by CI: it costs money and takes minutes.

## Why LoCoMo

It is the corpus OpenViking benchmarks on, which makes anything measured here
comparable to the system this project is positioned against. It is also shaped
like the problem: 10 conversations of ~20 dated sessions each, between two
people whose lives change across months, with questions labelled by what they
demand — single-hop, multi-hop, temporal, open-domain, and adversarial
questions whose premise the conversation never supports.

```sh
curl -sSL -o locomo10.json \
  https://raw.githubusercontent.com/snap-research/locomo/main/data/locomo10.json
export OPENAI_API_KEY=...
cargo run --release --manifest-path benches/locomo/Cargo.toml -- locomo10.json 0 [turn budget]
```

The turn budget is optional and exists so a first run costs pennies and can be
looked at before the whole corpus is paid for.

## What is measured, and what is not

Retrieval is scored against `evidence`: the turn ids LoCoMo says answer a
question. Every assertion carries the `dia_id` of the turn it came from in its
provenance, so "did recall surface the right turn" is a set-membership test and
needs no model to judge it.

That is deliberate. Answer accuracy would need an LLM judge, which puts a second
model's opinion between the measurement and the thing measured — and the first
number this project needs is one nobody has to trust.

Only questions whose evidence falls entirely inside the ingested prefix are
asked. Scoring the rest would measure the turn budget rather than the store.

**Category 5 is adversarial**: the premise is unsupported and LoCoMo's own
answer is that it is unanswerable. Those are reported separately and never
counted as retrieval failures — a store that surfaces nothing for them is right.

## Two gaps this hit before it could run at all

`rm-providers` cannot reach a network whose egress is a proxy:

- `ureq` 2's free functions (`ureq::post`) do not read `HTTPS_PROXY`. Only an
  `Agent` built with a `Proxy` does.
- The `tls` feature is rustls with webpki-roots, which does not read
  `SSL_CERT_FILE`, so a proxy substituting its own certificate is rejected.

Every corporate network and most CI sandboxes are one or both of those, so this
is a real deployment limitation rather than a quirk of one machine. It is not
this harness's to fix, so the harness takes `LOCOMO_BASE_URL` and
`proxy-shim.py` goes in front:

```sh
python3 benches/locomo/proxy-shim.py &          # honours HTTPS_PROXY and SSL_CERT_FILE
LOCOMO_BASE_URL=http://127.0.0.1:8731/v1 cargo run --release ...
```

The shim exists only for that reason and should be deleted when
`rm-providers` learns to use a proxy.

## A third gap, in the library rather than around it

`rm_host::command::remember` hardcodes `speaker: None`, and so does the MCP
`remember` tool — neither has a way to pass one. `Turn`'s own documentation says
the speaker is what lets first-person references resolve, and dialogue is mostly
first person: without it, "I moved to Chicago" names nobody.

This harness sets the speaker, because otherwise it would be measuring a
pipeline nobody would deploy. That means **its numbers are an upper bound on
what `rmem` and `rmem-mcp` can currently do** on dialogue, not a measurement of
them.

## Two runs: conversation 0, 419 turns

`gpt-4o-mini` extraction, `text-embedding-3-small`, 2026-08-21. Same input,
same config, two runs — reported together because the difference between them
is itself a result.

### Ingestion

| | run 1 | run 2 |
|---|---:|---:|
| turns ingested | 379 | 381 |
| turns refused | 40 (9.5%) | 38 (9.1%) |
| entities | 148 | 138 |
| assertions | 543 | 554 |
| relations | **17** | **16** |
| review band | 117 | 95 |

### Retrieval, recall@10 against LoCoMo evidence turns

| category | n | run 1 | run 2 |
|---|---:|---:|---:|
| **overall** | 149 | **0.295** | **0.289** |
| temporal | 37 | 0.405 | 0.486 |
| single-hop | 70 | 0.300 | 0.229 |
| multi-hop | 31 | 0.161 | 0.258 |
| open-domain | 11 | 0.273 | 0.091 |

Adversarial: 11 then 7 of 47 surfaced something for a question the conversation
does not answer.

### Single-run category numbers do not replicate

The overall figure is stable to within 0.006. Every category moved by 7 to 18
points on identical input, and two of them changed rank: run 1 said multi-hop
was the worst category at 0.161, run 2 says it is 0.258 and single-hop is worst.

The first run of this harness was reported with a per-category reading built on
exactly that. **It was not a finding.** With 11 to 70 questions in a bucket and
a non-deterministic extractor upstream, one run cannot separate a category from
noise, and the honest floor for any claim at this granularity is several runs
with the spread shown.

What survives both runs: overall retrieval near 0.29, and **temporal as the
strongest category** — the only one above 0.4 in either run, and the one the
bi-temporal store exists to serve. That one is worth believing. The rest of the
table is not yet evidence of anything.

### What the store actually contains

Run 2 wrote its store, so the aggregates could be explained rather than
inferred. The explanation contradicts the inference drawn from run 1.

**Resolution is not the problem.** Run 1's reading was that 148 entities and 117
review pairs meant the resolver was under-merging the same person repeatedly.
It is not: `Caroline` is one entity with 184 assertions, `Melanie` one with 111,
and there are zero duplicate names across all 138 entities.

**Extraction is emitting relationships as entity names.** The review band asks
about these:

```
5.59  'Melanie'      vs  "Melanie's kids"
5.45  'Melanie'      vs  "Melanie's family"
5.34  'Melanie'      vs  "Melanie's children"
5.39  "Melanie's family" vs "Melanie's kids"
5.68  'camping'      vs  'camping site'
5.39  'pottery'      vs  'pottery workshop'
5.05  'LGBTQ support group' vs 'LGBTQ+'
```

`Melanie`, `Melanie's family`, `Melanie's kids`, `Melanie's children` and
`Melanie's son` are five separate entities generating review pairs against each
other. The resolver is being asked "are these the same entity?" about things
whose true relationship is *possession* — and its answer, "I cannot tell", is
correct. The question is wrong.

**This is also why there are 16 relations.** A possessive is a relationship, and
extraction is encoding it in a name instead of emitting it as a relation.
`Melanie's son` should be an edge from Melanie to a person; instead it is an
entity literally called "Melanie's son". `rm-graph` is starved because the
relations are being spent on entity names.

So one chain produces three of the four bad numbers:

1. extraction emits possessive and compound noun phrases as entity names
2. those names share tokens with the real entity, so the resolver scores them
   as near-matches
3. near-matches land in the review band
4. and the relationship they encode is never emitted as a relation

138 entities, 95 review pairs and 16 relations are one bug, not three.

**A real and separate resolution gap.** `Melanie` vs `Mel` (5.19) and `Caroline`
vs `Caro` (5.51) are true matches the resolver missed — `Mel` is its own entity
with 29 assertions that belong to Melanie. Nicknames are exactly what the
deferred phonetic comparison is for. Small next to the extraction problem: two
pairs of ninety-five.

**Entity kinds are invented freely.** `abstract`, `concept`, `thing`, `object`,
`item`, `value`, `symbol` and `process` are eight near-synonymous kinds holding
23 entities between them. Nothing constrains the vocabulary.

**Refusals, by shape** (run 2, 38 of 419 turns):

```
16x  the model's response was not the JSON this crate asked for
14x  a fact names mention 0, but the response listed 0
 6x  a field was the wrong JSON type (boolean, integer, null, sequence for a string)
 1x  a relation runs from a mention to itself
```

Two thirds are the model failing to produce the requested shape at all. That is
a prompt and schema problem, not a parser problem.

### Where this points

Fix extraction first. It is upstream of the entity count, the review-band load
and the empty relation graph, and no threshold change touches any of them.
Resolution is working on the cases it is given.

### What this does not say

These numbers came from a pipeline that sets the speaker, which `rmem` and
`rmem-mcp` cannot. They are an upper bound on the shipped tools.

Two runs of one conversation with one model. Retrieval is not the quantity
LoCoMo's published baselines report, so the numbers must not be read against
them.


## Third run: salvage, conversation 0, 419 turns

Same corpus and model, after `rm-extract` learned to drop the offending item
instead of refusing the turn (and after the prompt change that preceded it).

| | before the prompt change | prompt, no salvage | prompt + salvage |
|---|---:|---:|---:|
| turns ingested | 381 | 284 | **389** |
| turns refused | 38 (9.1%) | 135 (32%) | **30 (7.2%)** |
| entities | 138 | 89 | 92 |
| review band | 95 | 38 | 48 |
| relations | 16 | 15 | **15** |
| **overall recall@10** | 0.289 | 0.342 | **0.376** |
| temporal | 0.486 | 0.405 | 0.486 |
| single-hop | 0.229 | 0.386 | 0.371 |
| multi-hop | 0.258 | 0.194 | 0.226 |
| open-domain | 0.091 | 0.273 | 0.455 |

### Salvage did what it was predicted to

The prediction was that refusals would fall to around 26 — the count of
responses that were not JSON at all, the one shape with no parsed half to keep.
They fell to 30, of which **28 are exactly that shape**. The rest of the 135
became per-item drops on turns that were otherwise kept.

The overall retrieval figure is the one worth reading. It was stable to within
0.006 across two identical runs, so a move from 0.289 to 0.376 is well outside
that noise. The per-category figures still are not: they swung 7 to 18 points
between identical runs, and open-domain has 11 questions in it.

### And it made the real problem visible instead of fatal

239 items were dropped from turns that were otherwise kept:

```
178x  fact -- it names mention 0, but the response listed 0
 49x  fact -- invalid type: boolean `true`, expected a string
  6x  relation -- it runs from mention 0 to itself
  4x  fact -- an integer where a string belongs
  2x  relation -- it names mention 2, but the response listed 2
```

**The prompt regression is still entirely present.** 178 facts naming a mention
that was not listed is the same failure that cost 76 whole turns before; salvage
converted a catastrophic loss into a large contained one, and did not fix it.
The prompt still tells the model to stop emitting mentions for unnamed groups
without saying what to do with the facts that referred to them.

The 49 booleans are a second, independent shape: a yes/no-flavoured attribute
answered `true` rather than `"true"`. Neither is addressed yet.

### Relations have now not moved four times

17, 16, 15, 15 — across a prompt change that explicitly told the model to prefer
a relation over a possessive, and a salvage change that stopped relations being
discarded with their turns. Six were dropped this run for running from a mention
to itself, which is not enough to explain anything.

The possessive theory is dead. Whatever starves `rm-graph`, none of the work so
far has touched it, and no further guess should be reported as a diagnosis
before it is measured.

### Speed

The extraction pass took **68 seconds**, against roughly twelve minutes in
series. The cache holds 419 completions and 364 embeddings, so a re-run that
does not change the prompt pays for neither.

## Fourth run: the subject and value rules

| | prompt + salvage | + subject/value rules |
|---|---:|---:|
| turns refused | 30 (7.2%) | 32 (7.6%) |
| items dropped | 239 | **184** |
| — booleans where a string belongs | 49 | **0** |
| — fact names a mention that was not listed | 178 | **170** |
| entities | 92 | 78 |
| assertions | 574 | **498** |
| relations | 15 | **13** |
| review band | 48 | 31 |
| **overall recall@10** | **0.376** | **0.329** |

### One rule worked and one did not

**The value rule worked.** 49 booleans became 0, and integers went 4 to 2. Saying
`"value" is a string, or null. Never a number and never true or false` and naming
the shapes the model actually produced removed them.

**The subject rule did not.** 178 to 170 is not a change. The prompt now says
plainly that a fact's subject must be an index into `mentions` and that a turn
with no mentions must emit no facts, and the model writes them anyway. Whatever
is happening, it is not a matter of the instruction being absent — that was the
hypothesis and it is wrong.

### And the change is a net regression on retrieval

0.376 to 0.329. The overall figure varied by 0.006 across two independent runs,
so 0.047 is outside its noise: this is a real move in the wrong direction, on the
one number in this file worth reading.

Assertions fell with it, 574 to 498. **A hypothesis, offered as one:** the new
"emit no facts either" sentence may be suppressing good facts as well as
unanchored ones — a model told to withhold facts when unsure has an easy way to
comply. That is testable by reverting the subject rule alone and keeping the
value rule, which is one extraction pass, and it should be tested rather than
believed.

Nothing here changed relations: 15 to 13. Five runs, no movement.

## Fifth run: the value rule alone

Reverting the subject rule and keeping the value rule separates the two.

| | salvage only | + both rules | **+ value rule only** |
|---|---:|---:|---:|
| **overall recall@10** | 0.376 | 0.329 | **0.389** |
| assertions | 574 | 498 | **607** |
| booleans dropped | 49 | 0 | **0** |
| unanchored facts dropped | 178 | 170 | 195 |
| entities | 92 | 78 | 91 |
| relations | 15 | 13 | **23** |
| turns refused | 30 | 32 | 31 |

### The hypothesis held

Recall recovered to 0.389 and facts stored to 607, both above where they were
before either rule existed. The withholding sentence was the cause: a model told
to emit no facts when it has no mentions complies with the good ones too.

The value rule keeps its gain — booleans stay at 0 — and costs nothing. This is
the best configuration measured, and it is the one in the tree.

0.376 to 0.389 is 0.013 against a noise band of 0.006, so "at least as good" is
the honest claim rather than "better".

### Relations moved for the first time

17, 16, 15, 15, 13, **23**. Every previous run sat in a band of 13 to 17; this
one is half again above it.

**This is not a fix and should not be read as one.** Nothing in this change
targeted relations — the value rule is about a field type, and the reverted rule
was about facts. There is no variance estimate for the relation count, no run has
ever been repeated at a fixed configuration to get one, and a single number
outside a band of five is exactly the shape of result that has already been wrong
twice in this file.

What it does justify is measuring it: two runs at this configuration would say
whether 23 is a real level or a good sample, and that costs one extraction pass
each now that responses are cached.

### The unanchored facts got worse, as expected

178 to 195. The reverted rule was suppressing perhaps a dozen of them, at the
cost of a hundred good facts and 0.047 of recall. They remain the largest single
shape, they remain untouched by two direct instructions, and `extract` continues
to drop them without losing their turns.

## The variance, measured at last — and what it withdraws

Three runs at one configuration, each with a cold cache so each draws a fresh
sample of model responses.

| | run 7 | run a | run b | mean | range |
|---|---:|---:|---:|---:|---:|
| **overall recall@10** | 0.389 | 0.315 | 0.342 | 0.349 | **0.074** |
| relations | 23 | 17 | 14 | 18 | 9 |
| assertions | 607 | 547 | 575 | 576 | 60 |
| entities | 91 | 82 | 88 | 87 | 9 |
| turns refused | 31 | 35 | 38 | 35 | 7 |
| review band | 43 | 30 | 41 | 38 | 13 |

**The overall figure varies by 0.074 at a fixed configuration.** Everything in
this file that compared two single runs of different configurations was reading
that noise.

### What this withdraws

Earlier sections claimed a noise band of 0.006, taken from two runs that
happened to land close together (0.295 and 0.289). Two samples cannot establish
a range; those two were luck, and the number was then used to validate three
conclusions. All three are withdrawn:

- **"0.289 to 0.376 is well outside the noise."** It is roughly one range. Not
  established.
- **"0.376 to 0.329 is a real regression."** It sits inside the range. The
  subject rule may have cost nothing measurable; the revert is still right,
  because the rule did not do the job it was added for, but the retrieval
  argument for it was noise.
- **"Relations moved for the first time: 23."** Relations run 14 to 23 at a
  fixed configuration. The 23 was a sample, not a level. It was recorded as
  "not a fix and should not be read as one" — which was the right instinct and
  is now the measured answer.

### What survives, and why

Two results are structural rather than sampled, and both hold:

- **Salvage: 135 refusals to about 30.** An order-of-magnitude mechanical
  change, far outside a range of 7, and it follows from what the code does
  rather than from what the model happened to say.
- **The value rule: 49 booleans to 0.** Confirmed independently by reading the
  cached responses, not inferred from a run metric.

### The rule this establishes

Retrieval numbers in this file are reported as **mean and range over at least
three cold-cache runs**, or not reported. A single run states nothing about a
configuration, and a difference smaller than 0.074 between single runs states
nothing at all.

Structural counts — refusals by shape, items dropped by shape, what the model
actually emitted — are worth more per run than the retrieval metric, and should
be preferred where a question can be put in those terms.

## Sixth result: the speaker as a mention

Three cold-cache runs before, three after, reported as mean and range per the
rule above.

### The direct test — structural, over ~1,200 responses each

| | before | after |
|---|---:|---:|
| responses listing no mentions | 45% | **1%** |
| responses listing two or more | 15% | **72%** |
| facts with nothing listed (the unanchored shape) | 258 | **0** |
| items dropped from kept turns | 205 | **3** (mean of 3, 4, 2) |

### The consequences

| | before: mean (range) | after: mean (range) |
|---|---:|---:|
| **overall recall@10** | 0.349 (0.315–0.389) | **0.615 (0.591–0.658)** |
| relations | 18 (14–23) | **115 (106–120)** |
| assertions | 576 (547–607) | **1494 (1482–1506)** |
| turns refused | 35 (31–38) | **16 (12–19)** |
| entities | 87 (82–91) | 107 (104–111) |
| review band | 38 (30–43) | 62 (56–66) |

**The ranges do not overlap on any of the first four.** Recall's two ranges are
separated by 0.202 — nearly three times the width of either. This is the first
change in this file whose effect is larger than the noise it is measured
against, and the only one that can be stated without hedging.

### What was actually wrong

The near-empty relation graph was a **ceiling, not a reluctance**. Only 15% of
turns listed two things, and a relation names two mention indices, so 85% of
turns could not carry one however the prompt was worded. Given two mentions the
model related them 26% of the time before and 34% after — barely changed. Five
prompt revisions aimed at persuading it to emit relations were aimed at the
wrong quantity, and a sixth would have been too.

The 258 unanchored facts had the same cause. The model wrote `subject: 0` while
listing nobody, because it treated the speaker as an implicit mention 0. Telling
it a subject must index a mention changed nothing, because it believed it was
indexing one.

One line fixed both: the speaker line now asks for the speaker *as a mention*
rather than only as the referent of "I".

### What it cost

Entities rose 87 to 107 and the review band 38 to 62 — more mentions means more
near-matches to adjudicate. 62 open questions from one conversation is better
than the 95 this file started with and worse than the 38 just before. Whether
those are duplicates worth merging or genuinely distinct entities is a question
for the store, which each run writes, and it has not been asked yet.

### Method note

Every earlier section reasoned from run metrics and got two hypotheses wrong.
This one came from `analyse-cache.py` reading what the model actually said, and
the answer — 45% of responses listing nothing — was not a quantity any run
metric reported. Structural counts first, metrics second.

## The review band, read at last

The previous section ended by saying the band's contents were "a question for
the store, which each run writes, and it has not been asked yet". This asks it,
over four written stores: the three speaker runs and one further run.

The answer is that the band is mostly noise. Roughly three quarters of the
pairs are ones resolution should never have raised.

| store | pairs | kinds differ | possessive | both start a stopword | union |
|---|---|---|---|---|---|
| speaker run 1 | 66 | 37 (56%) | 9 (14%) | 13 (20%) | 47 (71%) |
| speaker run 2 | 56 | 28 (50%) | 10 (18%) | 8 (14%) | 41 (73%) |
| speaker run 3 | 65 | 36 (55%) | 10 (15%) | 5 (8%) | 46 (71%) |
| instrumented run | 99 | 57 (58%) | 12 (12%) | 21 (21%) | 74 (75%) |

Four runs, four different band sizes, and the proportion barely moves: 71–75%.

### Why, mechanically

`Engine::ingest` resolves a mention on `Record::new().with("name", ...)` — one
field, scored by `jaro_winkler`, at `m = 0.9` and `u = 0.01`. Those probabilities
give agreement a weight of `log2(0.9/0.01)` = +6.49 bits and disagreement
`log2(0.1/0.99)` = −3.31, and `Ruleset::score` interpolates between them, so the
score is `-3.31 + 9.80 * similarity` and nothing else. Inverting it: the band —
4.0 to 6.0 bits — is exactly name similarity 0.746 to 0.950.

So every question in the queue is "these two strings are between 75% and 95%
alike". Three things land in that window that are not near-duplicate names:

- **Different kinds** (50–58%). `"the agency" [organisation] ~ "the beach" [place]`,
  `"the kids" [person] ~ "the book" [work]`, `"mentee" [person] ~ "mentors" [thing]`.
  The kind is *recorded on every entity* — `ingest` asserts it as an attribute —
  and resolution never sees it, because the `Record` holds only `name`.
- **Possessives** (12–18%). `"Melanie" ~ "Melanie's son"`,
  `"Caroline" ~ "Caroline's dad"`, `"you" ~ "your son"`. A name of the form
  *X's Y* contains all of X, so Jaro-Winkler scores it high, and the Winkler
  prefix bonus is precisely a bonus for that shared prefix. These are guaranteed
  non-matches — an entity named by its relation to X is definitionally not X.
- **Shared leading stopword** (8–21%). `"the beach" ~ "the book"`,
  `"Pride fest" ~ "priceless items"`. `[[resolution.blocking]] prefix n = 3`
  buckets every name beginning "the" together — "the" is three characters — and
  Winkler then rewards the same prefix a second time in the score.

The largest blocking buckets in these stores are `lgb` (10–12 entities), `the`
(6–10) and `mel` (5–6). A bucket of 12 is 66 comparisons among things whose only
established commonality is three characters.

### The obvious fix is a trap

`kind` is right there, so adding it as a second field looks like a one-line
config change. Measured against the real comparator (`Ruleset::score`, not
arithmetic on paper), with `Exact`, `m = 0.9` and `u = 0.38` — `u` estimated from
these stores as the rate at which two *blocked* entities happen to share a kind:

| pair | name only | with kind |
|---|---|---|
| `"the agency" [organisation] ~ "the beach" [place]` | 5.19 Review | 2.56 NonMatch |
| `"the kids" [person] ~ "the book" [work]` | 5.02 Review | 2.39 NonMatch |
| `"Mel" [person] ~ "Melanie" [person]` | 5.19 Review | 6.43 **Match** |
| `"Caroline" [person] ~ "Caroline's dad" [person]` | 5.74 Review | 6.98 **Match** |
| `"Melanie" [person] ~ "Melanie's son" [person]` | 5.68 Review | 6.92 **Match** |

It clears out the kind-mismatch half exactly as intended. It also promotes the
possessive pairs — which are same-kind, person against person — from a question
someone would have answered "no" into a silent automatic merge. Caroline would
be merged with her father.

That is worse than the noise it removes. A cluttered queue wastes attention; a
false merge corrupts the store and there is no signal that it happened. So the
kind field cannot ship on its own: it needs the possessive case handled first,
and that belongs either in extraction (which already forbids possessive names
and does not achieve it) or in a comparator that treats *X's Y* as disqualifying
rather than as 90% similar.

Both are real changes with their own measurements to make. Neither is made here.

### What this run does change

Two things, both about being able to see the band rather than about resolution:

- The harness prints every pair with both names and both kinds, and counts the
  kind disagreements. `review band 99 pairs` reads identically whether the queue
  is a working backlog or garbage; the pairs do not.
- `rmem review` and the MCP `review` tool print the names and kinds too. They
  used to emit `review 0  entity 0 vs entity 3  (5.19 bits)`, which is not a
  question anyone can answer — it cost two `about` calls per pair to find out
  what was even being asked. It now reads
  `review 0  "Melanie" [person] (entity 0)  vs  "Mel" [person] (entity 3)`.

The second is the more useful of the two. The review band is the project's
answer to "refuse rather than guess", and the refusal was being handed over in a
form nobody could act on.

## The possessive guard, measured

The previous section named three causes and shipped none of the fixes, because
the obvious one — adding `kind` as a second field — promotes the possessive
pairs from questions into silent merges. This removes the possessive pairs, so
that route is no longer blocked.

`Comparator::PossessiveAware` splits a name into what it belongs to and what it
is — `Melanie's son` is `(Melanie, son)`, `your kids` is `(your, kids)`, and a
plain name is `(nothing, itself)` — then compares owners with owners, heads with
heads, and takes the weaker of the two. Both halves must agree.

Keeping the owner is the part that is easy to get wrong. Comparing heads alone
would score `"Melanie's son"` against `"Caroline's son"` at 1.0 and merge two
different children.

### The run

Conversation 0 again, 402 turns ingested. The extraction cache was hit 2346
times and missed **zero**, so the model output was byte-identical to the run in
the previous section: this is the same extractions through a different resolver,
not two samples of a noisy process.

| | jaro_winkler | possessive_aware |
|---|---|---|
| entities | 124 | 124 |
| assertions | 1508 | 1508 |
| relations | 104 | 104 |
| **review band** | **99** | **86** |
| recall@10 overall | 0.617 | 0.617 |
| single-hop | 0.586 | 0.586 |
| multi-hop | 0.516 | 0.516 |
| temporal | 0.757 | 0.757 |
| open-domain | 0.636 | 0.636 |

Comparing the two written stores field by field: `store`, `index`, `identity`,
`assertions` and `rejected` are **byte-identical**. Only `review` differs.

Nothing merged differently. Thirteen questions stopped being asked, and every
other thing the run produced is the same object.

### The thirteen

```
"Melanie" [person]          ~  "Melanie's son" [person]
"Melanie" [person]          ~  "Melanie's husband" [person]
"Melanie" [person]          ~  "Melanie's kids" [person]
"Mel" [person]              ~  "Melanie's son" [person]
"Mel" [person]              ~  "Melanie's husband" [person]
"Mel" [person]              ~  "Melanie's kids" [person]
"Melanie's husband" [person] ~ "Melanie's son" [person]
"Melanie's husband" [person] ~ "Melanie's kids" [person]
"Melanie's kids" [person]   ~  "Melanie's son" [person]
"Caroline" [person]         ~  "Caroline's paintings" [work]
"Caro" [person]             ~  "Caroline's paintings" [work]
"kids" [thing]              ~  "kids' books" [thing]
"you" [person]              ~  "your son" [person]
```

Every one is a thing against what it belongs to, or two things belonging to the
same person. None is a near miss. No pair was *added*.

### Why one run is enough here, having insisted three are not

This file established that a single run cannot support a claim, after asserting
a ±0.006 noise band from n=2 that turned out to be 0.074. That rule stands, and
this is not an exception to it — the rule is about averaging out a stochastic
process, and there is no stochastic process in this comparison. The cache
returned every extraction verbatim, so the input was fixed; the resulting store
was byte-identical, so the output was fixed too. Re-running would produce the
same two files.

The claim that needs the caution is the *generalisation* — that 13 of 99 is what
this rule is worth. That is one conversation, two speakers, and one extractor's
habits. What replicates is the mechanism, not the count.

### What it does not fix

The 86 remaining pairs are still mostly noise: about 55% disagree on kind, and
the shared-stopword and shared-topic collisions (`"the beach" ~ "the book"`,
the `LGBTQ *` family) are untouched — those names own nothing, so this rule is
by construction a no-op on them.

What has changed is that `kind` as a second field is no longer a trap. The pairs
that adding it would have promoted into silent merges are the pairs this
removed. That measurement has not been run.

## The kind field, and what it cost to do honestly

The previous two sections established that over half the review band was pairs
whose kinds already disagreed, and that `kind` is asserted on every entity and
withheld from the thing that decides identity. This gives it to the resolver.

### The parameters said no

`Record` now carries `name` and `kind`, and the ruleset scores `kind` with
`exact`, `m = 0.9`, `u = 0.38` — `u` measured across four stores as the rate at
which two entities sharing a name prefix, which is the set blocking actually
compares, happen to share a kind.

Adding that field alone made things worse, and the test suite said so before the
benchmark did. Three tests failed on a fixture of `"Ben Severn"` against
`"Ben Sanderson"` — two different people, both `person` — which stopped being a
question and became an automatic merge. Agreement on kind is worth
`log2(0.9/0.38)` = +1.24 bits, and that was enough to push it over `match_at`.

The tempting response is to raise `u` until the answers look right. At `u = 0.7`
everything behaves. But `u` is a measured quantity, and 0.7 is not what it
measured — that is fitting a parameter to a conclusion.

The real problem is elsewhere. `name` contributes up to 6.49 bits on its own,
and `review_at = 4.0` / `match_at = 6.0` were calibrated when it was the only
field. *Any* second field with positive agreement weight shifts every pair up.
The thresholds were wrong, not the probabilities.

### Raising both thresholds by exactly the agreement weight

`review_at` 4.0 → 5.2439 and `match_at` 6.0 → 7.2439. Two consequences, both
asserted by a test:

- A pair whose kinds **agree** scores 1.2439 more against thresholds 1.2439
  higher, so it is decided exactly as it was before. The change is a no-op on
  it.
- A pair whose kinds **differ** can never be asked about. A name contributes at
  most 6.49 bits, a kind disagreement costs 2.63, and 6.49 − 2.63 = 3.86 is
  below `review_at`. "Paris" the city is not "Paris" the person and no spelling
  makes it so.

The second is a threshold policy, not something the probabilities imply: `m =
0.9` says one true match in ten disagrees on kind, and the veto discards those.
Lower the two thresholds and it becomes a penalty again.

### The run

Same corpus, same 402 turns, cache hit 2346 times and missed zero — so again one
set of extractions through two resolvers.

| | possessive_aware only | + kind |
|---|---|---|
| entities | 124 | 125 |
| assertions | 1508 | 1508 |
| relations | 104 | 104 |
| **review band** | **86** | **31** |
| — of which kinds differ | 55 | **0** |
| recall@10 overall | 0.617 | 0.617 |
| single-hop | 0.586 | 0.586 |
| multi-hop | 0.516 | 0.516 |
| temporal | 0.757 | 0.757 |
| open-domain | 0.636 | 0.636 |

The band is down 64% from the previous section and 69% from where #17 measured
it, and not one remaining pair disagrees on kind.

Unlike the possessive run, the store here genuinely differs — `store`,
`identity` and `assertions` all changed. The identical recall is therefore a
measurement rather than a consequence of nothing having moved.

### The one entity that changed, which is the interesting part

Entities went up by one, because something that used to merge no longer does.

Before, one entity `Oliver` held `kind: [animal, animal, person, animal]` — a
record contradicting itself — with both "hid a bone in a slipper" and
`favorite_food: parsley, veggies`. After, there are two: `Oliver [animal]` with
the bone, and `Oliver [person]` with the parsley.

The corpus says Oliver is Melanie's cat. The parsley is not his: that turn is
Caroline answering "can you show me one of Oscar?" with "check out this pic of
him eating parsley", so the food belongs to Oscar, her guinea pig. Extraction
misread the subject *and* the kind on one turn.

So the extra entity is not a true match being split. It is a phantom being kept
out of a real entity's record — the cat no longer eats parsley. The extraction
error is still there and this change does not fix it; what changed is that it no
longer contaminates something correct.

That is the veto's cost and its benefit in one example. It cannot tell a
mislabelled kind from a genuinely different one, so it acts on both. Here that
happened to be right. On `"pets" [animal]` against `"pet" [thing]` — the same
thing, kinded differently on two runs — it will be wrong, and those two will
never be offered for merging.

### What is left

Thirty-one pairs, none of them a kind disagreement. By eye, roughly half are
worth asking — `"Mel" ~ "Mell"`, `"Caroline" ~ "Caro"`,
`"adoption agency" ~ "adoption advice/assistance group"`, the several spellings
of the pride parade — and roughly half are the shared-prefix collisions this
file has named twice and not addressed: `"the beach" ~ "the café"`,
`"concert" ~ "connection"`, `"some people" ~ "some pretty cool stuff"`, and the
`LGBTQ *` family, which alone is a third of what remains.

That is the next thing, and it is a blocking problem rather than a scoring one:
`prefix n = 3` puts every name beginning "the" or "LGB" in one bucket, and
Jaro-Winkler then rewards the same prefix a second time in the score.

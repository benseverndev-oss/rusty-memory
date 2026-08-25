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

## Two gaps this hit before it could run at all — both now closed

`rm-providers` could not reach a network whose egress is a proxy:

- `ureq` 2's free functions (`ureq::post`) do not read `HTTPS_PROXY`. Only an
  `Agent` built with a `Proxy` does.
- The `tls` feature is rustls with webpki-roots, which does not read
  `SSL_CERT_FILE`, so a proxy substituting its own certificate is rejected.

Every corporate network and most CI sandboxes are one or both of those, so this
was a deployment limitation rather than a quirk of one machine. For most of this
file's history the workaround was `proxy-shim.py`, a plain-HTTP forwarder on
localhost with `LOCOMO_BASE_URL` pointing at it.

`rm_providers::network` fixed it, and the shim is gone. Measured on this
machine, against the real `api.openai.com`, as a 2×2:

| | webpki roots | `SSL_CERT_FILE` bundle |
|---|---|---|
| direct | **fails** | works |
| through the proxy | works | works |

The failing cell is exactly what the old code did. Direct connections here are
transparently intercepted and answered with a substituted certificate that no
Mozilla root vouches for; the proxy's `CONNECT` tunnel instead carries end-to-end
TLS to OpenAI, whose certificate the built-in roots do vouch for. So on this
machine either half of the fix is sufficient on its own — which is luck, not
design. A proxy that terminates TLS itself, which is the corporate norm and what
the sandbox README describes, needs the CA half too.

`LOCOMO_BASE_URL` survives the shim's deletion, because pointing the harness at
a local model is worth being able to do for its own sake.

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

## The attribute vocabulary, and why the temporal machinery never runs

Found while looking for something else. The search was for a suspected
extraction bug -- possessive turns collapsing onto the owner and misattributing
the fact -- and that bug is not what the corpus shows. Conversation 0 contains
thirty turns naming an unnamed relative ("my kids", "the kids", "my husband"),
and in twenty-three of them every fact landed on the speaker. That is the
prompt's unnamed-group rule working as written: *say what the turn says about
them as a fact about someone who is named*. The names it produces --
`children_excitement`, `kids_experience` -- put the relationship in the
attribute name.

Which turned out to be the thread worth pulling.

`analyse-store.py` reads a written snapshot and counts what is in it. Over
conversation 0:

```
entities            125
assertions          735   (excluding `kind`)
distinct attributes 498
used exactly once   408  (82% of names)
assertions per name 1.48

attributes with more than one version: 82 of 550  (15%)
  assertions inside them: 267 of 735  (36%)
```

Supersession, survivorship, valid intervals and `about` all operate *within one
attribute name on one entity*. A later fact only contradicts an earlier one if
both were recorded under the same name. So that 15% is the entire surface on
which any of this project's temporal machinery can act. The other 85% is inert
by construction.

Nothing in five runs of a retrieval metric could have shown this. Recall is
embedding search over a fact's own text and never reads the attribute name.

### It fails in both directions at once

Too many names, and nothing ever meets: `feeling`, `emotion`,
`emotional_response`, `feeling_about_art` and `emotional_impact` are five names
for one idea. Seventeen names share the stem `suppor*`, twelve share `feelin*`.

Too few names where they do meet, and unrelated facts are forced into one slot.
`entity 0` has fourteen versions of `feeling`: *happy*, *thankful*, *love for
horses*, *liberated and empowered*, *peace and serenity*, *alone*. Those are
fourteen moments, not fourteen claims about one thing. Driven through the built
binary against that store:

```
$ rmem about 0 feeling
survivorship refused: 2 different values share the latest observation time
(1697193060000); simultaneous contradictory assertions have no "most recent".

$ rmem about 0 experience
amazing journey                     # one of nine; the other eight unreachable

$ rmem about 0 goal
having a family                     # one of six
```

The refusal is the store behaving correctly on data it should never have been
given. `experience` is the quieter failure: eight facts are still in the log and
`about` will not return them, because the ninth superseded them for sharing a
name they should not have shared.

### The cause is a gap in the prompt

`kind` is a closed vocabulary of seven. `value` is a string or null. `days_ago`
is a whole number of days or null. The attribute name has no rule at all -- not
a vocabulary, not a preference for reuse, not even a sentence saying what an
attribute *is*. The model invents one per fact, which is the only thing it can
do when nothing has been asked of it.

That is the next thing to change, and it needs a measurement recall@10 cannot
give: the counts above, before and after. `analyse-store.py` exists so that
measurement has somewhere to come from.

## A second conversation, and what it settles

Everything above was measured on conversation 0. The corpus holds ten, and the
attempt to run all of them is written up in the next section -- it did not
finish. Two conversations completed cleanly, and two is not ten but it is twice
what every threshold in `rmem.toml` was tuned on.

| | conv 0 (Caroline & Melanie) | conv 1 (Jon & Gina) |
|---|---|---|
| turns ingested | 402 of 419 | 350 of 369 |
| turns refused | 17 (4%) | 19 (5%) |
| entities | 125 | 92 |
| assertions | 1508 | 1219 |
| relations | 104 | 69 |
| review band | 31 | 28 |
| recall@10 overall | 0.617 | 0.691 |

Conversation 2 ran during the failure described below -- 469 of its 663 turns
refused, on "the connection did not establish" -- so it ingested 29% of its
corpus and is excluded. Its numbers are consistent with the others, which is
worth nothing: a 29% sample agreeing with a 96% sample is not evidence.

### What replicates

**The attribute sprawl, almost exactly.**

| | conv 0 | conv 1 | conv 2 (partial) |
|---|---|---|---|
| distinct attribute names | 498 | 389 | 203 |
| used exactly once | **82%** | **82%** | 84% |
| assertions per name | 1.48 | 1.50 | 1.36 |
| attributes with >1 version | 15% | 18% | 12% |

Two independent conversations, four speakers, different subject matter, and the
singleton rate is 82% in both. This is not an artefact of one transcript.

Conversation 1 also shows the same collapse at the other end: `entity 1` has
sixteen versions of `determination` -- *determined to make it work*, *make it
work*, *persistent*, *not giving up*, *will not quit* -- which are restatements
of one thing rather than a value that changed sixteen times. One of them is the
string `"true"`, which the prompt forbids in as many words.

**The two resolution fixes hold.** Neither conversation's review band contains a
single pair whose kinds disagree, or a single possessive pair. Before those
changes, conversation 0's band was 57 kind-mismatches and 12 possessives out of
99.

### What does not replicate, and what got worse

The shared-prefix collisions this file has now named three times are a *larger*
share of conversation 1's band than of conversation 0's: 11 of 28 pairs against
7 of 31. `prefix n = 3` blocks on the first three characters of a name, so how
much damage it does depends entirely on what the speakers happened to talk
about. That is the argument for fixing it rather than tuning around it.

Recall differs by 0.074 between the two conversations, which is exactly the
range this file measured *within* one configuration on one conversation. So the
two numbers are not distinguishable and nothing should be read into the
difference.

## Seven conversations

The daily request quota reset and the remaining conversations were run in the
foreground, a few hundred turns per invocation, resuming from the cache each
time. Seven of ten completed cleanly: **3,657 turns ingested, fourteen
speakers.** Conversations 2, 4 and 6 remain, blocked only by the quota.

| conv | ingested | refused | entities | attrs | once | >1 ver | band | kind≠ | stopword | recall@10 |
|---|---|---|---|---|---|---|---|---|---|---|
| 0 | 402 | 4% | 125 | 498 | 82% | 15% | 31 | 0 | 7 | 0.617 |
| 1 | 350 | 5% | 92 | 389 | 82% | 18% | 28 | 0 | 11 | 0.691 |
| 3 | 591 | 6% | 192 | 544 | 83% | 14% | 50 | 0 | 3 | 0.553 |
| 5 | 639 | 5% | 257 | 562 | 83% | 14% | 99 | 0 | 21 | 0.675 |
| 7 | 658 | 3% | 224 | 565 | 82% | 16% | 42 | 0 | 9 | 0.707 |
| 8 | 482 | 5% | 119 | 490 | 80% | 16% | 10 | 0 | 1 | 0.686 |
| 9 | 535 | 6% | 197 | 592 | 80% | 15% | 68 | 0 | 17 | 0.626 |

### The attribute sprawl is established

**81.8% of attribute names are used exactly once** — 2,978 of 3,640 — and no
conversation departs from the band 80–83%. The share of attributes carrying more
than one version sits at 14–18% everywhere.

Seven independent conversations, fourteen speakers, subject matter ranging from
adoption to pottery to a recording studio, and the number does not move. This is
a property of the extraction contract, not of any transcript. Whatever the
bi-temporal machinery is worth, it is currently reachable on about a sixth of
what the store holds.

### Both resolution fixes hold everywhere

Not one pair in any of the seven bands disagrees on kind. Before #19,
conversation 0 alone had 57 of 99.

The crude classifier used above flags two pairs in conversation 7 as possessive,
and both are the comparator being right rather than wrong:

```
5.78  "Deborah's mom" ~ "Deborah's mum"        same owner, and the heads are the same word
6.70  "Jolene's partner" ~ "Jolene's parents"  string-similar heads, below match_at: asked, not merged
```

`PossessiveAware` is built to let same-owner pairs through when their heads
agree, which is exactly what the first of those is. Zero genuine failures in
seven conversations.

### What varies, and by how much

The band ranges from 10 pairs to 99 — a factor of ten — and the stopword
collisions inside it from 1 to 21. Conversation 8 produced a band of ten
questions from 482 turns; conversation 5 produced ninety-nine from 639. The
blocking key is doing wildly different amounts of damage depending on what the
speakers happened to call things, which is the strongest argument yet for
fixing it rather than tuning thresholds around it.

Recall ranges 0.553 to 0.707. That spread of 0.154 is twice the 0.074 this file
measured *within* one configuration, so unlike the two-conversation comparison
earlier, some of this is real: conversations differ in how answerable they are.
It is not a measurement of anything this project changed.

## The article rule, and a comparison that had to be thrown away

Three times this file called the shared-prefix collisions "a blocking problem
rather than a scoring one". That was wrong, and the reason is structural:
blocking is *disjunctive* -- a pair is compared if it shares **any** key -- so
no blocking change can remove a pair from the band. It can only add. The fix
had to be in the comparator.

Measured over every pair in seven conversations' review bands, 69 of 328 (21%)
were pairs whose names *both* began with an article:

```
6.80  "the car" ~ "the crowd"        6.76  "the park" ~ "the lake"
6.76  "the view" ~ "the idea"        6.71  "the store" ~ "the studio"
```

Scoring 6.5 to 6.9 on the strength of a word that says nothing about which
thing is meant. None of the 69 had a possessive determiner on both sides, so
articles (`the`, `a`, `an`) can be stripped while `my`/`your`/`their` stay
owners -- `"my kids"` and `"your kids"` are different children.

### What it costs

Two of the 69 are lost: `"the whole gang" ~ "the gang"` and
`"the main stage" ~ "the stage"`. Once the article is gone these are compared
from the front, and Jaro-Winkler rewards a shared *prefix*, so an extra word at
the end survives (`"the event next month" ~ "the event"`, 0.863; `"the car" ~
"the car Dave is restoring"`, 0.800) and an extra word at the start does not.
That asymmetry is an artefact of the comparator rather than a judgement about
names, and it is pinned by a test so it is a known price rather than a
surprise.

Sixty-seven coincidences removed against two questions lost. A lost question is
the safe direction: the two entities stay apart, which is what they already
were, and nothing in the store is corrupted. A kept coincidence costs attention
every time someone reads the queue.

### Measured

| conv | band | entities | recall@10 |
|---|---|---|---|
| 0 | 31 → 24 | 125 → 125 | 0.617 → 0.617 |
| 1 | 25 → 23 | 90 → 90 | 0.679 → 0.679 |
| 8 | 10 → 9 | 119 → 119 | 0.686 → 0.686 |

Every assertion identical on both sides of all three, so this is one set of
extractions through two resolvers. The queue shrinks and nothing else moves.

### The comparison that was thrown away

The first attempt at conversation 1 reported entities falling 92 to 90 and
recall falling 0.691 to 0.679, and both were artefacts. The "before" store had
been written the previous day against a different cache file, and comparing
assertion contents showed only **732 of 1219 identical** -- two different sets
of extractions, not two resolvers over one.

Re-run against the same cache, conversation 1's entities do not move at all.
The check that caught it is cheap and worth repeating on any before/after here:
if the two stores do not share ~100% of their `(attribute, value)` pairs, they
are not measuring what they appear to measure.

## All ten

Conversations 2, 4 and 6 completed once the daily request quota reset.
Conversation 2's earlier partial run -- 194 of 663 turns, excluded above -- is
replaced by a clean one at 637.

| conv | ingested | refused | entities | attrs | once | >1 ver | band | recall@10 |
|---|---|---|---|---|---|---|---|---|
| 0 | 402 | 4% | 125 | 498 | 82% | 15% | 31 | 0.617 |
| 1 | 350 | 5% | 92 | 389 | 82% | 18% | 28 | 0.691 |
| 2 | 637 | 4% | 194 | 652 | 80% | 18% | 21 | 0.697 |
| 3 | 591 | 6% | 192 | 544 | 83% | 14% | 50 | 0.553 |
| 4 | 653 | 4% | 321 | 615 | 80% | 15% | 118 | 0.729 |
| 5 | 639 | 5% | 257 | 562 | 83% | 14% | 99 | 0.675 |
| 6 | 666 | 3% | 263 | 634 | 82% | 13% | 50 | 0.711 |
| 7 | 658 | 3% | 224 | 565 | 82% | 16% | 42 | 0.707 |
| 8 | 482 | 5% | 119 | 490 | 80% | 16% | 10 | 0.686 |
| 9 | 535 | 6% | 197 | 592 | 80% | 15% | 68 | 0.626 |

**5,613 turns, twenty speakers, ten conversations.**

**81.3% of attribute names are used exactly once** — 4,506 of 5,541 — and the
range across ten conversations is 80% to 83%. The share of attributes carrying
more than one version is 13% to 18%. Whatever the bi-temporal machinery is
worth, it is reachable on about a sixth of what the store holds, everywhere,
and that is now measured on the whole corpus rather than argued from one
transcript.

One inconsistency in the band column, stated rather than smoothed over:
conversations 2, 4 and 6 were run after the article rule landed and the other
seven before it. The rule changes only which pairs are asked about — entities
and assertions were measured unchanged — so every other column is comparable
and the band column is not, by roughly the 15% the rule removes.

The band still ranges from 10 pairs to 118, a factor of twelve, on corpora of
comparable size. Recall ranges 0.553 to 0.729. Neither is a measurement of
anything this project changed; both are measurements of how much the
conversations differ.

## The attribute rule: measured, and withdrawn

The prompt says what `kind` may be and what `value` may be, and had never said
what an *attribute name* is. A rule was added asking for the plainest reusable
name and forbidding names built out of the value — `enjoyment_of_grand_canyon`
is `enjoyment`, `kids_experience` is `experience` — and measured on
conversation 1 against its own baseline.

| | before | after |
|---|---|---|
| distinct attribute names | 389 | **187** |
| assertions per name | 1.50 | **3.05** |
| used exactly once | 82% | 77% |
| assertions inside multi-version attributes | 40% | **71%** |
| entities | 92 | 115 |
| recall@10 | 0.691 | 0.667 |

Read as a table it looks like a win: the vocabulary halved, reuse doubled, and
the surface the temporal machinery can act on went from 40% of assertions to
71%. It is not a win, and the store says why:

```
entity 1  goal     86 versions: 'starting my own business', 'to share dancing
                   with others', 'start a dance studio', 'spread intensity...'
entity 1  feeling  71 versions: 'excited', 'amazing', 'glad', 'positive'...
```

Eighty-six goals in one slot. Those are not successive values of one goal that
supersede each other; they are eighty-six different goals, and `most_recent`
returns exactly one of them. The rule traded a store where nothing ever met for
a store where unrelated things are forced together, and "71% of assertions are
inside a contested attribute" counts both alike. **The metric could not tell a
value that changed from two facts colliding**, which is why it read as success.

The change is reverted. Two prompt changes in this project have now been
measured and withdrawn, and the rule that keeps being confirmed is that a
plausible prompt improvement is worth nothing until the artefact is read.

### What it actually established

The attribute name is being asked to do two jobs at once:

- say what **kind** of fact this is, so that a later fact about the same thing
  finds the earlier one;
- say **which** fact this is, so that unrelated facts do not collide.

One string cannot do both, and every version of the prompt trades one failure
for the other. Loosen it and nothing ever supersedes; tighten it and unrelated
facts share a slot.

That is not a prompt problem. It is the store's grain: `feeling` and `goal` are
not single-valued attributes where a later value replaces an earlier one, they
are *accumulating* ones where a later value is an additional observation.
`rm-survivor` already distinguishes these — `valid_interval` keeps disjoint
spans rather than picking a winner, and `two_employers_at_once_both_stand` is a
test that exists — and `rmem.toml` already carries per-attribute policy. What
is missing is anything that decides *which* attributes are which, and nothing
in the pipeline currently asks.

That is the next thing worth designing, and it is a design question rather than
a wording one.

## The baseline nobody had run

Every result above compares the store against an earlier version of itself. This
compares it against not having one.

The pipeline embeds an *extracted fact* — one vector per assertion, over a short
sentence the model wrote to state that fact on its own. The obvious control is
to skip all of it: embed the raw turn text, `"<speaker>: <text>"`, one vector per
turn, and search those. No completions, no entities, no resolution. Twenty lines.

Scored the way everything else here is scored, and made fair by taking **k
distinct turns from each side** — the store returns several assertions per turn,
so comparing ten assertions against ten turns would flatter the turns:

| conversation | questions | extracted facts | raw turns | delta |
|---|---|---|---|---|
| 0 | 149 | 0.685 | **0.799** | +0.114 |
| 1 | 81 | 0.728 | **0.778** | +0.049 |
| 2 | 152 | 0.717 | **0.796** | +0.079 |
| **pooled** | **382** | **0.707** | **0.793** | **+0.086** |

Never negative, on any conversation. The 0.707 is the number this project has
been reporting all along, which is the check that the comparison is set up right.

They are complementary — at k=10, raw turns find 53 questions the store misses
and the store finds 20 the raw turns miss — so the obvious next move is to keep
both. It does not work:

| method | recall@10 turns |
|---|---|
| extracted only | 0.707 |
| **raw turns only** | **0.793** |
| interleave | 0.791 |
| reciprocal rank fusion (k=10) | 0.780 |
| reciprocal rank fusion (k=60) | 0.759 |

Nothing beats the raw turns alone. The extracted-fact retriever contributes no
recall that the turn text does not already carry.

### What this does and does not say

It does **not** say the store is pointless, and reading it that way would be a
category error. Raw turns cannot answer `about(entity, attribute, valid_t,
tx_t)`. They cannot say a fact was corrected, or that two names are one person,
or what was believed last Tuesday about last May. None of that is retrieval and
none of it is in this number.

What it says is narrower and still bad: **finding the right turn is not a job
the assertion index does well**, and it has been the project's headline number.
Three prompt changes, a subject boost, a reranking sweep and a hybrid lexical
retriever were all measured against a baseline that a twenty-line control beats.

The reading that fits the evidence is that these are two different jobs sharing
one index. Recall wants the turn; `about` wants the resolved, superseded,
bi-temporal assertion. The assertion index is very good at the second and is
being asked to do the first.

### Ruled out on the way, each with its own measurement

Read off the same 149 questions, simulated offline over a dumped top-200 so each
policy costs nothing to test. Control is 0.671 (recall@10 over assertions, which
is what the harness reports — the 0.707 above is the same runs re-scored over
distinct turns, and the two are not interchangeable).

| policy | recall@10 | delta |
|---|---|---|
| control | 0.671 | — |
| drop tombstones (`value = None`) | 0.577 | −0.094 |
| drop `kind` assertions | 0.678 | +0.007 |
| drop tombstones and `kind` | 0.584 | −0.087 |
| one hit per entity | 0.450 | −0.181 |
| two hits per entity | 0.497 | −0.134 |
| one hit per turn | 0.638 | +0.007 |

Two of these were worth the trouble of being wrong about.

**Tombstones are not dead weight.** 17% of every hit handed back has a null value
— `family_moments = None`, `interest_in_art = None` — which reads like the model
using null as a flag rather than as the "this attribute has no value" the prompt
asks for. They answer nothing on their own. Dropping them costs **0.094**,
because an assertion is a pointer to the turn it came from whether or not it
carries a value, and the metric scores turns.

**The result set is not redundant.** The top ten averages **9.0 distinct turns**,
so there is no crowding to squeeze out: deduplicating by turn gains 0.007. The
answer is not being pushed out by near-duplicates of itself. It simply ranks
below ten genuinely different wrong turns — median rank **30**, with 78% of
missed answers somewhere in the top 200.

### A confound worth naming

`demote_replaced` runs over whatever `k` returned, so demoting inside a top-10
and demoting inside a top-200 give different top-tens. Taking ten from a demoted
200 scores **0.631** against the pipeline's **0.671** — the demotion has more
material to work with and uses it worse. Not how the pipeline runs, so not a
result about shipped behaviour, but it invalidates any simulation that dumps
deep and slices shallow. Sorting the dumped list back into pure similarity order
reproduces 0.671 exactly, and that is the control every table above uses.

## Can the store tell when it has no answer?

LoCoMo's category 5 is adversarial: the premise is unsupported and the corpus
cannot answer. That makes it a labelled set for a question the store has never
been able to answer — **382 answerable against 112 unanswerable**, over three
conversations, read off the same dumped top-200 lists as everything else here.

Six candidate signals, scored by Youden's J against that label:

| signal | best J | at cutoff | answerable kept | unanswerable refused |
|---|---|---|---|---|
| **top score** | **0.494** | 0.7058 | 62.8% | 86.6% |
| mean of the top 5 | 0.473 | 0.6372 | 65.2% | 82.1% |
| top − mean(all 200) | 0.378 | 0.1785 | 73.6% | 64.3% |
| top − 10th | 0.246 | 0.1488 | 39.8% | 84.8% |
| top − mean(2..10) | 0.217 | 0.1163 | 36.9% | 84.8% |
| top ÷ mean(2..10) | 0.187 | 1.2124 | 33.0% | 85.7% |

The raw score wins and every shape-based signal loses, which is the opposite of
the obvious guess — that an answerable query has a *peak* and an unanswerable
one is flat. It does not; it is simply lower everywhere.

### J hides the trade, and the trade is bad

| keep this much of answerable | refuses this much of unanswerable |
|---|---|
| 99% | 4.5% |
| 95% | 14.3% |
| 90% | 36.6% |
| 62.8% (best J) | 86.6% |

To drop enough unanswerable queries to matter you throw away between a tenth
and a third of real answers. **So nothing is dropped.** `[retrieval] weak_below`
labels the answer instead: every hit still comes back, with a line saying how
near the nearest one actually was.

### And the cutoff does not travel

0.62 was taken off that table and tried on this project's own decision log,
seeded by `docs/seed-decision-log.sh`. It marked *"should we add a reranker"* —
a question with a perfect answer, the rejected reranking decision at 0.531 —
as having nothing near it. Same embedding model, different corpus, and the
scale moved out from under the number. On that log the equivalent bar is
around 0.35.

So the default is **0.0, off**. A cutoff that has not been measured against the
corpus it will run on produces confident warnings on good answers, which is
worse than the silence it was meant to fix. The mechanism ships; the number
does not.

This is the fourth thing measured here and left switched off, and the pattern
across all four is the same: the signal was real, and too weak to act on
without doing damage somewhere else.

## Owning the embedder: built, measured, not enough

Every vector in this workspace came from a remote service — a per-call cost, a
network dependency, an API key, and for a memory store, every fact you record
leaving the machine. `rm-embed` is the first implementation of the `Embedder`
port that opens no socket: subword hashing, about a hundred lines of
arithmetic, no model file at all.

Built first because it is the cheapest thing that could work. A distilled
static table — real vectors from a real model, looked up and averaged — has
semantics and costs tens of megabytes of weights in the repository, and there
is no point paying that until the free thing has been shown not to be enough.

### It is not enough

The decision log seeded by `docs/seed-decision-log.sh`, 31 decisions, indexed
twice: once through `text-embedding-3-small`, once locally, using `rmem reindex`
to swap without re-recording anything.

**Asked for each decision by its own exact title**, both are perfect:

| | rank-1 | in top 3 |
|---|---|---|
| local | 31/31 | 31/31 |
| OpenAI | 31/31 | 31/31 |

Which is the wrong test, and worth keeping only to say so: a query that reuses a
title's words is the case subword hashing is *built* to win, and a test both
methods ace measures nothing.

**Asked in different words** — twelve paraphrases that deliberately avoid the
title's vocabulary — they come apart:

| | rank-1 | in top 3 |
|---|---|---|
| local | **6/12** | 8/12 |
| OpenAI | **10/12** | 11/12 |

Roughly half. And the failures are exactly the predicted shape:

```
"does the extractor know who is talking"     -> missed "Let a caller say who is speaking"
"how do we know two names are the same"      -> missed "Resolve identity with Fellegi-Sunter"
"should we score candidates a second time"   -> missed "Rerank the recall results"
```

Nothing lexical connects *talking* to *speaking*, or *the same person* to
*Fellegi-Sunter*. The crate's own test says so in as many words — `car` and
`automobile` are asserted to land orthogonal — and this is that assertion
costing something real.

One instructive near-miss: asked *"should we add a reranker to improve
retrieval"*, local returns **Hybrid lexical retrieval**, because that title
contains *retrieval* exactly while *reranker* only shares stem letters with
*Rerank*. An exact word beats a near one, every time, which is the whole
difference between matching and understanding.

### What it is good for anyway

It ships, defaulted off, for two reasons that are not recall quality.

It makes the decision path **fully offline**: `decide`, `recall`, `reindex` and
`decisions` need no credential and open no socket, because `decide` reaches no
completion model either. 100 assertions were re-embedded in a shell with no
`OPENAI_API_KEY` set, which is how that claim was checked.

And it makes this measurement possible at all. Comparing two embedders on one
corpus needs `rmem reindex`, and reindexing needs a second embedder to reindex
*to*.

### What it says about owning the embedder

The question was whether the embedding could be owned rather than rented. The
answer so far is **not this way**. Hashing is fully owned, dependency-free and
deterministic, and it retrieves about half as well on the corpus most
favourable to it — a decision log of titles chosen to be findable.

What remains untested is the expensive option: a static table distilled from a
real model, which has genuine semantics and costs a weights artifact. That is
now a swap rather than a migration, since `reindex` exists and this crate has
shown the seam works. Whether tens of megabytes in the repository is a better
trade than an API key is a judgement, not a measurement — but it should be made
knowing that the free version loses four of twelve.

### The next step, tried and not taken

The obvious improvement to hashing is a static table: real vectors from a real
model, looked up per word and pooled. That is what model2vec does, and it was
the plan — until it was measured.

Same 31-decision log, same twelve paraphrased queries. Every distinct word (531
of them) embedded through `text-embedding-3-small`, then pooled five ways:

| | rank-1 |
|---|---|
| full text embedding | **10/12** |
| plain mean of word vectors | 3/12 |
| IDF weighted | 4/12 |
| zipf, 1/√df | 2/12 |
| stopwords dropped | 6/12 |
| IDF + stopwords | 6/12 |
| subword hashing, no weights at all | **6/12** |

**What dimension these were measured at is not recorded here, and should
have been.** `src/main.rs` builds its index from `config.provider.dimension`,
so the figure describes whatever the `rmem.toml` of the day said -- almost
certainly the template's 1536, since nothing tells a reader to change it.
`rmem init --local` writes 1536 for that reason: so the number above keeps
describing the configuration `init` actually produces. Noticed while adding
that flag, not while measuring this.

The best pooling reaches exactly where free hashing already is — for the price
of a bootstrap pass over a vocabulary, a weights artifact in the repository, and
a megabyte budget. Plain averaging is *worse than hashing*, and the zipf
weighting that model2vec relies on is the worst of the lot.

**Why it cannot work this way.** An OpenAI embedding of a single word is an
embedding of a one-word *document*. The model's document geometry is not linear
in its words, so averaging those vectors is not the model's own pooling and
throws away most of what the model did. Model2vec is not doing this: it distils
a transformer's *token embedding layer*, before attention, with PCA and zipf
weighting on top. That layer is an internal object, and the OpenAI API does not
expose it.

So the structural finding, which is worth more than the number: **a static table
cannot be distilled from an API-only model.** Owning semantics means owning a
model — running an open one in-process (ONNX or candle, and a dependency tree
this workspace has four times declined) or distilling its embedding layer
offline with tooling that does not belong in this repository.

Two rounds of the cheap thing have now been measured. Neither reached the
service, and the second says the first was not the limiting factor.

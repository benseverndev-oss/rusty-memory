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


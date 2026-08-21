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

## First run: conversation 0, 419 turns

`gpt-4o-mini` extraction, `text-embedding-3-small`, 2026-08-21.

### Ingestion

| | |
|---|---:|
| turns ingested | 379 |
| turns refused | 40 (9.5%) |
| entities | 148 |
| assertions | 543 |
| relations | **17** |
| review band | **117 pairs** (27.9 per 100 turns) |

### Retrieval, recall@10 against LoCoMo evidence turns

| category | | |
|---|---:|---:|
| **overall** | 44/149 | **0.295** |
| temporal | 15/37 | 0.405 |
| single-hop | 21/70 | 0.300 |
| open-domain | 3/11 | 0.273 |
| multi-hop | 5/31 | 0.161 |

Adversarial: 11 of 47 surfaced something for a question the conversation does
not answer; 36 correctly surfaced nothing.

### What this says

**Retrieval is weak, and the shape of the weakness is diagnosable.**

**Temporal is the strongest category.** That is the thesis doing what it was
built for: questions about when something was true are exactly where a
bi-temporal store should beat a flat vector index, and it is the one category
above 0.4. It is the only encouraging number here and it is a real one.

**Multi-hop is the worst, and 17 relations explains it.** Four hundred turns of
two people discussing their lives produced seventeen relationships. There is
essentially no graph, so there is nothing to hop over, and `rm-graph` — a whole
crate — is being fed almost nothing. Extraction is not finding relations.

**148 entities is too many.** Two named speakers across nineteen sessions;
even counting every person, place, employer and pet, this should be a few dozen.
Together with 117 review pairs — nearly one per entity — the likely reading is
that resolution is *under*-merging: the same person arriving repeatedly as new
entities, each near-duplicate then generating review questions. That is an
inference from two aggregates, not a measurement, which is why the harness now
writes its store: the next run can confirm or refute it by looking.

**The review band is impractical at this rate.** 117 questions for a human, from
one conversation. The band is not dead — it fires on real ambiguity — but at
27.9 per 100 turns it is asking more than anyone will answer. If the
under-merging reading is right, most of these are the same question about the
same person, and fixing resolution shrinks this without touching the thresholds.

**9.5% of turns were dropped.** The refusals are correct — they are the
discipline working — but a tenth of a real conversation never entering the store
is a quality problem, not a robustness success. The dominant shape is the model
naming a mention index that does not exist.

### What this does not say

These numbers came from a pipeline that sets the speaker, which `rmem` and
`rmem-mcp` cannot do. They are an upper bound on the shipped tools, not a
measurement of them.

One conversation, one model, one run. Nothing here is a published-baseline
comparison: LoCoMo's own baselines answer questions, and this measures
retrieval, so the numbers are not the same quantity and must not be read
against each other.

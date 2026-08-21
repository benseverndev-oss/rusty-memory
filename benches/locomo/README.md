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

# What reading documents actually did

**Measured 2026-08-27** against Apache Arrow's API reference — `arrow-schema`,
`arrow-select` and `arrow-cast` at 59.2.0, 322 sections — with `gpt-4o-mini`
and the offline embedder, into a scratch store. Nothing was written anywhere
permanent.

The corpus comes from `docs.rs`, which serves rustdoc JSON for any published
crate. `scripts/rustdoc-corpus.py` turns that into a markdown tree, so these
numbers can be re-measured rather than taken on trust.

An earlier version of this document measured three of this repository's own
documents. That run is kept below where it still says something, and corrected
where this one overturned it — which is most of its conclusions.

## The one thing that works

Idempotency holds at corpus scale:

```
first run:   322 chunks, 322 read,   0 unchanged, 238 facts
second run:  322 chunks,   0 read, 322 unchanged,   0 facts
```

A section is identified by its path, its heading path and a hash of its text.
Sections that produced nothing are still recorded as read, which is what the
earlier run got wrong.

## Five defects, none of which the first corpus could find

Every one of these was invisible against three documents of this project's own
prose, and every one shows up in the first ten minutes against a real API
reference.

**A code fence is not structure.** The chunker split on any line starting with
`#`, which is a heading in prose and an attribute in Rust. `#[derive(Debug)]`
became a heading, and everything after it reached the model attached to that
subject. `docs/` split into 705 sections before the fix and 534 after: **a
quarter of what a run would have paid for was a spurious split on a line of
code.** The three documents measured first contain zero such lines, which is
exactly why that run came back clean.

**One bad response cost the whole run.** The first corpus-scale attempt died
sixteen minutes in, on a single response that was not the JSON the extractor
asked for, and took every completion already paid for with it. Across hundreds
of calls a malformed one is ordinary. Planning now records the failure and
continues; the failed section is not marked read, so the next run retries
exactly it. Tolerance stops at five consecutive failures, which is a broken key
rather than an unlucky run.

**The extractor returned its own worked example as fact** — the subject of the
next section.

**`rmem ingest --dry-run <dir>` was refused** while the directory sat in the
argument list, because the parser read position 1 and nothing else.

## The store filled up with a person who does not exist

Sixteen of 213 facts in the first complete run were **Alex Chen, employer
Globex** — the names from the extraction prompt's own JSON example — asserted
as facts about Apache Arrow. They came from six sections, and the six have one
thing in common: there is nothing in them. `DataType::Null`, whose entire
documentation is `Null type`. Three `TimeUnit` variants. `JsonMetadata`.

**A model given text with nothing extractable in it does not answer "nothing".
It answers with the example it was shown.** For a store whose whole claim is
that it can tell you what it does not know, inventing a person is the worst
thing it can do.

The prompt already carried a rule saying an empty answer is valid. It is the
last rule, and it loses to the example.

### Rewording it was tried first, and measured

| | leaked facts | facts kept, of 37 |
| --- | --- | --- |
| before | 15 | 22 |
| prompt reworded, strong | 0 | **0** |
| prompt reworded, soft | 0 | 7 |
| guard in code | 0 | 15 / 24 / 33 / 23 |

The strong rewording took the leak to zero by taking everything to zero. A
sentence forceful enough to stop a model copying an example also stops it
reading a definition, and an API reference is definitions. **Checking only
whether the leak was gone would have shipped an extractor that extracts
nothing.**

So the guard is in code. `extract` drops a mention whose name is one the prompt
shows *and* which the turn does not contain; the second half is what makes it
safe, and someone who really does work at Globex is untouched. The four guarded
runs span 15 to 33 facts on identical input, which is the model's own variance,
and the pre-fix legitimate yield of 22 sits inside it.

### The first guard was not enough, and better is not fixed

It took sixteen leaks to one. The survivor:

```
entity : Alex [ person ]
fact   : employer = 'Globex'
```

The mention was named `Alex`, which is the example's `text` rather than its
`name`, and only the `name` was listed. The fact's *value* was not guarded at
all. **A guard that catches most of a leak turns a loud failure into a quiet
one, which is worse than the failure.**

The drift test could not have caught either gap. It asserted that every guarded
name appears in the prompt — true, and useless, because the failure was a name
in the prompt missing from the guard. It now derives the list from the example's
own JSON and checks the direction that matters.

## Reference documentation is not the answer, and the earlier run was wrong

The earlier version of this document concluded that **the documents worth
ingesting are reference, not reasoning**. That was inference from a low yield on
argument, and it does not survive contact with an actual reference corpus:

| | sections | produced a fact | facts | facts per section |
| --- | --- | --- | --- | --- |
| this repo's `docs/`, 3 files | 30 | 9 (30%) | 70 | 2.3 |
| arrow API reference | 322 | 41 (**13%**) | 238 | 0.74 |

Arrow's reference produced **less** than this project's design argument, on both
measures. Of the 238, **159 are the `kind` assertion every entity gets** — the
substantive yield is **79 facts from 322 sections**.

The reason is visible in the corpus. Reference documentation is mostly type
signatures, cross-links and examples, and its prose says what a function
*does* — which is a definition, not a fact about a thing that could later
change. There is no bi-temporal fact in "returns a filtered `values` array where
the corresponding elements of `predicate` are true".

## Resolution does not fit identifiers at all

**80 open review questions against 114 entities.** Fifty of the entities are
caught up in at least one.

```
7.21  'DataType::Float32'   vs  'DataType::Float64'
7.14  'TimestampSecondType' vs  'TimestampNanosecondType'
7.08  'DataType'            vs  'DataType::List'
6.95  'Date32Type'          vs  'Date32'
6.82  'Int64Array'          vs  'Int32Array'
```

This is not a threshold that wants moving. The name comparator rests on the
assumption that **similar spelling means probably the same thing**, which holds
for people and inverts for identifiers: `Float32` and `Float64` are similar *by
design*, and are never the same. Raising the bar to separate them would also
stop the pairs that genuinely are the same.

Eighty questions for a person to answer, out of one small corpus, is not a queue
— it is a refusal to be used this way. Nothing here is a bug: the entity model
was built for people and organisations, and it is being pointed at a type
system.

## Twenty facts, read by hand

The plan asked for this and the first run could not supply it. Sampled with a
fixed seed from the 79 content facts, and judged against the documentation each
one came from.

| what it was | n |
| --- | --- |
| correct and worth keeping | 5 |
| correct, but a qualifier was dropped | 1 |
| weak, defensible | 2 |
| attached to the wrong subject | 3 |
| simply wrong | 2 |
| the entity is a category, not a thing | 3 |
| self-referential | 2 |
| an absence nobody asserted | 2 |

**Five of twenty are facts worth having.** Some of the rest are worth naming.

*Simply wrong.* `TimestampNanosecondType --is_a--> 'parsing function'` and
`Date32Type --is_a--> 'parsing function'`, both from a `Parser` example showing
how to parse a string *into* those types. The model read the example's topic as
the subject's category.

*The entity is a category.* `function --returns--> 'filtered values'`,
`utility trait --description--> ...`, `predicate --value--> 'true'`. The prompt
already forbids this — "a name must be able to identify something on its own" —
and it happens anyway when the text has no proper noun in it.

*A qualifier was dropped.* The source says most fields of `FormatOptions` are
compared by value, *except* `formatter_factory`. The store holds "compared by
value", which is the sentence with its exception removed.

*Self-referential.* `timestamp --related_to--> 'timestamp'`,
`IANA database --reference--> 'IANA database'`. Three across the whole store.

### The one that matters most

**Nine of 79 content facts (11%) assert an absence.** Not "we do not know" —
*asserted to have none*, which in this store is a different and load-bearing
answer:

```
'Field' has no definition
'Opaque' has no type
'DataType::Decimal128' has no default_scale
'timezone' has no round_trip_behavior
```

Through the actual interface, on the actual store:

```
$ rmem about 15 definition
no value — asserted to have none

$ rmem about 15 colour
nothing known — this was never discussed
```

The store will tell you that arrow's `Field` **is asserted to have no
definition**, and correctly tell you it knows nothing about its colour. The
distinction between those two answers is the reason this project exists, and
extraction is manufacturing false positives on the side of it that cannot be
recovered from: an `absent` is a claim someone made, and nobody made this one.

The prompt teaches it directly — *"Null means the attribute has no value: 'he is
between jobs' is a fact with a null value, not a missing fact."* That is right
for dialogue, where a person really can say someone has none. It is wrong for a
document, which is not a witness and cannot assert an absence by failing to
mention something.

**This is fixed**; see "What this changes" below. The numbers above are from the
run that found it.

## What this changes

**Extraction from a document can no longer assert an absence.** Fixed after
this was written. `rm_extract::without_absences` removes facts with a null
value, and `plan_remember` takes a `Witness` saying whether the source is a
speaker or a document — dialogue is untouched, because a person really can say
someone has none, and a test asserts the guard is *not* applied there.

Re-run over the same 322 sections:

```
$ rmem about 2 definition
nothing known — this was never discussed
```

Zero tombstones in the store, from nine. Guaranteed by construction rather than
by luck: every null-valued fact from a document is removed, and each removal is
reported by section and reason rather than counted.

That last part was itself wrong when first written. `commit_tree` kept only the
assertion count and discarded the reasons, so the fix was silent on exactly the
run — hundreds of sections — where a reader cannot go and look. `Read` now
carries them out.

**The declines spec now has evidence, and the evidence changes its argument.**
It was written around saving completions on ambiguous readings. The measured
case is different and stronger: on 87% of sections the extractor has nothing to
say and says something anyway, and what it says is drawn from its own prompt,
from a cross-reference, or from a category noun standing in for a name.

**Nothing here recommends pointing this at a live store**, and `commit_tree`
still refuses to. That refusal was written as a temporary guard until declines
existed. It has since earned its keep twice.

## What is still open

The resolution mismatch has no fix in this repository, and may not want one: a
comparator that suits identifiers is a different comparator, not a tuned version
of this one. Whether it is worth building depends on whether reading API
documentation is worth doing at all, and the yield table above argues that it is
not the place to start.

Graphiti/Zep and Letta remain unmeasured on the two-scenario comparison.

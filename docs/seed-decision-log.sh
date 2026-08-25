#!/usr/bin/env bash
#
# The rusty-memory decision log, recorded from its own history.
#
# Eleven merged pull requests' worth of decisions, the options that were tried
# and turned down with the measurements that killed them, and the three
# supersession chains that actually happened. Run it against an empty store to
# get a log worth reading:
#
#     mkdir log && cd log && rmem init
#     RMEM_BIN=$(command -v rmem) ../docs/seed-decision-log.sh
#     rmem decisions
#     rmem decision "still_stands means nothing replaced the choice"
#
# It is a demo and it is also the real log: every entry below is something this
# project decided, and every `because` is the reason or the number it turned on.
# Costs a few embeddings per decision and no completions at all -- `decide`
# never reaches a completion model.
#
# Reading this log is what turned up two things since fixed: `recall` hits now
# lead with what they are about rather than an id to chase, and `decide --at`
# exists because every entry below used to be dated the day the script ran.
#
# The dates are still not set here. They could be -- each `d` call could carry
# `--at` with the day its pull request merged -- and doing that is the obvious
# next pass over this file. It is left undone deliberately rather than half
# done: a wrong date is worse than an obviously absent one, and the merge dates
# want reading off git rather than guessing.
# The rusty-memory decision log, recorded from eleven merged pull requests.
set -euo pipefail
R="${RMEM_BIN:-rmem}"
d() { "$R" decide "$@" >/dev/null; printf '.'; }

echo -n "accepted "
d "Store bi-temporally" "every assertion carries valid time and transaction time" \
  --because "one axis makes a stale answer indistinguishable from a bug" \
  --context "choosing the storage model before anything was built"
d "Resolve identity with Fellegi-Sunter" "score fields into bits of evidence, with a review band between match and non-match" \
  --because "a threshold pair states what evidence is required rather than hiding it in a similarity cutoff"
d "Search exactly, not approximately" "a linear scan over the vectors" \
  --because "the corpus is thousands of assertions, not millions; an ANN index is a dependency and an approximation bought for nothing at this size"
d "Parse arguments by hand" "sixty lines instead of clap" \
  --because "five subcommands against a dependency tree that pulls in syn, quote and proc-macro2"
d "Give the resolver the kind field" "compare kind as an exact field, and recalibrate both thresholds by log2(0.9/0.38)" \
  --because "over half the review band was pairs whose kinds already disagreed; Paris the city is not Paris the person"
d "A thing is never confused with what it belongs to" "a possessive-aware comparator, not plain jaro_winkler" \
  --because "\"Melanie's son\" scored 0.92 against \"Melanie\" and asked whether a woman is her own child"
d "Let a caller say who is speaking" "an optional speaker on every turn" \
  --because "without it, responses listing no mentions ran at 45%; with it, 1%, and one conversation went from ~576 assertions to ~1494"
d "One writer at a time, enforced" "an advisory lock on a sidecar file, spanning each read-modify-write" \
  --because "locking the save alone loses the other writer's update rather than tearing the file"
d "Strip code fences before parsing" "unfence the model's reply in both parsers" \
  --because "three backticks were costing 4.8% of every turn -- 269 refusals became 15, and recall rose in 10 of 10 conversations"
d "Decisions skip the extractor" "write status, choice, because and context directly under fixed names" \
  --because "81% of extracted attribute names are singletons; a record nobody can name twice cannot be looked up or superseded"
d "A decision's title is an identifier" "match it exactly, never through the resolver" \
  --because "\"Adopt SQLite\" swallowed \"Adopt SQLite WAL\" and kept the first title, so the second decision existed nowhere"
d "Supersession is an edge" "draw new -supersedes-> old rather than setting a flag" \
  --because "a status alone left the chain unrecoverable: you could see a decision was replaced and not by what"
d "Model calls happen outside the store lock" "plan above the lock, commit inside it" \
  --because "the lock spanned extraction and embeddings, so the fourth concurrent writer was refused; now twelve land with no lost updates"
d "Pin the compiler" "rust-toolchain.toml names the version and CI asserts it bound" \
  --because "CI took whatever stable had become that week; a lint fired there that did not exist locally"

echo; echo -n "rejected "
d "Rerank the recall results" "a cross-encoder over a deep candidate list" --status rejected \
  --because "the k-curve is still 0.926 at k=200, so there is nothing to rerank into"
d "Hybrid lexical retrieval" "BM25 beside the vector index" --status rejected \
  --because "rescues 14 of 116 misses; not worth a second index and a second thing to keep consistent"
d "Boost assertions about the question's subject" "add a constant to hits on the named entity" --status rejected \
  --because "measured null across the sweep; turning a name into an entity is itself at J = 0.33, so the boost lands on the wrong entity as often as the right one"
d "Ask the model whether a fact replaces an earlier one" "a replaces boolean per fact" --status rejected \
  --because "it answered well and cost 19% of the facts; a worse trade than leaving supersession unstated"
d "Tell the extractor which attribute names to prefer" "a naming rule in the prompt" --status rejected \
  --because "cost facts, and the vocabulary did not converge; two other prompt rules were withdrawn for the same reason"
d "Tell the extractor to prefer events over feelings" "selection guidance rather than a per-fact question" --status rejected \
  --because "the first instruction that did not cost facts, but recall did not replicate (+0.020, +0.013, -0.020) and the metric it targeted moved by two questions in 115"
d "Drop tombstones from recall results" "filter assertions whose value is null" --status rejected \
  --because "costs 0.094 recall -- an assertion points at the turn it came from whether or not it carries a value"
d "Deduplicate recall by entity" "at most one hit per entity" --status rejected \
  --because "costs 0.181; the top ten already averages 9.0 distinct turns, so there is no crowding to squeeze out"
d "Fuse the assertion index with raw-turn retrieval" "reciprocal rank fusion over both rankings" --status rejected \
  --because "0.780 against 0.793 for raw turns alone; the assertion index contributes no recall the turn text does not already carry"

echo; echo -n "superseded "
d "Arrival order is contradiction" "treat a later assertion in the same slot as replacing the earlier one" \
  --context "the first rule for deciding what still holds"
d "Supersession is stated, not inferred" "the extractor says Corrects, Joins or Unstated; the store never guesses" \
  --because "arrival order flagged 26% of every assertion as replaced, and the sample is dominated by facts that are all still true -- three things attended, two pets" \
  --supersedes "Arrival order is contradiction"

d "still_stands means nothing replaced the choice" "count the versions of choice" \
  --context "the first definition, when accepted and superseded were the whole vocabulary"
d "still_stands means the displayed choice stands" "read the supersession edge, not the version count" \
  --because "a title re-decided under itself shows its latest choice, and marking that replaced said the opposite of what the line shows" \
  --supersedes "still_stands means nothing replaced the choice"
d "still_stands means in force" "accepted and unsuperseded" \
  --because "with five statuses, unsuperseded and actionable stopped being the same predicate, and \"can I act on this\" is the question a reader has" \
  --supersedes "still_stands means the displayed choice stands"

d "Embed locally with subword hashing" "hash character n-grams into the vector, no model file" --status proposed   --because "fully owned and dependency-free, and the decision path needs no key at all -- but 6/12 against the service's 10/12 on paraphrased queries, because it has morphology and no semantics"
d "Distil a static word table from the embedding API" "embed a vocabulary once, then look up and pool" --status rejected   --because "best pooling reaches 6/12, exactly tying free hashing, for a bootstrap pass and a weights artifact; an API embedding of one word is a one-word document, and the model's geometry is not linear in words -- model2vec distils the token embedding layer, which the API does not expose"

d "Carry the handshake in an Mcp-Session-Id" "mint at initialize, look it up per request" \
  --because "each HTTP request gets its own connection and its own server, so the client's name and the revision it agreed to were both dropped -- every write recorded as mcp, and a client that handshaked before structuredContent existed sent it anyway"
d "Mint session ids from RandomState rather than a CSPRNG" "128 bits of SipHash under an OS-seeded key" \
  --because "/dev/urandom is not on Windows, which this is measured on, and the alternative was a dependency or a cfg fork; a guessed id buys attribution forgery from someone already authorised to write, not access, and provenance was never a security boundary"
d "Refuse a request whose session id is unknown" "404, and re-handshake" \
  --because "a client holding a stale id would otherwise be served as a stranger and never learn its session had gone; a request with no id at all is still served, because refusing it would reverse the decision already made for a client that never handshakes"

echo; echo -n "proposed "
d "Index raw turn text beside assertions" "one vector per turn as well as one per assertion" --status proposed \
  --because "raw turns beat the pipeline by 0.086 pooled over three conversations, never negative -- but fusing the two loses to raw turns alone, so the shape is unsettled"
d "Retire recall@10 as the headline metric" "measure what the store is for -- contradiction, supersession, time -- instead" --status proposed \
  --because "a twenty-line control beats the pipeline on it, and none of the distinctive machinery serves it; but LoCoMo labels no ground truth for the alternative"
echo

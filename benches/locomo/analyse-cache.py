#!/usr/bin/env python3
"""What the model actually said, over every cached response.

Five prompt changes were made against run metrics before anyone read the
responses. Two of them were built on hypotheses that turned out to be wrong,
and the retrieval metric they were judged by has a range of 0.074 at a fixed
configuration -- wide enough to have justified any of them.

This reads the raw responses instead. It answers structural questions ("how
often does the model list two things?") over a thousand-odd samples, which is
worth more per run than the metric and costs nothing once a run has been paid
for.

    python3 benches/locomo/analyse-cache.py locomo-cache.json

What it found the first time it was run, and the prompt changes that followed:

  * 45% of responses listed no mentions at all; only 15% listed two or more.
    A relation names two mention indices, so most turns could not carry one
    however the prompt was worded -- which is why five attempts to ask for
    relations more clearly did nothing.

  * 258 responses carried facts with no mentions listed, every sampled one of
    them about the speaker. The model was writing `subject: 0` against an empty
    list: treating the speaker as an implicit mention it had never been asked
    to list.
"""
import collections
import json
import sys


def main(path):
    completions = json.load(open(path))["completions"]
    parsed, unparsable = [], 0
    for raw in completions.values():
        try:
            d = json.loads(raw.strip())
            if isinstance(d, dict):
                parsed.append(d)
        except Exception:
            unparsable += 1

    print(f"responses: {len(completions)}  parsed: {len(parsed)}  not JSON: {unparsable}")

    mentions = collections.Counter()
    with_relation = 0
    two_plus = two_plus_related = 0
    unanchored = 0
    for d in parsed:
        m, r, f = d.get("mentions") or [], d.get("relations") or [], d.get("facts") or []
        mentions[len(m)] += 1
        if r:
            with_relation += 1
        if len(m) >= 2:
            two_plus += 1
            if r:
                two_plus_related += 1
        if not m and f:
            unanchored += 1

    n = max(len(parsed), 1)
    zero = mentions[0]
    print(f"\nmentions per response:")
    for k in sorted(mentions):
        print(f"  {k:>2}: {mentions[k]:>4}  ({mentions[k]/n:.0%})")
    print(f"\nlisted nothing:            {zero}/{n} = {zero/n:.0%}")
    print(f"listed two or more:        {two_plus}/{n} = {two_plus/n:.0%}   <- the ceiling on relations")
    if two_plus:
        print(f"  of those, related them:  {two_plus_related}/{two_plus} = {two_plus_related/two_plus:.0%}")
    print(f"emitted any relation:      {with_relation}/{n} = {with_relation/n:.0%}")
    print(f"facts with nothing listed: {unanchored}/{n} = {unanchored/n:.0%}   <- the unanchored shape")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    main(sys.argv[1])

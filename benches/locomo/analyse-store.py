#!/usr/bin/env python3
"""What a written store actually holds, structurally.

Companion to `analyse-cache.py`, which reads what the model said. This reads
what the engine kept, and answers one question the retrieval metric cannot:

    can the bi-temporal machinery ever run?

Supersession, survivorship, valid intervals and `about` all operate *within one
attribute name on one entity*. A later fact only contradicts an earlier one if
both were recorded under the same name. So the store's attribute vocabulary is
not cosmetic -- it decides whether any of that code is ever reached.

Run it over a snapshot the harness wrote:

    python3 benches/locomo/analyse-store.py locomo-0.json

What it found the first time it was run, over conversation 0:

  * 498 distinct attribute names for 735 assertions, 82% of them used exactly
    once. The model is free to invent an attribute name per fact and does:
    `feeling`, `emotion`, `emotional_response`, `feeling_about_art` and
    `emotional_impact` are five names for one idea, so five facts about how
    someone felt never meet.

  * Almost nothing to supersede. Recall@10 was unmoved by this and always will
    be -- retrieval is embedding search over a fact's own text and does not
    read the attribute name at all -- which is why five runs of a retrieval
    metric never showed it.
"""
import collections
import json
import re
import sys


def load(path):
    """A snapshot's engine-level fields, with the store's own JSON unwrapped."""
    outer = json.load(open(path))
    return outer, json.loads(outer["store"])


def main(path):
    outer, store = load(path)
    entities = store["entities"]

    # (entity, attribute) -> number of versions. `kind` is excluded throughout:
    # `ingest` asserts it for every mention, so it is the one attribute that is
    # always present and would flatter every count below.
    versions = {}
    attrs = collections.Counter()
    for eid, e in entities.items():
        for name, vs in e["attributes"].items():
            if name == "kind":
                continue
            versions[(eid, name)] = len(vs)
            attrs[name] += len(vs)

    total = sum(attrs.values())
    singles = sum(1 for a, n in attrs.items() if n == 1)
    n_attr = max(len(attrs), 1)

    print(f"entities            {len(entities)}")
    print(f"assertions          {total}   (excluding `kind`)")
    print(f"distinct attributes {len(attrs)}")
    print(f"used exactly once   {singles}  ({singles / n_attr:.0%} of names)")
    print(f"assertions per name {total / n_attr:.2f}")

    # The number that matters. Every temporal feature needs two versions of one
    # attribute on one entity; without that pair there is nothing to order,
    # nothing to supersede and nothing for a survivorship strategy to choose
    # between.
    contested = {k: v for k, v in versions.items() if v > 1}
    print(f"\nattributes with more than one version: {len(contested)} of {len(versions)}"
          f"  ({len(contested) / max(len(versions), 1):.0%})")
    print(f"  assertions inside them: {sum(contested.values())} of {total}"
          f"  ({sum(contested.values()) / max(total, 1):.0%})")
    print("  <- everything supersession, survivorship and valid-interval do lives here")

    if contested:
        print("\n  the contested ones:")
        for (eid, name), n in sorted(contested.items(), key=lambda kv: -kv[1])[:10]:
            vals = [v.get("value") for v in entities[eid]["attributes"][name]]
            print(f"    entity {eid:>3}  {name:<28} {n} versions: {vals}")

    # Near-synonyms, found by shared stem rather than by meaning. Crude on
    # purpose: a real synonym set would need a judgement this script has no way
    # to make, and the crude version already shows the scale.
    print("\nnames sharing a stem (a lower bound on the duplication):")
    families = collections.defaultdict(list)
    for a in attrs:
        stem = re.split(r"[^a-z]", a.lower())[0][:6]
        if len(stem) >= 4:
            families[stem].append(a)
    shown = 0
    for stem, fam in sorted(families.items(), key=lambda kv: -len(kv[1])):
        if len(fam) < 3:
            continue
        print(f"  {stem + '*':<10} {len(fam):>2} names, "
              f"{sum(attrs[a] for a in fam):>3} assertions: {sorted(fam)[:6]}")
        shown += 1
        if shown == 8:
            break

    print(f"\nmost-used names:")
    for a, n in attrs.most_common(8):
        print(f"  {n:>4}  {a}")

    # What the recall path would tell a reader about each assertion. Mirrors
    # `rm_engine::Engine::standing`: strictly later on the transaction axis,
    # one `corrects` settles the slot, otherwise unanimous `joins` leaves the
    # fact standing and any `unstated` leaves the question open.
    stand = collections.Counter()
    for eid, e in entities.items():
        for name, vs in e["attributes"].items():
            if name == "kind":
                continue
            for v in vs:
                later = [w for w in vs
                         if w["provenance"]["observed_at"] > v["provenance"]["observed_at"]]
                claims = {w.get("supersession", "unstated") for w in later}
                if not later:
                    stand["latest"] += 1
                elif "corrects" in claims:
                    stand["corrected"] += 1
                elif "unstated" in claims:
                    stand["unsettled"] += 1
                else:
                    stand["joined"] += 1

    print("\nwhat recall would say about each assertion:")
    for k in ("latest", "joined", "unsettled", "corrected"):
        print(f"  {k:<10} {stand[k]:>5}  ({stand[k] / max(total, 1):.1%})")
    print("  <- only `corrected` stops a reader stating the fact. Before the")
    print("     model was asked, everything but `latest` read as replaced.")

    band = outer.get("review", {})
    print(f"\nreview band: {len(band)} pairs")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(__doc__.strip().splitlines()[-1].strip())
    main(sys.argv[1])

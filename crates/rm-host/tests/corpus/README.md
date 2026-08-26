# The resolution corpus

Twelve mentions against eight people, with hand-written ground truth, scoring
the configuration this crate actually ships.

## It is synthetic, and that is a requirement

This repository is public. The corpus that produced the original measurements
was 27 real colleagues with their real addresses and must never be committed.
What matters is the *structure*, not whose details carry it.

## Why it lives in `rm-host`

The plan named `rm-resolve`. It is here instead because the point is to score
the configuration that **ships** — `TEMPLATE` and `Config::ruleset` are in this
crate, and `rm-resolve` cannot reach them without inverting the dependency.
Hand-writing the thresholds into the test would measure a configuration nobody
runs, which is the failure the corpus exists to prevent.

## The shapes, and why the negative ones matter most

A corpus of true matches measures nothing: a configuration that merges
everything scores perfectly on it. Two shapes pull against each other on
purpose, and any configuration that gets both right has earned it.

- **`changed-surname-stable-local-part`** — a true match whose surname changed
  and whose mail local part did not. An exact email comparator turns this into
  a silent duplicate.
- **`shared-given-name-shared-domain`** — a true non-match sharing a surname
  and a mail domain with a real person. A fuzzy email comparator merges it
  outright.

Both were found by deliberately building the case that would break the
configuration about to be recommended, and both did.

## The baseline

`baseline.json` is what the shipped configuration does today. A change that
turns a caught match into a silent duplicate, or a stranger into a merge, fails
the test. Updating the baseline is how you say a change was intended — a
deliberate act with a diff a reviewer can read.

Pairs carry their scores, so a diff says which pair moved and by how much
rather than that a count changed.

## One entry is expected to stay a silent miss

`Merrick -> p1 (-2.06)` is a surname alone against a full name, and the
evidence is **negative**: the name comparator is prefix-weighted, so a bare
surname looks less like its owner than like an unrelated name starting with the
same letter. No threshold fixes it, because a bare surname genuinely is
ambiguous — in a store holding two people who share one, it should be. Leave it
in the baseline; it is a property of the data, not a bug awaiting a fix.

## Adding a shape

Say where you saw it. A case invented to be difficult is worth less than one
drawn from real data where somebody was nearly misled, and the commit should
record which it is.

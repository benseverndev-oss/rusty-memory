# The absence corpus

Eight cases, each labelling all three answers this store can give.

## It is synthetic, and that is a requirement

This repository is public. No real person, employer, identifier or address
appears in `cases.json`, and none should be added. What matters is the
*structure* — a stated value, a stated absence, and a silence — not whose
details carry it.

## What the fields mean

`states` is what the conversation established. A string is a value; `null` is
an asserted absence, somebody saying there is none. It is written through the
ordinary write path, which is the same thing `rmem note --absent` does.

`truth` labels the correct answer for every attribute the case asks about, and
deliberately includes attributes that `states` never mentions. **Those gaps are
the corpus**, not an oversight: an attribute nobody discussed must read
`unknown`, and a store that answers `absent` there has invented a fact.

`why` says what acting on a wrong answer would cost. It is required. An absence
with no consequence attached is a toy, and the README quotes these lines when
it explains why the distinction is worth having — "has no pets" persuades
nobody, "has no prescribing authority" does.

## Why the cases look the way they do

Every case labels a `value`, an `absent` and an `unknown`, and the shape guard
in `absence.rs` enforces it. A corpus of values and silences would be one that
**any two-state system scores perfectly on**, because the whole claim lives in
the cases where something was stated to be missing.

The `unknown` attributes are chosen to be the ones a careless reader would
infer. `Case C` states a current employer and asks about a former one; `Case H`
states a start date and a stated-absent termination date, then asks about a
review nobody mentioned. Each is a place where the record looks complete enough
that filling the gap feels safe.

## Adding a case

Say where the shape came from. A case invented to be difficult is worth less
than one drawn from a real conversation where somebody was nearly misled, and
the commit that adds it should record which it is.

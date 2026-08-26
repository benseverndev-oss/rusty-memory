# Measured claims that recompute themselves

**Status:** proposed
**Date:** 2026-08-26

## The problem

This project's distinguishing asset is that it writes down why. The config does
not just set `review_at = 5.2439`, it derives it:

> These are calibrated against the fields below and are not portable away from
> them. They were 4.0 and 6.0 when `name` was the only field; adding `kind`
> adds log2(0.9/0.38) = 1.2439256 bits to every pair whose kinds agree, so both
> rose by that much and a pair that agrees on kind lands where it landed
> before.

That is a chain of arithmetic connecting four numbers. **Nothing recomputes
it.** Change `u` from `0.38` to `0.40` and the sentence becomes false, both
thresholds are silently miscalibrated by the difference, and every test in the
workspace still passes. The only assertions naming `5.2439` check that
`decide()` returns `Review` at that value — that the boundary is where the
config says, not that the config says the right thing.

There are **100 sentences containing "measured"** across the crates and README.
Two are checked.

This is not hypothetical. On 2026-08-26, `definitions()`' own doc comment
claimed eight tools cost ~1,700 tokens while the README's table said ~2,060 for
the same eight. The table had moved twice — once for the clocks, once for scope
— and the comment never followed. It was found by accident, while adding a
ninth tool, roughly a day after it went stale.

A wrong number is worse in this codebase than in most, precisely because the
surrounding prose has earned the reader's trust. It gets quoted.

## What this builds

A test module per crate holding claims, named for what it protects, that
recomputes the derivations the prose asserts.

```rust
/// The thresholds are the two-field figures, and this is the arithmetic the
/// config's comment states. If `kind`'s m or u move, this fails and the
/// comment above them is wrong -- which is the point: the comment is the
/// specification and this is the test of it.
#[test]
fn the_thresholds_are_the_one_field_figures_plus_kind_s_agreement_weight() {
    let kind = Rule::new("kind", Comparator::Exact, 0.9, 0.38);
    let shift = kind.agreement_weight();          // log2(0.9/0.38)
    assert!((shift - 1.2439256).abs() < 5e-8, "{shift}");

    // 4.0 and 6.0 were the thresholds when `name` was the only field.
    assert!((TEMPLATE_REVIEW_AT - (4.0 + shift)).abs() < 1e-4);
    assert!((TEMPLATE_MATCH_AT  - (6.0 + shift)).abs() < 1e-4);
}
```

The tolerance is `1e-4` because the config rounds to four places, and the
comment already says so: "written to four places, which leaves each boundary
0.000026 bits below the exact figure". The test encodes that deliberately
rather than hiding it — a tighter tolerance would fail on the rounding the
author chose.

## Which claims are worth this

Not all hundred. The test is only worth writing where a claim is **derived**
and its inputs live in the repository. Three tiers:

**Recompute (do this).** The claim is arithmetic over values in the code.
- `review_at` / `match_at` against `kind`'s agreement weight, above.
- `log2(0.9/0.01) ≈ 6.49` — the most a name can contribute — asserted in three
  separate doc comments and one test comment, none of which compute it.
- The kind-disagreement veto: `6.49 - 2.63 = 3.86` is below `review_at`, which
  is what makes a kind mismatch final rather than expensive. This is a
  load-bearing claim about behaviour and should be a behavioural test too.
- The MCP tool table's token figures, against the serialised byte count.

**Pin (do this where cheap).** The claim is a measurement of something in the
repo that can be re-measured cheaply, even if the absolute number needs a
human. The tool table is the model: assert the byte count, and let the token
figure be derived from a ratio stated in one place.

**Leave alone (do not fake this).** The claim is a measurement of the outside
world and cannot be recomputed here: LoCoMo's 382/112 split, the ANN bake-off
timings, "96.9% of the file was vectors", `u = 0.38` measured across four
stores. A test that asserts a hard-coded constant equals itself is exactly the
vacuous test this project already has a lesson about.

For that third tier the fix is not a test but a **provenance line**: what was
measured, when, and where the harness lives. Some already have this. The ones
that do not should get it, and `u = 0.38` is the most important of them,
because it is an input to the first tier.

## What it does not do

**It does not verify the outside world.** See the third tier above.

**It does not turn prose into generated text.** The comments stay hand-written.
The test asserts the numbers in them are still true; it does not produce them.
Generated documentation would lose the reasoning, which is the part worth
keeping.

**It is not a lint.** No mechanism scans for unverified numbers. Deciding which
claims are load-bearing is a judgement, and a rule that demanded a test per
number would produce a hundred vacuous ones.

## Testing

The tests are the deliverable. Each must be shown to fail when the claim it
protects is falsified — perturb the input, watch it fail, restore it. This is
the same check applied to the review-hint test on 2026-08-26, which was
verified to fail on the exact bug it was written for before being committed.

A claim test that has never been seen red is a claim test that might be
asserting a constant against itself.

## Risks

**False confidence.** A green suite would now imply "the numbers are
consistent", which is weaker than "the numbers are right". `u = 0.38` could be
wrong about the world and every test here would still pass, because they check
the arithmetic built on it, not the value itself. The tier-three provenance
lines are what keep that honest, and they should be written in the same change
rather than deferred.

**Rounding fights.** Several figures are deliberately rounded, and a test with
too tight a tolerance turns an intentional choice into a failure. Every
tolerance should carry a comment saying which rounding it is permitting and
why, as the example above does.

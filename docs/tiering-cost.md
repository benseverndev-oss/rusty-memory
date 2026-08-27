# What each recall depth costs

**Measured 2026-08-27.** Harness: `crates/rm-engine/tests/tiering.rs`, twenty
distinct subjects, one assertion each, `k = 20`.

| depth | chars | against `stated` |
|---|---|---|
| `located` | 2,370 | **−63%** |
| `stated` (today's `recall`) | 6,410 | — |
| `traced` | 54,924 | **+757%** |

## `located` ships

The spec's decision rule was that a small saving means the level should not
ship. 63% is not small, and `located_is_at_least_a_third_cheaper_than_stated`
holds it there: if a future change erodes the saving, that test fails and the
rule fires rather than the number quietly drifting.

## `traced` is expensive, and that was not anticipated

Eight and a half times `stated` on this corpus, and it grows with slot history:
every version carries a value, an interval and a provenance, and `traced`
returns all of them for every hit.

The spec framed the three levels as a ladder a caller walks up. On this
evidence `traced` is not the top of a ladder but a different tool — **it is for
one hit, not for a result set.** Twenty traced hits is roughly 14,000 tokens,
which is more than the entire MCP tool table costs per turn.

Nothing here caps it. The honest handling is that the tool description says
what it is for, and that a caller wanting provenance for one answer should ask
for one answer.

## `Debug` length is a proxy

These are `format!("{:?}")` lengths, not wire bytes. They overstate every level
similarly — field names, quoting and struct syntax that JSON would spell
differently — so the **ratios** are sound and the **absolutes** are not. The
MCP serialisation is what a client actually pays.

## The risk this does not measure

`located` trades bytes for round trips. For an MCP client a second call is a
turn, and a turn costs more than 4,000 chars saved. The saving only pays where
a query returns several hits and the caller wants text from one — which is the
case it was designed for, and not the case where a caller already knows it
needs everything.

Nothing here measures whether callers use it that way. That is a claim about
behaviour, and this repository has learned to say when it is asserting one.

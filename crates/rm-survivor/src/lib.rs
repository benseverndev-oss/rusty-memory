//! Survivorship for agent memory: choosing what is true when the store holds
//! two answers to the same question.
//!
//! # Lineage, and the one thing that changes
//!
//! The value-only strategies here are ported from Golden Suite's
//! `survivorship-core`, tie-break for tie-break. That crate *refuses* two of its
//! strategies:
//!
//! - `source_priority` — "needs a sources list, which the Spark path does not
//!   supply -- Python raises rather than guessing, and so does this. Guessing an
//!   order would silently prefer the wrong system of record."
//! - `most_recent` — "needs a dates list, which the Spark path does not supply
//!   ... picking the first row would be an arbitrary answer wearing a
//!   deterministic hat."
//!
//! It refuses them because its call site is a Spark batch job that passes bare
//! values. **Agent memory passes both.** Every assertion arrives with a
//! [`Provenance`] naming its source and when it was observed, so the two
//! strategies that are unimplementable in a batch job are the two that matter
//! most here — a memory store's whole difficulty is that facts change and
//! sources disagree.
//!
//! And having timestamps makes a third strategy expressible that neither system
//! could state before: [`Strategy::ValidInterval`] declines to pick a winner at
//! all, and instead writes every value with a disjoint validity range. "Acme
//! until July, Globex after" is not a conflict to resolve; it is two facts with
//! different valid times. Resolving it to one winner destroys information the
//! store is capable of keeping.
//!
//! # The discipline that is *not* changing
//!
//! Upstream refuses rather than approximates, on the grounds that a survivor
//! chosen by the wrong rule is "a wrong golden record that looks right": no
//! exception, no null, just a plausible value that nothing downstream can flag.
//!
//! That reasoning is *stronger* for memory, not weaker. A wrong golden record
//! sits in a table where a human might eventually notice it. A wrong memory gets
//! silently laundered into an LLM's context as established fact and shapes every
//! later turn. So every strategy here refuses when its inputs cannot answer the
//! question, and the refusal names what was missing.
//!
//! # Three states, not two
//!
//! Upstream's candidates are values or nulls. Memory needs a third: a source can
//! assert that a field is *empty* ("they are between jobs"), which is a claim
//! that competes, and distinct from having said nothing at all. See [`Asserted`].
//! `rm_store` already draws this line with `Known`; this crate needed it as soon
//! as anything read from that store and resolved with this one.

use rm_core::{Interval, Provenance, Source, Timestamp};
use serde::{Deserialize, Serialize};

/// A refusal to guess, and why.
///
/// Distinct from "no survivor" ([`Outcome::Survivor`]`(None)`), which is a real
/// answer: the candidates were considered and none won. A `Refused` means the
/// question could not be asked with the data supplied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refused(pub String);

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "survivorship refused: {}", self.0)
    }
}

impl std::error::Error for Refused {}

/// What one source said about a field.
///
/// Three states, not two, because `rm_store` writes three. A tombstone — "this
/// attribute has no value" — is a positive claim that competes for the survivor
/// slot. Silence is the absence of a claim and competes for nothing. Collapsing
/// them makes a deliberate "they are between jobs" indistinguishable from a
/// source that simply did not mention employment, which is how an agent ends up
/// asserting a stale employer forever.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Asserted<'a> {
    /// The source asserted this value.
    Value(&'a str),
    /// The source asserted the field has no value.
    Absent,
    /// The source had nothing to say about this field.
    Silent,
}

impl<'a> Asserted<'a> {
    /// The value, if one was asserted. `Absent` and `Silent` both give `None`,
    /// so only reach for this where the difference genuinely does not matter.
    pub fn value(self) -> Option<&'a str> {
        match self {
            Asserted::Value(v) => Some(v),
            _ => None,
        }
    }

    /// Whether this is a claim at all. `Silent` is not.
    pub fn is_assertion(self) -> bool {
        !matches!(self, Asserted::Silent)
    }
}

/// What survivorship concluded actually held. The owned counterpart to
/// [`Asserted`], minus `Silent` — silence never wins, so a result can never be
/// one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Held {
    Value(String),
    Absent,
}

impl Held {
    /// The value, if this is one. `Absent` gives `None`.
    pub fn value(&self) -> Option<&str> {
        match self {
            Held::Value(v) => Some(v),
            Held::Absent => None,
        }
    }
}

/// One assertion of a field's value, with the provenance that decides how it
/// competes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate<'a> {
    pub value: Asserted<'a>,
    pub provenance: &'a Provenance,
    /// When this held in the world, as against when it was heard.
    ///
    /// [`Strategy::ValidInterval`] is named for this and could not see it: the
    /// type carried a value and a provenance and nothing else, so the timeline
    /// it built was cut at `provenance.observed_at` and was a transaction-time
    /// timeline wearing a valid-time name. Asked the case `rm_store`'s own docs
    /// open with -- told in September that a job changed in July, what held in
    /// August -- it answered with the old employer.
    ///
    /// Defaults to [`Interval::since`] the observation, which is what the old
    /// behaviour assumed everywhere, so a caller that does not know better gets
    /// exactly what it got before.
    pub valid: Interval,
}

impl<'a> Candidate<'a> {
    /// A candidate from an optional value. `None` means the source said
    /// *nothing*; for a source asserting the field is empty, use
    /// [`Candidate::absent`].
    ///
    /// Valid from the moment it was observed. A caller that knows when the
    /// value actually held says so with [`Candidate::over`].
    pub fn new(value: Option<&'a str>, provenance: &'a Provenance) -> Self {
        Candidate {
            value: match value {
                Some(v) => Asserted::Value(v),
                None => Asserted::Silent,
            },
            provenance,
            valid: Interval::since(provenance.observed_at),
        }
    }

    /// A candidate asserting the field has no value — a tombstone.
    pub fn absent(provenance: &'a Provenance) -> Self {
        Candidate {
            value: Asserted::Absent,
            provenance,
            valid: Interval::since(provenance.observed_at),
        }
    }

    /// The same candidate, over the span it actually held.
    pub fn over(mut self, valid: Interval) -> Self {
        self.valid = valid;
        self
    }
}

/// What a span of valid time holds.
///
/// Two shapes, because a timeline over contradictory writes has regions where
/// no single value can be said to have stood. Naming those regions is what
/// lets a read refuse the instant it was asked about rather than the whole
/// history: a timeline with an unnamed hole cannot be indexed into, and one
/// whose holes are named can.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Span {
    /// One value stood here.
    Held(Held),
    /// Two or more values opened here sharing an `observed_at`, so nothing
    /// orders them and none of them can be said to have held.
    ///
    /// `observed_at` is carried because it is what a refusal hands back to
    /// whoever has to fix it: the timestamp naming which writes to separate.
    Contested {
        values: Vec<Held>,
        observed_at: Timestamp,
    },
}

/// A span of valid time and what stood over it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fact {
    pub span: Span,
    pub valid: Interval,
}

/// What survivorship produced.
///
/// Two shapes because the strategies genuinely answer different questions.
/// Every strategy but [`Strategy::ValidInterval`] answers "which one value
/// survives"; that one answers "what was true when".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    /// One winner, or `None` when no candidate asserted anything at all.
    Survivor(Option<Held>),
    /// A timeline of values with non-overlapping validity, oldest first. Empty
    /// when nothing was ever asserted.
    Timeline(Vec<Fact>),
}

impl Outcome {
    /// The surviving value, if this outcome names exactly one.
    ///
    /// A `Timeline` has no single survivor by construction, so this is `None`
    /// for one unless the timeline holds exactly one fact.
    ///
    /// Carries the same caveat as [`Outcome::as_of`], and for the same reason:
    /// a winning [`Held::Absent`] collapses to `None`, indistinguishable here
    /// from no candidate having asserted anything at all. Those are opposite
    /// statements — one is a positive claim that the attribute has no value,
    /// the other is silence — and the whole of [`Held`] exists to keep them
    /// apart. This method is the convenience; match on the [`Outcome`] itself
    /// where the difference matters, as a memory store must.
    pub fn survivor(&self) -> Option<&str> {
        match self {
            Outcome::Survivor(v) => v.as_ref().and_then(Held::value),
            Outcome::Timeline(facts) => match facts.as_slice() {
                [Fact {
                    span: Span::Held(v),
                    ..
                }] => v.value(),
                _ => None,
            },
        }
    }

    /// The value in force at `t`.
    ///
    /// Reports an asserted absence as `None`, the same as no coverage at all.
    /// Use [`Outcome::held_at`] where the difference matters — it does to a
    /// memory store, and this method is the convenience, not the precise answer.
    ///
    /// Fallible along with `held_at` rather than collapsing a contested span
    /// into `None`: `None` here already means *no coverage*, and flattening
    /// "two values and nothing orders them" into it is the same collapse the
    /// `Absent`/`Unknown` distinction exists to prevent.
    pub fn as_of(&self, t: Timestamp) -> Result<Option<&str>, Refused> {
        Ok(self.held_at(t)?.and_then(Held::value))
    }

    /// What held at `t`, distinguishing an asserted absence from no coverage.
    ///
    /// Refuses only when `t` lands in a contested span. A [`Outcome::Survivor`]
    /// never refuses: it has no time dimension -- that is what
    /// [`Strategy::keeps_a_timeline`] reports -- so it is `Ok` at every instant,
    /// and the `Result` is a shape the timeline arm needs rather than a
    /// behaviour every strategy acquires.
    pub fn held_at(&self, t: Timestamp) -> Result<Option<&Held>, Refused> {
        match self {
            Outcome::Survivor(v) => Ok(v.as_ref()),
            Outcome::Timeline(facts) => match facts.iter().find(|f| f.valid.contains(t)) {
                None => Ok(None),
                Some(Fact {
                    span: Span::Held(v),
                    ..
                }) => Ok(Some(v)),
                Some(Fact {
                    span:
                        Span::Contested {
                            values,
                            observed_at,
                        },
                    valid,
                }) => Err(Refused(contested(values, *observed_at, t, valid))),
            },
        }
    }
}

/// How to resolve competing assertions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Strategy {
    /// Longest value wins; ties go to the first seen.
    MostComplete,
    /// Alias of [`Strategy::MostComplete`], kept because the two names mean
    /// different things to callers even where the rule coincides.
    LongestValue,
    /// Most frequently asserted value wins; count ties go to the first seen.
    MajorityVote,
    /// Count-majority. Upstream documents this as the fallback shape of a
    /// confidence-weighted vote when no pair scores are supplied; no weighted
    /// form is implemented here, and the name is kept so the behaviour is
    /// requested explicitly rather than inherited by accident.
    ConfidenceMajority,
    /// The first non-null assertion in input order.
    FirstNonNull,
    /// The value if every non-null assertion agrees, otherwise nothing.
    /// For fields where a heuristic answer is worse than a gap.
    UnanimousOrNull,
    /// The most recently observed value.
    ///
    /// Refuses when the latest observation is a tie between different values:
    /// simultaneous contradictory assertions have no "most recent".
    MostRecent,
    /// The value from the highest-priority source that asserted one.
    ///
    /// Refuses when any asserting source is absent from the priority list —
    /// ranking an unlisted source would silently prefer the wrong system of
    /// record. Within the winning source, ties resolve by [`Strategy::MostRecent`].
    SourcePriority(Vec<Source>),
    /// Do not pick a winner. Emit each distinct value with the validity range
    /// over which it stood.
    ///
    /// Refuses only when two different values collide on *both* axes -- same
    /// `valid.from` and same `observed_at` -- because then nothing orders them
    /// and there is no way to say which superseded which.
    ///
    /// # The refusal is history-wide, not instant-local
    ///
    /// A collision anywhere in the visible history refuses the whole read,
    /// including a question about an instant nowhere near it. The outcome of
    /// this strategy is a timeline, and a timeline with a hole in it is not one
    /// that can be indexed into.
    ///
    /// `rm-contrast` measures what that costs where collisions are common: at a
    /// 25% tie rate the store refuses 4,067 of 6,353 questions that did have
    /// answers, against a flat control that refuses none because it has no way
    /// to. Making the refusal instant-local was considered and turned down --
    /// see the rejected decision "Make ValidInterval's refusal instant-local"
    /// in `docs/seed-decision-log.sh` -- because it has never fired on real
    /// data: zero collisions across 1,086 attribute slots in a live store,
    /// `observed_at` being millisecond-resolution and handed out per write.
    ///
    /// The condition that would reverse that: a bulk import carrying
    /// day-resolution timestamps on *both* axes, where `observed_at` collides
    /// routinely and ties stop being freak events.
    ///
    /// This sentence used to say "refuses when two different values share an
    /// observation timestamp", which was true when a `Candidate` carried a
    /// value and a provenance and no interval, so the timeline could only be
    /// cut at observation. A `Candidate` carries its validity now, and two
    /// values heard in the same instant are ordered by when they held. The
    /// code moved and the prose did not; `rm-conform`'s differential sweep
    /// found the gap by disagreeing on 53 generated histories.
    ValidInterval,
}

impl Strategy {
    /// Whether this strategy needs provenance beyond the values themselves.
    ///
    /// Exposed so a host can check its inputs once, up front, rather than
    /// discovering the gap per field.
    pub fn needs_provenance(&self) -> bool {
        matches!(
            self,
            Strategy::MostRecent | Strategy::SourcePriority(_) | Strategy::ValidInterval
        )
    }

    /// Whether this strategy's outcome can be asked about a moment in time.
    ///
    /// Only [`Strategy::ValidInterval`] emits a timeline. Every other strategy
    /// collapses a history to one winner, and a winner has no time dimension,
    /// so `held_at` returns the same value whatever instant it is handed.
    ///
    /// Exposed for the same reason as [`Self::needs_provenance`]: so a host can
    /// check up front rather than discovering it per read. Without it a caller
    /// asking what held in March gets an answer that is right about now and
    /// silent about the difference -- which is exactly what `rmem about
    /// --valid-at` did on every attribute but one.
    pub fn keeps_a_timeline(&self) -> bool {
        matches!(self, Strategy::ValidInterval)
    }
}

/// Resolve `candidates` under `strategy`.
///
/// Candidates are in the caller's insertion order, which is load-bearing:
/// count and length ties resolve to the first seen, so the caller controls
/// tie-breaks by controlling order.
pub fn merge(candidates: &[Candidate<'_>], strategy: &Strategy) -> Result<Outcome, Refused> {
    // ValidInterval answers a different question and has its own early-outs
    // (one distinct value still yields a timeline, not a bare survivor).
    if matches!(strategy, Strategy::ValidInterval) {
        return timeline(candidates).map(Outcome::Timeline);
    }

    let asserted: Vec<&Candidate<'_>> = candidates
        .iter()
        .filter(|c| c.value.is_assertion())
        .collect();
    if asserted.is_empty() {
        return Ok(Outcome::Survivor(None));
    }

    // Every assertion agrees -> that answer, whatever the strategy. Runs before
    // any strategy so unanimity never depends on the rule chosen.
    let first = asserted[0].value;
    if asserted.iter().all(|c| c.value == first) {
        return Ok(Outcome::Survivor(Some(held(first))));
    }

    let winner: Option<Held> = match strategy {
        Strategy::MostComplete | Strategy::LongestValue => longest(&asserted),
        Strategy::MajorityVote | Strategy::ConfidenceMajority => plurality(&asserted),
        Strategy::FirstNonNull => Some(held(first)),
        // Unanimity was handled by the early-out above, so reaching here means
        // the sources disagree. A gap is the point of this strategy.
        Strategy::UnanimousOrNull => None,
        Strategy::MostRecent => Some(most_recent(&asserted)?),
        Strategy::SourcePriority(order) => Some(by_source_priority(&asserted, order)?),
        Strategy::ValidInterval => unreachable!("handled above"),
    };

    Ok(Outcome::Survivor(winner))
}

/// The refusal for a question that landed in a contested span.
///
/// Names the interval so the answer is actionable rather than a dead end: a
/// caller learns both that this instant is contested and where the history
/// resumes being answerable. That is the whole difference between a refusal
/// that fits the question and one that does not.
fn contested(values: &[Held], observed_at: Timestamp, t: Timestamp, valid: &Interval) -> String {
    let named: Vec<String> = values
        .iter()
        .map(|v| match v {
            Held::Value(s) => format!("{s:?}"),
            Held::Absent => "an asserted absence".to_string(),
        })
        .collect();
    let resumes = match valid.to {
        Some(to) => format!("outside [{}, {to})", valid.from),
        None => format!("before {}", valid.from),
    };
    format!(
        "{} opened at {} and were all observed at {observed_at}, so none supersedes          the others and none can be said to have held at {t}. Distinguish their          observation times, or ask about an instant {resumes}.",
        named.join(" and "),
        valid.from
    )
}

/// An assertion as the owned result it would be if it won.
///
/// Never called with `Silent`: silence is filtered before any strategy runs, so
/// a `Silent` reaching here is a bug rather than a value to represent.
fn held(a: Asserted<'_>) -> Held {
    match a {
        Asserted::Value(v) => Held::Value(v.to_string()),
        Asserted::Absent => Held::Absent,
        Asserted::Silent => unreachable!("silence is filtered before any strategy runs"),
    }
}

/// Longest value, measured in characters.
///
/// Characters, not bytes: a byte length would rank "café" (4 chars, 5 bytes)
/// above "abcd" and tie it with "abcde", making completeness depend on the
/// accents in the text. Ties take the first seen. An `Absent` measures 0, so it
/// loses to any value — correct, since "most complete" prefers a value over an
/// absence.
fn longest(asserted: &[&Candidate<'_>]) -> Option<Held> {
    let max = asserted
        .iter()
        .map(|c| c.value.value().unwrap_or_default().chars().count())
        .max()?;
    asserted
        .iter()
        .find(|c| c.value.value().unwrap_or_default().chars().count() == max)
        .map(|c| held(c.value))
}

/// Most frequent value, breaking count ties by first appearance.
///
/// The tally is an insertion-ordered `Vec`, not a `HashMap`: iteration order
/// *is* the tie-break rule, and a hash map would make the winner depend on hash
/// seeding — a different answer per process for the same input.
fn plurality(asserted: &[&Candidate<'_>]) -> Option<Held> {
    let mut counts: Vec<(Asserted<'_>, usize)> = Vec::new();
    for c in asserted {
        let v = c.value;
        match counts.iter_mut().find(|(k, _)| *k == v) {
            Some((_, n)) => *n += 1,
            None => counts.push((v, 1)),
        }
    }
    // Strict `>` keeps the first maximum. `max_by_key` would keep the last.
    counts
        .iter()
        .fold(None::<(Asserted<'_>, usize)>, |best, &(v, n)| match best {
            Some((_, bn)) if n <= bn => best,
            _ => Some((v, n)),
        })
        .map(|(v, _)| held(v))
}

/// The value from the latest observation.
///
/// Refuses on a contradictory tie at the latest timestamp. Two different values
/// observed at the same instant have no order between them, and picking either
/// would be the arbitrary answer wearing a deterministic hat that upstream
/// refuses to give.
fn most_recent(asserted: &[&Candidate<'_>]) -> Result<Held, Refused> {
    let latest = asserted
        .iter()
        .map(|c| c.provenance.observed_at)
        .max()
        .expect("non-empty");

    let mut at_latest: Vec<Asserted<'_>> = asserted
        .iter()
        .filter(|c| c.provenance.observed_at == latest)
        .map(|c| c.value)
        .collect();
    at_latest.dedup();

    match at_latest.as_slice() {
        [only] => Ok(held(*only)),
        [first, ..] if at_latest.iter().all(|v| v == first) => Ok(held(*first)),
        _ => Err(Refused(format!(
            "{} different values share the latest observation time ({latest}); \
             simultaneous contradictory assertions have no \"most recent\". \
             Order them, or resolve with SourcePriority or ValidInterval.",
            at_latest.len()
        ))),
    }
}

/// The value from the first source in `order` that asserted one.
///
/// Refuses when an asserting source is unlisted rather than ranking it last:
/// an unranked source is an unanswered policy question, and defaulting it to
/// lowest priority silently prefers whatever the caller did remember to list.
fn by_source_priority(asserted: &[&Candidate<'_>], order: &[Source]) -> Result<Held, Refused> {
    if order.is_empty() {
        return Err(Refused(
            "SourcePriority was given an empty priority list, so it ranks nothing. \
             Supply the source order, or choose a strategy that does not need one."
                .to_string(),
        ));
    }

    let unlisted: Vec<String> = {
        let mut seen: Vec<&Source> = Vec::new();
        for c in asserted {
            let s = &c.provenance.source;
            if !order.contains(s) && !seen.contains(&s) {
                seen.push(s);
            }
        }
        seen.iter().map(|s| format!("{s:?}")).collect()
    };
    if !unlisted.is_empty() {
        return Err(Refused(format!(
            "these sources asserted a value but are absent from the priority list: {}. \
             Ranking them last would silently prefer the sources you did list; \
             add them to the order, or choose a strategy that does not rank sources.",
            unlisted.join(", ")
        )));
    }

    for source in order {
        let tier: Vec<&Candidate<'_>> = asserted
            .iter()
            .copied()
            .filter(|c| &c.provenance.source == source)
            .collect();
        if tier.is_empty() {
            continue;
        }
        // Within one source, later observations supersede earlier ones — the
        // same source revising itself is not a conflict between sources.
        return most_recent(&tier);
    }

    // Unreachable in practice: asserted is non-empty and every source is
    // listed, so some tier matched. Refuse rather than unwrap if that changes.
    Err(Refused(
        "no candidate matched any listed source, despite every source being listed. \
         This is a bug in rm-survivor; please report it."
            .to_string(),
    ))
}

/// Build a timeline of values from observation order.
///
/// Each distinct value holds from when it was observed until the next *different*
/// value was observed; the last holds open-ended. Repeated assertions of the same
/// value extend its span rather than starting a new one — re-hearing a fact is
/// not a change. A value that returns after being superseded (Acme, Globex, Acme)
/// correctly yields three spans: it was true, then not, then true again.
fn timeline(candidates: &[Candidate<'_>]) -> Result<Vec<Fact>, Refused> {
    let mut asserted: Vec<&Candidate<'_>> = candidates
        .iter()
        .filter(|c| c.value.is_assertion())
        .collect();
    if asserted.is_empty() {
        return Ok(Vec::new());
    }

    // Stable sort: candidates sharing a timestamp keep their input order, which
    // matters for the conflict check below reporting the caller's own ordering.
    // By when each held, with the observation breaking ties. Valid time is the
    // axis this strategy is named for; observation is what orders two things
    // said to have begun at the same moment, and is a total order because the
    // store stamps every write.
    asserted.sort_by_key(|c| (c.valid.from, c.provenance.observed_at));

    for pair in asserted.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if a.valid.from == b.valid.from
            && a.provenance.observed_at == b.provenance.observed_at
            && a.value != b.value
        {
            return Err(Refused(format!(
                "{:?} and {:?} were both observed at {}, so neither supersedes the other \
                 and no validity range can be assigned. Distinguish their observation \
                 times, or resolve with SourcePriority.",
                a.value, b.value, a.provenance.observed_at
            )));
        }
    }

    let mut facts: Vec<Fact> = Vec::new();
    for c in &asserted {
        let value = held(c.value);
        if facts
            .last()
            .is_some_and(|f| f.span == Span::Held(value.clone()))
        {
            continue; // same value restated: extends the open span, no new fact
        }
        facts.push(Fact {
            span: Span::Held(value),
            valid: Interval::since(c.valid.from),
        });
    }

    // Close each span where the next one opens, leaving the last open-ended.
    for i in 0..facts.len().saturating_sub(1) {
        let next_start = facts[i + 1].valid.from;
        facts[i].valid.to = Some(next_start);
    }

    Ok(facts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prov(source: Source, observed_at: Timestamp) -> Provenance {
        Provenance::new(source, observed_at, "test")
    }

    /// Build candidates from `(value, source, observed_at)` triples.
    macro_rules! cands {
        ($($v:expr, $s:expr, $t:expr);* $(;)?) => {{
            let provs: Vec<Provenance> = vec![$(prov($s, $t)),*];
            let vals: Vec<Option<&str>> = vec![$($v),*];
            (provs, vals)
        }};
    }

    fn build<'a>(provs: &'a [Provenance], vals: &'a [Option<&'a str>]) -> Vec<Candidate<'a>> {
        vals.iter()
            .zip(provs.iter())
            .map(|(v, p)| Candidate::new(*v, p))
            .collect()
    }

    // ---- ported behaviour: the value-only strategies -----------------------

    #[test]
    fn all_null_yields_no_survivor_for_every_value_strategy() {
        let (p, v) = cands![None, Source::UserAssertion, 1; None, Source::ToolOutput, 2];
        let c = build(&p, &v);
        for s in [
            Strategy::MostComplete,
            Strategy::LongestValue,
            Strategy::MajorityVote,
            Strategy::ConfidenceMajority,
            Strategy::FirstNonNull,
            Strategy::UnanimousOrNull,
            Strategy::MostRecent,
        ] {
            assert_eq!(merge(&c, &s).unwrap(), Outcome::Survivor(None), "{s:?}");
        }
    }

    #[test]
    fn a_single_distinct_value_wins_regardless_of_strategy() {
        let (p, v) = cands![Some("a"), Source::UserAssertion, 1; None, Source::ToolOutput, 2; Some("a"), Source::AgentInference, 3];
        let c = build(&p, &v);
        for s in [
            Strategy::MostComplete,
            Strategy::MajorityVote,
            Strategy::FirstNonNull,
            Strategy::UnanimousOrNull,
            Strategy::MostRecent,
        ] {
            assert_eq!(merge(&c, &s).unwrap().survivor(), Some("a"), "{s:?}");
        }
    }

    #[test]
    fn most_complete_takes_the_longest_then_the_first() {
        let (p, v) =
            cands![Some("ab"), Source::UserAssertion, 1; Some("abcd"), Source::ToolOutput, 2];
        assert_eq!(
            merge(&build(&p, &v), &Strategy::MostComplete)
                .unwrap()
                .survivor(),
            Some("abcd")
        );
        let (p, v) =
            cands![Some("ab"), Source::UserAssertion, 1; Some("cd"), Source::ToolOutput, 2];
        assert_eq!(
            merge(&build(&p, &v), &Strategy::MostComplete)
                .unwrap()
                .survivor(),
            Some("ab")
        );
    }

    #[test]
    fn length_is_counted_in_characters_not_bytes() {
        // "café" is 4 chars / 5 bytes; "abcde" is 5 chars / 5 bytes. Comparing
        // bytes would call these tied and take the first.
        let (p, v) =
            cands![Some("café"), Source::UserAssertion, 1; Some("abcde"), Source::ToolOutput, 2];
        assert_eq!(
            merge(&build(&p, &v), &Strategy::LongestValue)
                .unwrap()
                .survivor(),
            Some("abcde")
        );
    }

    #[test]
    fn majority_vote_breaks_count_ties_by_first_seen() {
        let (p, v) = cands![Some("b"), Source::UserAssertion, 1; Some("a"), Source::ToolOutput, 2; Some("a"), Source::ToolOutput, 3];
        assert_eq!(
            merge(&build(&p, &v), &Strategy::MajorityVote)
                .unwrap()
                .survivor(),
            Some("a")
        );
        // 1-1: insertion order decides, so "b".
        let (p, v) = cands![Some("b"), Source::UserAssertion, 1; Some("a"), Source::ToolOutput, 2];
        assert_eq!(
            merge(&build(&p, &v), &Strategy::MajorityVote)
                .unwrap()
                .survivor(),
            Some("b")
        );
    }

    #[test]
    fn unanimous_or_null_emits_nothing_on_disagreement() {
        let (p, v) = cands![Some("a"), Source::UserAssertion, 1; Some("b"), Source::ToolOutput, 2];
        assert_eq!(
            merge(&build(&p, &v), &Strategy::UnanimousOrNull).unwrap(),
            Outcome::Survivor(None)
        );
        // Absence is not contradiction.
        let (p, v) = cands![Some("a"), Source::UserAssertion, 1; None, Source::ToolOutput, 2; Some("a"), Source::ToolOutput, 3];
        assert_eq!(
            merge(&build(&p, &v), &Strategy::UnanimousOrNull)
                .unwrap()
                .survivor(),
            Some("a")
        );
    }

    #[test]
    fn first_non_null_skips_leading_nulls() {
        let (p, v) = cands![None, Source::UserAssertion, 1; Some("x"), Source::ToolOutput, 2; Some("y"), Source::ToolOutput, 3];
        assert_eq!(
            merge(&build(&p, &v), &Strategy::FirstNonNull)
                .unwrap()
                .survivor(),
            Some("x")
        );
    }

    // ---- the strategies upstream had to refuse ----------------------------

    #[test]
    fn most_recent_uses_observation_time_not_input_order() {
        // "old" is listed last but observed first; recency must beat position.
        let (p, v) =
            cands![Some("new"), Source::ToolOutput, 500; Some("old"), Source::UserAssertion, 100];
        assert_eq!(
            merge(&build(&p, &v), &Strategy::MostRecent)
                .unwrap()
                .survivor(),
            Some("new")
        );
    }

    #[test]
    fn most_recent_refuses_a_contradictory_simultaneous_tie() {
        let (p, v) = cands![Some("acme"), Source::UserAssertion, 100; Some("globex"), Source::ToolOutput, 100];
        let err = merge(&build(&p, &v), &Strategy::MostRecent).unwrap_err();
        assert!(err.0.contains("no \"most recent\""), "{}", err.0);
    }

    #[test]
    fn most_recent_tolerates_agreement_at_the_latest_instant() {
        let (p, v) = cands![Some("old"), Source::UserAssertion, 1; Some("new"), Source::ToolOutput, 9; Some("new"), Source::AgentInference, 9];
        assert_eq!(
            merge(&build(&p, &v), &Strategy::MostRecent)
                .unwrap()
                .survivor(),
            Some("new")
        );
    }

    #[test]
    fn source_priority_prefers_the_higher_ranked_source() {
        let (p, v) = cands![Some("guessed"), Source::AgentInference, 900; Some("stated"), Source::UserAssertion, 100];
        let order = vec![Source::UserAssertion, Source::AgentInference];
        assert_eq!(
            merge(&build(&p, &v), &Strategy::SourcePriority(order))
                .unwrap()
                .survivor(),
            // Rank beats recency: a fresh inference does not outrank what the
            // user actually said.
            Some("stated")
        );
    }

    #[test]
    fn source_priority_breaks_within_tier_ties_by_recency() {
        let (p, v) = cands![Some("first"), Source::UserAssertion, 100; Some("second"), Source::UserAssertion, 200];
        let order = vec![Source::UserAssertion];
        assert_eq!(
            merge(&build(&p, &v), &Strategy::SourcePriority(order))
                .unwrap()
                .survivor(),
            Some("second")
        );
    }

    #[test]
    fn source_priority_refuses_an_unlisted_source() {
        let (p, v) = cands![Some("a"), Source::UserAssertion, 1; Some("b"), Source::External("crm".into()), 2];
        let order = vec![Source::UserAssertion];
        let err = merge(&build(&p, &v), &Strategy::SourcePriority(order)).unwrap_err();
        assert!(err.0.contains("absent from the priority list"), "{}", err.0);
        assert!(
            err.0.contains("crm"),
            "the refusal should name the source: {}",
            err.0
        );
    }

    #[test]
    fn source_priority_refuses_an_empty_order() {
        let (p, v) = cands![Some("a"), Source::UserAssertion, 1; Some("b"), Source::ToolOutput, 2];
        let err = merge(&build(&p, &v), &Strategy::SourcePriority(vec![])).unwrap_err();
        assert!(err.0.contains("empty priority list"), "{}", err.0);
    }

    // ---- the strategy neither system could state -------------------------

    #[test]
    fn valid_interval_keeps_both_values_instead_of_picking_one() {
        // The thesis: this is not a conflict, it is two facts.
        let (p, v) = cands![Some("acme"), Source::UserAssertion, 100; Some("globex"), Source::UserAssertion, 200];
        let out = merge(&build(&p, &v), &Strategy::ValidInterval).unwrap();
        assert_eq!(
            out,
            Outcome::Timeline(vec![
                Fact {
                    span: Span::Held(Held::Value("acme".to_string())),
                    valid: Interval::between(100, 200)
                },
                Fact {
                    span: Span::Held(Held::Value("globex".to_string())),
                    valid: Interval::since(200)
                },
            ])
        );
        // And the store can answer "as of when".
        assert_eq!(out.as_of(150).unwrap(), Some("acme"));
        assert_eq!(out.as_of(200).unwrap(), Some("globex"));
        assert_eq!(out.as_of(99).unwrap(), None); // nothing known before the first observation
    }

    #[test]
    fn valid_interval_orders_by_observation_not_input_order() {
        let (p, v) = cands![Some("globex"), Source::UserAssertion, 200; Some("acme"), Source::UserAssertion, 100];
        let out = merge(&build(&p, &v), &Strategy::ValidInterval).unwrap();
        assert_eq!(out.as_of(150).unwrap(), Some("acme"));
    }

    #[test]
    fn restating_a_value_extends_its_span_rather_than_splitting_it() {
        let (p, v) = cands![Some("acme"), Source::UserAssertion, 100; Some("acme"), Source::ToolOutput, 150; Some("globex"), Source::UserAssertion, 200];
        let Outcome::Timeline(facts) = merge(&build(&p, &v), &Strategy::ValidInterval).unwrap()
        else {
            panic!("expected a timeline")
        };
        assert_eq!(
            facts.len(),
            2,
            "re-hearing a fact is not a change: {facts:?}"
        );
        assert_eq!(facts[0].valid, Interval::between(100, 200));
    }

    #[test]
    fn a_value_that_returns_gets_a_second_span() {
        // Worked at Acme, left, came back. Three spans, not two.
        let (p, v) = cands![Some("acme"), Source::UserAssertion, 100; Some("globex"), Source::UserAssertion, 200; Some("acme"), Source::UserAssertion, 300];
        let Outcome::Timeline(facts) = merge(&build(&p, &v), &Strategy::ValidInterval).unwrap()
        else {
            panic!("expected a timeline")
        };
        assert_eq!(facts.len(), 3);
        assert_eq!(facts[2].valid, Interval::since(300));
        assert_eq!(
            merge(&build(&p, &v), &Strategy::ValidInterval)
                .unwrap()
                .as_of(250)
                .unwrap(),
            Some("globex")
        );
    }

    #[test]
    fn valid_interval_spans_never_overlap() {
        let (p, v) = cands![Some("a"), Source::UserAssertion, 10; Some("b"), Source::UserAssertion, 20; Some("c"), Source::UserAssertion, 30];
        let Outcome::Timeline(facts) = merge(&build(&p, &v), &Strategy::ValidInterval).unwrap()
        else {
            panic!("expected a timeline")
        };
        for t in 0..40 {
            let hits = facts.iter().filter(|f| f.valid.contains(t)).count();
            assert!(hits <= 1, "t={t} matched {hits} spans: {facts:?}");
        }
    }

    #[test]
    fn valid_interval_refuses_simultaneous_contradictions() {
        let (p, v) = cands![Some("acme"), Source::UserAssertion, 100; Some("globex"), Source::ToolOutput, 100];
        let err = merge(&build(&p, &v), &Strategy::ValidInterval).unwrap_err();
        assert!(err.0.contains("neither supersedes the other"), "{}", err.0);
    }

    #[test]
    fn valid_interval_on_nothing_is_an_empty_timeline() {
        let (p, v) = cands![None, Source::UserAssertion, 1];
        assert_eq!(
            merge(&build(&p, &v), &Strategy::ValidInterval).unwrap(),
            Outcome::Timeline(vec![])
        );
    }

    #[test]
    fn one_value_still_yields_a_timeline_open_ended_from_its_observation() {
        let (p, v) = cands![Some("acme"), Source::UserAssertion, 100];
        let out = merge(&build(&p, &v), &Strategy::ValidInterval).unwrap();
        assert_eq!(
            out,
            Outcome::Timeline(vec![Fact {
                span: Span::Held(Held::Value("acme".to_string())),
                valid: Interval::since(100)
            }])
        );
        assert_eq!(out.survivor(), Some("acme"));
    }

    // ---- the discipline ---------------------------------------------------

    #[test]
    fn a_refusal_beats_a_plausible_wrong_survivor() {
        // The property every refusal exists for: these must NOT quietly return
        // a value. A wrong memory raises nothing and looks right.
        let (p, v) = cands![Some("acme"), Source::UserAssertion, 100; Some("globex"), Source::ToolOutput, 100];
        let c = build(&p, &v);
        assert!(merge(&c, &Strategy::MostRecent).is_err());
        assert!(merge(&c, &Strategy::ValidInterval).is_err());
        assert!(merge(&c, &Strategy::SourcePriority(vec![])).is_err());
    }

    #[test]
    fn every_refusal_explains_what_was_missing() {
        let (p, v) = cands![Some("a"), Source::UserAssertion, 5; Some("b"), Source::ToolOutput, 5];
        let c = build(&p, &v);
        for s in [
            Strategy::MostRecent,
            Strategy::ValidInterval,
            Strategy::SourcePriority(vec![]),
        ] {
            let err = merge(&c, &s).unwrap_err();
            assert!(
                err.0.len() > 40,
                "{s:?} refused without a reason: {}",
                err.0
            );
        }
    }

    #[test]
    fn needs_provenance_flags_exactly_the_strategies_that_read_it() {
        assert!(Strategy::MostRecent.needs_provenance());
        assert!(Strategy::ValidInterval.needs_provenance());
        assert!(Strategy::SourcePriority(vec![]).needs_provenance());
        assert!(!Strategy::MajorityVote.needs_provenance());
        assert!(!Strategy::MostComplete.needs_provenance());
        assert!(!Strategy::FirstNonNull.needs_provenance());
        assert!(!Strategy::UnanimousOrNull.needs_provenance());
        assert!(!Strategy::ConfidenceMajority.needs_provenance());
    }

    #[test]
    fn an_absence_competes_rather_than_being_treated_as_silence() {
        // "They left Acme and are between jobs" is an assertion, not a gap in
        // what we heard. Under MostRecent it has to be able to win.
        let early = prov(Source::UserAssertion, 1);
        let late = prov(Source::UserAssertion, 2);
        let candidates = vec![
            Candidate::new(Some("Acme"), &early),
            Candidate::absent(&late),
        ];
        let outcome = merge(&candidates, &Strategy::MostRecent).unwrap();
        assert_eq!(outcome.held_at(0).unwrap(), Some(&Held::Absent));
    }

    #[test]
    fn silence_still_never_contradicts_an_assertion() {
        // The existing rule is unchanged: a source with nothing to say does not
        // compete, even when it is the most recent thing we heard.
        let early = prov(Source::UserAssertion, 1);
        let late = prov(Source::UserAssertion, 2);
        let candidates = vec![
            Candidate::new(Some("Acme"), &early),
            Candidate::new(None, &late),
        ];
        let outcome = merge(&candidates, &Strategy::MostRecent).unwrap();
        assert_eq!(outcome.as_of(0).unwrap(), Some("Acme"));
    }

    #[test]
    fn a_timeline_can_hold_a_gap_between_two_values() {
        // Acme, then unemployed, then Globex: three spans, the middle one absent.
        let p1 = prov(Source::UserAssertion, 10);
        let p2 = prov(Source::UserAssertion, 20);
        let p3 = prov(Source::UserAssertion, 30);
        let candidates = vec![
            Candidate::new(Some("Acme"), &p1),
            Candidate::absent(&p2),
            Candidate::new(Some("Globex"), &p3),
        ];
        let outcome = merge(&candidates, &Strategy::ValidInterval).unwrap();
        assert_eq!(
            outcome.held_at(15).unwrap(),
            Some(&Held::Value("Acme".to_string()))
        );
        assert_eq!(outcome.held_at(25).unwrap(), Some(&Held::Absent));
        assert_eq!(
            outcome.held_at(35).unwrap(),
            Some(&Held::Value("Globex".to_string()))
        );
        assert_eq!(
            outcome.as_of(25).unwrap(),
            None,
            "as_of reports an absence as no value"
        );
    }

    /// The mirror of `needs_provenance_flags_exactly_the_strategies_that_read_it`.
    /// A strategy added later that emits a timeline and is not listed here
    /// would be silently unaskable about time.
    #[test]
    fn keeps_a_timeline_flags_exactly_the_strategy_that_emits_one() {
        assert!(Strategy::ValidInterval.keeps_a_timeline());
        for s in [
            Strategy::MostRecent,
            Strategy::MostComplete,
            Strategy::LongestValue,
            Strategy::MajorityVote,
            Strategy::ConfidenceMajority,
            Strategy::FirstNonNull,
            Strategy::UnanimousOrNull,
            Strategy::SourcePriority(vec![]),
        ] {
            assert!(
                !s.keeps_a_timeline(),
                "{s:?} collapses to a winner, which has no time dimension"
            );
        }
    }
}

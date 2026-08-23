//! Two measurements recall@10 cannot make.
//!
//! # Why this module exists
//!
//! Six real defects were found on this corpus and fixed: the review band was
//! unanswerable, possessives matched their owners, `kind` was withheld from the
//! resolver, articles carried identity, the speaker could not be passed, and
//! arrival order was being reported as contradiction. **Retrieval recall moved
//! for none of them.** It never could: `recall` is embedding search over a
//! fact's own text, and it reads neither the attribute name, nor the entity a
//! fact is attached to, nor whether anything later replaced it.
//!
//! So the metric was scoring a narrower thing than the project was building.
//! These two score the rest of it, and neither needs a model to judge them.
//!
//! # What LoCoMo's category 5 actually is
//!
//! It is documented as adversarial — "the question presumes something the
//! conversation does not support" — and the harness used to report it as
//! "surfaced something for a question the conversation does not answer".
//!
//! That reading is wrong, and reading four of them is enough to see it:
//!
//! ```text
//!   "What did Caroline realize after her charity race?"
//!        -> D2:3 is MELANIE realizing self-care is important
//!   "What are Melanie's plans for the summer with respect to adoption?"
//!        -> D2:8 is CAROLINE researching adoption agencies
//! ```
//!
//! The fact is in the conversation. It belongs to the other speaker. Category 5
//! is a **misattribution** trap, and `adversarial_answer` is the true statement
//! about the wrong person that a store is expected to hand over.
//!
//! Tested across all ten conversations rather than trusted from four examples,
//! over every question naming exactly one of the two speakers:
//!
//! ```text
//!   category            questions   evidence ENTIRELY by the other speaker
//!   multi-hop, open-domain    316                                       0%
//!   temporal, single-hop    1,051                                     3-4%
//!   adversarial               443                                      75%
//! ```
//!
//! Which means surfacing the evidence turn was never the failure — it is the
//! *right* thing to retrieve. The failure is handing it over as a fact about
//! the person who was asked about. A store that keys facts to resolved
//! entities can tell the difference; a flat pile of embedded sentences cannot.
//! That is the claim five PRs of resolution work were making, and nothing has
//! ever measured it.
//!
//! # What the two report
//!
//! All ten conversations, stores built from cached extractions so the only
//! variable is the scoring. 1,435 answerable questions and 372 misattribution
//! traps.
//!
//! One margin is applied to every conversation rather than each conversation's
//! own best. Per-conversation bests range from +0.180 to +0.489, and quoting
//! that spread as the result would be reporting a cutoff fitted ten times:
//!
//! ```text
//!   margin   sensitivity   specificity        J
//!    -1.00         0.737         0.546   +0.283
//!    -0.15         0.822         0.508   +0.330   <- pooled best
//!    -0.10         0.868         0.433   +0.300
//!    -0.05         0.928         0.312   +0.239
//!    +0.00         0.969         0.153   +0.123
//!    +1.00         1.000         0.003   +0.003
//! ```
//!
//! -0.15 is also the best margin for 8 of the 10 conversations taken singly,
//! which is the only reason to trust it as more than a fitted number.
//!
//! J of 0.33 is the first figure in this project that responds to the
//! resolution work at all. It is also nowhere near solved: at that operating
//! point half the traps still come back answered as though they were about the
//! person asked after.
//!
//! Staleness, pooled the same way:
//!
//! ```text
//!   hits corrected                            805 / 18,380   4.4%
//!   questions whose top hit was corrected      63 / 1,973    3.2%
//!     ...with the live value ranked below it   19 / 63        30%
//! ```
//!
//! That last line was read off three conversations first and called "almost
//! none", which was wrong -- pooled it is a third. So a third of the questions
//! that lead with a replaced fact are a ranking failure the store could fix by
//! reordering what it already holds, and two thirds are the store not holding
//! the current answer at all. Both are worth fixing and they need different
//! work, which is the distinction the counter exists to draw.
//!
//! Both numbers are a floor rather than a verdict. `Supersession` is filled in
//! by tombstones and resolved survivorship only -- the prompt that asked a
//! model to fill in the rest was withdrawn for costing 19% of the facts -- so
//! every ordinary correction in these stores reads as `Unstated` and is counted
//! here as still standing.

use std::collections::BTreeMap;

use rm_engine::{Engine, Recalled, StableId, Standing};

/// Whether a question's answer should come from the person it names.
///
/// Ground truth is taken from the corpus, not from the category label. A
/// category-5 question whose evidence *is* spoken by the person named is not a
/// misattribution trap whatever LoCoMo calls it, and scoring it as one would
/// credit the store for refusing something it should have answered -- 25% of
/// them, which is too many to wave through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expected {
    /// The named person is the subject. Answering is right.
    Answer,
    /// Every evidence turn belongs to the other speaker. Refusing is right.
    Refuse,
}

/// Every question's best evidence, on the subject and off it.
///
/// Scores rather than a verdict, because the verdict is the thing under
/// examination. A first attempt asked "is any of the ten hits attached to the
/// person named", scored 1.000 on both classes and separated nothing: with two
/// speakers and a few hundred entities, every result list contains something
/// for everybody. That is not a store that cannot refuse -- it is a question
/// that was not worth asking.
///
/// What the trap is actually about is *comparative*. "What did Caroline realize
/// after her charity race?" pulls up Melanie realizing self-care matters,
/// because that is the sentence the question sounds like. The store has grounds
/// to refuse when its best evidence sits on somebody else and nothing near as
/// good sits on the person asked about. So both are recorded and the margin
/// between them is swept afterwards, which reports whether the signal exists at
/// all rather than assuming a cutoff and reporting one number.
#[derive(Default)]
pub struct Attribution {
    /// `(what should happen, best on-subject score, best off-subject score)`.
    /// A missing score means nothing in the top-k was attached to that side.
    scored: Vec<(Expected, Option<f32>, Option<f32>)>,
    /// Questions naming both speakers or neither, which make no attribution
    /// claim to test. Reported so the totals reconcile with the question count.
    pub skipped: usize,
}

impl Attribution {
    /// Record one question.
    ///
    /// A hit counts as on-subject if the entity it is attached to carries that
    /// name -- resolved identity, not the text of the fact, which is the whole
    /// point: the fact's text says "self-care is important" either way, and
    /// only the entity says whose.
    pub fn observe(&mut self, engine: &Engine, hits: &[Recalled], subject: &str, want: Expected) {
        let best = |on: bool| {
            hits.iter()
                .filter(|h| names(engine, h.entity, subject) == on)
                .map(|h| h.score)
                .max_by(f32::total_cmp)
        };
        self.scored.push((want, best(true), best(false)));
    }

    /// The margin rule at one cutoff: refuse when the best evidence off the
    /// subject beats the best evidence on it by more than `margin`.
    ///
    /// A question with nothing on-subject at all refuses at every cutoff; one
    /// with nothing off-subject answers at every cutoff. Neither is a special
    /// case worth a branch -- they are the ends of the same scale.
    fn at(&self, margin: f32) -> (usize, usize, usize, usize) {
        let (mut ar, mut rw, mut rr, mut aw) = (0, 0, 0, 0);
        for (want, on, off) in &self.scored {
            let refuse = match (on, off) {
                (None, None) => true,
                (None, Some(_)) => true,
                (Some(_), None) => false,
                (Some(on), Some(off)) => off - on > margin,
            };
            match (want, refuse) {
                (Expected::Answer, false) => ar += 1,
                (Expected::Answer, true) => rw += 1,
                (Expected::Refuse, true) => rr += 1,
                (Expected::Refuse, false) => aw += 1,
            }
        }
        (ar, rw, rr, aw)
    }

    pub fn report(&self) {
        if self.scored.is_empty() {
            println!("\n=== attribution ===\nno question named exactly one speaker");
            return;
        }
        let answerable = self
            .scored
            .iter()
            .filter(|(w, _, _)| *w == Expected::Answer)
            .count();
        let refusable = self.scored.len() - answerable;

        println!("\n=== attribution (is the best evidence on the person who was asked about?) ===");
        println!(
            "questions scored     {}  ({answerable} answerable, {refusable} misattribution traps, \
             {} named both speakers or neither)",
            self.scored.len(),
            self.skipped
        );
        if answerable == 0 || refusable == 0 {
            println!("  only one class present; nothing to separate");
            return;
        }

        // Swept rather than tuned. A single cutoff picked here would be fitted
        // to this conversation and would read as a result; the sweep shows the
        // shape, and a flat one is the honest report that no cutoff works.
        println!("  margin   answered right   refused right   separation (J)");
        let mut best = (f32::NEG_INFINITY, 0.0f32);
        for margin in [-1.0f32, -0.15, -0.10, -0.05, -0.02, 0.0, 0.02, 0.05, 0.10, 1.0] {
            let (ar, _, rr, _) = self.at(margin);
            let sens = ar as f64 / answerable as f64;
            let spec = rr as f64 / refusable as f64;
            let j = sens + spec - 1.0;
            // Neither end is "always". Scores live in [0, 1], so a margin of
            // -1 refuses whenever any off-subject evidence exists at all, and
            // +1 refuses only when nothing on-subject came back -- the `(Some,
            // None)` and `(None, Some)` branches in `at` are what stop the
            // sweep degenerating, and mislabelling them as the constant rules
            // would hide where the separation actually comes from.
            let tag = match margin {
                m if m <= -1.0 => "  (any off-subject evidence -> refuse)",
                m if m >= 1.0 => "  (refuse only if nothing on-subject)",
                _ => "",
            };
            println!(
                "  {margin:>+6.2}   {ar:>4}/{answerable:<4} {sens:.3}   {rr:>4}/{refusable:<4} \
                 {spec:.3}   {j:+.3}{tag}"
            );
            if j as f32 > best.0 {
                best = (j as f32, margin);
            }
        }
        println!(
            "  <- J is (sensitivity + specificity - 1): 0.0 for any rule that ignores the\n                  question, 1.0 for perfect separation. Best here {:+.3} at margin {:+.2}.",
            best.0, best.1
        );
    }
}

/// Whether the entity `id` is known by `name`.
///
/// A containment test on lowercased text, because the corpus asks about
/// "Melanie" and the store may hold "Melanie Torres" -- and because the
/// alternative, running the resolver's own comparator here, would score the
/// store with the very thing under test.
fn names(engine: &Engine, id: StableId, name: &str) -> bool {
    engine
        .identity_of(id)
        .and_then(|r| r.get("name"))
        .is_some_and(|held| held.to_lowercase().contains(&name.to_lowercase()))
}

/// How often recall puts a fact that was corrected at the top.
///
/// `Standing::Corrected` means a later assertion in the same slot said it
/// replaces this one. Surfacing such a fact is not itself wrong -- "what did I
/// believe about her employer in May" needs it, and it comes back marked. The
/// failure is *ranking*: an agent that reads the first hit and states it will
/// state the stale one, and the mark it ignored was the only thing standing
/// between that and a confident wrong answer.
///
/// The third counter is the one worth having. When the top hit is stale, was
/// the current value sitting further down the same result list? That separates
/// "the store does not hold the answer" from "the store holds it and ranked the
/// dead one first", and only the second is fixable by ranking.
#[derive(Default)]
pub struct Staleness {
    pub questions: usize,
    pub hits_returned: usize,
    pub hits_corrected: usize,
    /// Questions whose best hit had been corrected.
    pub top_corrected: usize,
    /// ...of those, ones where a live version of the *same slot* was also
    /// returned, lower down.
    pub top_corrected_with_live_below: usize,
}

impl Staleness {
    pub fn observe(&mut self, hits: &[Recalled]) {
        // `kind` is excluded throughout, and the first run of this eval is why:
        // it reported 25.3% of every hit as corrected, which sounded like a
        // finding and was an artefact. `ingest` re-asserts `kind` for every
        // mention of a thing and each assertion corrects the last, so a
        // frequently-mentioned entity carries dozens of them and they are all
        // stale by construction. Nothing is learned from measuring that.
        let hits: Vec<&Recalled> = hits.iter().filter(|h| h.attribute != "kind").collect();

        self.questions += 1;
        self.hits_returned += hits.len();
        self.hits_corrected += hits
            .iter()
            .filter(|h| h.standing == Standing::Corrected)
            .count();

        let Some(top) = hits.first() else { return };
        if top.standing != Standing::Corrected {
            return;
        }
        self.top_corrected += 1;

        // The same slot means the same attribute on the same entity. A live
        // value for a *different* attribute is not the answer this question was
        // ranked wrongly against.
        let live_below = hits.iter().skip(1).any(|h| {
            h.entity == top.entity && h.attribute == top.attribute && h.standing.still_stands()
        });
        if live_below {
            self.top_corrected_with_live_below += 1;
        }
    }

    pub fn report(&self) {
        println!("\n=== staleness (does recall lead with a fact that was replaced?) ===");
        if self.questions == 0 {
            println!("nothing asked");
            return;
        }
        println!(
            "hits returned        {} across {} questions  (`kind` excluded -- see `observe`)",
            self.hits_returned, self.questions
        );
        println!(
            "  corrected          {} = {:.1}% of every hit handed back",
            self.hits_corrected,
            100.0 * self.hits_corrected as f64 / self.hits_returned.max(1) as f64
        );
        println!(
            "top hit corrected    {}/{} = {:.3}",
            self.top_corrected,
            self.questions,
            self.top_corrected as f64 / self.questions as f64
        );
        println!(
            "  ...with the live value further down the same list: {}",
            self.top_corrected_with_live_below
        );
        println!(
            "  <- those are a ranking failure and nothing else: the store held the\n     \
             current answer and offered the dead one first."
        );
    }
}

/// Every entity the store knows by this name.
///
/// A set rather than one id, because resolution is imperfect and the same
/// person often ends up on more than one entity -- "Caroline" and "Caroline
/// Reyes" may never have merged. Boosting all of them is right: they are all
/// the subject as far as anyone can tell, and a boost costs nothing where it
/// lands on a genuine namesake.
pub fn entities_named(engine: &Engine, name: &str) -> Vec<StableId> {
    engine
        .entity_ids()
        .into_iter()
        .filter(|id| names(engine, *id, name))
        .collect()
}

/// Which of the two speakers a question is about, if exactly one of them.
///
/// A question naming both ("did Caroline tell Melanie about the parade") makes
/// no single attribution claim, and one naming neither cannot be scored against
/// a subject at all. Both are counted out rather than guessed at.
pub fn subject_of<'a>(question: &str, a: &'a str, b: &'a str) -> Option<&'a str> {
    match (question.contains(a), question.contains(b)) {
        (true, false) => Some(a),
        (false, true) => Some(b),
        _ => None,
    }
}

/// What the corpus says should happen, read from the evidence rather than the
/// category label.
///
/// `speaker_of` maps a turn id to who said it. A question is a misattribution
/// trap when every turn offered as its evidence was spoken by someone other
/// than the person it names -- which is 75% of category 5 and under 4% of
/// everything else, measured over all ten conversations.
///
/// Turns outside the ingested prefix are absent from `speaker_of` and are
/// treated as unknown: a question is only judged when every one of its evidence
/// turns can be attributed, so a truncated run scores fewer questions instead
/// of scoring them wrongly.
pub fn expected(
    evidence: &[String],
    subject: &str,
    speaker_of: &BTreeMap<String, String>,
) -> Option<Expected> {
    if evidence.is_empty() {
        return None;
    }
    let speakers: Option<Vec<&String>> = evidence.iter().map(|e| speaker_of.get(e)).collect();
    let speakers = speakers?;
    if speakers.iter().all(|s| s.as_str() != subject) {
        Some(Expected::Refuse)
    } else {
        Some(Expected::Answer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn speakers() -> BTreeMap<String, String> {
        [("D1:1", "Caroline"), ("D2:3", "Melanie")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn a_question_naming_one_speaker_is_about_them() {
        assert_eq!(
            subject_of("What did Caroline realize?", "Caroline", "Melanie"),
            Some("Caroline")
        );
        assert_eq!(
            subject_of("Where does Melanie work?", "Caroline", "Melanie"),
            Some("Melanie")
        );
    }

    #[test]
    fn a_question_naming_both_or_neither_is_not_scored() {
        // Not a defect to skip these -- there is no single subject to hold the
        // store to, so any verdict would be about the scoring and not the store.
        assert_eq!(
            subject_of("Did Caroline tell Melanie?", "Caroline", "Melanie"),
            None
        );
        assert_eq!(subject_of("What happened?", "Caroline", "Melanie"), None);
    }

    #[test]
    fn evidence_spoken_by_someone_else_is_the_trap() {
        // The shape of every LoCoMo category-5 question that is one: the fact
        // is real, and it is about the other person.
        assert_eq!(
            expected(&["D2:3".into()], "Caroline", &speakers()),
            Some(Expected::Refuse)
        );
    }

    #[test]
    fn evidence_spoken_by_the_subject_is_answerable_whatever_the_label_says() {
        // A quarter of category 5 is like this. Scoring it as a trap would
        // credit the store for going quiet on something it should have found.
        assert_eq!(
            expected(&["D1:1".into()], "Caroline", &speakers()),
            Some(Expected::Answer)
        );
    }

    #[test]
    fn evidence_outside_the_ingested_prefix_is_not_judged() {
        // Absent from the map rather than attributed to nobody: a truncated run
        // must score fewer questions, never score them wrongly.
        assert_eq!(expected(&["D9:9".into()], "Caroline", &speakers()), None);
        assert_eq!(expected(&[], "Caroline", &speakers()), None);
    }

    #[test]
    fn mixed_evidence_counts_as_answerable() {
        // One turn by the subject is grounds. "All evidence belongs to someone
        // else" is the trap; "some of it does" is an ordinary multi-hop
        // question, and 100% of multi-hop questions in the corpus are that.
        assert_eq!(
            expected(
                &["D1:1".into(), "D2:3".into()],
                "Caroline",
                &speakers()
            ),
            Some(Expected::Answer)
        );
    }
}

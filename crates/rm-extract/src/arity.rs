//! Whether an attribute admits one value at a time, asked once per name.
//!
//! # Why this is not part of the extraction prompt
//!
//! `rm_core::Supersession` needs someone to say whether a later fact under one
//! attribute replaces the earlier ones or joins them. The obvious place to ask
//! is the extraction prompt, and that was tried: a `"replaces"` boolean on
//! every fact. It answered well and cost 19% of the facts, measured against a
//! ~4% noise floor from two runs of the unchanged prompt. See [`crate::prompt`]
//! for the table. A model given one more question per fact answers it by
//! emitting fewer of them, which is the second time this project has measured
//! that exact trade.
//!
//! But the question was never about the fact. `employer` admits one value at a
//! time whatever turn it came from; `pet` accumulates whatever turn it came
//! from. **Arity is a property of the name.** So it can be asked away from
//! extraction, once per distinct name, and cached -- which cannot cost an
//! extraction anything, because it does not touch one.
//!
//! The scale is the argument. Conversation 0 of LoCoMo produces 616 facts under
//! 418 distinct attribute names, from 419 turns. Asking per fact is 616
//! questions welded onto 419 prompts that were doing something else. Asking per
//! name is 418 questions in a handful of batched calls that share no context
//! with extraction at all.
//!
//! # The tie-break, and why it goes that way
//!
//! A name that could plausibly go either way is answered `Joins`. The two
//! errors are not the same size: a slot wrongly marked as accumulating keeps
//! two facts where one would have done, and a reader sees both and can tell.
//! A slot wrongly marked as correcting tells an agent that a fact which is
//! still true has been replaced, and the fact is gone from anything that reads
//! `Standing::still_stands`. Recoverable against unrecoverable.
//!
//! # Asking about the bare name did not work
//!
//! [`Arity::resolve`] asks about names on their own, and the tie-break above is
//! stated in its prompt. It did not take. Conversation 0, 512 names in 7 calls,
//! came back **55% `Corrects`** -- and on the slots where the store itself shows
//! the answer, it is wrong in the expensive direction:
//!
//! ```text
//!   activity   swimming | camping at the beach | running | pottery workshop
//!   enjoys     hiking | pottery class | music | art as a creative outlet
//!   belief     hope and love exist | equality and inclusivity | community
//! ```
//!
//! All marked one-at-a-time. Asked about `activity` in the abstract a model
//! reads "the activity someone is currently doing", which is a fair reading of
//! the name and the wrong reading of this slot -- the store is using it as a log
//! of occasions. **The name alone underdetermines the arity**, and no rewording
//! of a question about a bare string fixes that.
//!
//! Measured over the 44 slots of conversation 0 that hold several distinct
//! values recorded on several different turns -- the cases where a wrong
//! `Corrects` provably discards something still true:
//!
//! ```text
//!                       marked Corrects        calls   names asked
//!   nothing asked            10 / 44   23%         0             0
//!   bare name                29 / 44   66%         7           512
//!   name plus values         13 / 44   30%         3            57
//! ```
//!
//! The 23% floor is tombstones, which correct a slot legitimately -- "she has
//! none of them now" really does clear a list. So the marginal cost of the
//! contextual pass is three slots, and of the bare pass nineteen.
//!
//! # So ask only where it matters, and show the evidence
//!
//! [`Arity::resolve_in_context`] asks about the names that appear in a slot
//! holding more than one value, and quotes those values. That is 57 names of
//! 512 in conversation 0, because arity cannot change an answer for a slot with
//! one value in it. Three calls instead of seven, and the verdict inverts:
//! **19% `Corrects`** rather than 55%, replicated on conversation 1 at 19%.
//!
//! What that buys, conversation 0, against a control whose store is identical
//! slot for slot -- same 568 slots, same values, same provenance, verified,
//! because none of this touches extraction:
//!
//! ```text
//!                  latest      joined    unsettled   corrected
//!   nothing asked  587 76.9%     0  0%   122  16.0%   54  7.1%
//!   in context     587 76.9%   108 14.2%   9   1.2%   59  7.7%
//! ```
//!
//! Read that honestly. `still_stands` is true for `Unsettled` and for `Joined`
//! alike, so the 108 assertions that moved between them changed no behaviour --
//! what changed is that a reader is now told "one of several, still true"
//! instead of "something later exists and nobody said what it meant". The
//! behavioural delta is the five assertions that became corrections, and on the
//! numbers above roughly three slots' worth of those are wrong.
//!
//! That is a small gain for three calls, and it is stated as a small gain
//! rather than dressed up. The larger point is the shape: this is the first
//! mechanism that fills `Supersession` at all without costing an extraction,
//! and a host with a real schema skips the model entirely --
//! [`Arity::from_pairs`] is exact, free, and the case this design is actually
//! for.

use std::collections::{BTreeMap, BTreeSet};

use rm_core::Supersession;

use crate::{claim, Completer};

/// How many names go in one request.
///
/// Large enough that a conversation's whole vocabulary fits in a handful of
/// calls, small enough that one unparseable response does not cost all of it --
/// a failed batch leaves its names [`Supersession::Unstated`], so the blast
/// radius of a bad response is exactly this number.
pub const BATCH: usize = 80;

/// What the model said about each attribute name it was shown.
///
/// Names it was never shown, and names in a batch that failed, are absent --
/// [`Arity::of`] answers [`Supersession::Unstated`] for those, which is the
/// same thing the store records for a host that never asked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Arity {
    known: BTreeMap<String, Supersession>,
}

/// What one [`Arity::resolve`] cost and what it could not answer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Resolved {
    /// Distinct names asked about.
    pub asked: usize,
    /// Requests made. `asked.div_ceil(BATCH)` unless something failed.
    pub calls: usize,
    /// Names left unanswered: a batch that failed, or a response that omitted
    /// them. Reported rather than defaulted quietly, because a run where every
    /// batch failed and a run where nothing needed asking produce the same
    /// empty [`Arity`] and should not read the same.
    pub unanswered: Vec<String>,
    /// The first failure, for a caller that wants to say why.
    pub first_error: Option<String>,
}

impl Arity {
    /// What this attribute claims about the values already in its slot.
    pub fn of(&self, attribute: &str) -> Supersession {
        self.known
            .get(attribute)
            .copied()
            .unwrap_or(Supersession::Unstated)
    }

    /// How many names have an answer.
    pub fn len(&self) -> usize {
        self.known.len()
    }

    pub fn is_empty(&self) -> bool {
        self.known.is_empty()
    }

    /// How many of the answers were `Corrects`, for a caller reporting the
    /// shape of what came back. A vocabulary answered entirely one way is a
    /// signal about the prompt, not about the vocabulary.
    pub fn corrects(&self) -> usize {
        self.known
            .values()
            .filter(|s| **s == Supersession::Corrects)
            .count()
    }

    /// Ask about every name, in batches.
    ///
    /// Deduplicated and sorted before asking, so the same vocabulary produces
    /// the same requests in the same order and a cache in front of `completer`
    /// hits on a re-run. That is not a nicety here: it is what makes a
    /// before/after comparison on one corpus free after the first run.
    pub fn resolve(names: &[String], completer: &impl Completer) -> (Self, Resolved) {
        let unique: BTreeSet<&String> = names.iter().collect();
        let unique: Vec<&String> = unique.into_iter().collect();

        let mut out = Arity::default();
        let mut report = Resolved {
            asked: unique.len(),
            ..Default::default()
        };

        for batch in unique.chunks(BATCH) {
            report.calls += 1;
            match completer.complete(&prompt(batch)) {
                Err(e) => {
                    report
                        .first_error
                        .get_or_insert_with(|| e.to_string().lines().next().unwrap_or("").into());
                    report.unanswered.extend(batch.iter().map(|n| (*n).clone()));
                }
                Ok(response) => {
                    match serde_json::from_str::<serde_json::Value>(unfenced(&response)) {
                        Err(e) => {
                            report.first_error.get_or_insert_with(|| e.to_string());
                            report.unanswered.extend(batch.iter().map(|n| (*n).clone()));
                        }
                        Ok(value) => {
                            for name in batch {
                                // `claim` is the same lenient reader the fact
                                // parser uses: `true`, `"yes"`, a number, anything.
                                // An answer nothing can read is worth what an
                                // absent one is worth, and neither is a reason to
                                // fail the batch beside it.
                                match claim(value.get(name.as_str())) {
                                    Supersession::Unstated => {
                                        report.unanswered.push((*name).clone())
                                    }
                                    answered => {
                                        out.known.insert((*name).clone(), answered);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        (out, report)
    }

    /// Ask again about the names that turned out to matter, showing what they
    /// hold.
    ///
    /// Asking about a bare name is answerable but not always answered well.
    /// `activity` in the abstract reads as "the activity someone is currently
    /// doing", and a model says one-at-a-time; the store is using it as a log
    /// of occasions -- swimming, camping, running, a pottery workshop. Measured
    /// on conversation 0, **66% of the slots holding several distinct values
    /// across several turns came back `Corrects`**, which is the unrecoverable
    /// direction: every value but the last stops standing.
    ///
    /// The evidence removes the ambiguity, and it is cheap to show, because
    /// arity only changes an answer where a slot holds more than one value.
    /// Conversation 0 has 568 slots and 74 of those. So this asks about the
    /// names that appear in a contested slot, quotes their values, and leaves
    /// every other name to the cheap pass -- where being wrong costs nothing,
    /// since a slot with one value reads the same under either verdict.
    ///
    /// `held` maps a name to values recorded under it. Names with nothing to
    /// show are skipped rather than asked about blind.
    pub fn resolve_in_context(
        held: &BTreeMap<String, Vec<String>>,
        completer: &impl Completer,
    ) -> (Self, Resolved) {
        let with_evidence: Vec<(&String, &Vec<String>)> =
            held.iter().filter(|(_, v)| v.len() > 1).collect();

        let mut out = Arity::default();
        let mut report = Resolved {
            asked: with_evidence.len(),
            ..Default::default()
        };
        for batch in with_evidence.chunks(BATCH / 4) {
            report.calls += 1;
            let answer = completer
                .complete(&context_prompt(batch))
                .map_err(|e| e.to_string())
                .and_then(|r| {
                    serde_json::from_str::<serde_json::Value>(unfenced(&r))
                        .map_err(|e| e.to_string())
                });
            match answer {
                Err(e) => {
                    report.first_error.get_or_insert(e);
                    report
                        .unanswered
                        .extend(batch.iter().map(|(n, _)| (*n).clone()));
                }
                Ok(value) => {
                    for (name, _) in batch {
                        match claim(value.get(name.as_str())) {
                            Supersession::Unstated => report.unanswered.push((*name).clone()),
                            answered => {
                                out.known.insert((*name).clone(), answered);
                            }
                        }
                    }
                }
            }
        }
        (out, report)
    }

    /// Everything in `other` that this does not already answer.
    ///
    /// The two passes compose: the contextual pass covers the names that can
    /// change an answer, the bare pass covers the rest, and the contextual one
    /// wins wherever both spoke.
    pub fn or(mut self, other: &Arity) -> Self {
        for (k, v) in &other.known {
            self.known.entry(k.clone()).or_insert(*v);
        }
        self
    }

    /// Build one directly, for a host that knows its own schema.
    ///
    /// The whole point of keying on the name is that the answer does not have
    /// to come from a model. An application with fixed attributes knows which
    /// of them are single-valued and should say so rather than pay to be told.
    pub fn from_pairs<K: Into<String>>(pairs: impl IntoIterator<Item = (K, Supersession)>) -> Self {
        Arity {
            known: pairs.into_iter().map(|(k, v)| (k.into(), v)).collect(),
        }
    }
}

/// The JSON inside a markdown code fence, or the whole string if there is none.
///
/// Measured, not anticipated. The first real run of this asked for 512 names in
/// 7 batches and parsed none of them: every response came back as
/// ```` ```json\n{...}\n``` ````, and `serde_json` fails such a string at line
/// 1 column 1 -- an error that reads like the model refused when in fact it
/// answered perfectly. The eight-name probe used while writing the prompt came
/// back bare, so the behaviour only appears at the batch size the code actually
/// uses.
///
/// Deliberately not applied to [`crate::extract`]. That parser is unchanged and
/// its results are the baseline every measurement in this crate is quoted
/// against; a fence-stripper there would be a real fix and would move numbers,
/// so it belongs in its own change with its own before and after.
fn unfenced(response: &str) -> &str {
    let text = response.trim();
    let Some(rest) = text.strip_prefix("```") else {
        return text;
    };
    // ```json, ```JSON, or just ``` -- the language tag runs to the first
    // newline, and a fence with no newline at all has no body to find.
    let body = match rest.split_once('\n') {
        Some((_tag, body)) => body,
        None => return text,
    };
    body.trim().strip_suffix("```").unwrap_or(body).trim()
}

/// The question, for one batch of names.
///
/// Public for the same reason [`crate::prompt`] is: a host may want to read it,
/// log it, or build its own. The crate owning the contract does not make the
/// contract a secret.
pub fn prompt(names: &[&String]) -> String {
    let list = names
        .iter()
        .map(|n| n.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        r#"Below are attribute names used to record facts about people and things.

For each one, say whether something can have only ONE value for it at a time.

- true — a later value replaces the earlier one. A person has one employer, one
  address, one age, one height, one marital status, one current mood.
- false — a later value joins the earlier ones. A person accumulates pets,
  hobbies, goals, skills, places they have visited, things they own, events they
  attended, opinions they have voiced, people they know.

Judge the name on its own, not any particular fact that might use it.

If a name could reasonably go either way, answer false. Keeping two facts that
turned out to be one is a small mess anybody can see; discarding one that was
still true is a loss nothing downstream can detect.

Reply with only a JSON object mapping every name below to true or false, and
nothing else. No explanation, and no markdown code fence around it. Like this:

{{"employer": true, "pet": false}}

Names:
{list}
"#
    )
}

/// The question, for names shown with what they hold.
pub fn context_prompt(held: &[(&String, &Vec<String>)]) -> String {
    let mut list = String::new();
    for (name, values) in held {
        // Capped, because the question is what KIND of slot this is and twenty
        // examples answer that no better than six while costing tokens that
        // push the other names out of the batch.
        let sample: Vec<&str> = values.iter().take(6).map(String::as_str).collect();
        list.push_str(&format!("{name}: {}\n", sample.join(" | ")));
    }
    format!(
        r#"Below are attribute names, each followed by values recorded under it for
one person, in the order they were said.

For each name, say whether the later values REPLACED the earlier ones or were
recorded ALONGSIDE them.

- true — later replaced earlier. The earlier value stopped being true when the
  later one arrived: a change of employer, a move, a new age.
- false — they accumulate. All of them are still true; the name is being used
  as a running list of things said on different occasions.

Look at the values, not just the name. A name like "activity" or "feeling" may
sound like one-at-a-time, but if the values are four different things said on
four different days then they are a list and the answer is false.

Reply with only a JSON object mapping every name below to true or false, and
nothing else. No explanation, and no markdown code fence.

{list}"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CompleterError;

    struct Canned(&'static str);
    impl Completer for Canned {
        fn complete(&self, _prompt: &str) -> Result<String, CompleterError> {
            Ok(self.0.to_string())
        }
    }

    struct Broken;
    impl Completer for Broken {
        fn complete(&self, _prompt: &str) -> Result<String, CompleterError> {
            Err(CompleterError("no route to host".into()))
        }
    }

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn an_answered_name_carries_its_arity() {
        let (arity, report) = Arity::resolve(
            &names(&["employer", "pet"]),
            &Canned(r#"{"employer": true, "pet": false}"#),
        );
        assert_eq!(arity.of("employer"), Supersession::Corrects);
        assert_eq!(arity.of("pet"), Supersession::Joins);
        assert_eq!(report.asked, 2);
        assert_eq!(report.calls, 1);
        assert!(report.unanswered.is_empty());
    }

    #[test]
    fn a_name_never_asked_about_is_unstated_not_guessed() {
        let (arity, _) = Arity::resolve(&names(&["employer"]), &Canned(r#"{"employer": true}"#));
        assert_eq!(arity.of("mood"), Supersession::Unstated);
        assert_eq!(arity.of(""), Supersession::Unstated);
    }

    #[test]
    fn duplicate_names_are_asked_about_once() {
        // The vocabulary is the unit, not the fact. 616 facts under 418 names
        // is the whole reason this is cheaper than asking per fact, and it only
        // holds if the duplicates collapse before the request is built.
        let (arity, report) = Arity::resolve(
            &names(&["pet", "pet", "pet", "employer"]),
            &Canned(r#"{"employer": true, "pet": false}"#),
        );
        assert_eq!(report.asked, 2, "two distinct names, not four facts");
        assert_eq!(report.calls, 1);
        assert_eq!(arity.len(), 2);
        assert_eq!(arity.of("pet"), Supersession::Joins);
    }

    #[test]
    fn a_failed_batch_costs_its_answers_and_nothing_else() {
        let (arity, report) = Arity::resolve(&names(&["employer", "pet"]), &Broken);
        assert!(arity.is_empty());
        assert_eq!(report.unanswered.len(), 2);
        assert_eq!(
            report.first_error.as_deref(),
            Some("the completer failed: no route to host"),
            "and it says why, because an empty Arity from a broken connection \
             must not read like a vocabulary nobody needed to ask about"
        );
    }

    #[test]
    fn a_response_that_omits_a_name_leaves_it_unstated_and_says_so() {
        // Not an error and not a default. The model answered the batch and
        // skipped one, which is exactly the state `Unstated` exists for -- and
        // a caller that wants to retry needs to be told which.
        let (arity, report) = Arity::resolve(
            &names(&["employer", "pet"]),
            &Canned(r#"{"employer": true}"#),
        );
        assert_eq!(arity.of("employer"), Supersession::Corrects);
        assert_eq!(arity.of("pet"), Supersession::Unstated);
        assert_eq!(report.unanswered, vec!["pet".to_string()]);
    }

    #[test]
    fn a_garbled_answer_for_one_name_does_not_cost_the_others() {
        // The lesson this crate keeps re-learning, applied here before it can
        // be learned a third time: a model that writes something unreadable in
        // one field must not take the batch down with it.
        let (arity, report) = Arity::resolve(
            &names(&["employer", "pet", "age"]),
            &Canned(r#"{"employer": "yes", "pet": "no", "age": "depends"}"#),
        );
        assert_eq!(arity.of("employer"), Supersession::Corrects);
        assert_eq!(arity.of("pet"), Supersession::Joins);
        assert_eq!(arity.of("age"), Supersession::Unstated);
        assert_eq!(report.unanswered, vec!["age".to_string()]);
    }

    #[test]
    fn an_answer_wrapped_in_a_markdown_fence_is_still_an_answer() {
        // What the first real run actually returned, for all seven batches.
        // The prompt now asks for no fence and this strips one anyway: the
        // instruction is a request and the parser is the guarantee.
        let (arity, report) = Arity::resolve(
            &names(&["employer", "pet"]),
            &Canned("```json\n{\"employer\": true, \"pet\": false}\n```"),
        );
        assert_eq!(arity.of("employer"), Supersession::Corrects);
        assert_eq!(arity.of("pet"), Supersession::Joins);
        assert!(report.unanswered.is_empty());
    }

    #[test]
    fn a_bare_fence_with_no_language_tag_is_also_read() {
        let (arity, _) = Arity::resolve(&names(&["pet"]), &Canned("```\n{\"pet\": false}\n```"));
        assert_eq!(arity.of("pet"), Supersession::Joins);
    }

    #[test]
    fn something_that_only_looks_like_a_fence_is_left_alone() {
        // No newline, so there is no body to unwrap and nothing to strip. It
        // fails to parse, which is the right answer -- silently returning the
        // whole string as if it were unfenced would be the same bug in reverse.
        assert!(Arity::resolve(&names(&["pet"]), &Canned("```"))
            .0
            .is_empty());
    }

    #[test]
    fn a_malformed_response_fails_only_its_own_batch() {
        assert!(
            Arity::resolve(&names(&["employer"]), &Canned("sorry, I can't"))
                .0
                .is_empty()
        );
    }

    #[test]
    fn the_prompt_carries_every_name_and_the_tie_break() {
        let held = names(&["employer", "favourite_food"]);
        let p = prompt(&held.iter().collect::<Vec<_>>());
        assert!(p.contains("employer"));
        assert!(p.contains("favourite_food"));
        assert!(
            p.contains("answer false"),
            "the tie-break has to be stated: the two errors are different sizes \
             and a model left to pick will not know that: {p}"
        );
    }

    #[test]
    fn a_host_with_its_own_schema_need_not_ask_at_all() {
        let arity = Arity::from_pairs([
            ("employer", Supersession::Corrects),
            ("pet", Supersession::Joins),
        ]);
        assert_eq!(arity.of("employer"), Supersession::Corrects);
        assert_eq!(arity.corrects(), 1);
    }
}

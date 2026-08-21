//! What rusty-memory does with a real conversation.
//!
//! Everything in the workspace's own tests is synthetic: fixtures written to
//! state a property, and stub providers that return what the test wants. That
//! is the right way to pin behaviour and it cannot tell you whether the
//! behaviour is any good. The `m`/`u` probabilities in `rmem.toml` are numbers
//! somebody chose; whether the review band catches real ambiguity or fires on
//! every pair of names in a conversation is not something a fixture can answer.
//!
//! So this runs the real pipeline — real extraction, real embeddings — over
//! LoCoMo: multi-session dialogue with dated sessions, and questions carrying
//! the turn ids that answer them.
//!
//! # Why LoCoMo
//!
//! It is the corpus OpenViking benchmarks on, which makes anything measured
//! here comparable to the system this project is positioned against. It is also
//! shaped like the problem: 10 conversations of ~20 dated sessions each,
//! between two people who change jobs and move house across months, with
//! questions labelled by what they demand — single-hop, multi-hop, temporal,
//! open-domain, and adversarial questions whose premise the conversation never
//! supports.
//!
//! # What is measured, and what is not
//!
//! Retrieval is scored against `evidence`: the turn ids LoCoMo says answer a
//! question. Every assertion carries the `dia_id` of the turn it came from in
//! its provenance, so "did recall surface the right turn" is a set membership
//! test and needs no model to judge it. That is the whole reason this reports
//! retrieval rather than answer accuracy — an LLM judge would put a second
//! model's opinion between the measurement and the thing being measured, and
//! the first number this project needs is one nobody has to trust.
//!
//! Category 5 is adversarial: the question presumes something the conversation
//! does not support, and LoCoMo's own answer is that it is unanswerable. Those
//! are reported separately and never counted as retrieval failures — a store
//! that surfaces nothing for them is right.

use std::collections::BTreeSet;
use std::path::PathBuf;

use rm_engine::{Engine, Metric, Query, VectorIndex};
use rm_extract::Turn;
use rm_host::config::Config;
use rm_providers::HttpProvider;
use serde_json::Value;

/// How many hits `recall` is asked for when scoring retrieval.
const K: usize = 10;

fn main() {
    let mut args = std::env::args().skip(1);
    let corpus = PathBuf::from(args.next().unwrap_or_else(usage));
    let which: usize = args
        .next()
        .unwrap_or_else(|| "0".into())
        .parse()
        .expect("conversation index");
    // A turn budget, so a first run can cost pennies and be looked at before
    // the whole corpus is paid for.
    let budget: usize = args
        .next()
        .unwrap_or_else(|| usize::MAX.to_string())
        .parse()
        .expect("turn budget");

    let corpus: Value =
        serde_json::from_str(&std::fs::read_to_string(&corpus).expect("read the corpus"))
            .expect("parse the corpus");
    let sample = &corpus.as_array().expect("a list of conversations")[which];

    let config = Config::from_template();
    // Overridable because `rm-providers` cannot reach a network whose egress
    // is a proxy: `ureq`'s free functions ignore `HTTPS_PROXY`, and its
    // webpki-roots build ignores `SSL_CERT_FILE`, so a proxy substituting its
    // own certificate is rejected. Both are real gaps for anyone behind a
    // corporate proxy; neither is this harness's to fix, so it takes a base
    // URL and a plain-HTTP forwarder goes in front.
    let base_url = std::env::var("LOCOMO_BASE_URL").unwrap_or(config.provider.base_url.clone());
    let provider = HttpProvider::new(
        base_url,
        std::env::var(&config.provider.api_key_env).expect("the api key variable is set"),
        config.provider.completion_model.clone(),
        config.provider.embedding_model.clone(),
    );

    let mut engine = Engine::new(
        VectorIndex::new(config.provider.dimension, Metric::Cosine),
        config.ruleset().expect("template ruleset"),
        config.policy_for_engine().expect("template policy"),
    );

    let turns = turns_of(sample);
    let total = turns.len().min(budget);
    eprintln!(
        "conversation {which}: {} turns ({total} within budget), {} questions",
        turns.len(),
        sample["qa"].as_array().map_or(0, |q| q.len())
    );

    // ---- ingest ------------------------------------------------------------

    let mut refused: Vec<(String, String)> = Vec::new();
    let mut assertions = 0usize;
    let mut relations = 0usize;

    for (i, turn) in turns.iter().take(total).enumerate() {
        if i % 25 == 0 {
            eprintln!("  ingesting {i}/{total} ...");
        }
        let t = Turn {
            text: turn.text.clone(),
            // Set, unlike `command::remember`, which hardcodes `None`. A
            // dialogue corpus is mostly first person: without a speaker, "I
            // moved to Chicago" names nobody, and `Turn`'s own documentation
            // says the speaker is what lets that resolve.
            speaker: Some(turn.speaker.clone()),
            observed_at: turn.at,
            session: turn.id.clone(),
        };
        match rm_engine::extract(&t, &provider) {
            Err(e) => refused.push((turn.id.clone(), e.to_string())),
            Ok(extraction) => match engine.ingest(&t, &extraction, &provider) {
                Err(e) => refused.push((turn.id.clone(), e.to_string())),
                Ok(ingested) => {
                    assertions += ingested.assertions.len();
                    relations += extraction.relations.len();
                }
            },
        }
    }

    // Keep the store. The first version of this harness dropped the engine
    // when it exited, so the only way to ask "what are these 148 entities?"
    // was to pay for the whole run again. A benchmark whose output is a
    // handful of aggregates and no artefact can tell you a number is bad and
    // never why.
    let snapshot = PathBuf::from(
        std::env::var("LOCOMO_SNAPSHOT").unwrap_or_else(|_| format!("locomo-{which}.json")),
    );
    if let Err(e) = std::fs::write(&snapshot, engine.snapshot()) {
        eprintln!("  could not write the snapshot: {e}");
    } else {
        eprintln!("  store written to {}", snapshot.display());
    }

    let reviews = engine.pending_review().len();
    println!("\n=== ingestion ===");
    println!("turns ingested       {}", total - refused.len());
    println!("turns refused        {}", refused.len());
    println!("entities             {}", engine.entity_ids().len());
    println!("assertions           {assertions}");
    println!("relations            {relations}");
    println!(
        "review band          {reviews} pairs ({:.1} per 100 turns)",
        reviews as f64 * 100.0 / total.max(1) as f64
    );
    // What the 148 are. An entity count alone cannot distinguish a
    // conversation that really mentioned that many things from a resolver
    // that made four Carolines.
    let mut kinds: std::collections::BTreeMap<String, usize> = Default::default();
    for e in engine.entity_ids() {
        let kind = engine
            .store_history(e, "kind")
            .last()
            .and_then(|v| v.value.clone())
            .unwrap_or_else(|| "?".to_string());
        *kinds.entry(kind).or_default() += 1;
    }
    println!("entities by kind     {kinds:?}");

    // Why the refusals happened, not just how many. One line per distinct
    // shape: 40 instances of one bug and 40 different bugs need different
    // work, and a count cannot tell them apart.
    let mut shapes: std::collections::BTreeMap<&str, usize> = Default::default();
    for (_, why) in &refused {
        *shapes.entry(first_line(why)).or_default() += 1;
    }
    println!("\nrefusals by shape:");
    for (why, n) in &shapes {
        println!("  {n:>3}x {why}");
    }

    // ---- retrieval ---------------------------------------------------------

    let mut scored: Vec<(u64, bool)> = Vec::new();
    let mut adversarial_hits = 0usize;
    let mut adversarial_total = 0usize;
    let ingested_ids: BTreeSet<&str> = turns
        .iter()
        .take(total)
        .map(|t| t.id.as_str())
        .collect();

    let questions = sample["qa"].as_array().cloned().unwrap_or_default();
    let mut asked = 0usize;
    for q in &questions {
        let Some(question) = q["question"].as_str() else {
            continue;
        };
        let category = q["category"].as_u64().unwrap_or(0);
        let evidence: BTreeSet<String> = q["evidence"]
            .as_array()
            .map(|e| {
                e.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();

        // Only questions whose evidence is inside the ingested prefix can be
        // answered at all. Scoring the rest would measure the budget.
        if evidence.is_empty() || !evidence.iter().all(|e| ingested_ids.contains(e.as_str())) {
            continue;
        }
        asked += 1;

        let embedding = match provider_embed(&provider, question) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("  embed failed for a question: {}", first_line(&e));
                continue;
            }
        };
        let hits = match engine.recall(&Query::new(embedding, K)) {
            Ok(hits) => hits,
            Err(e) => {
                eprintln!("  recall failed: {}", first_line(&e.to_string()));
                continue;
            }
        };
        let found = hits
            .iter()
            .any(|h| evidence.contains(&h.provenance.source_ref));

        if category == 5 {
            adversarial_total += 1;
            if found {
                adversarial_hits += 1;
            }
        } else {
            scored.push((category, found));
        }
    }

    println!("\n=== retrieval (recall@{K}, scored against LoCoMo evidence turns) ===");
    println!("questions asked      {asked} of {}", questions.len());
    if scored.is_empty() {
        println!("nothing answerable within the ingested prefix");
    } else {
        let hit = scored.iter().filter(|(_, f)| *f).count();
        println!(
            "overall              {hit}/{} = {:.3}",
            scored.len(),
            hit as f64 / scored.len() as f64
        );
        for (cat, name) in [
            (4u64, "single-hop"),
            (1, "multi-hop"),
            (2, "temporal"),
            (3, "open-domain"),
        ] {
            let of_cat: Vec<bool> = scored
                .iter()
                .filter(|(c, _)| *c == cat)
                .map(|(_, f)| *f)
                .collect();
            if of_cat.is_empty() {
                continue;
            }
            let h = of_cat.iter().filter(|f| **f).count();
            println!(
                "  {name:<18} {h}/{} = {:.3}",
                of_cat.len(),
                h as f64 / of_cat.len() as f64
            );
        }
    }
    if adversarial_total > 0 {
        println!(
            "\nadversarial          {adversarial_hits}/{adversarial_total} surfaced something \
             for a question the conversation does not answer"
        );
        println!("  (not a failure by itself -- recall is not asked to refuse. Reported because");
        println!("   what the store does with an unsupported premise is this project's thesis.)");
    }
}

/// One turn, flattened out of LoCoMo's session-keyed shape.
struct Line {
    id: String,
    speaker: String,
    text: String,
    at: rm_engine::Timestamp,
}

/// Sessions in order, with each turn carrying its session's date.
///
/// The date matters: it is the observation time every assertion is stamped
/// with, and the whole temporal story depends on turns from May not looking
/// simultaneous with turns from September.
fn turns_of(sample: &Value) -> Vec<Line> {
    let conv = &sample["conversation"];
    let obj = conv.as_object().expect("conversation object");
    let mut sessions: Vec<usize> = obj
        .keys()
        .filter_map(|k| k.strip_prefix("session_"))
        .filter(|k| !k.contains("date_time"))
        .filter_map(|k| k.parse().ok())
        .collect();
    sessions.sort_unstable();

    let mut out = Vec::new();
    for n in sessions {
        let at = obj
            .get(&format!("session_{n}_date_time"))
            .and_then(Value::as_str)
            .and_then(parse_when)
            .unwrap_or(0);
        for turn in obj[&format!("session_{n}")].as_array().into_iter().flatten() {
            let (Some(id), Some(speaker), Some(text)) = (
                turn["dia_id"].as_str(),
                turn["speaker"].as_str(),
                turn["text"].as_str(),
            ) else {
                continue;
            };
            out.push(Line {
                id: id.to_string(),
                speaker: speaker.to_string(),
                text: text.to_string(),
                at,
            });
        }
    }
    out
}

/// `"1:56 pm on 8 May, 2023"` to epoch milliseconds.
///
/// Hand-rolled rather than pulling in a date crate for one format that only
/// appears in this fixture. Days-from-civil is the standard algorithm; the
/// clock time is kept because two sessions can share a date.
fn parse_when(s: &str) -> Option<rm_engine::Timestamp> {
    let (clock, date) = s.split_once(" on ")?;
    let (hm, meridiem) = clock.trim().rsplit_once(' ')?;
    let (h, m) = hm.split_once(':')?;
    let mut hour: i64 = h.trim().parse().ok()?;
    let minute: i64 = m.trim().parse().ok()?;
    match meridiem.trim().to_ascii_lowercase().as_str() {
        "pm" if hour != 12 => hour += 12,
        "am" if hour == 12 => hour = 0,
        _ => {}
    }

    let date = date.replace(',', "");
    let mut parts = date.split_whitespace();
    let day: i64 = parts.next()?.parse().ok()?;
    let month = match &parts.next()?.to_ascii_lowercase()[..3] {
        "jan" => 1,
        "feb" => 2,
        "mar" => 3,
        "apr" => 4,
        "may" => 5,
        "jun" => 6,
        "jul" => 7,
        "aug" => 8,
        "sep" => 9,
        "oct" => 10,
        "nov" => 11,
        "dec" => 12,
        _ => return None,
    };
    let year: i64 = parts.next()?.parse().ok()?;

    // days_from_civil, Howard Hinnant's algorithm.
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;

    Some(((days * 86_400 + hour * 3_600 + minute * 60) * 1_000) as rm_engine::Timestamp)
}

fn provider_embed(p: &HttpProvider, text: &str) -> Result<Vec<f32>, String> {
    use rm_engine::Embedder;
    p.embed(text).map_err(|e| e.to_string())
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or(s)
}

fn usage() -> String {
    eprintln!("usage: locomo <locomo10.json> [conversation index] [turn budget]");
    std::process::exit(2);
}

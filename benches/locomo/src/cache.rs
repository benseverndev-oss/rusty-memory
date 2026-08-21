//! A caching, concurrent front end for the provider.
//!
//! # Why the harness owns this and the library does not
//!
//! Nothing here changes what is measured. Extraction is a pure function of a
//! prompt and embedding a pure function of a string, so serving either from a
//! file produces the response the model would have produced — which is exactly
//! why the library must not do this (a memory store that answered from a stale
//! cache would be lying) and exactly why a benchmark may.
//!
//! # What it buys
//!
//! A run was thirteen minutes: 419 extractions at about 1.8s each, then ~550
//! embeddings at about 0.2s, all in series. That made the benchmark unusable as
//! a feedback loop — it is why the fixtures in `rm-extract/tests` exist at all.
//!
//! Two things fix it, and the second matters more than the first.
//!
//! **Extraction is per-turn and independent**, so it parallelises even though
//! *ingestion* cannot: resolution depends on what is already in the store, so
//! the order turns are ingested in is part of the result. Extracting everything
//! up front and ingesting from the answers keeps that order exactly.
//!
//! **Cached responses make a re-run nearly free**, and that is the real prize.
//! The cache is keyed by the full prompt, so a prompt edit invalidates exactly
//! the entries it should and nothing else. Changing resolution, survivorship,
//! the index or the store — everything downstream of extraction — costs no API
//! calls at all, which turns the benchmark from a thing to be rationed into a
//! thing to be run.
//!
//! Keys are `DefaultHasher`, which is neither stable across Rust releases nor
//! cryptographic. Both are fine for a cache that can be deleted: a changed hash
//! is a cold cache, not a wrong answer.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use rm_engine::{Embedder, EmbedderError};
use rm_extract::{Completer, CompleterError};
use rm_providers::HttpProvider;

fn key(s: &str) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// Responses already paid for, on disk.
pub struct Cache {
    path: PathBuf,
    completions: Mutex<HashMap<String, String>>,
    embeddings: Mutex<HashMap<String, Vec<f32>>>,
    pub hits: AtomicUsize,
    pub misses: AtomicUsize,
}

impl Cache {
    pub fn open(path: &Path) -> Self {
        let (completions, embeddings) = std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .map(|v| {
                (
                    serde_json::from_value(v["completions"].clone()).unwrap_or_default(),
                    serde_json::from_value(v["embeddings"].clone()).unwrap_or_default(),
                )
            })
            .unwrap_or_default();
        Cache {
            path: path.to_path_buf(),
            completions: Mutex::new(completions),
            embeddings: Mutex::new(embeddings),
            hits: AtomicUsize::new(0),
            misses: AtomicUsize::new(0),
        }
    }

    pub fn save(&self) {
        let body = serde_json::json!({
            "completions": *self.completions.lock().unwrap(),
            "embeddings": *self.embeddings.lock().unwrap(),
        });
        if let Err(e) = std::fs::write(&self.path, body.to_string()) {
            eprintln!("  could not write the cache: {e}");
        }
    }

    pub fn len(&self) -> (usize, usize) {
        (
            self.completions.lock().unwrap().len(),
            self.embeddings.lock().unwrap().len(),
        )
    }
}

/// The provider, answered from the cache where possible.
pub struct Cached<'a> {
    pub inner: &'a HttpProvider,
    pub cache: &'a Cache,
}

impl Completer for Cached<'_> {
    fn complete(&self, prompt: &str) -> Result<String, CompleterError> {
        let k = key(prompt);
        if let Some(hit) = self.cache.completions.lock().unwrap().get(&k) {
            self.cache.hits.fetch_add(1, Ordering::Relaxed);
            return Ok(hit.clone());
        }
        self.cache.misses.fetch_add(1, Ordering::Relaxed);
        let answer = self.inner.complete(prompt)?;
        self.cache
            .completions
            .lock()
            .unwrap()
            .insert(k, answer.clone());
        Ok(answer)
    }
}

impl Embedder for Cached<'_> {
    fn embed(&self, text: &str) -> Result<Vec<f32>, EmbedderError> {
        let k = key(text);
        if let Some(hit) = self.cache.embeddings.lock().unwrap().get(&k) {
            self.cache.hits.fetch_add(1, Ordering::Relaxed);
            return Ok(hit.clone());
        }
        self.cache.misses.fetch_add(1, Ordering::Relaxed);
        let answer = self.inner.embed(text)?;
        self.cache
            .embeddings
            .lock()
            .unwrap()
            .insert(k, answer.clone());
        Ok(answer)
    }
}

/// How a pre-fetch went.
pub struct Prewarmed {
    pub attempted: usize,
    pub failed: usize,
    /// The first failure's message, for a caller that wants to say why.
    pub first_error: Option<String>,
}

/// Fill the cache for `prompts` using `workers` threads.
///
/// Failures are left out of the cache rather than recorded: an entry that is
/// absent is fetched again on the sequential pass, where the error reaches the
/// run's own reporting. Caching a failure would make one bad minute permanent.
///
/// They are, however, *counted* and returned. They were not, and a run where
/// every request failed was indistinguishable from a run that was entirely
/// cached -- both finish instantly and print the same thing.
pub fn prewarm(
    prompts: &[String],
    provider: &HttpProvider,
    cache: &Cache,
    workers: usize,
) -> Prewarmed {
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let failed = AtomicUsize::new(0);
    let first_error = std::sync::Mutex::new(None::<String>);
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| {
                let cached = Cached {
                    inner: provider,
                    cache,
                };
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(prompt) = prompts.get(i) else { return };
                    // Counted, not discarded. This used to be `let _ = ...`,
                    // and a run in which every single request failed looked
                    // from the outside exactly like a run in which every one
                    // was already cached: the pass finished in milliseconds and
                    // said nothing. That happened, and cost a corpus run.
                    if let Err(e) = cached.complete(prompt) {
                        failed.fetch_add(1, Ordering::Relaxed);
                        let mut slot = first_error
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        if slot.is_none() {
                            *slot = Some(e.to_string());
                        }
                    }
                    let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                    if n.is_multiple_of(50) {
                        eprintln!("  extracted {n}/{}", prompts.len());
                    }
                }
            });
        }
    });

    let first_error = first_error
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    Prewarmed {
        attempted: prompts.len(),
        failed: failed.load(Ordering::Relaxed),
        first_error,
    }
}

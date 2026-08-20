//! Helpers the tests share, in this crate and in the crates that host it.
//!
//! A temp-directory guard and a stub provider, both hand-written rather than
//! pulled in. `tempfile` and an HTTP mocking crate would each be a dependency
//! bought for a few dozen lines, in a workspace that has twice chosen to write
//! the small thing instead.
//!
//! # Why this is not `cfg(test)`
//!
//! It was, while `rm-cli` was the only host. A `cfg(test)` module is invisible
//! to every other crate, so `rm-mcp`'s tests could not have used it -- and a
//! stub `Completer` and `Embedder` are exactly what a *consumer* needs in
//! order to drive `remember` and `recall` end to end without a socket, which
//! is the entire reason those are ports rather than an HTTP client.
//!
//! A `testing` feature is the usual answer and it does not work here. Cargo
//! unifies features across normal and dev dependencies within one build, so a
//! binary that dev-depends on `rm-host/testing` compiles the stubs into its
//! release build regardless. The cost is the same either way; this way it is
//! visible, and CI's `--all-features` run does not quietly become the only
//! configuration anyone tests.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use rm_engine::{Embedder, EmbedderError};
use rm_extract::{Completer, CompleterError};

static NEXT: AtomicUsize = AtomicUsize::new(0);

/// A directory that removes itself.
pub struct TempDir(PathBuf);

impl TempDir {
    // `Default` would suggest a directory conjured from nothing is meaningful on its own; this one only exists paired with the `path()` a test is about to write into, so `new` stays the one way to make one.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        // Process id and a counter: unique across concurrent test binaries and
        // across threads within one, without a dependency or a clock.
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("rmem-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&path).expect("could not make a temp directory");
        TempDir(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        // Best effort: a leaked temp directory is untidy, and panicking in a
        // drop during a failing test would hide the real failure.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A provider that answers from a script and embeds by hashing.
pub struct StubProvider {
    responses: std::cell::RefCell<Vec<String>>,
}

impl StubProvider {
    pub fn new(responses: Vec<&str>) -> Self {
        StubProvider {
            responses: std::cell::RefCell::new(responses.into_iter().map(str::to_string).collect()),
        }
    }
}

impl Completer for StubProvider {
    fn complete(&self, _prompt: &str) -> Result<String, CompleterError> {
        let mut left = self.responses.borrow_mut();
        if left.is_empty() {
            return Err(CompleterError(
                "the stub was asked for more responses than it was given".to_string(),
            ));
        }
        Ok(left.remove(0))
    }
}

impl Embedder for StubProvider {
    fn embed(&self, text: &str) -> Result<Vec<f32>, EmbedderError> {
        let mut v = [0.0f32; 3];
        for (i, b) in text.bytes().enumerate() {
            v[i % 3] += f32::from(b);
        }
        // The zero vector is refused under cosine, and an empty string would
        // produce one.
        if v.iter().all(|x| *x == 0.0) {
            v[0] = 1.0;
        }
        Ok(v.to_vec())
    }
}

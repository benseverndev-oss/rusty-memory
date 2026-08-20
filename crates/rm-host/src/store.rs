//! Reading and writing the store file.
//!
//! # The path is named as a field, not printed
//!
//! Every message here says "the store at the path named by `[store] path` in
//! rmem.toml" rather than the path itself, because the path is a value read
//! out of the config and this workspace prints none of those. Eight credential
//! leaks came out of doing otherwise, each closed one shape at a time until the
//! rule was made categorical.
//!
//! This module carried an exception for a while, on the argument that a path in
//! an IO error *is* the location and that a pasted credential could not reach
//! one anyway: every message but `save`'s needs a file to already exist at that
//! path, so a nonsense path takes the `NotFound` branch and is not an error at
//! all. The first half is still a fair argument. The second half is false on
//! Windows: `ERROR_INVALID_NAME` gives `InvalidFilename` for any path
//! containing `* ? | " < >` — and an Azure SAS token begins with `?` — while a
//! path naming a directory gives `PermissionDenied`. Both reach the read below
//! with nothing at that path at all.
//!
//! The trade was defensible on its premise and the premise was wrong, so the
//! exception is gone. What is lost is real: for a relative path a reader can no
//! longer tell from the message which directory was tried. What is kept is the
//! OS error, which says *why* — not found, permission denied, invalid name —
//! and the name of the one field to look at. That is simpler to defend than an
//! exception carrying a caveat.
//!
//! `{dimension}` and `{metric:?}` in the mismatch messages stay: they are a
//! `usize` and a two-variant enum `Config::metric` has already validated, so
//! neither can carry file text whatever is written in the file.
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rm_engine::{Engine, Metric, Policy, Ruleset, VectorIndex};

use crate::HostError;

/// How the six messages below refer to the store, since none of them prints
/// the path itself. One constant so they cannot drift apart.
const WHERE: &str = "the store at the path named by [store] path in rmem.toml";

/// Open the store at `path`, or an empty one if it is not there yet.
///
/// The ruleset and policy come from the config rather than the file, because
/// `Engine::open` deliberately does not persist them. That means there is one
/// source for both and a stale copy inside a snapshot cannot quietly override
/// what `rmem.toml` says.
///
/// The dimension and metric are checked against the restored store rather
/// than merely passed through to `Engine::new`'s empty-store branch. `init`
/// probes the embedding model instead of trusting a human to write the
/// dimension down, for the same reason this checks it on the other end: a
/// disagreement is not a parse error, so nothing about `Engine::open`
/// succeeding proves the store agrees with what the current config expects
/// to feed it. Left unchecked, that disagreement would still be caught --
/// `VectorIndex::check` rejects a wrong-length vector -- but only on the
/// first `remember` or `recall`, far from a message that could have named
/// the actual cause.
pub fn load(
    path: &Path,
    ruleset: Ruleset,
    policy: Policy,
    dimension: usize,
    metric: Metric,
) -> Result<Engine, HostError> {
    match std::fs::read_to_string(path) {
        // Not there yet is not an error: the first command should not be a
        // special case a user has to know about.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Engine::new(
            VectorIndex::new(dimension, metric),
            ruleset,
            policy,
        )),
        Err(e) => Err(HostError::Store(format!("could not read {WHERE}: {e}"))),
        Ok(text) => {
            let engine = Engine::open(&text, ruleset, policy).map_err(|e| {
                HostError::Store(format!("{WHERE} is not a store this build can open: {e}"))
            })?;
            let (stored_dimension, stored_metric) = engine.index_shape();
            if stored_dimension != dimension {
                return Err(HostError::Store(format!(
                    "{WHERE} holds {stored_dimension}-dimensional vectors, but rmem.toml's [provider] section currently names dimension = {dimension} -- if the embedding model changed, run `rmem init --force` to rediscover the dimension, or point the config back at the model this store was built with"
                )));
            }
            if stored_metric != metric {
                return Err(HostError::Store(format!(
                    "{WHERE} was built under metric {stored_metric:?}, but rmem.toml's [provider] section currently names metric = {metric:?} -- distances computed under the wrong metric are silently meaningless rather than merely different, so this is refused rather than reinterpreted"
                )));
            }
            Ok(engine)
        }
    }
}

/// How long to wait for another process to finish before giving up.
///
/// Bounded rather than indefinite. Every holder of this lock is doing a load,
/// an in-memory operation and a save — milliseconds — so a wait this long
/// means the other side is wedged, not busy, and blocking forever on it turns
/// one stuck process into two. Long enough that honest contention never
/// surfaces to a user; short enough that a wedged holder is reported rather
/// than waited out.
const LOCK_WAIT: Duration = Duration::from_secs(5);

/// The sidecar file the lock is taken on.
///
/// Deliberately *not* the store file. [`save`] replaces the store through a
/// rename, so a lock held on it is a lock on an inode that the next save
/// unlinks: the second writer would take a lock on the new file, hold something
/// the first writer has never heard of, and both would believe they were alone.
/// The sidecar is created once and never renamed, so every process locks the
/// same inode for as long as the store exists.
/// Public because it is operationally visible: a user who lists the directory
/// sees this file, and someone debugging a stuck store needs to know which
/// path to look at. It holds no data and is never read.
pub fn lock_path(store: &Path) -> PathBuf {
    store.with_extension("json.lock")
}

/// An advisory lock over one store, released when this value is dropped.
///
/// # Why an OS lock and not a lock file we create and delete
///
/// The obvious implementation — create a file with `create_new`, delete it on
/// the way out — is atomic enough to be tempting and has a failure mode worse
/// than the bug it fixes: a process that is killed, panics, or loses power
/// leaves the file behind, and every later run refuses to start against a
/// holder that no longer exists. Recovering means telling a user to delete a
/// file, which is both an unpleasant thing to document and an invitation to
/// delete it while a real holder is running.
///
/// An OS advisory lock has no stale state to recover from. The kernel drops it
/// when the file descriptor closes, which happens on `Drop`, on `panic`, on
/// `SIGKILL` and on power loss alike. That property was checked rather than
/// assumed: a holder killed with `SIGKILL`, leaving the lock file on disk,
/// releases the lock to the next process.
///
/// Advisory, so it binds the processes that ask. Everything that writes this
/// store goes through this module, and anything editing `memory.json` by hand
/// was never going to be coordinated with.
#[derive(Debug)]
pub struct Lock {
    file: File,
}

impl Drop for Lock {
    fn drop(&mut self) {
        // Best effort, and unimportant: closing the descriptor releases the
        // lock whatever this returns. Explicit so the release is visible at
        // the point it happens rather than implied by a field going away.
        let _ = self.file.unlock();
    }
}

impl Lock {
    /// Take the lock, waiting up to [`LOCK_WAIT`] for a holder to finish.
    ///
    /// `exclusive` for anything that will write; a shared lock otherwise, so
    /// that reading the store while another process reads it does not
    /// serialise, and reading it *while it is being written* still waits.
    fn acquire(store: &Path, exclusive: bool) -> Result<Self, HostError> {
        Self::acquire_within(store, exclusive, LOCK_WAIT)
    }

    /// [`Lock::acquire`], with the wait as an argument.
    ///
    /// Separate so the tests can state a contention case without spending
    /// [`LOCK_WAIT`] proving it. Private: the bound is a property of the
    /// operation, not something a caller should be choosing per call site.
    fn acquire_within(store: &Path, exclusive: bool, wait: Duration) -> Result<Self, HostError> {
        let path = lock_path(store);
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(|e| {
                HostError::Store(format!("could not open the lock file beside {WHERE}: {e}"))
            })?;

        let deadline = Instant::now() + wait;
        let mut backoff = Duration::from_millis(1);
        loop {
            let attempt = if exclusive {
                file.try_lock()
            } else {
                file.try_lock_shared()
            };
            // `TryLockError` separates "someone holds it" from "the lock is
            // broken" in the type, so the two cannot be confused for each
            // other here the way an `ErrorKind` comparison could.
            match attempt {
                Ok(()) => return Ok(Lock { file }),
                Err(std::fs::TryLockError::WouldBlock) => {
                    if Instant::now() >= deadline {
                        return Err(HostError::Store(format!(
                            "another process is using {WHERE} and has held it for more than {:.0?} -- rmem and rmem-mcp share one store and take turns, so this usually means the other one is wedged rather than busy. Nothing was written.",
                            wait
                        )));
                    }
                    std::thread::sleep(backoff);
                    // Capped so a long wait is still responsive rather than
                    // sleeping through the deadline it is measured against.
                    backoff = (backoff * 2).min(Duration::from_millis(50));
                }
                // Not contention: the lock file itself is unusable, which is a
                // different problem and must not be reported as a busy peer.
                Err(std::fs::TryLockError::Error(e)) => {
                    return Err(HostError::Store(format!("could not lock {WHERE}: {e}")))
                }
            }
        }
    }
}

/// Read the store, run `f` over it, and write it back — all under one lock.
///
/// # The lock has to span the read as well as the write
///
/// Locking [`save`] alone would look like a fix and not be one. The failure
/// this closes is a lost update, not a torn file:
///
/// ```text
/// A: load ................................ save   <- writes what it read
/// B:        load ... change ... save              <- and this is gone
/// ```
///
/// Both saves are individually fine. What is lost is B's change, because A's
/// snapshot predates it and A never looked again. Only a lock held across the
/// whole read-modify-write stops that, which is why this is a closure and not
/// a pair of functions a caller is trusted to bracket.
///
/// It also means a caller cannot keep the [`Engine`] afterwards: the borrow
/// ends with the lock. That is the point rather than a limitation — an engine
/// outliving its lock is exactly the stale snapshot above.
pub fn with_write<T>(
    path: &Path,
    ruleset: Ruleset,
    policy: Policy,
    dimension: usize,
    metric: Metric,
    f: impl FnOnce(&mut Engine) -> Result<T, HostError>,
) -> Result<T, HostError> {
    let _lock = Lock::acquire(path, true)?;
    let mut engine = load(path, ruleset, policy, dimension, metric)?;
    let out = f(&mut engine)?;
    save(path, &engine)?;
    Ok(out)
}

/// Read the store and run `f` over it, under a shared lock.
///
/// Shared, so several readers do not queue behind each other, and a reader
/// still waits for a writer that is mid-save rather than reading whatever the
/// rename left behind.
pub fn with_read<T>(
    path: &Path,
    ruleset: Ruleset,
    policy: Policy,
    dimension: usize,
    metric: Metric,
    f: impl FnOnce(&Engine) -> Result<T, HostError>,
) -> Result<T, HostError> {
    let _lock = Lock::acquire(path, false)?;
    let engine = load(path, ruleset, policy, dimension, metric)?;
    f(&engine)
}

/// Write the store to `path`.
///
/// Through a temporary file in the same directory, then renamed over the
/// target. An interrupted write leaves the previous snapshot intact rather than
/// a truncated one — this store's whole value is that it stays reconstructible,
/// and a half-written file is the one way to lose that outright. Same directory
/// because a rename across filesystems is a copy, which is not atomic.
///
/// That guarantee covers an interrupted *process* and not a power cut. The
/// temporary file is never fsynced, and neither is the directory, so a
/// filesystem is free to commit the rename before the bytes it points at:
/// after a crash the store can be an empty or truncated file where the
/// previous snapshot used to be. Named rather than fixed — the lock that used
/// to be named beside it is now [`with_write`], and this one is a different
/// kind of limit: a CLI a person runs at a prompt loses the turn it was in the
/// middle of either way, and
/// paying two fsyncs on every `remember` to narrow a window that closes on
/// the next command is not obviously the right trade. Anything that needs it
/// should fsync the temporary file before the rename and the directory
/// after.
pub fn save(path: &Path, engine: &Engine) -> Result<(), HostError> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, engine.snapshot())
        .map_err(|e| HostError::Store(format!("could not write beside {WHERE}: {e}")))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        // Leave nothing behind on the failing path either.
        let _ = std::fs::remove_file(&tmp);
        HostError::Store(format!("could not replace {WHERE}: {e}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    fn engine_parts() -> (Ruleset, Policy, usize, Metric) {
        let config: crate::config::Config = toml::from_str(crate::config::TEMPLATE).unwrap();
        (
            config.ruleset().unwrap(),
            config.policy_for_engine().unwrap(),
            3,
            Metric::Cosine,
        )
    }

    #[test]
    fn a_store_that_does_not_exist_yet_opens_empty_rather_than_failing() {
        // The first command is not a special case a user should have to know
        // about.
        let dir = TempDir::new();
        let (r, p, d, m) = engine_parts();
        let engine = load(&dir.path().join("absent.json"), r, p, d, m).unwrap();
        assert_eq!(engine.entity_count(), 0);
    }

    // ---- the lock -----------------------------------------------------------

    /// Short enough that a contention test costs milliseconds rather than
    /// [`LOCK_WAIT`], long enough that a slow machine does not fail it.
    const BRIEF: Duration = Duration::from_millis(60);

    fn observation(value: &str, at: rm_engine::Timestamp) -> rm_engine::Observation {
        rm_engine::Observation {
            kind: "person".to_string(),
            mention: rm_engine::Record::new().with("name", "Ben Severn"),
            attribute: "employer".to_string(),
            value: Some(value.to_string()),
            valid: rm_engine::Interval::since(at),
            provenance: rm_engine::Provenance::new(rm_engine::Source::UserAssertion, at, "test"),
            embedding: vec![1.0, 0.0, 0.0],
        }
    }

    #[test]
    fn an_exclusive_lock_excludes_a_second_one() {
        let dir = TempDir::new();
        let path = dir.path().join("memory.json");

        let held = Lock::acquire_within(&path, true, BRIEF).unwrap();
        let second = Lock::acquire_within(&path, true, BRIEF);
        assert!(second.is_err(), "a second exclusive lock was granted");

        // And the refusal says what is happening and that nothing was lost,
        // because this is the one message a user hits during normal use.
        let HostError::Store(why) = second.unwrap_err() else {
            panic!("expected a store error")
        };
        assert!(why.contains("another process is using"), "{why}");
        assert!(why.contains("Nothing was written"), "{why}");

        // Released on drop, so the next caller gets it.
        drop(held);
        assert!(Lock::acquire_within(&path, true, BRIEF).is_ok());
    }

    #[test]
    fn readers_do_not_queue_behind_each_other_but_do_behind_a_writer() {
        let dir = TempDir::new();
        let path = dir.path().join("memory.json");

        let reader = Lock::acquire_within(&path, false, BRIEF).unwrap();
        assert!(
            Lock::acquire_within(&path, false, BRIEF).is_ok(),
            "two readers must not serialise"
        );
        assert!(
            Lock::acquire_within(&path, true, BRIEF).is_err(),
            "a writer must wait for a reader"
        );
        drop(reader);

        let writer = Lock::acquire_within(&path, true, BRIEF).unwrap();
        assert!(
            Lock::acquire_within(&path, false, BRIEF).is_err(),
            "a reader must wait for a writer -- mid-save is exactly when the \
             file on disk is not the store"
        );
        drop(writer);
    }

    #[test]
    fn the_lock_is_a_sidecar_that_save_does_not_disturb() {
        // `save` replaces the store through a rename. A lock taken on the
        // store file itself would be a lock on an unlinked inode afterwards,
        // and the next process would lock the new file and believe it was
        // alone. This is the property that stops that.
        let dir = TempDir::new();
        let path = dir.path().join("memory.json");
        let (r, p, d, m) = engine_parts();

        let held = Lock::acquire_within(&path, true, BRIEF).unwrap();
        let engine = load(&path, r, p, d, m).unwrap();
        save(&path, &engine).unwrap();

        assert!(
            Lock::acquire_within(&path, true, BRIEF).is_err(),
            "the lock did not survive a save"
        );
        drop(held);
        assert_ne!(
            lock_path(&path),
            path,
            "the lock must not be the store file"
        );
    }

    // ---- the brackets -------------------------------------------------------

    #[test]
    fn with_write_persists_and_with_read_does_not() {
        let dir = TempDir::new();
        let path = dir.path().join("memory.json");

        let (r, p, d, m) = engine_parts();
        with_write(&path, r, p, d, m, |engine| {
            engine.remember(observation("Acme", 1)).unwrap();
            Ok(())
        })
        .unwrap();

        let (r, p, d, m) = engine_parts();
        let after_write = with_read(&path, r, p, d, m, |engine| Ok(engine.snapshot())).unwrap();

        // A change made inside `with_read` reaches nothing: the engine is a
        // local value the bracket drops without saving.
        let (r, p, d, m) = engine_parts();
        with_read(&path, r, p, d, m, |engine| {
            assert!(engine.snapshot().contains("Acme"));
            Ok(())
        })
        .unwrap();

        let (r, p, d, m) = engine_parts();
        let unchanged = with_read(&path, r, p, d, m, |engine| Ok(engine.snapshot())).unwrap();
        assert_eq!(after_write, unchanged);
    }

    #[test]
    fn a_second_write_sees_the_first_rather_than_overwriting_it() {
        // The lost update this whole arrangement exists to prevent. Both calls
        // load inside their own lock, so the second reads what the first
        // wrote; the API gives a caller no way to hold an engine across the
        // boundary and save a snapshot that predates someone else's work.
        let dir = TempDir::new();
        let path = dir.path().join("memory.json");

        for (value, at) in [("Acme", 1), ("Globex", 2)] {
            let (r, p, d, m) = engine_parts();
            with_write(&path, r, p, d, m, |engine| {
                engine.remember(observation(value, at)).unwrap();
                Ok(())
            })
            .unwrap();
        }

        let (r, p, d, m) = engine_parts();
        let snapshot = with_read(&path, r, p, d, m, |engine| Ok(engine.snapshot())).unwrap();
        assert!(snapshot.contains("Acme"), "the first write was lost");
        assert!(snapshot.contains("Globex"), "the second write was lost");
    }

    #[test]
    fn a_failing_closure_writes_nothing_and_frees_the_lock() {
        let dir = TempDir::new();
        let path = dir.path().join("memory.json");

        let (r, p, d, m) = engine_parts();
        let out: Result<(), HostError> = with_write(&path, r, p, d, m, |engine| {
            engine.remember(observation("Acme", 1)).unwrap();
            Err(HostError::Store("refused".to_string()))
        });
        assert!(out.is_err());
        assert!(!path.exists(), "a refused write must not create the store");
        assert!(
            Lock::acquire_within(&path, true, BRIEF).is_ok(),
            "the lock outlived the failure"
        );
    }

    #[test]
    fn a_bracket_over_a_store_that_does_not_exist_yet_still_works() {
        // `load` treats a missing file as an empty store, and the lock is
        // taken before that decision, so the very first command is not a
        // special case either.
        let dir = TempDir::new();
        let path = dir.path().join("memory.json");
        let (r, p, d, m) = engine_parts();
        with_write(&path, r, p, d, m, |_| Ok(())).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn a_saved_store_reads_back_as_the_same_store() {
        let dir = TempDir::new();
        let path = dir.path().join("memory.json");
        let (r, p, d, m) = engine_parts();
        let mut engine = load(&path, r, p, d, m).unwrap();
        engine
            .remember(rm_engine::Observation {
                kind: "person".to_string(),
                mention: rm_engine::Record::new().with("name", "Ben Severn"),
                attribute: "employer".to_string(),
                value: Some("Acme".to_string()),
                valid: rm_engine::Interval::since(1),
                provenance: rm_engine::Provenance::new(rm_engine::Source::UserAssertion, 1, "test"),
                embedding: vec![1.0, 0.0, 0.0],
            })
            .unwrap();
        save(&path, &engine).unwrap();

        let (r, p, d, m) = engine_parts();
        let restored = load(&path, r, p, d, m).unwrap();
        assert_eq!(restored.snapshot(), engine.snapshot());
    }

    #[test]
    fn saving_leaves_no_temporary_file_behind() {
        // A temp file left in the store's directory is both litter and a
        // half-store that a later reader could mistake for the real one.
        let dir = TempDir::new();
        let path = dir.path().join("memory.json");
        let (r, p, d, m) = engine_parts();
        let engine = load(&path, r, p, d, m).unwrap();
        save(&path, &engine).unwrap();

        let left: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n != "memory.json")
            .collect();
        assert!(left.is_empty(), "left behind: {left:?}");
    }

    #[test]
    fn saving_over_an_existing_store_replaces_it_whole() {
        let dir = TempDir::new();
        let path = dir.path().join("memory.json");
        std::fs::write(&path, "this is not a store and must not survive").unwrap();

        let (r, p, d, m) = engine_parts();
        // A file that is not a store is a hard error, not something to
        // overwrite silently -- the user pointed at the wrong path.
        assert!(load(&path, r, p, d, m).is_err());
    }

    #[test]
    fn a_store_that_is_not_a_store_says_so_and_names_the_field_that_points_at_it() {
        let dir = TempDir::new();
        let path = dir.path().join("memory.json");
        std::fs::write(&path, "{ not json").unwrap();
        let (r, p, d, m) = engine_parts();
        // Not `.unwrap_err()`: that needs `Engine: Debug` for the panic message
        // on the `Ok` arm, and adding a derive to a public type just to satisfy
        // a test is the wrong way round -- rm-engine makes the same call.
        let Err(err) = load(&path, r, p, d, m) else {
            panic!("a file that is not a store must not open");
        };
        let text = err.to_string();
        assert!(text.contains("[store] path"), "{text}");
        assert!(
            !text.contains("memory.json"),
            "the path is a value out of rmem.toml, so it is named and not printed: {text}"
        );
    }

    #[test]
    fn no_store_message_prints_the_path_it_was_given() {
        // The rule this module used to hold an exception to. The exception's
        // argument was that a pasted credential could not reach these messages
        // because a nonsense path takes the `NotFound` branch -- true on Unix,
        // false on Windows, where `ERROR_INVALID_NAME` gives `InvalidFilename`
        // for a path containing `* ? | " < >` (an Azure SAS token begins with
        // `?`) and a directory gives `PermissionDenied`.
        //
        // So the fixtures are the shapes that argument missed: a path that is
        // a directory, one with characters Windows refuses outright, and a
        // temp file whose contents are not a store. Between them they reach
        // the read arm, the parse arm and the save arm on either platform.
        const CANARY: &str = "REALSECRETabc123DEF456";
        let dir = TempDir::new();

        let a_directory = dir.path().join(CANARY);
        std::fs::create_dir_all(&a_directory).unwrap();
        let illegal = dir.path().join(format!("?{CANARY}*"));
        let not_a_store = dir.path().join(format!("{CANARY}.json"));
        std::fs::write(&not_a_store, "{ not json").unwrap();

        let mut refused = 0;
        for path in [&a_directory, &illegal, &not_a_store] {
            let (r, p, d, m) = engine_parts();
            if let Err(err) = load(path, r, p, d, m) {
                refused += 1;
                let text = err.to_string();
                assert!(text.contains("[store] path"), "{text}");
                assert!(!text.contains(CANARY), "the path came back out: {text}");
            }

            // And the write side, which the exception's argument never covered
            // at all: `save` names the path whether or not anything is there.
            let (r, p, d, m) = engine_parts();
            let engine = Engine::new(VectorIndex::new(d, m), r, p);
            if let Err(err) = save(&a_directory, &engine) {
                refused += 1;
                let text = err.to_string();
                assert!(!text.contains(CANARY), "the path came back out: {text}");
            }
        }
        assert!(
            refused >= 2,
            "the fixtures stopped being refused, so this stopped testing anything"
        );
    }

    #[test]
    fn a_restored_store_built_at_a_different_dimension_is_refused_naming_both() {
        // Not a parse error -- `Engine::open` has no way to know the caller's
        // config disagrees with what the snapshot was built for. Left
        // unchecked here it would still be caught, but only on the first
        // `remember` or `recall`, far from a message that names the cause.
        let dir = TempDir::new();
        let path = dir.path().join("memory.json");
        let (r, p, d, m) = engine_parts();
        let engine = load(&path, r, p, d, m).unwrap();
        save(&path, &engine).unwrap();

        let (r, p, _, m) = engine_parts();
        let Err(err) = load(&path, r, p, 4, m) else {
            panic!("a dimension that disagrees with the stored index must not open silently");
        };
        assert!(err.to_string().contains('3'), "{err}");
        assert!(err.to_string().contains('4'), "{err}");
    }

    #[test]
    fn a_restored_store_built_under_a_different_metric_is_refused_naming_both() {
        let dir = TempDir::new();
        let path = dir.path().join("memory.json");
        let (r, p, d, m) = engine_parts();
        let engine = load(&path, r, p, d, m).unwrap();
        save(&path, &engine).unwrap();

        let (r, p, d, _) = engine_parts();
        let Err(err) = load(&path, r, p, d, Metric::L2) else {
            panic!("a metric that disagrees with the stored index must not open silently");
        };
        assert!(err.to_string().contains("Cosine"), "{err}");
        assert!(err.to_string().contains("L2"), "{err}");
    }

    #[test]
    fn a_save_that_cannot_reach_its_temporary_file_leaves_the_original_untouched() {
        // The literal path `save` writes its temp file to, pre-occupied by a
        // directory so the temp write cannot succeed. A plain overwrite of
        // `path` would pass every other test in this module -- see
        // `saving_leaves_no_temporary_file_behind`, which a direct write also
        // satisfies trivially, having never made a temp file to leave behind.
        // This is the one arrangement that only the temp-file-then-rename
        // implementation survives: it fails before `path` is ever touched,
        // where a direct write would already have clobbered it.
        let dir = TempDir::new();
        let path = dir.path().join("memory.json");
        let (r, p, d, m) = engine_parts();
        let mut engine = load(&path, r, p, d, m).unwrap();
        engine
            .remember(rm_engine::Observation {
                kind: "person".to_string(),
                mention: rm_engine::Record::new().with("name", "Ben Severn"),
                attribute: "employer".to_string(),
                value: Some("Acme".to_string()),
                valid: rm_engine::Interval::since(1),
                provenance: rm_engine::Provenance::new(rm_engine::Source::UserAssertion, 1, "test"),
                embedding: vec![1.0, 0.0, 0.0],
            })
            .unwrap();
        save(&path, &engine).unwrap();
        let original = std::fs::read(&path).unwrap();

        std::fs::create_dir(path.with_extension("json.tmp")).unwrap();

        let (r, p, d, m) = engine_parts();
        let mut other = load(&path, r, p, d, m).unwrap();
        other
            .remember(rm_engine::Observation {
                kind: "person".to_string(),
                mention: rm_engine::Record::new().with("name", "Someone Else"),
                attribute: "employer".to_string(),
                value: Some("Other Co".to_string()),
                valid: rm_engine::Interval::since(2),
                provenance: rm_engine::Provenance::new(rm_engine::Source::UserAssertion, 2, "test"),
                embedding: vec![0.0, 1.0, 0.0],
            })
            .unwrap();
        assert!(
            save(&path, &other).is_err(),
            "the temp file has nowhere to go, so the save must fail rather than clobber path directly"
        );

        let after = std::fs::read(&path).unwrap();
        assert_eq!(
            original, after,
            "a save that cannot complete must not leave the original store altered"
        );
    }
}

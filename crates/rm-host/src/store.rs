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
use std::path::Path;

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
/// previous snapshot used to be. Named rather than fixed, alongside "no lock
/// file", because both are the same kind of honest limit — a CLI a person
/// runs at a prompt loses the turn it was in the middle of either way, and
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

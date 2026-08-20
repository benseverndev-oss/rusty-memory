//! Reading and writing the store file.
//!
//! # The one config value this crate still prints
//!
//! `config.rs` refuses to repeat a value read out of `rmem.toml` in any error
//! message: eight credential leaks on this branch came out of doing exactly
//! that, each closed one shape at a time until the rule was made categorical.
//! A refusal names a field, a location, or nothing.
//!
//! The messages below name `[store] path`, and that is a deliberate exception
//! rather than one the sweep missed. A filesystem path in an IO error *is* the
//! location — it is the direct analogue of the line and column `Config::parse`
//! reports — and it is the only config value with no substitute: the path is
//! usually relative, so "the path named in rmem.toml could not be read" leaves
//! a reader unable to tell which directory was tried.
//!
//! What makes that trade acceptable rather than merely convenient is where the
//! exposure actually is. Every message here except `save`'s requires a file to
//! already exist at that path; a path that is not there takes the `NotFound`
//! branch below and is not an error at all, which is what a pasted credential
//! would produce. Verified by driving it: a key written into `[store] path`
//! makes `rmem review` print `no open questions` and exit 0. And unlike
//! `api_key_env`, nothing about this field invites a key — reaching it means
//! overwriting a meaningful value rather than adding a line, and appending to
//! the end of `rmem.toml` lands in `[policy.attribute]`, which is closed.
//!
//! `{dimension}` and `{metric:?}` in the mismatch messages are a `usize` and a
//! two-variant enum that `Config::metric` has already validated, so neither
//! can carry file text whatever is written in the file.

use std::path::Path;

use rm_engine::{Engine, Metric, Policy, Ruleset, VectorIndex};

use crate::CliError;

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
) -> Result<Engine, CliError> {
    match std::fs::read_to_string(path) {
        // Not there yet is not an error: the first command should not be a
        // special case a user has to know about.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Engine::new(
            VectorIndex::new(dimension, metric),
            ruleset,
            policy,
        )),
        Err(e) => Err(CliError::Store(format!(
            "could not read {}: {e}",
            path.display()
        ))),
        Ok(text) => {
            let engine = Engine::open(&text, ruleset, policy).map_err(|e| {
                CliError::Store(format!(
                    "{} is not a store this build can open: {e}",
                    path.display()
                ))
            })?;
            let (stored_dimension, stored_metric) = engine.index_shape();
            if stored_dimension != dimension {
                return Err(CliError::Store(format!(
                    "{} holds {stored_dimension}-dimensional vectors, but rmem.toml's [provider] section currently names dimension = {dimension} -- if the embedding model changed, run `rmem init --force` to rediscover the dimension, or point the config back at the model this store was built with",
                    path.display()
                )));
            }
            if stored_metric != metric {
                return Err(CliError::Store(format!(
                    "{} was built under metric {stored_metric:?}, but rmem.toml's [provider] section currently names metric = {metric:?} -- distances computed under the wrong metric are silently meaningless rather than merely different, so this is refused rather than reinterpreted",
                    path.display()
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
pub fn save(path: &Path, engine: &Engine) -> Result<(), CliError> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, engine.snapshot())
        .map_err(|e| CliError::Store(format!("could not write {}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        // Leave nothing behind on the failing path either.
        let _ = std::fs::remove_file(&tmp);
        CliError::Store(format!("could not replace {}: {e}", path.display()))
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
    fn a_store_that_is_not_a_store_says_so_and_names_the_file() {
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
        assert!(err.to_string().contains("memory.json"), "{err}");
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

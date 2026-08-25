//! The control: a store with one slot per key and no idea what time it is.

use std::collections::HashMap;

use rm_engine::StableId;

/// A flat latest-wins store.
///
/// One value per `(entity, attribute)`, overwritten in arrival order. **It
/// takes no time parameter at all** -- that is the design, not a handicap.
/// Asked what held in March it returns what it holds now, because that is all
/// it has.
///
/// Deliberately the naive thing, in the spirit of the control that beat the
/// pipeline in `benches/locomo`. A cleverer control -- latest by *valid* time
/// -- was considered and turned down: it is not what anyone actually builds, so
/// beating it would prove less about the real alternative.
#[derive(Default)]
pub struct Flat {
    latest: HashMap<(StableId, String), Option<String>>,
}

impl Flat {
    pub fn new() -> Self {
        Flat::default()
    }

    /// Write, overwriting whatever was there. `None` is a tombstone.
    pub fn remember(&mut self, entity: StableId, attribute: &str, value: Option<&str>) {
        self.latest
            .insert((entity, attribute.to_string()), value.map(str::to_string));
    }

    /// What it holds, whatever was asked.
    ///
    /// The outer `Option` is whether this key was ever written; the inner is
    /// the value, `None` for a tombstone. The two are kept apart because the
    /// store they are compared against keeps them apart -- `Believed::Unknown`
    /// is "it has never come up" and `Believed::Absent` is "someone said there
    /// is none". Collapsing them here would give the control an answer it has
    /// not got.
    pub fn about(&self, entity: StableId, attribute: &str) -> Option<Option<String>> {
        self.latest.get(&(entity, attribute.to_string())).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_holds_one_value_per_key_and_the_last_writer_wins() {
        let mut f = Flat::new();
        assert_eq!(f.about(1, "employer"), None, "never written");

        f.remember(1, "employer", Some("Acme"));
        assert_eq!(f.about(1, "employer"), Some(Some("Acme".to_string())));

        f.remember(1, "employer", Some("Globex"));
        assert_eq!(
            f.about(1, "employer"),
            Some(Some("Globex".to_string())),
            "arrival order, not valid time -- that is the whole control"
        );

        // Keys are independent.
        f.remember(2, "employer", Some("Initech"));
        assert_eq!(f.about(1, "employer"), Some(Some("Globex".to_string())));
        assert_eq!(f.about(2, "employer"), Some(Some("Initech".to_string())));
        assert_eq!(f.about(1, "spouse"), None);
    }

    /// A tombstone is a value, not an absence of one. Collapsing the two would
    /// give the control an answer it has not got and flatter it.
    #[test]
    fn a_tombstone_is_remembered_as_a_tombstone() {
        let mut f = Flat::new();
        f.remember(1, "employer", Some("Acme"));
        f.remember(1, "employer", None);
        assert_eq!(f.about(1, "employer"), Some(None), "written, and empty");
    }
}

//! Which survivorship strategy applies to which attribute.

use std::collections::BTreeMap;

use rm_survivor::Strategy;

/// Attribute name to strategy, with a fallback.
///
/// Set once rather than at every call site: "employer resolves by valid
/// interval, display name by most recent" is a property of the domain, not of
/// the question being asked.
#[derive(Clone, Debug, PartialEq)]
pub struct Policy {
    default: Strategy,
    by_attribute: BTreeMap<String, Strategy>,
}

impl Policy {
    pub fn new(default: Strategy) -> Self {
        Policy {
            default,
            by_attribute: BTreeMap::new(),
        }
    }

    /// Builder-style override for one attribute.
    pub fn with(mut self, attribute: impl Into<String>, strategy: Strategy) -> Self {
        self.by_attribute.insert(attribute.into(), strategy);
        self
    }

    pub fn for_attribute(&self, attribute: &str) -> &Strategy {
        self.by_attribute.get(attribute).unwrap_or(&self.default)
    }
}

//! A set of bound names with cheap scope entry and exit.
//!
//! The parser used to clone a `BTreeSet<String>` of bound names at every
//! function, block, `for`, match arm, closure and handler clause and restore
//! the clone when the scope ended. One function with thousands of locals then
//! cost time quadratic in its locals. Scopes only ever add names before they
//! are restored, so an undo trail gives the same answers: entering a scope
//! records the trail length, and leaving it removes exactly the names added
//! since. Membership stays a hash lookup.

use std::collections::{BTreeSet, HashMap};

#[derive(Debug, Default)]
pub(crate) struct ScopedNames {
    /// How many live bindings carry each name; a name is bound while its
    /// count is above zero.
    counts: HashMap<String, usize>,
    /// Every insertion since construction, in order, so a scope can undo its
    /// own insertions.
    trail: Vec<String>,
}

/// A scope entry point returned by [`ScopedNames::mark`].
#[derive(Clone, Copy, Debug)]
#[must_use]
pub(crate) struct Mark(usize);

impl ScopedNames {
    pub(crate) fn contains(&self, name: &str) -> bool {
        self.counts.contains_key(name)
    }
    pub(crate) fn insert(&mut self, name: String) {
        *self.counts.entry(name.clone()).or_insert(0) += 1;
        self.trail.push(name);
    }
    pub(crate) fn extend(&mut self, names: impl IntoIterator<Item = String>) {
        for name in names {
            self.insert(name);
        }
    }
    /// Enters a scope.
    pub(crate) fn mark(&self) -> Mark {
        Mark(self.trail.len())
    }
    /// Leaves every scope entered at or after `mark`, removing the names they
    /// added. Equivalent to restoring a clone taken when `mark` was made.
    pub(crate) fn restore(&mut self, mark: Mark) {
        while self.trail.len() > mark.0 {
            let Some(name) = self.trail.pop() else { break };
            if let Some(count) = self.counts.get_mut(&name) {
                *count -= 1;
                if *count == 0 {
                    self.counts.remove(&name);
                }
            }
        }
    }
    /// The bound names in order, for contexts stored past this parse.
    pub(crate) fn to_set(&self) -> BTreeSet<String> {
        self.counts.keys().cloned().collect()
    }
}

impl From<BTreeSet<String>> for ScopedNames {
    fn from(names: BTreeSet<String>) -> Self {
        let mut scoped = Self::default();
        scoped.extend(names);
        scoped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_matches_a_restored_clone() {
        let mut names = ScopedNames::from(BTreeSet::from(["f".to_owned()]));
        let outer = names.mark();
        names.insert("x".into());
        let inner = names.mark();
        names.insert("x".into());
        names.insert("y".into());
        assert!(names.contains("x") && names.contains("y"));
        names.restore(inner);
        assert!(names.contains("x") && !names.contains("y"));
        names.insert("z".into());
        names.restore(outer);
        assert_eq!(names.to_set(), BTreeSet::from(["f".to_owned()]));
    }
}

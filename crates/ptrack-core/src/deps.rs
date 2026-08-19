//! Pure dependency-graph checks for plan and task edges.
//!
//! The task graph and the plan graph are separate graphs; callers pass one
//! graph at a time.

use std::collections::{BTreeMap, BTreeSet};

/// Reports whether adding the edge `from -> to` ("`from` depends on `to`")
/// would create a cycle in the dependency graph `deps`, a map from record ID
/// to the IDs it depends on.
///
/// A self-edge is a cycle. Otherwise the new edge closes a cycle exactly when
/// `to` can already reach `from`, which a depth-first walk from `to` decides.
#[must_use]
pub fn would_create_cycle(deps: &BTreeMap<u64, Vec<u64>>, from: u64, to: u64) -> bool {
    if from == to {
        return true;
    }
    let mut stack = vec![to];
    let mut visited = BTreeSet::new();
    while let Some(node) = stack.pop() {
        if node == from {
            return true;
        }
        if visited.insert(node)
            && let Some(next) = deps.get(&node)
        {
            stack.extend(next);
        }
    }
    false
}

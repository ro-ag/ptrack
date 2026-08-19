use std::collections::BTreeMap;

use crate::would_create_cycle;

fn graph(edges: &[(u64, &[u64])]) -> BTreeMap<u64, Vec<u64>> {
    edges
        .iter()
        .map(|(id, deps)| (*id, deps.to_vec()))
        .collect()
}

#[test]
fn a_self_edge_is_always_a_cycle() {
    assert!(would_create_cycle(&BTreeMap::new(), 1, 1));
}

#[test]
fn a_direct_back_edge_is_a_cycle() {
    let deps = graph(&[(2, &[1])]);
    assert!(would_create_cycle(&deps, 1, 2));
}

#[test]
fn a_transitive_back_edge_is_a_cycle() {
    // 4 -> 3 -> 2 -> 1, so 1 -> 4 closes the loop.
    let deps = graph(&[(4, &[3]), (3, &[2]), (2, &[1])]);
    assert!(would_create_cycle(&deps, 1, 4));
}

#[test]
fn forward_and_diamond_edges_are_not_cycles() {
    let deps = graph(&[(2, &[1]), (3, &[1]), (4, &[2, 3])]);
    assert!(!would_create_cycle(&deps, 4, 1));
    assert!(!would_create_cycle(&deps, 3, 2));
    assert!(!would_create_cycle(&deps, 5, 4));
}

#[test]
fn an_unrelated_existing_cycle_does_not_hang_or_flag_the_new_edge() {
    // 2 <-> 3 is already broken data; the walk must still terminate and the
    // disjoint edge 5 -> 6 is still acyclic.
    let deps = graph(&[(2, &[3]), (3, &[2])]);
    assert!(!would_create_cycle(&deps, 5, 6));
    assert!(would_create_cycle(&deps, 2, 3));
}

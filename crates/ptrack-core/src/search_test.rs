use crate::search;
use crate::test_support::snapshot;

#[test]
fn search_markdown_carries_the_same_hold_marker_as_every_other_surface() {
    let mut held = snapshot();
    held.plans[0].hold_reason = Some("budget freeze".to_owned());
    held.tasks[1].hold_reason = Some("waiting on review".to_owned());

    assert_eq!(
        search(&held, "cli").markdown(),
        "# Search: \"cli\"\n\
\n\
## Plans\n\
- #1 Build CLI [active] [on hold: budget freeze]\n\
\n"
    );
    assert!(search(&held, "context").markdown().contains(
        "## Tasks\n- [doing] #1 context command (plan 1) [on hold: waiting on review]\n"
    ));
}

#[test]
fn search_markdown_carries_the_claim_marker_on_plan_rows() {
    let mut claimed = snapshot();
    claimed.plans[0].claim_owner = Some("01hzvyekq3s7m8w9x0abcdefgh".to_owned());
    claimed
        .meta
        .actors
        .push(("01hzvyekq3s7m8w9x0abcdefgh".to_owned(), "Alice".to_owned()));

    assert_eq!(
        search(&claimed, "cli").markdown(),
        "# Search: \"cli\"\n\
\n\
## Plans\n\
- #1 Build CLI [active] [claimed: Alice]\n\
\n"
    );
}

#[test]
fn search_matches_exact_fields_case_insensitively_in_snapshot_order() {
    let snapshot = snapshot();
    let commands = search(&snapshot, "COMMAND");
    assert_eq!(
        commands
            .tasks
            .iter()
            .map(|task| task.id)
            .collect::<Vec<_>>(),
        vec![2, 1]
    );
    assert_eq!(
        commands.markdown(),
        "# Search: \"COMMAND\"\n\
\n\
## Tasks\n\
- [done] #2 init command (plan 1)\n\
- [doing] #1 context command (plan 1)\n\
\n"
    );

    let milestone = search(&snapshot, "ship");
    assert_eq!(milestone.milestones.len(), 1);
    assert!(milestone.plans.is_empty());
    assert!(milestone.tasks.is_empty());
    assert!(milestone.issues.is_empty());
    assert!(milestone.notes.is_empty());

    let issue_body = search(&snapshot, "REGISTRY");
    assert_eq!(
        issue_body
            .issues
            .iter()
            .map(|issue| issue.id)
            .collect::<Vec<_>>(),
        vec![1]
    );
    let note_body = search(&snapshot, "DEPENDENCY-FREE");
    assert_eq!(
        note_body
            .notes
            .iter()
            .map(|note| note.id)
            .collect::<Vec<_>>(),
        vec![2]
    );
}

#[test]
fn search_uses_go_simple_rune_lowercase_for_dotted_capital_i() {
    let mut snapshot = snapshot();
    snapshot.tasks[0].title = "İSTANBUL".to_owned();

    let view = search(&snapshot, "istanbul");
    assert_eq!(view.tasks.len(), 1);
    assert_eq!(view.tasks[0].title, "İSTANBUL");
}

#[test]
fn empty_search_matches_every_item_without_a_result_cap() {
    let snapshot = snapshot();
    let view = search(&snapshot, "");
    assert_eq!(view.milestones.len(), snapshot.milestones.len());
    assert_eq!(view.plans.len(), snapshot.plans.len());
    assert_eq!(view.tasks.len(), snapshot.tasks.len());
    assert_eq!(view.issues.len(), snapshot.issues.len());
    assert_eq!(view.notes.len(), snapshot.notes.len());
}

#[test]
fn search_heading_uses_go_quoted_string_syntax() {
    let view = search(&snapshot(), "a\"\n\u{0007}");
    assert_eq!(
        view.markdown(),
        "# Search: \"a\\\"\\n\\a\"\n\n_no matches_\n"
    );

    let unicode = search(&snapshot(), "\u{00a0}\u{200b}é🦀");
    assert_eq!(
        unicode.markdown(),
        "# Search: \"\\u00a0\\u200bé🦀\"\n\n_no matches_\n"
    );
}

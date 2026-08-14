use crate::{GUIDE_BEGIN, GUIDE_END};
use crate::{guide_block, guide_body, render_guide, upsert_guide};

#[test]
fn guide_body_matches_the_go_contract_shape() {
    assert!(guide_body().starts_with("## ptrack — session context\n\n"));
    assert!(guide_body().ends_with("`ptrack init --goal \"...\"`.\n"));
    assert_eq!(render_guide("  "), guide_body());
    assert!(render_guide(" rule ").ends_with("\n---\n\nrule\n"));
}

#[test]
fn guide_upsert_is_idempotent_and_preserves_surrounding_text() {
    let original = "intro\n\n<!-- ptrack:begin -->\nold\n<!-- ptrack:end -->\n\noutro\n";
    let (once, changed) = upsert_guide(original, "");
    assert!(changed);
    assert!(once.contains("intro"));
    assert!(once.contains("outro"));
    assert_eq!(once.matches(GUIDE_BEGIN).count(), 1);
    assert_eq!(once.matches(GUIDE_END).count(), 1);
    let (twice, changed) = upsert_guide(&once, "");
    assert!(!changed);
    assert_eq!(twice, once);
    assert_eq!(
        guide_block(""),
        format!("{GUIDE_BEGIN}\n{}{GUIDE_END}\n", guide_body())
    );
}

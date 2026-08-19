pub const GUIDE_BEGIN: &str = "<!-- ptrack:begin -->";
pub const GUIDE_END: &str = "<!-- ptrack:end -->";

/// Returns the compatibility guide body without marker lines.
#[must_use]
pub fn guide_body() -> &'static str {
    "## ptrack — session context\n\
\n\
This project uses `ptrack` to persist planning state so a fresh agent can\n\
resume after a previous session grew too large.\n\
\n\
**At session start** — reload context:\n\
- `ptrack context` — goal, summary, active plan, open tasks, blockers, open issues, inventory (add `--json` to parse).\n\
\n\
**If the project is empty** — populate it from this repo (README, docs, code, git\n\
log, open issues), then keep it current:\n\
- Goal: `ptrack goal set \"north star\"`\n\
- Milestones (checkpoints): `ptrack milestone add \"v1.0\" [--due YYYY-MM-DD]`\n\
- Plans (workstreams): `ptrack plan add \"...\" [--milestone N]`, then `ptrack plan use N` (also claims it)\n\
- Tasks with status: `ptrack task add \"...\" [--plan N]` then `task start` (in progress) / `task done` / `task block` (todo = pending)\n\
- Issues (bugs/problems): `ptrack issue add \"...\" [--severity high] [--task N]`\n\
- Decisions: `ptrack note add \"...\" [--task N | --plan N]`\n\
\n\
**Titles are names, not status.** Do not prefix titles with \"Pending:\", \"In\n\
progress:\", \"Done:\", etc. — ptrack tracks status separately. Set it with\n\
`task start|done|block`, `plan done|use`, `milestone done`, `issue close`. Rename with\n\
`ptrack <plan|task|milestone|issue> rename <id> \"new title\"`.\n\
\n\
**Pausing work.** A plan or task waiting on something external goes on hold with\n\
a reason, independently of its status: `ptrack task hold <id> \"waiting on review\"`\n\
/ `ptrack task resume <id>` (same for `plan hold|resume`). Completing the item\n\
clears its hold too. Do not pick up a held item; `ptrack next` skips them.\n\
\n\
**Working with other developers.** Configure your identity once per machine:\n\
`ptrack config set user \"<your name>\"` (a stable ID is minted the first time;\n\
renaming later keeps it). `ptrack plan use <id>` then claims the plan for you\n\
as well as making it your active plan; content changes to a plan claimed by\n\
someone else are refused. Holds, notes, and issue links stay open to everyone\n\
— use them to talk across a claim. `ptrack plan release <id>` frees your\n\
claim, finishing a plan releases it automatically, and\n\
`ptrack plan use <id> --steal` takes over an abandoned one.\n\
\n\
**Record decisions, not narration.** Notes are the human-visible audit trail of\n\
what you did and *why*. When you make a choice, hit a blocker, or find a\n\
constraint, capture it — one decision per note:\n\
`ptrack note add \"chose X over Y because Z\" --task N`. Do not log routine\n\
steps, tool output, or restate the code.\n\
\n\
**Commits are tracked.** Reference the task in commit messages as `#<id>` so the\n\
commit links to it (`ptrack hook install` records commits automatically; each\n\
commit's `#<id>` links it to that task, otherwise the active plan).\n\
\n\
**Before ending** — save the narrative for the next agent:\n\
- `ptrack summary set \"where we are\"`\n\
\n\
**Query on demand** (all bounded, `--json` available):\n\
- `ptrack next` · `ptrack board` · `ptrack milestone list` · `ptrack plan show <id>` · `ptrack task show <id>` · `ptrack task list --status doing,blocked` · `ptrack issue list` · `ptrack search <term>` · `ptrack note list`\n\
\n\
If no project exists yet: `ptrack init --goal \"...\"`.\n"
}

#[must_use]
pub fn render_guide(extra: &str) -> String {
    let extra = extra.trim();
    if extra.is_empty() {
        guide_body().to_owned()
    } else {
        format!("{}\n---\n\n{extra}\n", guide_body())
    }
}

#[must_use]
pub fn guide_block(extra: &str) -> String {
    format!("{GUIDE_BEGIN}\n{}{GUIDE_END}\n", render_guide(extra))
}

/// Pure compatibility upsert. Malformed and duplicate markers are removed
/// without deleting surrounding user text, then one canonical block is added.
#[must_use]
pub fn upsert_guide(content: &str, extra: &str) -> (String, bool) {
    let block = guide_block(extra);
    let begins: Vec<_> = content.match_indices(GUIDE_BEGIN).map(|(i, _)| i).collect();
    let ends: Vec<_> = content.match_indices(GUIDE_END).map(|(i, _)| i).collect();
    if begins.len() == 1 && ends.len() == 1 && ends[0] > begins[0] {
        let before = &content[..begins[0]];
        let after = content[ends[0] + GUIDE_END.len()..]
            .strip_prefix('\n')
            .unwrap_or(&content[ends[0] + GUIDE_END.len()..]);
        let updated = format!("{before}{block}{after}");
        let changed = updated != content;
        return (updated, changed);
    }

    let mut stripped = String::new();
    let mut rest = content;
    loop {
        let Some(begin) = rest.find(GUIDE_BEGIN) else {
            stripped.push_str(rest);
            break;
        };
        stripped.push_str(&rest[..begin]);
        let after_begin = &rest[begin + GUIDE_BEGIN.len()..];
        if let Some(end) = after_begin.find(GUIDE_END) {
            rest = &after_begin[end + GUIDE_END.len()..];
            rest = rest.strip_prefix('\n').unwrap_or(rest);
        } else {
            rest = after_begin;
        }
    }
    let stripped = stripped.replace(GUIDE_BEGIN, "").replace(GUIDE_END, "");
    let base = stripped.trim_end_matches([' ', '\t', '\n']);
    let updated = if base.is_empty() {
        block
    } else {
        format!("{base}\n\n{block}")
    };
    let changed = updated != content;
    (updated, changed)
}

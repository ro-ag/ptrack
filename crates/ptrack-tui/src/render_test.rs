use std::path::PathBuf;

use ptrack_core::{
    Commit, Issue, IssueStatus, MemoryKind, Meta, Milestone, MilestoneStatus, Note, NoteTarget,
    Plan, PlanStatus, ProjectSnapshot, Severity, Task, TaskStatus, Timestamp,
};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};

use crate::model::DetailTarget;
use crate::{Key, Model, RuntimeContext, Tab, draw, update};

fn model() -> Model {
    Model::new(
        ProjectSnapshot::new(
            Meta {
                goal: "Ship Unicode safely".to_owned(),
                summary: String::new(),
                active_plan: 0,
                created_at: Timestamp::Zero,
                updated_at: Timestamp::Zero,
                format_version: 4,
                last_write_version: "test".to_owned(),
                active_plans: Vec::new(),
                actors: Vec::new(),
            },
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
        ),
        RuntimeContext {
            project_root: PathBuf::from("/tmp/example"),
            database: PathBuf::from("/tmp/example/.ptrack/ptrack.redb"),
            global_home: PathBuf::from("/tmp/home"),
        },
    )
}

fn rendered(model: &Model, width: u16, height: u16) -> String {
    rendered_buffer(model, width, height)
        .content()
        .iter()
        .map(ratatui::buffer::Cell::symbol)
        .collect()
}

fn rendered_buffer(model: &Model, width: u16, height: u16) -> Buffer {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| draw(frame, model)).unwrap();
    terminal.backend().buffer().clone()
}

fn rendered_lines(model: &Model, width: u16, height: u16) -> Vec<String> {
    rendered_buffer(model, width, height)
        .content()
        .chunks(usize::from(width))
        .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect())
        .collect()
}

fn row_containing<'a>(buffer: &'a Buffer, needle: &str) -> (u16, &'a [ratatui::buffer::Cell]) {
    let width = usize::from(buffer.area.width);
    buffer
        .content()
        .chunks(width)
        .enumerate()
        .find_map(|(row, cells)| {
            let text: String = cells.iter().map(ratatui::buffer::Cell::symbol).collect();
            text.contains(needle)
                .then_some((u16::try_from(row).unwrap(), cells))
        })
        .expect("rendered row")
}

fn cell_with_symbol<'a>(
    cells: &'a [ratatui::buffer::Cell],
    symbol: &str,
) -> &'a ratatui::buffer::Cell {
    cells
        .iter()
        .find(|cell| cell.symbol() == symbol)
        .expect("rendered cell")
}

fn cell_at_text_offset<'a>(
    cells: &'a [ratatui::buffer::Cell],
    needle: &str,
    offset: usize,
) -> &'a ratatui::buffer::Cell {
    let start = cells
        .iter()
        .enumerate()
        .find_map(|(index, _)| {
            cells[index..]
                .iter()
                .map(ratatui::buffer::Cell::symbol)
                .collect::<String>()
                .starts_with(needle)
                .then_some(index)
        })
        .expect("rendered text");
    &cells[start + offset]
}

fn stamp(seconds: i64) -> Timestamp {
    Timestamp::Fixed {
        seconds,
        nanoseconds: 0,
        offset_seconds: 0,
    }
}

fn populated_model() -> Model {
    let snapshot = ProjectSnapshot::new(
        Meta {
            goal: "Deliver a terminal dashboard with careful Unicode wrapping and visible state"
                .to_owned(),
            summary: String::new(),
            active_plan: 1,
            created_at: stamp(1_700_000_000),
            updated_at: stamp(1_700_000_100),
            format_version: 4,
            last_write_version: "test".to_owned(),
        active_plans: Vec::new(), actors: Vec::new(),},
        vec![Milestone {
            id: 7,
            title: "Parity".to_owned(),
            status: MilestoneStatus::Open,
            due: stamp(1_800_000_000),
            order: 1,
            created_at: stamp(1_700_000_000),
            updated_at: stamp(1_700_000_100),
        actor: None, ulid: None,}],
        vec![Plan {
            id: 1,
            title: "Rust terminal UI".to_owned(),
            status: PlanStatus::Active,
            milestone_id: 7,
            order: 1,
            created_at: stamp(1_700_000_000),
            updated_at: stamp(1_700_000_100),
            hold_reason: None,
        actor: None, claim_conflict: false, claim_epoch: 0,claim_owner: None, ulid: None,}],
        vec![Task {
            id: 3,
            plan_id: 1,
            title: "Render every screen".to_owned(),
            status: TaskStatus::Doing,
            order: 1,
            created_at: stamp(1_700_000_000),
            updated_at: stamp(1_700_000_100),
            hold_reason: None,
        actor: None, ulid: None,}],
        vec![Issue {
            id: 9,
            title: "Narrow view clips".to_owned(),
            body: "The complete explanation must wrap rather than disappear.".to_owned(),
            status: IssueStatus::Open,
            severity: Severity::High,
            task_id: 3,
            created_at: stamp(1_700_000_000),
            updated_at: stamp(1_700_000_100),
        actor: None, ulid: None,}],
        vec![
            Note {
                id: 1,
                target: NoteTarget::Plan,
                target_id: 1,
                kind: MemoryKind::Decision,
                body: "decision-note".to_owned(),
                created_at: stamp(1_700_000_001),
            actor: None, ulid: None,},
            Note {
                id: 2,
                target: NoteTarget::Task,
                target_id: 3,
                kind: MemoryKind::Handoff,
                body: "This deliberately long Unicode note 界🙂 must wrap inside the detail box so its complete tail remains visible: TAIL".to_owned(),
                created_at: stamp(1_700_000_002),
            actor: None, ulid: None,},
        ],
        vec![
            Commit {
                id: 1,
                sha: "11111111aaaa".to_owned(),
                subject: "oldest-commit".to_owned(),
                plan_id: 1,
                task_id: 3,
                created_at: stamp(1_700_000_003),
            actor: None, ulid: None,},
            Commit {
                id: 2,
                sha: "22222222bbbb".to_owned(),
                subject: "newest-commit".to_owned(),
                plan_id: 1,
                task_id: 3,
                created_at: stamp(1_700_000_004),
            actor: None, ulid: None,},
        ],
    );
    Model::new(snapshot, model().context)
}

#[test]
fn welcome_and_command_menu_render_directly_to_cells() {
    let mut value = model();
    let welcome = rendered(&value, 80, 24);
    assert!(welcome.contains("PERSISTENT PROJECT MEMORY"));
    update(&mut value, &Key::Enter);
    update(&mut value, &Key::Char('?'));
    let menu = rendered(&value, 100, 30);
    for label in [
        "Overview",
        "Board",
        "Milestones",
        "Issues",
        "Maintenance",
        "Create backup",
    ] {
        assert!(menu.contains(label), "missing {label}");
    }
}

#[test]
fn welcome_wordmark_uses_distinct_display_rows() {
    let lines = rendered_lines(&model(), 60, 24);
    let wordmark = lines
        .iter()
        .position(|line| line.trim() == "P-TRACK")
        .expect("compact wordmark row");
    assert!(lines[wordmark - 1].contains('━'));
    assert!(lines[wordmark + 1].contains('━'));
}

#[test]
fn header_tabs_panels_and_footer_match_dashboard_chrome() {
    let mut value = populated_model();
    value.welcome = false;
    let screen = rendered(&value, 120, 30);
    for expected in [
        "p-track · example",
        "▰",
        "▱",
        "Plans · 1",
        "enter view · a add",
        "? menu · tab switch",
    ] {
        assert!(screen.contains(expected), "missing {expected:?}");
    }

    let narrow = rendered_lines(&value, 3, 12);
    assert_eq!(narrow[4].trim(), "Ove");
}

#[test]
fn list_window_centers_cursor_truncates_and_fills_selection_row() {
    let mut value = model();
    value.snapshot.meta.active_plan = 1;
    value.snapshot.plans = (1..=15)
        .map(|id| Plan {
            id,
            title: if id == 13 {
                "A selected plan title that must end with an ellipsis".to_owned()
            } else {
                format!("plan-{id}")
            },
            status: PlanStatus::Active,
            milestone_id: 0,
            order: i64::try_from(id).unwrap(),
            created_at: Timestamp::Zero,
            updated_at: Timestamp::Zero,
            hold_reason: None,
            actor: None,
            claim_conflict: false,
            claim_epoch: 0,
            claim_owner: None,
            ulid: None,
        })
        .collect();
    value.plan_cursor = 12;
    value.welcome = false;
    let buffer = rendered_buffer(&value, 60, 14);
    let lines: Vec<String> = buffer
        .content()
        .chunks(60)
        .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect())
        .collect();
    let selected_row = lines
        .iter()
        .position(|line| line.contains("#13"))
        .expect("selected plan row");
    assert!(lines[selected_row].contains('…'));
    assert!(!lines.iter().any(|line| line.contains("#1 plan-1")));
    for x in 2..27 {
        assert_eq!(
            buffer
                .cell((x, u16::try_from(selected_row).unwrap()))
                .unwrap()
                .bg,
            Color::Rgb(0x31, 0x32, 0x44),
            "selection background stopped at x={x}"
        );
    }
}

#[test]
fn detail_boxes_wrap_scroll_and_show_task_only_actions() {
    let mut value = populated_model();
    value.welcome = false;
    value.detail = Some(DetailTarget::Task(3));
    value.resize(80, 24);
    let top = rendered(&value, 80, 24);
    assert!(top.contains("╭─ Notes"));
    assert!(top.contains("M move · P to plan"));

    value.resize(46, 18);
    value.detail_offset = crate::render::detail_scroll_max(&value);
    let bottom = rendered(&value, 46, 18);
    assert!(bottom.contains("TAIL"), "{bottom}");
    assert!(bottom.contains("╭─ Commits"));

    value.detail = Some(DetailTarget::Plan(1));
    value.detail_offset = 0;
    let plan = rendered(&value, 80, 30);
    assert!(!plan.contains("M move · P to plan"));
    assert!(plan.contains("[decision] decision-note"));
    assert!(plan.find("newest-commit").unwrap() < plan.find("oldest-commit").unwrap());
}

#[test]
fn milestone_issue_and_maintenance_rows_preserve_source_parity() {
    let mut value = populated_model();
    value.welcome = false;
    value.tab = Tab::Milestones;
    value.snapshot.plans.clear();
    value.snapshot.tasks.clear();
    let milestones = rendered(&value, 120, 25);
    assert!(milestones.contains("--milestone 7"));
    assert!(milestones.contains("tasks: 0 done · 0 open"));

    value = populated_model();
    value.welcome = false;
    value.tab = Tab::Issues;
    let issues = rendered(&value, 100, 25);
    for expected in ["high", "open", "#9 Narrow view clips", "(task 3)"] {
        assert!(issues.contains(expected), "missing issue row {expected:?}");
    }

    value.tab = Tab::Maintenance;
    value.snapshot.meta.goal.clear();
    value.snapshot.meta.summary.clear();
    let maintenance = rendered(&value, 120, 25);
    // The renderer builds this line with Path::join, so the expected string is
    // derived the same way to keep the assertion platform-exact.
    let destination = format!(
        "Destination: {}",
        PathBuf::from("/tmp/home").join("backups").display()
    );
    for expected in [
        "Project health",
        "Maintenance actions",
        "(unset)",
        destination.as_str(),
        "ptrack guide",
        "ptrack hook install",
    ] {
        assert!(maintenance.contains(expected), "missing {expected:?}");
    }
}

#[test]
fn palette_and_per_span_styles_match_the_go_dashboard() {
    const ACCENT: Color = Color::Rgb(0x3d, 0xd6, 0xa3);
    const LAVENDER: Color = Color::Rgb(0xaf, 0xa8, 0xff);
    const BLUE: Color = Color::Rgb(0x5f, 0xaf, 0xff);
    const GREEN: Color = Color::Rgb(0x5f, 0xff, 0x87);
    const AMBER: Color = Color::Rgb(0xff, 0xd7, 0x5f);
    const RED: Color = Color::Rgb(0xff, 0x5f, 0x87);
    const TEXT: Color = Color::Rgb(0xe6, 0xe9, 0xf0);
    const GRAY: Color = Color::Rgb(0xb7, 0xc0, 0xd8);
    const DIM: Color = Color::Rgb(0x72, 0x7a, 0x8e);
    const FAINT: Color = Color::Rgb(0x31, 0x32, 0x44);
    const BORDER: Color = Color::Rgb(0x45, 0x47, 0x5a);
    const DARK_CYAN: Color = Color::Rgb(0x17, 0x8f, 0x95);
    const BLUE_GREEN: Color = Color::Rgb(0x3c, 0xd1, 0xa5);

    let mut value = populated_model();
    value.welcome = false;
    let overview = rendered_buffer(&value, 120, 30);
    let (_, selected_plan) = row_containing(&overview, "#1 Rust terminal UI");
    assert_eq!(cell_with_symbol(selected_plan, "▌").fg, ACCENT);
    assert_eq!(cell_with_symbol(selected_plan, "▌").bg, FAINT);
    assert_eq!(cell_with_symbol(selected_plan, "★").bg, FAINT);

    let header_rule = overview.content().chunks(120).nth(3).expect("header rule");
    assert_eq!(header_rule[0].fg, DARK_CYAN);
    assert_eq!(header_rule[119].fg, BLUE_GREEN);
    let header = &overview.content()[..120 * 3];
    for color in [LAVENDER, BLUE, GREEN, RED, BORDER] {
        assert!(
            header.iter().any(|cell| cell.fg == color),
            "missing {color:?}"
        );
    }

    value.focus = crate::model::PaneFocus::Tasks;
    let tasks = rendered_buffer(&value, 120, 30);
    let (_, parked_plan) = row_containing(&tasks, "#1 Rust terminal UI");
    assert_eq!(cell_with_symbol(parked_plan, "▏").fg, DIM);
    assert_eq!(cell_with_symbol(parked_plan, "★").fg, GREEN);
    let (_, selected_task) = row_containing(&tasks, "#3 Render every screen");
    assert_eq!(cell_with_symbol(selected_task, "▌").fg, ACCENT);

    value.focus = crate::model::PaneFocus::Plans;
    value.snapshot.meta.active_plan = 0;
    let task_rows = rendered_buffer(&value, 120, 30);
    let (_, task) = row_containing(&task_rows, "#3 Render every screen");
    assert_eq!(cell_with_symbol(task, "◐").fg, AMBER);
    assert!(
        task.iter()
            .any(|cell| cell.symbol() == "R" && cell.fg == TEXT)
    );

    value.tab = Tab::Milestones;
    let milestones = rendered_buffer(&value, 120, 30);
    let (_, milestone) = row_containing(&milestones, "#7 Parity");
    assert_eq!(cell_with_symbol(milestone, "▌").fg, ACCENT);
    let (_, plan) = row_containing(&milestones, "#1 Rust terminal UI");
    assert!(
        plan.iter()
            .any(|cell| cell.symbol() == "R" && cell.fg == TEXT)
    );
    assert!(
        plan.iter()
            .any(|cell| cell.symbol() == "[" && cell.fg == DIM)
    );

    value.detail = Some(DetailTarget::Plan(1));
    let detail = rendered_buffer(&value, 120, 35);
    let (_, commit) = row_containing(&detail, "22222222");
    assert!(
        commit
            .iter()
            .any(|cell| cell.symbol() == "2" && cell.fg == AMBER)
    );

    let (_, footer) = row_containing(&overview, "? menu · tab switch");
    let footer_key = cell_at_text_offset(footer, "? menu · tab switch", 0);
    assert_eq!(footer_key.fg, ACCENT);
    assert!(footer_key.modifier.contains(Modifier::BOLD));
    assert_eq!(
        cell_at_text_offset(footer, "? menu · tab switch", 2).fg,
        GRAY
    );
    assert_eq!(
        cell_at_text_offset(footer, "? menu · tab switch", 7).fg,
        DIM
    );
    assert_eq!(
        cell_at_text_offset(footer, "←/→ ↑/↓ navigate", 4).fg,
        ACCENT
    );
    assert_eq!(cell_at_text_offset(footer, "←/→ ↑/↓ navigate", 8).fg, GRAY);

    let (_, panel_hints) = row_containing(&overview, "enter view · a add");
    let panel_key = cell_at_text_offset(panel_hints, "enter view · a add", 0);
    assert_eq!(panel_key.fg, ACCENT);
    assert!(panel_key.modifier.contains(Modifier::BOLD));
    assert_eq!(
        cell_at_text_offset(panel_hints, "enter view · a add", 6).fg,
        GRAY
    );
    assert_eq!(
        cell_at_text_offset(panel_hints, "enter view · a add", 11).fg,
        DIM
    );
}

#[test]
fn board_header_hints_use_key_action_and_separator_styles() {
    let mut value = populated_model();
    value.welcome = false;
    value.tab = Tab::Board;
    let board = rendered_buffer(&value, 120, 30);
    let (_, board_hints) = row_containing(&board, "H/L status · a/e add/edit");
    let board_key = cell_at_text_offset(board_hints, "H/L status · a/e add/edit", 0);
    assert_eq!(board_key.fg, Color::Rgb(0x3d, 0xd6, 0xa3));
    assert!(board_key.modifier.contains(Modifier::BOLD));
    assert_eq!(
        cell_at_text_offset(board_hints, "H/L status · a/e add/edit", 4).fg,
        Color::Rgb(0xb7, 0xc0, 0xd8)
    );
    assert_eq!(
        cell_at_text_offset(board_hints, "H/L status · a/e add/edit", 11).fg,
        Color::Rgb(0x72, 0x7a, 0x8e)
    );
}

#[test]
fn inactive_plan_rows_reserve_the_active_star_column() {
    let mut value = populated_model();
    value.welcome = false;
    value.snapshot.plans.push(Plan {
        id: 2,
        title: "Inactive plan".to_owned(),
        status: PlanStatus::Done,
        milestone_id: 0,
        order: 2,
        created_at: Timestamp::Zero,
        updated_at: Timestamp::Zero,
        hold_reason: None,
        actor: None,
        claim_conflict: false,
        claim_epoch: 0,
        claim_owner: None,
        ulid: None,
    });
    value.focus = crate::model::PaneFocus::Tasks;

    let buffer = rendered_buffer(&value, 100, 24);
    let (_, active) = row_containing(&buffer, "#1 Rust terminal UI");
    let (_, inactive) = row_containing(&buffer, "#2 Inactive plan");
    let active_hash = active
        .iter()
        .position(|cell| cell.symbol() == "#")
        .expect("active plan hash");
    let inactive_hash = inactive
        .iter()
        .position(|cell| cell.symbol() == "#")
        .expect("inactive plan hash");
    assert_eq!(inactive_hash, active_hash);
}

#[test]
fn held_plans_and_tasks_keep_their_column_and_gain_a_pause_marker() {
    let mut value = populated_model();
    value.welcome = false;
    value.snapshot.plans[0].hold_reason = Some("waiting on design".to_owned());
    value.snapshot.tasks[0].hold_reason = Some("waiting on review".to_owned());

    // Overview rows carry the marker but never the reason.
    let overview = rendered(&value, 120, 30);
    assert!(overview.contains("⏸ #1 Rust terminal UI"), "{overview}");
    assert!(overview.contains("⏸ #3 Render every screen"), "{overview}");
    assert!(!overview.contains("waiting on"), "{overview}");

    // The board keeps the held task in its own Doing column — no hold lane.
    value.tab = Tab::Board;
    let board = rendered_lines(&value, 120, 30);
    let titles = board
        .iter()
        .find(|line| line.contains("Doing"))
        .expect("column titles");
    assert!(!titles.to_lowercase().contains("hold"), "{titles}");
    let card = board
        .iter()
        .find(|line| line.contains("#3 Render every screen"))
        .expect("board card");
    assert!(card.contains("⏸ #3 Render every screen"), "{card}");

    // Only the item view spells the reason out.
    value.tab = Tab::Overview;
    value.detail = Some(DetailTarget::Task(3));
    let task_detail = rendered(&value, 100, 30);
    assert!(
        task_detail.contains("On hold   ⏸ waiting on review"),
        "{task_detail}"
    );

    value.detail = Some(DetailTarget::Plan(1));
    let plan_detail = rendered(&value, 100, 30);
    assert!(
        plan_detail.contains("On hold   ⏸ waiting on design"),
        "{plan_detail}"
    );
    assert!(
        plan_detail.contains("⏸ #3 Render every screen"),
        "{plan_detail}"
    );
}

const ACTOR_A: &str = "01hzvyekq3s7m8w9x0abcdefgh";

#[test]
fn claimed_plans_show_the_owners_resolved_name() {
    let mut value = populated_model();
    value.welcome = false;
    value.snapshot.meta.actors = vec![(ACTOR_A.to_owned(), "Alice".to_owned())];
    value.snapshot.plans[0].claim_owner = Some(ACTOR_A.to_owned());

    // The plan list row spells out the resolved name, not just a marker.
    let overview = rendered(&value, 120, 30);
    assert!(overview.contains("Alice"), "{overview}");

    // The detail pane names the claim owner in its own row.
    value.detail = Some(DetailTarget::Plan(1));
    let plan_detail = rendered(&value, 100, 30);
    assert!(plan_detail.contains("Claimed by"), "{plan_detail}");
    assert!(plan_detail.contains("Alice"), "{plan_detail}");
}

#[test]
fn panels_clip_rows_and_input_cursor_stays_in_the_terminal() {
    let mut value = populated_model();
    value.welcome = false;
    value.menu = true;
    value.menu_cursor = 11;
    let menu = rendered_buffer(&value, 34, 18);
    let (_, backup) = row_containing(&menu, "Create backup");
    assert_eq!(
        backup
            .iter()
            .filter(|cell| cell.bg == Color::Rgb(0x31, 0x32, 0x44))
            .count(),
        30
    );

    value.menu = false;
    update(&mut value, &Key::Char('g'));
    for (width, height) in [(1, 1), (3, 5), (8, 6)] {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &value)).unwrap();
        let position = terminal.get_cursor_position().unwrap();
        assert!(
            position.x < width && position.y < height,
            "{width}x{height}: {position:?}"
        );
    }
}

#[test]
fn welcome_uses_house_gradient_and_accent_selection_surface() {
    let wide = rendered_buffer(&model(), 82, 24);
    let art_cells = &wide.content()[..82 * 12];
    assert!(
        art_cells
            .iter()
            .any(|cell| cell.fg == Color::Rgb(0x17, 0x8f, 0x95))
    );
    assert!(
        art_cells
            .iter()
            .any(|cell| cell.fg == Color::Rgb(0x3c, 0xd1, 0xa5))
    );
    let (_, action) = row_containing(&wide, "Open dashboard");
    assert_eq!(
        cell_with_symbol(action, "▌").fg,
        Color::Rgb(0x3d, 0xd6, 0xa3)
    );
    assert_eq!(
        cell_with_symbol(action, "▌").bg,
        Color::Rgb(0x31, 0x32, 0x44)
    );
}

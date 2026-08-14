use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Padding, Paragraph};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use ptrack_core::{
    Counts, Issue, IssueStatus, MemoryKind, MilestoneStatus, PlanStatus, Severity, TaskStatus,
    Timestamp,
};

use crate::model::{BOARD_STATUSES, BOARD_TITLES, DetailTarget, Model, PaneFocus, TAB_NAMES, Tab};

const ACCENT: Color = Color::Rgb(0x3d, 0xd6, 0xa3);
const ACCENT_DIM: Color = Color::Rgb(0x2a, 0xa7, 0xa1);
const INK: Color = Color::Rgb(0x08, 0x13, 0x16);
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
const NIGHT: Color = Color::Rgb(0x0c, 0x10, 0x16);

const MENU: [(&str, &str, &str, &str); 12] = [
    ("Navigate", "1", "Overview", "Plans and tasks"),
    ("Navigate", "2", "Board", "Kanban workflow"),
    ("Navigate", "3", "Milestones", "Project checkpoints"),
    ("Navigate", "4", "Issues", "Problems and bugs"),
    ("Navigate", "5", "Maintenance", "Storage health and upkeep"),
    ("Project", "g", "Edit goal", "Update the north star"),
    ("Project", "m", "Edit summary", "Refresh handoff context"),
    (
        "Selected",
        "e",
        "Edit selected",
        "Rename the selected entry",
    ),
    (
        "Selected",
        "M",
        "Move task",
        "Move the selected task to another plan",
    ),
    (
        "Selected",
        "P",
        "Convert task to plan",
        "Promote the selected task",
    ),
    ("Maintain", "r", "Reload", "Read the latest project state"),
    (
        "Maintain",
        "B",
        "Create backup",
        "Copy the project database",
    ),
];

pub fn draw(frame: &mut Frame<'_>, model: &Model) {
    let area = frame.area();
    if model.welcome {
        draw_welcome(frame, area);
        return;
    }
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(if model.input.is_some() { 2 } else { 1 }),
        ])
        .split(area);
    draw_header(frame, chunks[0], model);
    draw_tabs(frame, chunks[1], model);
    if model.menu {
        draw_menu(frame, chunks[2], model);
    } else if model.detail.is_some() {
        draw_detail(frame, chunks[2], model);
    } else {
        match model.tab {
            Tab::Overview => draw_overview(frame, chunks[2], model),
            Tab::Board => draw_board(frame, chunks[2], model),
            Tab::Milestones => draw_milestones(frame, chunks[2], model),
            Tab::Issues => draw_issues(frame, chunks[2], model),
            Tab::Maintenance => draw_maintenance(frame, chunks[2], model),
        }
    }
    draw_footer(frame, chunks[3], model);
}

fn draw_welcome(frame: &mut Frame<'_>, area: Rect) {
    let wide = area.width >= 76;
    let art = if wide {
        vec![
            " ███████████             ███████████                              █████     "
                .to_owned(),
            "░░███░░░░░███           ░█░░░███░░░█                             ░░███      "
                .to_owned(),
            " ░███    ░███           ░   ░███  ░  ████████   ██████    ██████  ░███ █████"
                .to_owned(),
            " ░██████████  ██████████    ░███    ░░███░░███ ░░░░░███  ███░░███ ░███░░███ "
                .to_owned(),
            " ░███░░░░░░  ░░░░░░░░░░     ░███     ░███ ░░░   ███████ ░███ ░░░  ░██████░  "
                .to_owned(),
            " ░███                       ░███     ░███      ███░░███ ░███  ███ ░███░░███ "
                .to_owned(),
            " █████                      █████    █████    ░░████████░░██████  ████ █████"
                .to_owned(),
            "░░░░░                      ░░░░░    ░░░░░      ░░░░░░░░  ░░░░░░  ░░░░ ░░░░░ "
                .to_owned(),
        ]
    } else {
        let rule_width = usize::from(area.width.saturating_sub(2)).clamp(1, 58);
        let rule = "━".repeat(rule_width);
        vec![rule.clone(), "P-TRACK".to_owned(), rule]
    };
    let mut lines = art
        .into_iter()
        .enumerate()
        .map(|(index, line)| {
            if !wide && index == 1 {
                Line::styled(
                    line,
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                )
            } else {
                gradient_line(&line, DARK_CYAN, BLUE_GREEN)
            }
        })
        .collect::<Vec<_>>();
    let menu_width = usize::from(area.width.saturating_sub(4)).clamp(4, 58);
    lines.extend([
        Line::raw(""),
        Line::styled(
            "PERSISTENT PROJECT MEMORY  ·  HUMANS + AI AGENTS",
            Style::default().fg(DIM),
        ),
        Line::raw(""),
        selected(" ENTER  Open dashboard", true, true, TEXT, menu_width),
        Line::from(vec![
            key("1–5"),
            Span::styled(" screens    ", Style::default().fg(GRAY)),
            key("?"),
            Span::styled(" menu    ", Style::default().fg(GRAY)),
            key("q"),
            Span::styled(" quit", Style::default().fg(GRAY)),
        ]),
    ]);
    let content = Text::from(lines);
    frame.render_widget(
        Paragraph::new(content)
            .alignment(Alignment::Center)
            .block(Block::default()),
        centered(area, area.width.min(82), if wide { 16 } else { 10 }),
    );
}

#[allow(clippy::too_many_lines)]
fn draw_header(frame: &mut Frame<'_>, area: Rect, model: &Model) {
    let name = model
        .context
        .project_root
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    let counts = model.snapshot.counts();
    let goal_unset = model.snapshot.meta.goal.trim().is_empty();
    let goal = if goal_unset {
        "no goal — press g"
    } else {
        model.snapshot.meta.goal.as_str()
    };
    let width = usize::from(area.width);
    let brand = format!(" p-track · {name} ");
    let right = "? menu";
    let fade_width = 24_usize.min(width.saturating_sub(display_width(&brand) + 2 + 6));
    let mut row1 = vec![Span::styled(
        brand,
        Style::default()
            .fg(INK)
            .bg(ACCENT)
            .add_modifier(Modifier::BOLD),
    )];
    for index in 0..fade_width {
        row1.push(Span::styled(
            " ",
            Style::default().bg(lerp_color(ACCENT, NIGHT, index, fade_width)),
        ));
    }
    let used = row1.iter().map(Span::width).sum::<usize>();
    row1.push(Span::raw(
        " ".repeat(width.saturating_sub(used + display_width(right))),
    ));
    row1.push(key("?"));
    row1.push(Span::styled(" menu", Style::default().fg(GRAY)));

    let (stats1, stats2, stats_width) = header_stats(counts);
    let goal_width = width.saturating_sub(stats_width + 4);
    let goal_text_width = goal_width.saturating_sub(2).max(1);
    let goal_rows = wrap_plain(goal, goal_text_width);
    let goal_second = if goal_rows.len() > 1 {
        truncate_cells(&goal_rows[1..].join(" "), goal_text_width, true)
    } else {
        String::new()
    };
    let goal1 = Line::from(vec![
        Span::styled(
            "✦ ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            goal_rows.first().cloned().unwrap_or_default(),
            Style::default().fg(if goal_unset { DIM } else { TEXT }),
        ),
    ]);
    let goal2 = Line::styled(format!("  {goal_second}"), Style::default().fg(GRAY));
    let (row2, row3) = if goal_width >= 34 {
        (
            join_left_right(goal1, stats1, width),
            join_left_right(goal2, stats2, width),
        )
    } else {
        (goal1, compact_stats(counts))
    };
    let rule = Line::from(
        (0..width)
            .map(|index| {
                Span::styled(
                    "─",
                    Style::default().fg(lerp_color(DARK_CYAN, BLUE_GREEN, index, width)),
                )
            })
            .collect::<Vec<_>>(),
    );
    frame.render_widget(
        Paragraph::new(Text::from(vec![Line::from(row1), row2, row3, rule])),
        area,
    );
}

fn draw_tabs(frame: &mut Frame<'_>, area: Rect, model: &Model) {
    if area.width < 4 {
        frame.render_widget(Paragraph::new(TAB_NAMES[model.tab.index()]), area);
        return;
    }
    let mut spans = Vec::new();
    for (index, name) in TAB_NAMES.iter().enumerate() {
        let text = format!(" {} {name} ", index + 1);
        let style = if index == model.tab.index() {
            Style::default()
                .fg(INK)
                .bg(ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(GRAY)
        };
        spans.push(Span::styled(text, style));
        spans.push(Span::raw(" "));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

#[allow(clippy::too_many_lines)]
fn draw_overview(frame: &mut Frame<'_>, area: Rect, model: &Model) {
    let chunks = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
        .spacing(1)
        .split(area);
    let left_width = panel_content_width(chunks[0]);
    let right_width = panel_content_width(chunks[1]);
    let plan_range = window_range(
        model.snapshot.plans.len(),
        model.plan_cursor,
        usize::from(chunks[0].height.saturating_sub(2)),
    );
    let plans = model
        .snapshot
        .plans
        .iter()
        .enumerate()
        .skip(plan_range.0)
        .take(plan_range.1.saturating_sub(plan_range.0))
        .map(|(index, plan)| {
            let mut spans = Vec::new();
            if plan.id == model.snapshot.meta.active_plan {
                spans.push(Span::styled(
                    "★ ",
                    Style::default().fg(GREEN).add_modifier(Modifier::BOLD),
                ));
            } else if index != model.plan_cursor || model.focus != PaneFocus::Plans {
                // Go reserves the active-star column for every unselected row.
                spans.push(Span::raw("  "));
            }
            spans.push(Span::styled(
                format!("#{} {}", plan.id, plan.title),
                Style::default().fg(plan_color(plan.status)),
            ));
            if plan.status != PlanStatus::Active {
                spans.push(Span::styled(
                    format!("  {}", plan.status),
                    Style::default().fg(DIM),
                ));
            }
            styled_row(
                Line::from(spans),
                index == model.plan_cursor,
                model.focus == PaneFocus::Plans,
                left_width,
            )
        })
        .collect::<Vec<_>>();
    let current_tasks = model.current_tasks().collect::<Vec<_>>();
    let task_range = window_range(
        current_tasks.len(),
        model.task_cursor,
        usize::from(chunks[1].height.saturating_sub(2)),
    );
    let tasks = current_tasks
        .into_iter()
        .enumerate()
        .skip(task_range.0)
        .take(task_range.1.saturating_sub(task_range.0))
        .map(|(index, task)| {
            styled_row(
                Line::from(vec![
                    Span::styled(
                        task_icon(task.status),
                        Style::default().fg(task_color(task.status)),
                    ),
                    Span::styled(
                        format!(" #{} {}", task.id, task.title),
                        Style::default().fg(TEXT),
                    ),
                ]),
                index == model.task_cursor,
                model.focus == PaneFocus::Tasks,
                right_width,
            )
        })
        .collect::<Vec<_>>();
    draw_panel(
        frame,
        chunks[0],
        "Plans",
        Some(model.snapshot.plans.len()),
        model.focus == PaneFocus::Plans,
        "enter view · a add · e rename · u activate · x done",
        if plans.is_empty() {
            vec![Line::styled(
                "press 'a' to add a plan",
                Style::default().fg(DIM),
            )]
        } else {
            plans
        },
    );
    draw_panel(
        frame,
        chunks[1],
        "Tasks",
        Some(model.current_tasks().count()),
        model.focus == PaneFocus::Tasks,
        "a/e add/edit · s/d/b status · n note · M move · P promote",
        if tasks.is_empty() && model.current_plan().is_some() {
            vec![Line::styled(
                "press 'a' to add a task",
                Style::default().fg(DIM),
            )]
        } else {
            tasks
        },
    );
}

fn draw_board(frame: &mut Frame<'_>, area: Rect, model: &Model) {
    let Some(plan) = model.current_plan() else {
        draw_panel(
            frame,
            area,
            "Board",
            Some(0),
            true,
            "",
            vec![Line::styled(
                "No plan selected — add one in Overview",
                Style::default().fg(DIM),
            )],
        );
        return;
    };
    let header = Rect::new(area.x, area.y, area.width, 1);
    frame.render_widget(
        Paragraph::new(join_left_right(
            Line::from(vec![
                Span::styled(
                    "Board",
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("  /  Plan #{}  ", plan.id),
                    Style::default().fg(DIM),
                ),
                Span::styled(plan.title.clone(), Style::default().fg(TEXT)),
            ]),
            styled_hints("H/L status · a/e add/edit · n note · M plan · P promote"),
            usize::from(area.width),
        )),
        header,
    );
    let body = Rect::new(
        area.x,
        area.y.saturating_add(1),
        area.width,
        area.height.saturating_sub(1),
    );
    let chunks = Layout::horizontal([Constraint::Ratio(1, 4); 4])
        .spacing(1)
        .split(body);
    for column in 0..4 {
        let tasks = model.board_tasks(column).collect::<Vec<_>>();
        let range = window_range(
            tasks.len(),
            if column == model.board_col {
                model.board_row
            } else {
                0
            },
            usize::from(chunks[column].height.saturating_sub(2)),
        );
        let content_width = panel_content_width(chunks[column]);
        let rows = tasks
            .into_iter()
            .enumerate()
            .skip(range.0)
            .take(range.1.saturating_sub(range.0))
            .map(|(index, task)| {
                selected(
                    &format!("#{} {}", task.id, task.title),
                    column == model.board_col && index == model.board_row,
                    column == model.board_col,
                    task_color(task.status),
                    content_width,
                )
            })
            .collect::<Vec<_>>();
        draw_panel(
            frame,
            chunks[column],
            &format!(
                "{} {}",
                task_icon(BOARD_STATUSES[column]),
                BOARD_TITLES[column]
            ),
            Some(model.board_tasks(column).count()),
            column == model.board_col,
            "",
            if rows.is_empty() {
                vec![Line::styled("—", Style::default().fg(DIM))]
            } else {
                rows
            },
        );
    }
}

#[allow(clippy::too_many_lines)]
fn draw_milestones(frame: &mut Frame<'_>, area: Rect, model: &Model) {
    let chunks = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
        .spacing(1)
        .split(area);
    let left_width = panel_content_width(chunks[0]);
    let range = window_range(
        model.snapshot.milestones.len(),
        model.milestone_cursor,
        usize::from(chunks[0].height.saturating_sub(2)),
    );
    let milestones = model
        .snapshot
        .milestones
        .iter()
        .enumerate()
        .skip(range.0)
        .take(range.1.saturating_sub(range.0))
        .map(|(index, milestone)| {
            let due = milestone
                .due
                .stored_date()
                .map_or_else(String::new, |date| format!(" ⏰ {date}"));
            if index == model.milestone_cursor {
                return selected(
                    &format!(
                        "#{} {}  {}",
                        milestone.id, milestone.title, milestone.status
                    ),
                    true,
                    true,
                    if milestone.status == MilestoneStatus::Done {
                        GREEN
                    } else {
                        LAVENDER
                    },
                    left_width,
                );
            }
            styled_row(
                Line::from(vec![
                    Span::styled(
                        format!("#{} {}", milestone.id, milestone.title),
                        Style::default().fg(if milestone.status == MilestoneStatus::Done {
                            GREEN
                        } else {
                            LAVENDER
                        }),
                    ),
                    Span::styled(
                        format!(" [{}]{due}", milestone.status),
                        Style::default().fg(DIM),
                    ),
                ]),
                false,
                true,
                left_width,
            )
        })
        .collect::<Vec<_>>();
    draw_panel(
        frame,
        chunks[0],
        "Milestones",
        Some(model.snapshot.milestones.len()),
        true,
        "enter view · a add · e rename · x done · o reopen",
        if milestones.is_empty() {
            vec![Line::styled(
                "press 'a' to add a milestone",
                Style::default().fg(DIM),
            )]
        } else {
            milestones
        },
    );
    let mut plans = Vec::new();
    if let Some(milestone) = model.current_milestone() {
        let mut done = 0;
        let mut open = 0;
        for plan in model.snapshot.plans_for_milestone(milestone.id) {
            plans.push(Line::from(vec![
                Span::styled(
                    format!("#{} {}", plan.id, plan.title),
                    Style::default().fg(TEXT),
                ),
                Span::styled(format!(" [{}]", plan.status), Style::default().fg(DIM)),
            ]));
            for task in model.snapshot.tasks_for_plan(plan.id) {
                if task.status == TaskStatus::Done {
                    done += 1;
                } else {
                    open += 1;
                }
            }
        }
        if plans.is_empty() {
            plans.push(Line::styled(
                format!(
                    "no plans — assign with 'ptrack plan add --milestone {}'",
                    milestone.id
                ),
                Style::default().fg(DIM),
            ));
        }
        plans.push(Line::raw(""));
        plans.push(Line::styled(
            format!("tasks: {done} done · {open} open"),
            Style::default().fg(DIM),
        ));
    }
    draw_panel(
        frame,
        chunks[1],
        "Plans in milestone",
        None,
        false,
        "",
        plans,
    );
}

fn draw_issues(frame: &mut Frame<'_>, area: Rect, model: &Model) {
    let content_width = panel_content_width(area);
    let range = window_range(
        model.snapshot.issues.len(),
        model.issue_cursor,
        usize::from(area.height.saturating_sub(2)),
    );
    let rows = model
        .snapshot
        .issues
        .iter()
        .enumerate()
        .skip(range.0)
        .take(range.1.saturating_sub(range.0))
        .map(|(index, issue)| issue_row(issue, index == model.issue_cursor, content_width))
        .collect::<Vec<_>>();
    draw_panel(
        frame,
        area,
        "Issues",
        Some(model.snapshot.issues.len()),
        true,
        "enter view · a add · e rename · c close · o reopen",
        if rows.is_empty() {
            vec![Line::styled(
                "press 'a' to add an issue",
                Style::default().fg(DIM),
            )]
        } else {
            rows
        },
    );
}

fn draw_maintenance(frame: &mut Frame<'_>, area: Rect, model: &Model) {
    let chunks = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
        .spacing(1)
        .split(area);
    let meta = &model.snapshot.meta;
    let project = model
        .context
        .project_root
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    let health = vec![
        line_kv("Project", project),
        line_kv("Goal", or_unset(&meta.goal)),
        line_kv("Summary", or_unset(&meta.summary)),
        line_kv("Root", &model.context.project_root.display().to_string()),
        line_kv("Database", &model.context.database.display().to_string()),
        line_kv("Schema", &format!("v{}", meta.format_version)),
        line_kv("Writer", or_unset(&meta.last_write_version)),
        line_kv("Updated", &format_timestamp(meta.updated_at)),
        Line::raw(""),
        Line::styled(
            "p-track opens the database only for each action,",
            Style::default().fg(DIM),
        ),
        Line::styled(
            "so agents and this dashboard can work side by side.",
            Style::default().fg(DIM),
        ),
    ];
    draw_panel(
        frame,
        chunks[0],
        "Project health",
        None,
        true,
        "r reload · B backup · g goal · m summary",
        health,
    );
    draw_panel(
        frame,
        chunks[1],
        "Maintenance actions",
        None,
        false,
        "",
        vec![
            Line::from(vec![
                key("r"),
                Span::styled("  Reload project state", Style::default().fg(TEXT)),
            ]),
            Line::styled(
                "   Pull in changes written by an agent or CLI.",
                Style::default().fg(DIM),
            ),
            Line::raw(""),
            Line::from(vec![
                key("B"),
                Span::styled("  Create database backup", Style::default().fg(TEXT)),
            ]),
            Line::from(vec![
                Span::styled("   Destination: ", Style::default().fg(DIM)),
                Span::styled(
                    model
                        .context
                        .global_home
                        .join("backups")
                        .display()
                        .to_string(),
                    Style::default().fg(TEXT),
                ),
            ]),
            Line::raw(""),
            Line::styled(
                "Agent upkeep",
                Style::default().fg(ACCENT_DIM).add_modifier(Modifier::BOLD),
            ),
            Line::from(vec![
                Span::styled("ptrack guide", Style::default().fg(DIM)),
                Span::styled(
                    "         refresh agent instructions",
                    Style::default().fg(TEXT),
                ),
            ]),
            Line::from(vec![
                Span::styled("ptrack hook install", Style::default().fg(DIM)),
                Span::styled("  record git commits", Style::default().fg(TEXT)),
            ]),
            Line::raw(""),
            Line::from(vec![
                key("?"),
                Span::styled(
                    "  Open the command menu from any screen",
                    Style::default().fg(TEXT),
                ),
            ]),
        ],
    );
}

fn draw_menu(frame: &mut Frame<'_>, area: Rect, model: &Model) {
    let mut rows = Vec::new();
    let mut group = "";
    let mut cursor_row = 0;
    let content_width = panel_content_width(area);
    for (index, (next_group, key_name, title, description)) in MENU.iter().enumerate() {
        if *next_group != group {
            if !group.is_empty() {
                rows.push(Line::raw(""));
            }
            group = next_group;
            rows.push(Line::styled(
                group.to_uppercase(),
                Style::default().fg(ACCENT_DIM).add_modifier(Modifier::BOLD),
            ));
        }
        if index == model.menu_cursor {
            cursor_row = rows.len();
        }
        rows.push(if index == model.menu_cursor {
            selected(
                &format!("{key_name:<3} {title:<16}{description}"),
                true,
                true,
                TEXT,
                content_width,
            )
        } else {
            Line::from(vec![
                Span::styled(
                    format!(" {key_name:<3}"),
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("{title:<16}"), Style::default().fg(TEXT)),
                Span::styled(*description, Style::default().fg(DIM)),
            ])
        });
    }
    let range = window_range(
        rows.len(),
        cursor_row,
        usize::from(area.height.saturating_sub(2)),
    );
    let rows = rows
        .into_iter()
        .skip(range.0)
        .take(range.1.saturating_sub(range.0))
        .collect();
    draw_panel(
        frame,
        area,
        "Command menu",
        None,
        true,
        "↑/↓ select · enter open · esc close",
        rows,
    );
}

fn draw_detail(frame: &mut Frame<'_>, area: Rect, model: &Model) {
    let (title, _) = detail_content(model);
    let rows = detail_display_rows(model, panel_content_width(area));
    let visible = usize::from(area.height.saturating_sub(2)).max(1);
    let maximum = rows.len().saturating_sub(visible);
    let offset = model.detail_offset.min(maximum);
    let title = if rows.len() > visible {
        format!(
            "{title}  {}–{}/{}",
            offset + 1,
            (offset + visible).min(rows.len()),
            rows.len()
        )
    } else {
        title
    };
    let hints = if matches!(model.detail, Some(DetailTarget::Task(_))) {
        "↑/↓ scroll · e edit · M move · P to plan · esc back"
    } else {
        "↑/↓ scroll · e edit · esc back"
    };
    draw_panel(
        frame,
        area,
        &title,
        None,
        true,
        hints,
        rows.into_iter().skip(offset).take(visible).collect(),
    );
}

pub(crate) fn detail_scroll_max(model: &Model) -> usize {
    if model.detail.is_none() {
        return 0;
    }
    let width = usize::from(model.width.saturating_sub(4)).max(1);
    let viewport = usize::from(model.height.saturating_sub(8)).max(1);
    detail_display_rows(model, width)
        .len()
        .saturating_sub(viewport)
}

fn detail_display_rows(model: &Model, width: usize) -> Vec<Line<'static>> {
    let (_, logical) = detail_content(model);
    let mut rows = Vec::new();
    let mut section_open = false;
    for (index, line) in logical.iter().enumerate() {
        let plain = line.to_string();
        if let Some(name) = plain.strip_prefix('\u{1e}') {
            if section_open {
                rows.push(section_bottom(width));
            }
            rows.push(section_top(name, width));
            section_open = true;
            continue;
        }
        if section_open
            && plain.is_empty()
            && logical
                .get(index + 1)
                .is_some_and(|next| next.to_string().starts_with('\u{1e}'))
        {
            continue;
        }
        let line_width = if section_open {
            width.saturating_sub(4).max(1)
        } else {
            width.max(1)
        };
        for wrapped in wrap_line(line.clone(), line_width) {
            rows.push(if section_open {
                section_body(wrapped, width)
            } else {
                wrapped
            });
        }
    }
    if section_open {
        rows.push(section_bottom(width));
    }
    rows
}

#[allow(clippy::too_many_lines)]
fn detail_content(model: &Model) -> (String, Vec<Line<'static>>) {
    match model.detail.expect("detail exists") {
        DetailTarget::Task(id) => {
            let Some(task) = model.snapshot.task(id) else {
                return (format!("Task #{id}"), vec![missing("task")]);
            };
            let mut rows = vec![
                Line::styled(
                    task.title.clone(),
                    Style::default()
                        .fg(task_color(task.status))
                        .add_modifier(Modifier::BOLD),
                ),
                Line::raw(""),
                line_kv(
                    "Status",
                    &format!("{} {}", task_icon(task.status), task.status),
                ),
            ];
            if let Some(plan) = model.snapshot.plan(task.plan_id) {
                rows.push(line_kv("Plan", &format!("#{} {}", plan.id, plan.title)));
            }
            rows.extend([
                line_kv("Created", &format_timestamp(task.created_at)),
                line_kv("Updated", &format_timestamp(task.updated_at)),
                Line::raw(""),
                section("Notes"),
            ]);
            let notes: Vec<_> = model.snapshot.notes_for_task(id).collect();
            if notes.is_empty() {
                rows.push(missing("none"));
            } else {
                rows.extend(notes.into_iter().map(|note| {
                    let kind = if note.kind == MemoryKind::Legacy {
                        String::new()
                    } else {
                        format!("[{}] ", note.kind)
                    };
                    Line::from(vec![
                        Span::raw("• "),
                        Span::styled(
                            format!("{}  ", format_timestamp(note.created_at)),
                            Style::default().fg(DIM),
                        ),
                        Span::styled(format!("{kind}{}", note.body), Style::default().fg(TEXT)),
                    ])
                }));
            }
            rows.extend([Line::raw(""), section("Commits")]);
            let commits: Vec<_> = model
                .snapshot
                .commits
                .iter()
                .rev()
                .filter(|commit| commit.task_id == id)
                .collect();
            if commits.is_empty() {
                rows.push(missing("none"));
            } else {
                rows.extend(commits.into_iter().map(|commit| {
                    let sha: String = commit.sha.chars().take(8).collect();
                    Line::from(vec![
                        Span::raw("• "),
                        Span::styled(sha, Style::default().fg(AMBER)),
                        Span::styled(format!("  {}", commit.subject), Style::default().fg(TEXT)),
                    ])
                }));
            }
            (format!("Task #{id}"), rows)
        }
        DetailTarget::Plan(id) => {
            let Some(plan) = model.snapshot.plan(id) else {
                return (format!("Plan #{id}"), vec![missing("plan")]);
            };
            let mut rows = vec![
                Line::styled(
                    plan.title.clone(),
                    Style::default()
                        .fg(plan_color(plan.status))
                        .add_modifier(Modifier::BOLD),
                ),
                Line::raw(""),
                line_kv("Status", plan.status.as_str()),
            ];
            if let Some(milestone) = model.snapshot.milestone(plan.milestone_id) {
                rows.push(line_kv(
                    "Milestone",
                    &format!("#{} {}", milestone.id, milestone.title),
                ));
            }
            rows.extend([
                line_kv("Created", &format_timestamp(plan.created_at)),
                Line::raw(""),
                section("Tasks"),
            ]);
            let tasks: Vec<_> = model.snapshot.tasks_for_plan(id).collect();
            if tasks.is_empty() {
                rows.push(missing("none"));
            } else {
                rows.extend(tasks.into_iter().map(|task| {
                    Line::from(vec![
                        Span::raw("  "),
                        Span::styled(
                            task_icon(task.status),
                            Style::default().fg(task_color(task.status)),
                        ),
                        Span::styled(
                            format!(" #{} {}", task.id, task.title),
                            Style::default().fg(TEXT),
                        ),
                    ])
                }));
            }
            rows.extend([Line::raw(""), section("Notes")]);
            let notes: Vec<_> = model.snapshot.notes_for_plan(id).collect();
            if notes.is_empty() {
                rows.push(missing("none"));
            } else {
                rows.extend(notes.into_iter().map(|note| {
                    let kind = if note.kind == MemoryKind::Legacy {
                        String::new()
                    } else {
                        format!("[{}] ", note.kind)
                    };
                    Line::from(vec![
                        Span::raw("• "),
                        Span::styled(
                            format!("{}  ", format_timestamp(note.created_at)),
                            Style::default().fg(DIM),
                        ),
                        Span::styled(format!("{kind}{}", note.body), Style::default().fg(TEXT)),
                    ])
                }));
            }
            rows.extend([Line::raw(""), section("Commits")]);
            let commits: Vec<_> = model
                .snapshot
                .commits
                .iter()
                .rev()
                .filter(|commit| commit.plan_id == id)
                .collect();
            if commits.is_empty() {
                rows.push(missing("none"));
            } else {
                rows.extend(commits.into_iter().map(|commit| {
                    Line::from(vec![
                        Span::raw("• "),
                        Span::styled(
                            commit.sha.chars().take(8).collect::<String>(),
                            Style::default().fg(AMBER),
                        ),
                        Span::styled(format!("  {}", commit.subject), Style::default().fg(TEXT)),
                    ])
                }));
            }
            (format!("Plan #{id}"), rows)
        }
        DetailTarget::Milestone(id) => {
            let Some(milestone) = model.snapshot.milestone(id) else {
                return (format!("Milestone #{id}"), vec![missing("milestone")]);
            };
            let mut rows = vec![
                Line::styled(
                    milestone.title.clone(),
                    Style::default()
                        .fg(if milestone.status == MilestoneStatus::Done {
                            GREEN
                        } else {
                            TEXT
                        })
                        .add_modifier(Modifier::BOLD),
                ),
                Line::raw(""),
                line_kv("Status", milestone.status.as_str()),
            ];
            if let Some(date) = milestone.due.stored_date() {
                rows.push(line_kv("Due", &date.to_string()));
            }
            rows.extend([Line::raw(""), section("Plans")]);
            let mut found = false;
            let mut done = 0;
            let mut open = 0;
            for plan in model.snapshot.plans_for_milestone(id) {
                found = true;
                rows.push(Line::from(vec![
                    Span::styled(
                        format!("  #{} {} ", plan.id, plan.title),
                        Style::default().fg(TEXT),
                    ),
                    Span::styled(format!("[{}]", plan.status), Style::default().fg(DIM)),
                ]));
                for task in model.snapshot.tasks_for_plan(plan.id) {
                    if task.status == TaskStatus::Done {
                        done += 1;
                    } else {
                        open += 1;
                    }
                }
            }
            if !found {
                rows.push(missing("none"));
            }
            rows.push(Line::raw(""));
            rows.push(Line::styled(
                format!("tasks: {done} done · {open} open"),
                Style::default().fg(DIM),
            ));
            (format!("Milestone #{id}"), rows)
        }
        DetailTarget::Issue(id) => {
            let Some(issue) = model.snapshot.issue(id) else {
                return (format!("Issue #{id}"), vec![missing("issue")]);
            };
            let mut rows = vec![
                Line::styled(
                    issue.title.clone(),
                    Style::default()
                        .fg(severity_color(issue.severity))
                        .add_modifier(Modifier::BOLD),
                ),
                Line::raw(""),
                line_kv("Status", issue.status.as_str()),
                line_kv("Severity", issue.severity.as_str()),
            ];
            if let Some(task) = model.snapshot.task(issue.task_id) {
                rows.push(line_kv("Task", &format!("#{} {}", task.id, task.title)));
            }
            rows.extend([
                line_kv("Created", &format_timestamp(issue.created_at)),
                Line::raw(""),
                section("Explanation"),
            ]);
            rows.push(if issue.body.is_empty() {
                Line::styled(
                    "  (none — add with 'ptrack issue add ... --body \"...\"')",
                    Style::default().fg(DIM),
                )
            } else {
                Line::styled(issue.body.clone(), Style::default().fg(TEXT))
            });
            (format!("Issue #{id}"), rows)
        }
    }
}

fn draw_footer(frame: &mut Frame<'_>, area: Rect, model: &Model) {
    if let Some(input) = &model.input {
        let prompt_width = display_width(&input.prompt).saturating_add(1);
        let available = usize::from(area.width).saturating_sub(prompt_width);
        let (value, cursor) = input.editor.visible(available);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    format!("{} ", input.prompt),
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                ),
                Span::styled(value, Style::default().fg(TEXT)),
            ])),
            Rect::new(area.x, area.y, area.width, 1),
        );
        if area.height > 1 {
            frame.render_widget(
                Paragraph::new(styled_hints("enter confirm · esc cancel")),
                Rect::new(area.x, area.y + 1, area.width, 1),
            );
        }
        if area.width > 0 && area.height > 0 {
            let cursor_x = area
                .x
                .saturating_add(u16::try_from(prompt_width).unwrap_or(u16::MAX))
                .saturating_add(cursor)
                .min(area.right().saturating_sub(1));
            frame.set_cursor_position((cursor_x, area.y));
        }
        return;
    }
    let navigation = if model.menu || model.detail.is_some() {
        ""
    } else {
        " · ←/→ ↑/↓ navigate"
    };
    let global_text = format!(
        "? menu · tab switch · 1–5 jump{navigation} · g goal · m summary · r reload · B backup · q quit"
    );
    let global = styled_hints(&global_text);
    let line = if model.status.is_empty() {
        global
    } else {
        let toast = format!("● {}", model.status);
        let global_width = display_width(&global_text);
        if global_width + display_width(&toast) + 2 <= usize::from(area.width) {
            let mut spans = global.spans;
            spans.push(Span::raw(" ".repeat(
                usize::from(area.width).saturating_sub(global_width + display_width(&toast)),
            )));
            spans.push(Span::styled(toast, Style::default().fg(AMBER)));
            Line::from(spans)
        } else {
            let mut spans = vec![
                Span::styled(toast, Style::default().fg(AMBER)),
                Span::styled("  ·  ", Style::default().fg(DIM)),
            ];
            spans.extend(global.spans);
            Line::from(spans)
        }
    };
    frame.render_widget(Paragraph::new(line), area);
}

fn draw_panel(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    count: Option<usize>,
    focused: bool,
    hints: &str,
    rows: Vec<Line<'static>>,
) {
    if area.width < 4 || area.height < 2 {
        frame.render_widget(Paragraph::new(rows), area);
        return;
    }
    let mut count = count.map_or_else(String::new, |value| format!(" · {value}"));
    let use_caps = area.width >= 12;
    let overhead = if use_caps { 8 } else { 6 };
    let available = usize::from(area.width).saturating_sub(overhead).max(1);
    let title = if display_width(&count) < available {
        truncate_cells(title, available - display_width(&count), true)
    } else {
        count.clear();
        truncate_cells(title, available, true)
    };
    let (title_lead, title_tail) = if use_caps {
        ("─┤ ", " ├")
    } else {
        ("─ ", " ")
    };
    let title_color = if focused { ACCENT } else { GRAY };
    let title = Line::from(vec![
        Span::styled(
            title_lead,
            Style::default()
                .fg(title_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            title,
            Style::default()
                .fg(title_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(count, Style::default().fg(DIM)),
        Span::styled(
            title_tail,
            Style::default()
                .fg(title_color)
                .add_modifier(Modifier::BOLD),
        ),
    ]);
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .padding(Padding::horizontal(1))
        .title(title)
        .border_style(Style::default().fg(if focused { ACCENT } else { BORDER }));
    if focused && !hints.is_empty() && display_width(hints) + 7 <= usize::from(area.width) {
        let mut spans = vec![Span::styled("┤ ", Style::default().fg(ACCENT))];
        spans.extend(styled_hints(hints).spans);
        spans.push(Span::styled(" ├", Style::default().fg(ACCENT)));
        block = block.title_bottom(Line::from(spans).right_aligned());
    }
    frame.render_widget(Paragraph::new(rows).block(block), area);
    if focused {
        color_focused_border(frame, area);
    }
}

fn selected(text: &str, cursor: bool, focused: bool, color: Color, width: usize) -> Line<'static> {
    let content = truncate_cells(text, width.saturating_sub(2), true);
    if cursor && focused {
        let body = pad_cells(&format!(" {content}"), width.saturating_sub(1));
        Line::from(vec![
            Span::styled("▌", Style::default().fg(ACCENT).bg(FAINT)),
            Span::styled(
                body,
                Style::default()
                    .fg(Color::Indexed(231))
                    .bg(FAINT)
                    .add_modifier(Modifier::BOLD),
            ),
        ])
    } else {
        Line::from(vec![
            Span::styled(
                if cursor { "▏ " } else { "  " },
                Style::default().fg(if cursor { DIM } else { color }),
            ),
            Span::styled(
                pad_cells(&content, width.saturating_sub(2)),
                Style::default().fg(color),
            ),
        ])
    }
}

fn styled_row(content: Line<'static>, cursor: bool, focused: bool, width: usize) -> Line<'static> {
    if cursor && focused {
        return selected(&content.to_string(), true, true, TEXT, width);
    }
    let mut spans = vec![Span::styled(
        if cursor { "▏ " } else { "  " },
        Style::default().fg(if cursor { DIM } else { TEXT }),
    )];
    let content = truncate_styled_line(content, width.saturating_sub(2));
    let used = content.width();
    spans.extend(content.spans);
    spans.push(Span::raw(
        " ".repeat(width.saturating_sub(2).saturating_sub(used)),
    ));
    Line::from(spans)
}

fn truncate_styled_line(line: Line<'static>, width: usize) -> Line<'static> {
    if line.width() <= width {
        return line;
    }
    let budget = width.saturating_sub(1);
    let mut remaining = budget;
    let mut output = Vec::new();
    let mut last_style = Style::default();
    for span in line.spans {
        if remaining == 0 {
            break;
        }
        let value = truncate_cells(&span.content, remaining, false);
        let used = display_width(&value);
        if !value.is_empty() {
            last_style = span.style;
            output.push(Span::styled(value, span.style));
        }
        remaining = remaining.saturating_sub(used);
        if used < span.width() {
            break;
        }
    }
    if width > 0 {
        output.push(Span::styled("…", last_style));
    }
    Line::from(output)
}

fn issue_row(issue: &Issue, cursor: bool, width: usize) -> Line<'static> {
    let link = if issue.task_id == 0 {
        String::new()
    } else {
        format!(" (task {})", issue.task_id)
    };
    let plain = format!(
        "{:<8} {:<6} #{} {}{link}",
        issue.severity, issue.status, issue.id, issue.title
    );
    if cursor {
        return selected(&plain, true, true, TEXT, width);
    }
    let fixed = 2 + 8 + 1 + 6 + 1 + display_width(&link);
    let title = truncate_cells(
        &format!("#{} {}", issue.id, issue.title),
        width.saturating_sub(fixed),
        true,
    );
    Line::from(vec![
        Span::raw("  "),
        Span::styled(
            format!("{:<8}", issue.severity),
            Style::default()
                .fg(severity_color(issue.severity))
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            format!("{:<6}", issue.status),
            Style::default().fg(if issue.status == IssueStatus::Open {
                AMBER
            } else {
                DIM
            }),
        ),
        Span::raw(" "),
        Span::styled(title, Style::default().fg(TEXT)),
        Span::styled(link, Style::default().fg(DIM)),
    ])
}

fn line_kv(key_name: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{key_name:<10}"), Style::default().fg(DIM)),
        Span::styled(value.to_owned(), Style::default().fg(TEXT)),
    ])
}

fn section(name: &str) -> Line<'static> {
    Line::raw(format!("\u{1e}{name}"))
}

fn section_top(name: &str, width: usize) -> Line<'static> {
    if width < 6 {
        return Line::styled(
            truncate_cells(name, width, false),
            Style::default().fg(ACCENT_DIM).add_modifier(Modifier::BOLD),
        );
    }
    let title = truncate_cells(name, width.saturating_sub(5), true);
    let tail = "─".repeat(width.saturating_sub(display_width(&title) + 5));
    Line::from(vec![
        Span::styled("╭─ ", Style::default().fg(BORDER)),
        Span::styled(
            title,
            Style::default().fg(ACCENT_DIM).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" {tail}╮"), Style::default().fg(BORDER)),
    ])
}

fn section_body(line: Line<'static>, width: usize) -> Line<'static> {
    if width < 4 {
        return line;
    }
    let body_width = width - 4;
    let mut spans = vec![Span::styled("│ ", Style::default().fg(BORDER))];
    let used = line.width();
    spans.extend(line.spans);
    spans.push(Span::raw(" ".repeat(body_width.saturating_sub(used))));
    spans.push(Span::styled(" │", Style::default().fg(BORDER)));
    Line::from(spans)
}

fn section_bottom(width: usize) -> Line<'static> {
    if width < 2 {
        return Line::raw(" ".repeat(width));
    }
    Line::styled(
        format!("╰{}╯", "─".repeat(width - 2)),
        Style::default().fg(BORDER),
    )
}

fn missing(name: &str) -> Line<'static> {
    Line::styled(format!("  ({name})"), Style::default().fg(DIM))
}

fn key(value: &str) -> Span<'static> {
    Span::styled(
        value.to_owned(),
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
    )
}

fn styled_hints(value: &str) -> Line<'static> {
    let mut spans = Vec::new();
    for (index, hint) in value.split(" · ").enumerate() {
        if index > 0 {
            spans.push(Span::styled(" · ", Style::default().fg(DIM)));
        }
        let (key_name, action) = hint.strip_prefix("←/→ ↑/↓ ").map_or_else(
            || hint.split_once(' ').unwrap_or((hint, "")),
            |action| ("←/→ ↑/↓", action),
        );
        spans.push(key(key_name));
        if !action.is_empty() {
            spans.push(Span::styled(
                format!(" {action}"),
                Style::default().fg(GRAY),
            ));
        }
    }
    Line::from(spans)
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let vertical = Layout::vertical([
        Constraint::Length(area.height.saturating_sub(height) / 2),
        Constraint::Length(height.min(area.height)),
        Constraint::Min(0),
    ])
    .split(area);
    Layout::horizontal([
        Constraint::Length(area.width.saturating_sub(width) / 2),
        Constraint::Length(width.min(area.width)),
        Constraint::Min(0),
    ])
    .split(vertical[1])[1]
}

fn display_width(value: &str) -> usize {
    UnicodeWidthStr::width(value)
}

fn panel_content_width(area: Rect) -> usize {
    usize::from(area.width.saturating_sub(4)).max(1)
}

fn window_range(length: usize, cursor: usize, height: usize) -> (usize, usize) {
    if height == 0 || length <= height {
        return (0, length);
    }
    let cursor = cursor.min(length.saturating_sub(1));
    let mut start = cursor.saturating_sub(height / 2);
    if start + height > length {
        start = length - height;
    }
    (start, start + height)
}

fn truncate_cells(value: &str, width: usize, ellipsis: bool) -> String {
    if width == 0 {
        return String::new();
    }
    if display_width(value) <= width {
        return value.to_owned();
    }
    let suffix = if ellipsis && width > 1 { "…" } else { "" };
    let budget = width.saturating_sub(display_width(suffix));
    let mut output = String::new();
    let mut used = 0;
    for character in value.chars() {
        let character_width = character.width().unwrap_or(0);
        if used + character_width > budget {
            break;
        }
        output.push(character);
        used += character_width;
    }
    output.push_str(suffix);
    output
}

fn pad_cells(value: &str, width: usize) -> String {
    let value = truncate_cells(value, width, false);
    format!(
        "{value}{}",
        " ".repeat(width.saturating_sub(display_width(&value)))
    )
}

fn wrap_plain(value: &str, width: usize) -> Vec<String> {
    if value.is_empty() {
        return vec![String::new()];
    }
    wrap_line(Line::raw(value.to_owned()), width)
        .into_iter()
        .map(|line| line.to_string())
        .collect()
}

fn wrap_line(line: Line<'static>, width: usize) -> Vec<Line<'static>> {
    if width == 0 {
        return vec![Line::raw("")];
    }
    let base_style = line.style;
    let mut rows = Vec::new();
    let mut spans = Vec::new();
    let mut used = 0;
    for span in line.spans {
        let style = base_style.patch(span.style);
        for character in span.content.chars() {
            let character_width = character.width().unwrap_or(0);
            if used > 0 && used + character_width > width {
                rows.push(Line::from(std::mem::take(&mut spans)));
                used = 0;
            }
            if character_width > width {
                continue;
            }
            spans.push(Span::styled(character.to_string(), style));
            used += character_width;
        }
    }
    if !spans.is_empty() || rows.is_empty() {
        rows.push(Line::from(spans));
    }
    rows
}

fn join_left_right(left: Line<'static>, right: Line<'static>, width: usize) -> Line<'static> {
    let left_width = left.width();
    let right_width = right.width();
    if left_width + right_width + 2 > width {
        return wrap_line(left, width)
            .into_iter()
            .next()
            .unwrap_or_default();
    }
    let mut spans = left.spans;
    spans.push(Span::raw(" ".repeat(width - left_width - right_width)));
    spans.extend(right.spans);
    Line::from(spans)
}

fn header_stats(counts: Counts) -> (Line<'static>, Line<'static>, usize) {
    let milestone_count = format!("{}/{}", counts.milestones_done, counts.milestones);
    let task_count = format!("{}/{}", counts.tasks_done, counts.tasks);
    let plan_count = format!("{}/{}", counts.plans_done, counts.plans);
    let issue_count = counts.issues_open.to_string();
    let first_count_width = display_width(&milestone_count).max(display_width(&task_count));
    let second_count_width = display_width(&plan_count).max(display_width(&issue_count));

    let mut first = stat_cell(
        counts.milestones_done,
        counts.milestones,
        "milestones",
        10,
        &milestone_count,
        first_count_width,
        LAVENDER,
        false,
    );
    first.push(Span::raw("   "));
    first.extend(stat_cell(
        counts.plans_done,
        counts.plans,
        "plans",
        6,
        &plan_count,
        second_count_width,
        BLUE,
        false,
    ));
    let first = Line::from(first);

    let mut second = stat_cell(
        counts.tasks_done,
        counts.tasks,
        "tasks",
        10,
        &task_count,
        first_count_width,
        GREEN,
        false,
    );
    second.push(Span::raw("   "));
    second.extend(stat_cell(
        counts.issues_open,
        counts.issues,
        "issues",
        6,
        &issue_count,
        second_count_width,
        RED,
        counts.issues_open == 0,
    ));
    let second = Line::from(second);
    let width = first.width().max(second.width());
    (first, second, width)
}

#[allow(clippy::too_many_arguments)]
fn stat_cell(
    done: usize,
    total: usize,
    label: &str,
    label_width: usize,
    count: &str,
    count_width: usize,
    color: Color,
    quiet: bool,
) -> Vec<Span<'static>> {
    let mut spans = meter_spans(done, total, 5, color);
    spans.extend([
        Span::raw(" "),
        Span::styled(format!("{label:<label_width$}"), Style::default().fg(DIM)),
        Span::raw(" "),
        Span::styled(
            format!("{count:>count_width$}"),
            if quiet {
                Style::default().fg(DIM)
            } else {
                Style::default().fg(color).add_modifier(Modifier::BOLD)
            },
        ),
    ]);
    spans
}

fn meter_spans(done: usize, total: usize, width: usize, fill: Color) -> Vec<Span<'static>> {
    let mut filled = done.saturating_mul(width).checked_div(total).unwrap_or(0);
    if done > 0 && filled == 0 {
        filled = 1;
    }
    filled = filled.min(width);
    vec![
        Span::styled("▰".repeat(filled), Style::default().fg(fill)),
        Span::styled(
            "▱".repeat(width.saturating_sub(filled)),
            Style::default().fg(BORDER),
        ),
    ]
}

fn compact_stats(counts: Counts) -> Line<'static> {
    let count = |value: String, color: Color, quiet: bool| {
        Span::styled(
            value,
            if quiet {
                Style::default().fg(DIM)
            } else {
                Style::default().fg(color).add_modifier(Modifier::BOLD)
            },
        )
    };
    Line::from(vec![
        Span::styled("milestones ", Style::default().fg(DIM)),
        count(
            format!("{}/{}", counts.milestones_done, counts.milestones),
            LAVENDER,
            false,
        ),
        Span::styled(" · plans ", Style::default().fg(DIM)),
        count(
            format!("{}/{}", counts.plans_done, counts.plans),
            BLUE,
            false,
        ),
        Span::styled(" · tasks ", Style::default().fg(DIM)),
        count(
            format!("{}/{}", counts.tasks_done, counts.tasks),
            GREEN,
            false,
        ),
        Span::styled(" · issues ", Style::default().fg(DIM)),
        count(
            format!("{} open", counts.issues_open),
            RED,
            counts.issues_open == 0,
        ),
    ])
}

fn gradient_line(value: &str, from: Color, to: Color) -> Line<'static> {
    let length = value.chars().count();
    Line::from(
        value
            .chars()
            .enumerate()
            .map(|(index, character)| {
                Span::styled(
                    character.to_string(),
                    Style::default().fg(lerp_color(from, to, index, length)),
                )
            })
            .collect::<Vec<_>>(),
    )
}

fn lerp_color(from: Color, to: Color, index: usize, length: usize) -> Color {
    let (Color::Rgb(from_r, from_g, from_b), Color::Rgb(to_r, to_g, to_b)) = (from, to) else {
        return from;
    };
    let denominator = length.saturating_sub(1).max(1);
    let interpolate = |start: u8, end: u8| {
        let start_wide = i128::from(start);
        let delta = i128::from(end) - start_wide;
        let index = i128::try_from(index.min(denominator)).unwrap_or(i128::MAX);
        let denominator = i128::try_from(denominator).unwrap_or(i128::MAX);
        u8::try_from(start_wide + delta * index / denominator).unwrap_or(start)
    };
    Color::Rgb(
        interpolate(from_r, to_r),
        interpolate(from_g, to_g),
        interpolate(from_b, to_b),
    )
}

fn or_unset(value: &str) -> &str {
    if value.trim().is_empty() {
        "(unset)"
    } else {
        value
    }
}

fn color_focused_border(frame: &mut Frame<'_>, area: Rect) {
    if area.width < 2 || area.height < 2 {
        return;
    }
    let last_x = area.x + area.width - 1;
    let last_y = area.y + area.height - 1;
    for x in area.x..=last_x {
        let color = lerp_color(
            ACCENT_DIM,
            ACCENT,
            usize::from(x - area.x),
            usize::from(area.width),
        );
        for y in [area.y, last_y] {
            if let Some(cell) = frame.buffer_mut().cell_mut((x, y))
                && matches!(cell.symbol(), "─" | "╭" | "╮" | "╰" | "╯" | "┤" | "├")
            {
                cell.set_fg(color);
            }
        }
    }
    for y in area.y.saturating_add(1)..last_y {
        if let Some(cell) = frame.buffer_mut().cell_mut((area.x, y)) {
            cell.set_fg(ACCENT_DIM);
        }
        if let Some(cell) = frame.buffer_mut().cell_mut((last_x, y)) {
            cell.set_fg(ACCENT);
        }
    }
}

fn task_icon(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Todo => "○",
        TaskStatus::Doing => "◐",
        TaskStatus::Done => "✓",
        TaskStatus::Blocked => "✗",
    }
}

fn task_color(status: TaskStatus) -> Color {
    match status {
        TaskStatus::Todo => LAVENDER,
        TaskStatus::Doing => AMBER,
        TaskStatus::Done => GREEN,
        TaskStatus::Blocked => RED,
    }
}

fn plan_color(status: PlanStatus) -> Color {
    match status {
        PlanStatus::Active => BLUE,
        PlanStatus::Done => GREEN,
        PlanStatus::Archived => DIM,
    }
}

fn severity_color(severity: Severity) -> Color {
    match severity {
        Severity::Low => GRAY,
        Severity::Medium => BLUE,
        Severity::High => AMBER,
        Severity::Critical => RED,
    }
}

fn format_timestamp(timestamp: Timestamp) -> String {
    let Timestamp::Fixed {
        seconds,
        offset_seconds,
        ..
    } = timestamp
    else {
        return "0001-01-01 00:00".to_owned();
    };
    let date = timestamp
        .stored_date()
        .map_or_else(|| "0001-01-01".to_owned(), |value| value.to_string());
    let local = i128::from(seconds) + i128::from(offset_seconds);
    let seconds_in_day = local.rem_euclid(86_400);
    format!(
        "{date} {:02}:{:02}",
        seconds_in_day / 3_600,
        seconds_in_day.rem_euclid(3_600) / 60
    )
}

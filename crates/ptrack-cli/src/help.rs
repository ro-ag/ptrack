use std::io::Write;

use crate::error::CliError;

const ROOT_LONG: &str = concat!(
    "p-track keeps project plans alive across human and AI sessions. It stores\n",
    "goals, plans, tasks, issues, milestones, notes, and commit context in an embedded\n",
    "database so a fresh agent can reload project context. Every subcommand is\n",
    "non-interactive and exits non-zero on error."
);

const COMPLETION_LONG: &str = concat!(
    "Generate the autocompletion script for ptrack for the specified shell.\n",
    "See each sub-command's help for details on how to use the generated script."
);

const HELP_LONG: &str = concat!(
    "Help provides help for any command in the application.\n",
    "Simply type ptrack help [path to command] for full details."
);

const BASH_LONG: &str = concat!(
    "Generate the autocompletion script for the bash shell.\n\n",
    "This script depends on the 'bash-completion' package.\n",
    "If it is not installed already, you can install it via your OS's package manager.\n\n",
    "To load completions in your current shell session:\n\n",
    "\tsource <(ptrack completion bash)\n\n",
    "To load completions for every new session, execute once:\n\n",
    "#### Linux:\n\n",
    "\tptrack completion bash > /etc/bash_completion.d/ptrack\n\n",
    "#### macOS:\n\n",
    "\tptrack completion bash > $(brew --prefix)/etc/bash_completion.d/ptrack\n\n",
    "You will need to start a new shell for this setup to take effect."
);

const ZSH_LONG: &str = concat!(
    "Generate the autocompletion script for the zsh shell.\n\n",
    "If shell completion is not already enabled in your environment you will need\n",
    "to enable it.  You can execute the following once:\n\n",
    "\techo \"autoload -U compinit; compinit\" >> ~/.zshrc\n\n",
    "To load completions in your current shell session:\n\n",
    "\tsource <(ptrack completion zsh)\n\n",
    "To load completions for every new session, execute once:\n\n",
    "#### Linux:\n\n",
    "\tptrack completion zsh > \"${fpath[1]}/_ptrack\"\n\n",
    "#### macOS:\n\n",
    "\tptrack completion zsh > $(brew --prefix)/share/zsh/site-functions/_ptrack\n\n",
    "You will need to start a new shell for this setup to take effect."
);

const FISH_LONG: &str = concat!(
    "Generate the autocompletion script for the fish shell.\n\n",
    "To load completions in your current shell session:\n\n",
    "\tptrack completion fish | source\n\n",
    "To load completions for every new session, execute once:\n\n",
    "\tptrack completion fish > ~/.config/fish/completions/ptrack.fish\n\n",
    "You will need to start a new shell for this setup to take effect."
);

const POWERSHELL_LONG: &str = concat!(
    "Generate the autocompletion script for powershell.\n\n",
    "To load completions in your current shell session:\n\n",
    "\tptrack completion powershell | Out-String | Invoke-Expression\n\n",
    "To load completions for every new session, add the output of the above command\n",
    "to your powershell profile."
);

#[derive(Clone, Copy)]
struct Child {
    name: &'static str,
    short: &'static str,
}

#[derive(Clone, Copy)]
struct Flag {
    label: &'static str,
    help: &'static str,
}

struct Spec {
    use_line: String,
    text: &'static str,
    aliases: Vec<&'static str>,
    children: &'static [Child],
    flags: Vec<Flag>,
    group: bool,
}

const HELP_FLAG: Flag = Flag {
    label: "-h, --help",
    help: "",
};

const JSON_FLAG: Flag = Flag {
    label: "    --json",
    help: "emit JSON instead of Markdown",
};

const ROOT_CHILDREN: &[Child] = &[
    child("backup", "Back up the current project database"),
    child(
        "board",
        "Kanban board of a plan's tasks (todo/doing/blocked/done)",
    ),
    child(
        "capability",
        "Manage and invoke explicit project host capabilities",
    ),
    child("commit", "Track git commits in the project audit trail"),
    child(
        "completion",
        "Generate the autocompletion script for the specified shell",
    ),
    child(
        "config",
        "Show or set the machine-wide ptrack user identity",
    ),
    child(
        "context",
        "Print the bounded restore digest (Markdown by default, --json for JSON)",
    ),
    child("goal", "Show or set the project's north-star goal"),
    child("gui", "Open the p-track desktop project workspace"),
    child(
        "guide",
        "Install or print the ptrack agent guide (how an AI agent uses ptrack)",
    ),
    child("help", "Help about any command"),
    child(
        "hook",
        "Manage the git post-commit hook that auto-records commits",
    ),
    child(
        "init",
        "Create or refresh a ptrack project in the current repository",
    ),
    child("issue", "Manage issues (tracked problems or bugs)"),
    child(
        "milestone",
        "Manage milestones (high-level checkpoints grouping plans)",
    ),
    child(
        "next",
        "Print the single most-actionable task (active plan: doing, else todo)",
    ),
    child("note", "Manage notes"),
    child("plan", "Manage plans"),
    child("projects", "List registered projects"),
    child("search", "Search plan/task titles and note bodies"),
    child("status", "Print a short project overview"),
    child("summary", "Show or set the rolling context summary"),
    child("task", "Manage tasks"),
    child("version", "Print the ptrack version"),
];

const GOAL_CHILDREN: &[Child] = &[
    child("set", "Set the goal text (args joined with spaces)"),
    child("show", "Print the current goal"),
];
const SUMMARY_CHILDREN: &[Child] = &[
    child("set", "Set the summary text (args joined with spaces)"),
    child("show", "Print the current summary"),
];
const MILESTONE_CHILDREN: &[Child] = &[
    child("add", "Create a new milestone"),
    child("done", "Mark a milestone done"),
    child("due", "Set a milestone's due date (use '-' to clear)"),
    child("list", "List milestones"),
    child("open", "Reopen a milestone"),
    child("rename", "Rename a milestone"),
    child("show", "Show a milestone with its plans and task rollup"),
];
const PLAN_CHILDREN: &[Child] = &[
    child(
        "add",
        "Create a new active plan (optionally under a milestone)",
    ),
    child(
        "copy",
        "Copy a plan subtree into another project or duplicate it here",
    ),
    child(
        "delete",
        "Permanently delete a plan and its tasks and notes",
    ),
    child("dep", "Manage plan dependency edges"),
    child("done", "Mark a plan done"),
    child("hold", "Put a plan on hold with a reason"),
    child("list", "List plans"),
    child("move", "Move a plan subtree to another registered project"),
    child("release", "Release your claim on a plan"),
    child("rename", "Rename a plan"),
    child("resume", "Take a plan off hold"),
    child("show", "Show a plan with its tasks and notes"),
    child("use", "Claim a plan and make it your active plan"),
];
const TASK_CHILDREN: &[Child] = &[
    child(
        "add",
        "Create a new todo task (defaults to the active plan)",
    ),
    child("block", "Mark a task blocked"),
    child("convert", "Convert a task into a plan"),
    child("dep", "Manage task dependency edges"),
    child("done", "Mark a task done"),
    child("hold", "Put a task on hold with a reason"),
    child(
        "list",
        "List tasks (all, or filtered by --plan and/or --status)",
    ),
    child("move", "Move a task to another plan"),
    child("rename", "Rename a task"),
    child("resume", "Take a task off hold"),
    child("show", "Show a task with its plan and notes"),
    child("start", "Mark a task in progress (doing)"),
];
const DEP_CHILDREN: &[Child] = &[
    child(
        "add",
        "Add a dependency edge (the first id waits on the second)",
    ),
    child("list", "List what a record depends on"),
    child("remove", "Remove a dependency edge"),
];
const ISSUE_CHILDREN: &[Child] = &[
    child("add", "Create a new open issue"),
    child("close", "Close an issue"),
    child("list", "List issues (optionally --status open|closed)"),
    child("open", "Reopen an issue"),
    child("rename", "Rename an issue"),
    child("severity", "Set an issue's severity"),
    child("show", "Show an issue with its linked task"),
];
const NOTE_CHILDREN: &[Child] = &[
    child("add", "Add a note to the project, a plan, or a task"),
    child(
        "list",
        "List notes, newest first (scope with --plan/--task, bound with --limit)",
    ),
];
const COMMIT_CHILDREN: &[Child] = &[
    child(
        "add",
        "Record a commit (links to --task/--plan, else the active plan)",
    ),
    child("list", "List commits (optionally --task/--plan)"),
    child(
        "record",
        "Record HEAD from a git hook (parses #<id> from the subject)",
    ),
    child("show", "Show a tracked commit's diff (via git show)"),
];
const CONFIG_CHILDREN: &[Child] = &[
    child(
        "set",
        "Set a config value ('user <name>' mints your identity once)",
    ),
    child("show", "Print the configured user identity"),
];
const HOOK_CHILDREN: &[Child] = &[
    child(
        "install",
        "Install the post-commit hook (auto-records each commit into ptrack)",
    ),
    child("status", "Report whether the post-commit hook is installed"),
    child(
        "uninstall",
        "Remove the ptrack block from the post-commit hook",
    ),
];
const CAPABILITY_CHILDREN: &[Child] = &[
    child(
        "call",
        "Call a capability tool through the active host broker",
    ),
    child("mcp", "Serve provider-compatible MCP tools over stdio"),
];
const COMPLETION_CHILDREN: &[Child] = &[
    child("bash", "Generate the autocompletion script for bash"),
    child("fish", "Generate the autocompletion script for fish"),
    child(
        "powershell",
        "Generate the autocompletion script for powershell",
    ),
    child("zsh", "Generate the autocompletion script for zsh"),
];

const INIT_FLAGS: &[Flag] = &[
    flag(
        "    --force",
        "create even if a different project already exists above",
    ),
    flag("    --goal string", "initial north-star goal text"),
    HELP_FLAG,
    flag(
        "    --no-guide",
        "do not write the ptrack agent guide into AGENTS.md/CLAUDE.md",
    ),
    flag(
        "    --root string",
        "explicit project directory (default: git root, else cwd)",
    ),
];
const HELP_ONLY: &[Flag] = &[HELP_FLAG];
const JSON_FLAGS: &[Flag] = &[HELP_FLAG, JSON_FLAG];
const COMPLETION_FLAGS: &[Flag] = &[
    HELP_FLAG,
    flag("    --no-descriptions", "disable completion descriptions"),
];
const ROOT_FLAGS: &[Flag] = &[HELP_FLAG, flag("-v, --version", "version for ptrack")];

pub fn write(path: &[String], output: &mut dyn Write) -> Result<(), CliError> {
    let normalized = normalize(path);
    if normalized.first().is_some_and(|name| !known_root(name)) {
        return write_unknown_topic(&normalized[0], output);
    }
    let resolved = resolve(&normalized);
    let spec = specification(resolved);
    render(resolved, &spec, output)
}

fn render(path: &[String], spec: &Spec, output: &mut dyn Write) -> Result<(), CliError> {
    writeln!(output, "{}", spec.text)?;
    writeln!(output)?;
    writeln!(output, "Usage:")?;
    if path.is_empty() {
        writeln!(output, "  ptrack [flags]")?;
        writeln!(output, "  ptrack [command]")?;
    } else if spec.group {
        if matches!(path.first().map(String::as_str), Some("goal" | "summary")) {
            writeln!(output, "  ptrack {} [flags]", path.join(" "))?;
        }
        writeln!(output, "  ptrack {} [command]", path.join(" "))?;
    } else {
        let use_line = if spec.use_line == "completion bash" {
            "completion bash".to_owned()
        } else {
            format!("{} [flags]", spec.use_line)
        };
        writeln!(output, "  ptrack {use_line}")?;
    }
    if !spec.aliases.is_empty() {
        writeln!(output)?;
        writeln!(output, "Aliases:")?;
        writeln!(output, "  {}", spec.aliases.join(", "))?;
    }
    if !spec.children.is_empty() {
        writeln!(output)?;
        writeln!(output, "Available Commands:")?;
        for child in spec.children {
            writeln!(output, "  {:<12}{}", child.name, child.short)?;
        }
    }
    if !spec.flags.is_empty() {
        writeln!(output)?;
        writeln!(output, "Flags:")?;
        let width = spec
            .flags
            .iter()
            .map(|flag| flag.label.len())
            .max()
            .unwrap_or(0);
        let leaf = path.last().map_or("ptrack", String::as_str);
        for flag in &spec.flags {
            let help = if flag.help.is_empty() {
                format!("help for {leaf}")
            } else {
                flag.help.to_owned()
            };
            writeln!(output, "  {:<width$}   {help}", flag.label)?;
        }
    }
    if spec.group {
        writeln!(output)?;
        if path.is_empty() {
            writeln!(
                output,
                "Use \"ptrack [command] --help\" for more information about a command."
            )?;
        } else {
            writeln!(
                output,
                "Use \"ptrack {} [command] --help\" for more information about a command.",
                path.join(" ")
            )?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn specification(path: &[String]) -> Spec {
    let names = path.iter().map(String::as_str).collect::<Vec<_>>();
    match names.as_slice() {
        [] => Spec {
            flags: ROOT_FLAGS.to_vec(),
            ..group_spec("ptrack", ROOT_LONG, ROOT_CHILDREN)
        },
        ["help"] => leaf_spec("help [command]", HELP_LONG, HELP_ONLY),
        ["completion"] => group_spec("completion", COMPLETION_LONG, COMPLETION_CHILDREN),
        [
            "completion",
            shell @ ("bash" | "zsh" | "fish" | "powershell"),
        ] => leaf_spec(
            match *shell {
                "bash" => "completion bash",
                "zsh" => "completion zsh",
                "fish" => "completion fish",
                _ => "completion powershell",
            },
            match *shell {
                "bash" => BASH_LONG,
                "zsh" => ZSH_LONG,
                "fish" => FISH_LONG,
                _ => POWERSHELL_LONG,
            },
            COMPLETION_FLAGS,
        ),
        [
            group @ ("goal" | "summary" | "milestone" | "plan" | "task" | "issue" | "note"
            | "commit" | "config" | "hook" | "capability"),
        ] => {
            let (text, children) = match *group {
                "goal" => ("Show or set the project's north-star goal", GOAL_CHILDREN),
                "summary" => ("Show or set the rolling context summary", SUMMARY_CHILDREN),
                "milestone" => (
                    "Manage milestones (high-level checkpoints grouping plans)",
                    MILESTONE_CHILDREN,
                ),
                "plan" => ("Manage plans", PLAN_CHILDREN),
                "task" => ("Manage tasks", TASK_CHILDREN),
                "issue" => ("Manage issues (tracked problems or bugs)", ISSUE_CHILDREN),
                "note" => ("Manage notes", NOTE_CHILDREN),
                "commit" => (
                    "Track git commits in the project audit trail",
                    COMMIT_CHILDREN,
                ),
                "config" => (
                    "Show or set the machine-wide ptrack user identity",
                    CONFIG_CHILDREN,
                ),
                "hook" => (
                    "Manage the git post-commit hook that auto-records commits",
                    HOOK_CHILDREN,
                ),
                _ => (
                    "Manage and invoke explicit project host capabilities",
                    CAPABILITY_CHILDREN,
                ),
            };
            let aliases = if *group == "milestone" {
                vec!["milestone", "ms"]
            } else {
                Vec::new()
            };
            Spec {
                use_line: (*group).to_owned(),
                text,
                aliases,
                children,
                flags: HELP_ONLY.to_vec(),
                group: true,
            }
        }
        ["init"] => leaf_spec(
            "init",
            concat!(
                "Create a .ptrack/ptrack.redb database at the git root (or cwd, or an\n",
                "explicit --root) and optionally seed a north-star goal. Run again in the\n",
                "same project to refresh the agent guide (a no-op if unchanged). Refuses\n",
                "only to nest a NEW project inside a different existing one, unless --force."
            ),
            INIT_FLAGS,
        ),
        ["goal", "show"] => leaf_spec("goal show", "Print the current goal", HELP_ONLY),
        ["goal", "set"] => leaf_spec(
            "goal set <text...>",
            "Set the goal text (args joined with spaces)",
            HELP_ONLY,
        ),
        ["summary", "show"] => leaf_spec("summary show", "Print the current summary", HELP_ONLY),
        ["summary", "set"] => leaf_spec(
            "summary set <text...>",
            "Set the summary text (args joined with spaces)",
            HELP_ONLY,
        ),
        [group @ ("plan" | "task"), "dep"] => Spec {
            use_line: format!("{group} dep"),
            text: match *group {
                "plan" => "Manage plan dependency edges",
                _ => "Manage task dependency edges",
            },
            aliases: Vec::new(),
            children: DEP_CHILDREN,
            flags: HELP_ONLY.to_vec(),
            group: true,
        },
        [group @ ("plan" | "task"), "dep", leaf] => dep_leaf(group, leaf),
        ["milestone", leaf] => milestone_leaf(leaf),
        ["plan", leaf] => plan_leaf(leaf),
        ["task", leaf] => task_leaf(leaf),
        ["issue", leaf] => issue_leaf(leaf),
        ["note", leaf] => note_leaf(leaf),
        ["commit", leaf] => commit_leaf(leaf),
        ["config", leaf] => config_leaf(leaf),
        ["hook", leaf] => hook_leaf(leaf),
        ["capability", leaf] => capability_leaf(leaf),
        [leaf] => root_leaf(leaf),
        _ => group_spec("ptrack", ROOT_LONG, ROOT_CHILDREN),
    }
}

fn dep_leaf(group: &str, name: &str) -> Spec {
    match name {
        "add" => leaf_spec(
            &format!("{group} dep add <id> <dep-id>"),
            "Add a dependency edge (the first id waits on the second)",
            HELP_ONLY,
        ),
        "list" => leaf_spec(
            &format!("{group} dep list <id>"),
            "List what a record depends on",
            JSON_FLAGS,
        ),
        _ => leaf_spec(
            &format!("{group} dep remove <id> <dep-id>"),
            "Remove a dependency edge",
            HELP_ONLY,
        ),
    }
}

fn milestone_leaf(name: &str) -> Spec {
    match name {
        "add" => leaf_spec(
            "milestone add <title...>",
            "Create a new milestone",
            &[flag("    --due string", "due date (YYYY-MM-DD)"), HELP_FLAG],
        ),
        "list" => leaf_spec("milestone list", "List milestones", JSON_FLAGS),
        "show" => leaf_spec(
            "milestone show <id>",
            "Show a milestone with its plans and task rollup",
            JSON_FLAGS,
        ),
        "done" => leaf_spec("milestone done <id>", "Mark a milestone done", HELP_ONLY),
        "open" => leaf_spec("milestone open <id>", "Reopen a milestone", HELP_ONLY),
        "due" => leaf_spec(
            "milestone due <id> <YYYY-MM-DD>",
            "Set a milestone's due date (use '-' to clear)",
            HELP_ONLY,
        ),
        _ => leaf_spec(
            "milestone rename <id> <title...>",
            "Rename a milestone",
            HELP_ONLY,
        ),
    }
}

fn plan_leaf(name: &str) -> Spec {
    match name {
        "add" => leaf_spec(
            "plan add <title...>",
            "Create a new active plan (optionally under a milestone)",
            &[
                HELP_FLAG,
                flag("    --milestone uint", "assign the plan to this milestone"),
            ],
        ),
        "list" => leaf_spec("plan list", "List plans", JSON_FLAGS),
        "show" => leaf_spec(
            "plan show <id>",
            "Show a plan with its tasks and notes",
            JSON_FLAGS,
        ),
        "done" => leaf_spec(
            "plan done <id>",
            "Mark a plan done (clears any hold on it)",
            HELP_ONLY,
        ),
        "use" => leaf_spec(
            "plan use <id>",
            "Claim a plan and make it your active plan",
            &[
                HELP_FLAG,
                flag(
                    "    --steal",
                    "take over another developer's claim (bumps the claim epoch)",
                ),
            ],
        ),
        "release" => leaf_spec(
            "plan release <id>",
            "Release your claim on a plan",
            HELP_ONLY,
        ),
        "hold" => leaf_spec(
            "plan hold <id> <reason...>",
            "Put a plan on hold with a reason (keeps its status)",
            HELP_ONLY,
        ),
        "resume" => leaf_spec(
            "plan resume <id>",
            "Take a plan off hold (clears the hold reason)",
            HELP_ONLY,
        ),
        "rename" => leaf_spec("plan rename <id> <title...>", "Rename a plan", HELP_ONLY),
        "delete" => leaf_spec(
            "plan delete <id>",
            "Permanently delete a plan, its tasks, and their notes (issues detach, commits keep an unlinked audit record)",
            &[
                flag(
                    "    --force",
                    "actually delete; without it the command only prints what would be destroyed",
                ),
                HELP_FLAG,
            ],
        ),
        "move" => leaf_spec(
            "plan move <id> --to <project>",
            "Move a plan subtree to another registered project (arrives unclaimed; --as renames on arrival)",
            &[
                flag("    --as string", "rename the plan on arrival"),
                HELP_FLAG,
                flag(
                    "    --to string",
                    "target project name or path as shown by 'ptrack projects' (required)",
                ),
            ],
        ),
        _ => leaf_spec(
            "plan copy <id>",
            "Copy a plan subtree into another project, or duplicate it here (--as required without --to)",
            &[
                flag(
                    "    --as string",
                    "title for the copy (required when copying within this project)",
                ),
                HELP_FLAG,
                flag(
                    "    --to string",
                    "target project name or path (default: this project)",
                ),
            ],
        ),
    }
}

fn task_leaf(name: &str) -> Spec {
    match name {
        "add" => leaf_spec(
            "task add <title...>",
            "Create a new todo task (defaults to the active plan)",
            &[
                HELP_FLAG,
                flag(
                    "    --plan uint",
                    "plan id to add the task to (defaults to the active plan)",
                ),
            ],
        ),
        "list" => leaf_spec(
            "task list",
            "List tasks (all, or filtered by --plan and/or --status)",
            &[
                HELP_FLAG,
                JSON_FLAG,
                flag("    --plan uint", "only list tasks of this plan"),
                flag(
                    "    --status string",
                    "filter by status (comma-separated: todo,doing,done,blocked)",
                ),
            ],
        ),
        "show" => leaf_spec(
            "task show <id>",
            "Show a task with its plan and notes",
            JSON_FLAGS,
        ),
        "start" => leaf_spec(
            "task start <id>",
            "Mark a task in progress (doing)",
            HELP_ONLY,
        ),
        "done" => leaf_spec(
            "task done <id>",
            "Mark a task done (clears any hold on it)",
            HELP_ONLY,
        ),
        "block" => leaf_spec("task block <id>", "Mark a task blocked", HELP_ONLY),
        "hold" => leaf_spec(
            "task hold <id> <reason...>",
            "Put a task on hold with a reason (keeps its status)",
            HELP_ONLY,
        ),
        "resume" => leaf_spec(
            "task resume <id>",
            "Take a task off hold (clears the hold reason)",
            HELP_ONLY,
        ),
        "rename" => leaf_spec("task rename <id> <title...>", "Rename a task", HELP_ONLY),
        "move" => leaf_spec(
            "task move <id> --plan <plan>",
            "Move a task to another plan",
            &[
                HELP_FLAG,
                flag("    --plan uint", "target plan id (required)"),
            ],
        ),
        _ => Spec {
            aliases: vec!["convert", "promote"],
            ..leaf_spec("task convert <id>", "Convert a task into a plan", HELP_ONLY)
        },
    }
}

fn issue_leaf(name: &str) -> Spec {
    match name {
        "add" => leaf_spec(
            "issue add <title...>",
            "Create a new open issue",
            &[
                flag("    --body string", "longer description"),
                HELP_FLAG,
                flag(
                    "    --severity string",
                    "severity: low, medium (default), high, critical",
                ),
                flag("    --task uint", "link the issue to this task"),
            ],
        ),
        "list" => leaf_spec(
            "issue list",
            "List issues (optionally --status open|closed)",
            &[
                HELP_FLAG,
                JSON_FLAG,
                flag("    --status string", "filter by status: open or closed"),
            ],
        ),
        "show" => leaf_spec(
            "issue show <id>",
            "Show an issue with its linked task",
            JSON_FLAGS,
        ),
        "close" => leaf_spec("issue close <id>", "Close an issue", HELP_ONLY),
        "open" => leaf_spec("issue open <id>", "Reopen an issue", HELP_ONLY),
        "severity" => leaf_spec(
            "issue severity <id> <low|medium|high|critical>",
            "Set an issue's severity",
            HELP_ONLY,
        ),
        _ => leaf_spec("issue rename <id> <title...>", "Rename an issue", HELP_ONLY),
    }
}

fn note_leaf(name: &str) -> Spec {
    match name {
        "add" => leaf_spec(
            "note add <text...>",
            "Add a note to the project, a plan, or a task",
            &[
                HELP_FLAG,
                flag("    --plan uint", "attach the note to this plan"),
                flag("    --task uint", "attach the note to this task"),
            ],
        ),
        _ => leaf_spec(
            "note list",
            "List notes, newest first (scope with --plan/--task, bound with --limit)",
            &[
                HELP_FLAG,
                JSON_FLAG,
                flag(
                    "    --limit int",
                    "max notes to show (0 = all) (default 20)",
                ),
                flag("    --plan uint", "only notes attached to this plan"),
                flag("    --task uint", "only notes attached to this task"),
            ],
        ),
    }
}

fn commit_leaf(name: &str) -> Spec {
    match name {
        "add" => leaf_spec(
            "commit add <sha> <subject...>",
            "Record a commit (links to --task/--plan, else the active plan)",
            &[
                HELP_FLAG,
                flag(
                    "    --plan uint",
                    "link to this plan (default: active plan)",
                ),
                flag("    --task uint", "link to this task"),
            ],
        ),
        "record" => leaf_spec(
            "commit record",
            "Record HEAD from a git hook (parses #<id> from the subject)",
            &[
                HELP_FLAG,
                flag("    --sha string", "commit SHA"),
                flag("    --subject string", "commit subject line"),
            ],
        ),
        "list" => leaf_spec(
            "commit list",
            "List commits (optionally --task/--plan)",
            &[
                HELP_FLAG,
                JSON_FLAG,
                flag("    --plan uint", "only commits linked to this plan"),
                flag("    --task uint", "only commits linked to this task"),
            ],
        ),
        _ => leaf_spec(
            "commit show <id|sha>",
            "Show a tracked commit's diff (via git show)",
            &[
                HELP_FLAG,
                flag("    --stat", "show only the diffstat (changed files)"),
            ],
        ),
    }
}

fn config_leaf(name: &str) -> Spec {
    if name == "set" {
        leaf_spec(
            "config set user <name...>",
            "Set a config value ('user <name>' mints your identity once)",
            HELP_ONLY,
        )
    } else {
        leaf_spec(
            "config show",
            "Print the configured user identity",
            JSON_FLAGS,
        )
    }
}

fn hook_leaf(name: &str) -> Spec {
    match name {
        "install" => leaf_spec(
            "hook install",
            "Install the post-commit hook (auto-records each commit into ptrack)",
            HELP_ONLY,
        ),
        "uninstall" => leaf_spec(
            "hook uninstall",
            "Remove the ptrack block from the post-commit hook",
            HELP_ONLY,
        ),
        _ => leaf_spec(
            "hook status",
            "Report whether the post-commit hook is installed",
            HELP_ONLY,
        ),
    }
}

fn capability_leaf(name: &str) -> Spec {
    match name {
        "call" => leaf_spec(
            "capability call <tool>",
            "Call a capability tool through the active host broker",
            &[
                flag(
                    "    --arguments string",
                    "JSON object matching the tool input schema (default \"{}\")",
                ),
                HELP_FLAG,
            ],
        ),
        _ => leaf_spec(
            "capability mcp",
            "Serve provider-compatible MCP tools over stdio",
            HELP_ONLY,
        ),
    }
}

fn root_leaf(name: &str) -> Spec {
    match name {
        "context" => leaf_spec(
            "context",
            "Print the bounded restore digest (Markdown by default, --json for JSON)",
            JSON_FLAGS,
        ),
        "guide" => leaf_spec(
            "guide",
            "Install or print the ptrack agent guide (how an AI agent uses ptrack)",
            &[
                HELP_FLAG,
                flag(
                    "    --print",
                    "print the guide to stdout instead of writing files",
                ),
            ],
        ),
        "next" => leaf_spec(
            "next",
            "Print the single most-actionable task (active plan: doing, else todo)",
            JSON_FLAGS,
        ),
        "search" => leaf_spec(
            "search <term>",
            "Search plan/task titles and note bodies",
            JSON_FLAGS,
        ),
        "board" => leaf_spec(
            "board",
            "Kanban board of a plan's tasks (todo/doing/blocked/done)",
            &[
                flag("    --gui", "open the kanban board in a desktop window"),
                HELP_FLAG,
                JSON_FLAG,
                flag("    --plan uint", "plan id (default: active plan)"),
            ],
        ),
        "gui" => leaf_spec(
            "gui [PATH]",
            "Open the p-track desktop project workspace",
            HELP_ONLY,
        ),
        "status" => leaf_spec("status", "Print a short project overview", JSON_FLAGS),
        "projects" => leaf_spec("projects", "List registered projects", JSON_FLAGS),
        "backup" => leaf_spec("backup", "Back up the current project database", HELP_ONLY),
        _ => leaf_spec("version", "Print the ptrack version", HELP_ONLY),
    }
}

fn resolve(path: &[String]) -> &[String] {
    let Some(first) = path.first() else {
        return path;
    };
    if first == "help" {
        return &path[..1];
    }
    let group = group_children(first);
    if let Some(children) = group
        && path
            .get(1)
            .is_some_and(|leaf| !children.contains(&leaf.as_str()))
    {
        return &path[..1];
    }
    if group.is_none() && path.len() > 1 {
        return &path[..1];
    }
    // `dep` is a nested group: keep a known third component, drop the rest.
    if path.len() > 2 && path[1] == "dep" {
        if matches!(path[2].as_str(), "add" | "remove" | "list") {
            return &path[..3];
        }
        return &path[..2];
    }
    path
}

fn normalize(path: &[String]) -> Vec<String> {
    let mut path = path.to_vec();
    if path.first().is_some_and(|name| name == "ms") {
        "milestone".clone_into(&mut path[0]);
    }
    if path.first().is_some_and(|name| name == "task")
        && path.get(1).is_some_and(|name| name == "promote")
    {
        "convert".clone_into(&mut path[1]);
    }
    path
}

fn known_root(name: &str) -> bool {
    ROOT_CHILDREN.iter().any(|child| child.name == name)
}

fn group_children(name: &str) -> Option<&'static [&'static str]> {
    match name {
        "goal" | "summary" | "config" => Some(&["set", "show"]),
        "milestone" => Some(&["add", "done", "due", "list", "open", "rename", "show"]),
        "plan" => Some(&[
            "add", "copy", "delete", "dep", "done", "hold", "list", "move", "release", "rename",
            "resume", "show", "use",
        ]),
        "task" => Some(&[
            "add", "block", "convert", "dep", "done", "hold", "list", "move", "rename", "resume",
            "show", "start",
        ]),
        "issue" => Some(&["add", "close", "list", "open", "rename", "severity", "show"]),
        "note" => Some(&["add", "list"]),
        "commit" => Some(&["add", "list", "record", "show"]),
        "hook" => Some(&["install", "status", "uninstall"]),
        "capability" => Some(&["call", "mcp"]),
        "completion" => Some(&["bash", "fish", "powershell", "zsh"]),
        _ => None,
    }
}

fn write_unknown_topic(topic: &str, output: &mut dyn Write) -> Result<(), CliError> {
    writeln!(output, "Unknown help topic [`{topic}`]")?;
    writeln!(output, "Usage:")?;
    writeln!(output, "  ptrack")?;
    writeln!(output, "  ptrack [command]")?;
    writeln!(output)?;
    writeln!(output, "Available Commands:")?;
    for child in ROOT_CHILDREN {
        writeln!(output, "  {:<12}{}", child.name, child.short)?;
    }
    writeln!(output)?;
    writeln!(
        output,
        "Use \"ptrack [command] --help\" for more information about a command."
    )?;
    Ok(())
}

fn group_spec(use_line: &str, text: &'static str, children: &'static [Child]) -> Spec {
    Spec {
        use_line: use_line.to_owned(),
        text,
        aliases: Vec::new(),
        children,
        flags: HELP_ONLY.to_vec(),
        group: true,
    }
}

fn leaf_spec(use_line: &str, text: &'static str, flags: &[Flag]) -> Spec {
    Spec {
        use_line: use_line.to_owned(),
        text,
        aliases: Vec::new(),
        children: &[],
        flags: flags.to_vec(),
        group: false,
    }
}

const fn child(name: &'static str, short: &'static str) -> Child {
    Child { name, short }
}
const fn flag(label: &'static str, help: &'static str) -> Flag {
    Flag { label, help }
}

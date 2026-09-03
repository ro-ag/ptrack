use clap::{Arg, ArgAction, Command};

fn flag(name: &'static str) -> Arg {
    Arg::new(name).long(name).action(ArgAction::SetTrue)
}

fn option(name: &'static str) -> Arg {
    Arg::new(name).long(name).num_args(1).action(ArgAction::Set)
}

fn positional(name: &'static str, range: impl Into<clap::builder::ValueRange>) -> Arg {
    Arg::new(name).num_args(range).action(ArgAction::Append)
}

fn leaf(name: &'static str) -> Command {
    Command::new(name).disable_help_flag(true)
}

#[allow(clippy::too_many_lines)]
pub fn root() -> Command {
    Command::new("ptrack")
        .disable_help_flag(true)
        .disable_version_flag(true)
        .disable_help_subcommand(true)
        .subcommand(leaf("init").args([
            option("goal"),
            option("root"),
            flag("force"),
            flag("no-guide"),
        ]))
        .subcommand(leaf("relocate").arg(option("root")))
        .subcommand(
            group("goal", &["show", "set"])
                .mut_subcommand("set", |c| c.arg(positional("text", 1..))),
        )
        .subcommand(
            group("summary", &["show", "set"])
                .mut_subcommand("set", |c| c.arg(positional("text", 1..))),
        )
        .subcommand(
            group(
                "milestone",
                &["add", "list", "show", "done", "open", "due", "rename"],
            )
            .alias("ms")
            .mut_subcommand("add", |c| {
                c.arg(positional("title", 1..)).arg(option("due"))
            })
            .mut_subcommand("list", |c| c.arg(flag("json")))
            .mut_subcommand("show", |c| c.arg(positional("id", 1)).arg(flag("json")))
            .mut_subcommand("done", |c| c.arg(positional("id", 1)))
            .mut_subcommand("open", |c| c.arg(positional("id", 1)))
            .mut_subcommand("due", |c| c.arg(positional("values", 2)))
            .mut_subcommand("rename", |c| c.arg(positional("values", 2..))),
        )
        .subcommand(
            group(
                "plan",
                &[
                    "add", "list", "show", "done", "use", "release", "rename", "hold", "resume",
                    "delete", "move", "copy",
                ],
            )
            .mut_subcommand("add", |c| {
                c.arg(positional("title", 1..)).args([
                    option("milestone"),
                    flag("no-verify-task"),
                    flag("force"),
                ])
            })
            .mut_subcommand("list", |c| c.arg(flag("json")))
            .mut_subcommand("show", |c| c.arg(positional("id", 1)).arg(flag("json")))
            .mut_subcommand("done", |c| c.arg(positional("id", 1)).arg(flag("force")))
            .mut_subcommand("use", |c| c.arg(positional("id", 1)).arg(flag("steal")))
            .mut_subcommand("release", |c| c.arg(positional("id", 1)))
            .mut_subcommand("rename", |c| c.arg(positional("values", 2..)))
            .mut_subcommand("hold", |c| c.arg(positional("values", 2..)))
            .mut_subcommand("resume", |c| c.arg(positional("id", 1)))
            .mut_subcommand("delete", |c| c.arg(positional("id", 1)).arg(flag("force")))
            .mut_subcommand("move", |c| {
                c.arg(positional("id", 1))
                    .args([option("to"), option("as")])
            })
            .mut_subcommand("copy", |c| {
                c.arg(positional("id", 1))
                    .args([option("to"), option("as")])
            })
            .subcommand(dep_group()),
        )
        .subcommand(
            group(
                "task",
                &[
                    "add", "list", "show", "start", "done", "block", "rename", "move", "convert",
                    "hold", "resume",
                ],
            )
            .mut_subcommand("add", |c| {
                c.arg(positional("title", 1..))
                    .args([option("plan"), flag("force")])
            })
            .mut_subcommand("list", |c| {
                c.args([option("plan"), option("status"), flag("json")])
            })
            .mut_subcommand("show", |c| c.arg(positional("id", 1)).arg(flag("json")))
            .mut_subcommand("start", |c| c.arg(positional("id", 1)).arg(flag("force")))
            .mut_subcommand("done", |c| {
                c.arg(positional("id", 1))
                    .args([option("summary"), flag("force")])
            })
            .mut_subcommand("block", |c| c.arg(positional("id", 1)))
            .mut_subcommand("rename", |c| c.arg(positional("values", 2..)))
            .mut_subcommand("move", |c| c.arg(positional("id", 1)).arg(option("plan")))
            .mut_subcommand("convert", |c| c.alias("promote").arg(positional("id", 1)))
            .mut_subcommand("hold", |c| c.arg(positional("values", 2..)))
            .mut_subcommand("resume", |c| c.arg(positional("id", 1)))
            .subcommand(dep_group()),
        )
        .subcommand(
            group(
                "issue",
                &[
                    "add", "list", "show", "close", "open", "severity", "rename", "edit", "link",
                    "unlink", "schedule",
                ],
            )
            .mut_subcommand("add", |c| {
                c.arg(positional("title", 1..)).args([
                    option("severity"),
                    option("task"),
                    option("body"),
                ])
            })
            .mut_subcommand("list", |c| c.args([option("status"), flag("json")]))
            .mut_subcommand("show", |c| c.arg(positional("id", 1)).arg(flag("json")))
            .mut_subcommand("close", |c| c.arg(positional("id", 1)))
            .mut_subcommand("open", |c| c.arg(positional("id", 1)))
            .mut_subcommand("severity", |c| c.arg(positional("values", 2)))
            .mut_subcommand("edit", |c| {
                c.arg(positional("id", 1)).args([
                    option("title"),
                    option("body"),
                    option("severity"),
                    option("status"),
                ])
            })
            .mut_subcommand("link", |c| c.arg(positional("id", 1)).arg(option("task")))
            .mut_subcommand("unlink", |c| c.arg(positional("id", 1)))
            .mut_subcommand("schedule", |c| {
                c.arg(positional("id", 1))
                    .args([option("plan"), option("title")])
            })
            .mut_subcommand("rename", |c| c.arg(positional("values", 2..))),
        )
        .subcommand(
            group("note", &["add", "list"])
                .mut_subcommand("add", |c| {
                    c.arg(positional("text", 1..))
                        .args([option("task"), option("plan")])
                })
                .mut_subcommand("list", |c| {
                    c.args([
                        option("plan"),
                        option("task"),
                        option("limit"),
                        flag("json"),
                    ])
                }),
        )
        .subcommand(
            group("commit", &["add", "record", "list", "show"])
                .mut_subcommand("add", |c| {
                    c.arg(positional("values", 2..))
                        .args([option("task"), option("plan")])
                })
                .mut_subcommand("record", |c| c.args([option("sha"), option("subject")]))
                .mut_subcommand("list", |c| {
                    c.args([option("task"), option("plan"), flag("json")])
                })
                .mut_subcommand("show", |c| {
                    c.arg(positional("reference", 1)).arg(flag("stat"))
                }),
        )
        .subcommand(
            group("config", &["set", "show"])
                .mut_subcommand("set", |c| c.arg(positional("values", 2..)))
                .mut_subcommand("show", |c| c.arg(flag("json"))),
        )
        .subcommand(group("hook", &["install", "uninstall", "status"]))
        .subcommand(
            group("agent", &["list", "show", "inbox"])
                .mut_subcommand("list", |c| c.arg(flag("json")))
                .mut_subcommand("show", |c| c.arg(positional("run-id", 1)).arg(flag("json")))
                .mut_subcommand("inbox", |c| c.arg(flag("json"))),
        )
        .subcommand(leaf("context").arg(flag("json")))
        .subcommand(leaf("guide").arg(flag("print")))
        .subcommand(leaf("next").arg(flag("json")))
        .subcommand(leaf("checkpoint").arg(flag("json")))
        .subcommand(
            leaf("search")
                .arg(positional("term", 1..))
                .arg(flag("json")),
        )
        .subcommand(leaf("board").args([option("plan"), flag("gui"), flag("json")]))
        .subcommand(leaf("gui").arg(positional("path", 0..=1)))
        .subcommand(leaf("status").arg(flag("json")))
        .subcommand(leaf("projects").arg(flag("json")))
        .subcommand(leaf("backup"))
        .subcommand(leaf("mcp"))
        .subcommand(
            group("capability", &["call", "mcp"]).mut_subcommand("call", |c| {
                c.arg(positional("tool", 1)).arg(option("arguments"))
            }),
        )
        .subcommand(leaf("version"))
}

/// The `dep` subtree shared by `plan` and `task`: add/remove take the two
/// edge endpoints, list takes the record whose dependencies to print.
fn dep_group() -> Command {
    group("dep", &["add", "remove", "list"])
        .mut_subcommand("add", |c| c.arg(positional("values", 2)))
        .mut_subcommand("remove", |c| c.arg(positional("values", 2)))
        .mut_subcommand("list", |c| c.arg(positional("id", 1)).arg(flag("json")))
}

fn group(name: &'static str, children: &[&'static str]) -> Command {
    children.iter().fold(
        Command::new(name)
            .disable_help_flag(true)
            .subcommand_required(false),
        |command, child| command.subcommand(leaf(child)),
    )
}

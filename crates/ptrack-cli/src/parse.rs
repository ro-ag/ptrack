#![allow(clippy::assigning_clones, clippy::match_same_arms)]

use std::collections::BTreeSet;

use crate::command::{ArgCount, LEAVES};
use crate::error::CliError;

pub const ROOT_COMMANDS: &[&str] = &[
    "init",
    "relocate",
    "goal",
    "summary",
    "milestone",
    "plan",
    "task",
    "issue",
    "note",
    "commit",
    "config",
    "hook",
    "context",
    "guide",
    "next",
    "checkpoint",
    "search",
    "board",
    "gui",
    "status",
    "projects",
    "backup",
    "capability",
    "version",
];

const GROUPS: &[(&str, &[&str])] = &[
    ("goal", &["show", "set"]),
    ("summary", &["show", "set"]),
    (
        "milestone",
        &["add", "list", "show", "done", "open", "due", "rename"],
    ),
    (
        "plan",
        &[
            "add", "list", "show", "done", "use", "release", "rename", "hold", "resume", "delete",
            "move", "copy", "dep",
        ],
    ),
    (
        "task",
        &[
            "add", "list", "show", "start", "done", "block", "rename", "move", "convert", "hold",
            "resume", "dep",
        ],
    ),
    (
        "issue",
        &["add", "list", "show", "close", "open", "severity", "rename"],
    ),
    ("note", &["add", "list"]),
    ("commit", &["add", "record", "list", "show"]),
    ("config", &["set", "show"]),
    ("hook", &["install", "uninstall", "status"]),
    ("capability", &["call", "mcp"]),
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Preflight {
    Run {
        argv: Vec<String>,
        path: Vec<String>,
    },
    Help(Vec<String>),
    UnknownHelpTopic(String),
    Completion {
        shell: String,
        no_descriptions: bool,
    },
    GroupDefault(Vec<String>),
}

#[allow(clippy::too_many_lines)]
pub fn preflight(mut argv: Vec<String>) -> Result<Preflight, CliError> {
    if !argv.is_empty() {
        argv.remove(0);
    }
    if argv.is_empty() {
        return Ok(Preflight::Run {
            argv: vec!["ptrack".to_owned()],
            path: Vec::new(),
        });
    }
    if matches!(argv[0].as_str(), "-h" | "--help") {
        return Ok(Preflight::Help(Vec::new()));
    }
    if matches!(argv[0].as_str(), "-v" | "--version") {
        return Ok(Preflight::GroupDefault(vec!["--version".to_owned()]));
    }
    if argv[0] == "help" {
        if argv
            .get(1)
            .is_some_and(|value| matches!(value.as_str(), "-h" | "--help"))
        {
            return Ok(Preflight::Help(vec!["help".to_owned()]));
        }
        let topics = argv[1..]
            .iter()
            .take_while(|value| !value.starts_with('-'))
            .cloned()
            .collect::<Vec<_>>();
        if let Some(topic) = topics.first()
            && topic != "help"
            && topic != "completion"
            && topic != "ms"
            && !ROOT_COMMANDS.contains(&topic.as_str())
        {
            return Ok(Preflight::UnknownHelpTopic(topic.clone()));
        }
        return Ok(Preflight::Help(topics));
    }
    if argv[0] == "completion" {
        const SHELLS: &[&str] = &["bash", "fish", "powershell", "zsh"];
        let Some(shell) = argv.get(1) else {
            return Ok(Preflight::Help(vec!["completion".to_owned()]));
        };
        if matches!(shell.as_str(), "-h" | "--help") || !SHELLS.contains(&shell.as_str()) {
            return Ok(Preflight::Help(vec!["completion".to_owned()]));
        }
        if argv[2..]
            .iter()
            .any(|value| matches!(value.as_str(), "-h" | "--help"))
        {
            return Ok(Preflight::Help(vec![
                "completion".to_owned(),
                shell.clone(),
            ]));
        }
        let mut no_descriptions = false;
        for value in &argv[2..] {
            match value.as_str() {
                "--no-descriptions" => no_descriptions = true,
                value if value.starts_with("--") => {
                    return Err(CliError::message(format!("unknown flag: {value}")));
                }
                value if value.starts_with('-') => {
                    let shorthand = value.chars().nth(1).unwrap_or('-');
                    return Err(CliError::message(format!(
                        "unknown shorthand flag: {shorthand:?} in {value}"
                    )));
                }
                value => {
                    return Err(CliError::message(format!(
                        "unknown command {value:?} for {:?}",
                        format!("ptrack completion {shell}")
                    )));
                }
            }
        }
        return Ok(Preflight::Completion {
            shell: shell.clone(),
            no_descriptions,
        });
    }

    let mut root = argv[0].clone();
    if root == "ms" {
        root = "milestone".to_owned();
        argv[0] = root.clone();
    }
    if !ROOT_COMMANDS.contains(&root.as_str()) {
        return Err(unknown(&root, "ptrack", ROOT_COMMANDS));
    }
    let mut path = vec![root.clone()];
    if let Some((_, children)) = GROUPS.iter().find(|(group, _)| *group == root) {
        let Some(candidate) = argv.get(1) else {
            return if matches!(root.as_str(), "goal" | "summary") {
                Ok(Preflight::GroupDefault(path))
            } else {
                Ok(Preflight::Help(path))
            };
        };
        if matches!(candidate.as_str(), "-h" | "--help") {
            return Ok(Preflight::Help(path));
        }
        let mut child = candidate.as_str();
        if root == "task" && child == "promote" {
            child = "convert";
            argv[1] = child.to_owned();
        }
        if !children.contains(&child) {
            // Cobra groups without Run/Args render their help successfully for
            // a stray token. goal/summary instead invoke their default show.
            return if matches!(root.as_str(), "goal" | "summary") {
                Ok(Preflight::GroupDefault(path))
            } else {
                Ok(Preflight::Help(path))
            };
        }
        path.push(child.to_owned());
        // `dep` is itself a group one level down under plan/task.
        if child == "dep" {
            let Some(sub) = argv.get(2) else {
                return Ok(Preflight::Help(path));
            };
            if matches!(sub.as_str(), "-h" | "--help")
                || !matches!(sub.as_str(), "add" | "remove" | "list")
            {
                return Ok(Preflight::Help(path));
            }
            path.push(sub.clone());
        }
    }
    if argv
        .iter()
        .any(|value| matches!(value.as_str(), "-h" | "--help"))
    {
        return Ok(Preflight::Help(path));
    }
    validate_leaf(&path, &argv[path.len()..])?;
    let mut clap_argv = vec!["ptrack".to_owned()];
    clap_argv.extend(argv);
    Ok(Preflight::Run {
        argv: clap_argv,
        path,
    })
}

fn validate_leaf(path: &[String], raw: &[String]) -> Result<(), CliError> {
    let flags = flag_names(path);
    let mut positionals = 0;
    let mut first_positional = None;
    let mut index = 0;
    let mut after_separator = false;
    while index < raw.len() {
        let value = &raw[index];
        if !after_separator && value == "--" {
            after_separator = true;
            index += 1;
            continue;
        }
        if !after_separator && value.starts_with("--") {
            let (name, inline) = value
                .strip_prefix("--")
                .and_then(|rest| rest.split_once('='))
                .map_or((value.trim_start_matches("--"), false), |(name, _)| {
                    (name, true)
                });
            let Some(takes_value) = flags
                .iter()
                .find_map(|(known, takes)| (*known == name).then_some(*takes))
            else {
                return Err(CliError::message(format!("unknown flag: --{name}")));
            };
            if takes_value && !inline {
                index += 1;
                if index >= raw.len() {
                    return Err(CliError::message(format!(
                        "flag needs an argument: --{name}"
                    )));
                }
            }
        } else if !after_separator && value.starts_with('-') {
            let shorthand = value.chars().nth(1).unwrap_or('-');
            return Err(CliError::message(format!(
                "unknown shorthand flag: {shorthand:?} in {value}"
            )));
        } else {
            first_positional.get_or_insert(value.as_str());
            positionals += 1;
        }
        index += 1;
    }
    let spec = LEAVES.iter().find(|spec| {
        spec.path.len() == path.len()
            && spec
                .path
                .iter()
                .zip(path)
                .all(|(left, right)| *left == right)
    });
    if let Some(spec) = spec
        && !count_valid(spec.count, positionals)
    {
        if matches!(spec.count, ArgCount::None) {
            return Err(CliError::message(format!(
                "unknown command {:?} for {:?}",
                first_positional.unwrap_or_default(),
                format!("ptrack {}", path.join(" "))
            )));
        }
        return Err(arg_count(spec.count, positionals));
    }
    Ok(())
}

fn flag_names(path: &[String]) -> BTreeSet<(&'static str, bool)> {
    let names: &[(&str, bool)] = match path
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["init"] => &[
            ("goal", true),
            ("root", true),
            ("force", false),
            ("no-guide", false),
        ],
        ["relocate"] => &[("root", true)],
        ["milestone", "add"] => &[("due", true)],
        ["milestone", "list" | "show"] => &[("json", false)],
        ["plan", "add"] => &[
            ("milestone", true),
            ("no-verify-task", false),
            ("force", false),
        ],
        ["plan", "list" | "show"] => &[("json", false)],
        ["plan", "use"] => &[("steal", false)],
        ["plan", "done" | "delete"] => &[("force", false)],
        ["plan", "move" | "copy"] => &[("to", true), ("as", true)],
        ["task", "add"] => &[("plan", true), ("force", false)],
        ["task", "start"] => &[("force", false)],
        ["task", "done"] => &[("summary", true), ("force", false)],
        ["task", "list"] => &[("plan", true), ("status", true), ("json", false)],
        ["task", "show"] => &[("json", false)],
        ["task", "move"] => &[("plan", true)],
        ["plan" | "task", "dep", "list"] => &[("json", false)],
        ["issue", "add"] => &[("severity", true), ("task", true), ("body", true)],
        ["issue", "list"] => &[("status", true), ("json", false)],
        ["issue", "show"] => &[("json", false)],
        ["note", "add"] => &[("task", true), ("plan", true)],
        ["note", "list"] => &[
            ("task", true),
            ("plan", true),
            ("limit", true),
            ("json", false),
        ],
        ["commit", "add" | "list"] => &[("task", true), ("plan", true), ("json", false)],
        ["commit", "record"] => &[("sha", true), ("subject", true)],
        ["commit", "show"] => &[("stat", false)],
        ["config", "show"] => &[("json", false)],
        ["context" | "next" | "checkpoint" | "search" | "status" | "projects"] => {
            &[("json", false)]
        }
        ["guide"] => &[("print", false)],
        ["board"] => &[("plan", true), ("gui", false), ("json", false)],
        ["capability", "call"] => &[("arguments", true)],
        _ => &[],
    };
    names.iter().copied().collect()
}

const fn count_valid(count: ArgCount, actual: usize) -> bool {
    match count {
        ArgCount::None => actual == 0,
        ArgCount::Exact(want) => actual == want,
        ArgCount::Minimum(want) => actual >= want,
        ArgCount::Maximum(want) => actual <= want,
    }
}

fn arg_count(count: ArgCount, actual: usize) -> CliError {
    let message = match count {
        ArgCount::None => format!("unknown command for leaf: received {actual} arg(s)"),
        ArgCount::Exact(want) => format!("accepts {want} arg(s), received {actual}"),
        ArgCount::Minimum(want) => {
            format!("requires at least {want} arg(s), only received {actual}")
        }
        ArgCount::Maximum(want) => format!("accepts at most {want} arg(s), received {actual}"),
    };
    CliError::message(message)
}

fn unknown(value: &str, parent: &str, choices: &[&str]) -> CliError {
    let mut suggestions: Vec<_> = choices
        .iter()
        .copied()
        .filter(|choice| distance(value, choice) <= 2 || choice.starts_with(value))
        .collect();
    suggestions.sort_unstable();
    let mut message = format!("unknown command {value:?} for {parent:?}");
    if !suggestions.is_empty() {
        message.push_str("\n\nDid you mean this?\n");
        for suggestion in suggestions {
            message.push('\t');
            message.push_str(suggestion);
            message.push('\n');
        }
    }
    CliError::message(message)
}

fn distance(left: &str, right: &str) -> usize {
    let mut previous: Vec<usize> = (0..=right.chars().count()).collect();
    for (row, a) in left.chars().enumerate() {
        let mut current = vec![row + 1];
        for (column, b) in right.chars().enumerate() {
            current.push(
                (current[column] + 1)
                    .min(previous[column + 1] + 1)
                    .min(previous[column] + usize::from(a != b)),
            );
        }
        previous = current;
    }
    previous.last().copied().unwrap_or(0)
}

pub fn parse_u64(value: &str) -> Result<u64, CliError> {
    value.parse::<u64>().map_err(|error| {
        let reason = if error.kind() == &std::num::IntErrorKind::PosOverflow {
            "value out of range"
        } else {
            "invalid syntax"
        };
        CliError::message(format!(
            "invalid id {value:?}: strconv.ParseUint: parsing {value:?}: {reason}"
        ))
    })
}

pub fn parse_flag_u64(name: &str, value: Option<&String>) -> Result<u64, CliError> {
    value.map_or(Ok(0), |value| {
        value.parse::<u64>().map_err(|error| {
            let reason = if error.kind() == &std::num::IntErrorKind::PosOverflow {
                "value out of range"
            } else {
                "invalid syntax"
            };
            CliError::message(format!(
                "invalid argument {value:?} for \"--{name}\" flag: strconv.ParseUint: parsing {value:?}: {reason}"
            ))
        })
    })
}

pub fn parse_flag_i64(name: &str, value: Option<&String>, default: i64) -> Result<i64, CliError> {
    value.map_or(Ok(default), |value| {
        value.parse::<i64>().map_err(|error| {
            let reason = if matches!(
                error.kind(),
                std::num::IntErrorKind::PosOverflow | std::num::IntErrorKind::NegOverflow
            ) {
                "value out of range"
            } else {
                "invalid syntax"
            };
            CliError::message(format!(
                "invalid argument {value:?} for \"--{name}\" flag: strconv.ParseInt: parsing {value:?}: {reason}"
            ))
        })
    })
}

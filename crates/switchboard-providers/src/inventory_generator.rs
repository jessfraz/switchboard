use std::{
    collections::{BTreeSet, HashMap, HashSet},
    path::PathBuf,
    process::Command,
};

use switchboard_core::{Error, ProviderKind, Result};

use crate::inventory::{CliInventory, CliInventoryCommand, CliInventoryNodeKind, CliOperationKind, CliUndoSupport};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CliInventoryTarget {
    pub provider: ProviderKind,
    pub program: String,
    pub output_path: PathBuf,
    pub help_strategy: CliHelpStrategy,
}

impl CliInventoryTarget {
    pub fn default_targets() -> Vec<Self> {
        vec![
            Self {
                provider: ProviderKind::GitHub,
                program: "gh".to_owned(),
                output_path: PathBuf::from("crates/switchboard-providers/inventories/github.json"),
                help_strategy: CliHelpStrategy::AppendFlag {
                    fallback_subcommand: Some("help".to_owned()),
                },
            },
            Self {
                provider: ProviderKind::GoogleWorkspace,
                program: "gws".to_owned(),
                output_path: PathBuf::from("crates/switchboard-providers/inventories/google.json"),
                help_strategy: CliHelpStrategy::AppendFlag {
                    fallback_subcommand: None,
                },
            },
        ]
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CliHelpStrategy {
    AppendFlag { fallback_subcommand: Option<String> },
}

pub trait CliHelpRunner: Send + Sync {
    fn help(&self, target: &CliInventoryTarget, path: &[String]) -> Result<String>;
}

pub struct ProcessCliHelpRunner;

impl CliHelpRunner for ProcessCliHelpRunner {
    fn help(&self, target: &CliInventoryTarget, path: &[String]) -> Result<String> {
        let strategies = match &target.help_strategy {
            CliHelpStrategy::AppendFlag { fallback_subcommand } => {
                let mut strategies = vec![build_append_flag_invocation(&target.program, path)];
                if let Some(fallback_subcommand) = fallback_subcommand {
                    strategies.push(build_help_subcommand_invocation(
                        &target.program,
                        fallback_subcommand,
                        path,
                    ));
                }

                strategies
            }
        };

        let mut last_error = None;
        for invocation in strategies {
            match run_help_invocation(&invocation) {
                Ok(stdout) => return Ok(stdout),
                Err(error) => last_error = Some(error),
            }
        }

        Err(last_error.unwrap_or_else(|| {
            Error::Execution(format!("failed to load help for {} {}", target.program, path.join(" ")))
        }))
    }
}

pub fn generate_inventory(target: &CliInventoryTarget, runner: &dyn CliHelpRunner) -> Result<CliInventory> {
    let mut records = Vec::new();
    let mut visited = HashSet::new();
    walk_inventory(target, Vec::new(), runner, &mut visited, &mut records)?;
    records.sort_by(|left, right| left.path.cmp(&right.path));

    let sibling_index = records
        .iter()
        .fold(HashMap::<Vec<String>, BTreeSet<String>>::new(), |mut index, record| {
            if let Some((name, parent)) = record.path.split_last() {
                index.entry(parent.to_vec()).or_default().insert(name.clone());
            }

            index
        });

    for record in &mut records {
        if record.node_kind == CliInventoryNodeKind::Operation {
            record.undo_support = infer_undo_support(record, &sibling_index);
        }
    }

    Ok(CliInventory {
        provider: target.provider.clone(),
        program: target.program.clone(),
        commands: records,
    })
}

fn walk_inventory(
    target: &CliInventoryTarget,
    path: Vec<String>,
    runner: &dyn CliHelpRunner,
    visited: &mut HashSet<Vec<String>>,
    records: &mut Vec<CliInventoryCommand>,
) -> Result<()> {
    if !visited.insert(path.clone()) {
        return Ok(());
    }

    let help = runner.help(target, &path)?;
    let parsed = parse_help(&help);
    let subcommands = parsed
        .subcommands
        .into_iter()
        .filter(|entry| entry.name != "help")
        .collect::<Vec<_>>();

    let node_kind = if subcommands.is_empty() {
        CliInventoryNodeKind::Operation
    } else {
        CliInventoryNodeKind::Group
    };
    let operation_kind = match node_kind {
        CliInventoryNodeKind::Group => CliOperationKind::Unknown,
        CliInventoryNodeKind::Operation => classify_operation(path.last().map(String::as_str), &parsed.summary),
    };

    records.push(CliInventoryCommand {
        path: path.clone(),
        command: render_command(&target.program, &path),
        summary: parsed.summary,
        usage: parsed.usage,
        help_args: build_help_args(target, &path),
        node_kind,
        operation_kind,
        undo_support: CliUndoSupport::Unknown,
        subcommands: subcommands.iter().map(|entry| entry.name.clone()).collect(),
    });

    for entry in subcommands {
        let mut next_path = path.clone();
        next_path.push(entry.name);
        walk_inventory(target, next_path, runner, visited, records)?;
    }

    Ok(())
}

fn render_command(program: &str, path: &[String]) -> String {
    if path.is_empty() {
        return program.to_owned();
    }

    format!("{program} {}", path.join(" "))
}

fn build_help_args(target: &CliInventoryTarget, path: &[String]) -> Vec<String> {
    match &target.help_strategy {
        CliHelpStrategy::AppendFlag { fallback_subcommand } => {
            let mut args = path.to_vec();
            args.push("--help".to_owned());

            if fallback_subcommand.is_none() || path.is_empty() {
                return args;
            }

            let mut fallback = Vec::with_capacity(path.len() + 1);
            fallback.push(fallback_subcommand.clone().expect("checked above"));
            fallback.extend(path.iter().cloned());
            fallback
        }
    }
}

fn infer_undo_support(
    record: &CliInventoryCommand,
    sibling_index: &HashMap<Vec<String>, BTreeSet<String>>,
) -> CliUndoSupport {
    if record.operation_kind == CliOperationKind::Read {
        return CliUndoSupport::None;
    }

    let Some((verb, parent)) = record.path.split_last() else {
        return CliUndoSupport::Unknown;
    };
    let siblings = sibling_index.get(parent).cloned().unwrap_or_default();
    for &candidate in candidate_undo_verbs(verb) {
        if siblings.contains(candidate) {
            let mut path = parent.to_vec();
            path.push(candidate.to_owned());
            return CliUndoSupport::CandidateCommand { path };
        }
    }

    match record.operation_kind {
        CliOperationKind::Write => CliUndoSupport::Unknown,
        CliOperationKind::Read | CliOperationKind::Unknown => CliUndoSupport::None,
    }
}

fn candidate_undo_verbs(verb: &str) -> &'static [&'static str] {
    match normalize_verb(verb) {
        "create" | "insert" | "import" | "quickadd" | "draft" => &["delete", "remove"],
        "lock" => &["unlock"],
        "close" => &["reopen"],
        "merge" => &["revert"],
        "watch" => &["stop", "delete"],
        _ => &[],
    }
}

fn classify_operation(verb: Option<&str>, summary: &str) -> CliOperationKind {
    let Some(verb) = verb else {
        return CliOperationKind::Unknown;
    };
    let verb = normalize_verb(verb);
    let summary = summary.to_ascii_lowercase();

    if matches!(
        verb,
        "list"
            | "get"
            | "view"
            | "read"
            | "search"
            | "status"
            | "diff"
            | "checks"
            | "show"
            | "agenda"
            | "instances"
            | "getprofile"
    ) || summary.starts_with("list ")
        || summary.starts_with("search ")
        || summary.starts_with("read ")
        || summary.starts_with("view ")
        || summary.starts_with("show ")
        || summary.starts_with("returns ")
        || summary.starts_with("gets ")
    {
        return CliOperationKind::Read;
    }

    if matches!(
        verb,
        "create"
            | "insert"
            | "delete"
            | "remove"
            | "update"
            | "patch"
            | "move"
            | "import"
            | "quickadd"
            | "comment"
            | "edit"
            | "merge"
            | "close"
            | "reopen"
            | "lock"
            | "unlock"
            | "ready"
            | "review"
            | "watch"
            | "stop"
    ) || summary.starts_with("create ")
        || summary.starts_with("delete ")
        || summary.starts_with("update ")
        || summary.starts_with("edit ")
        || summary.starts_with("move ")
        || summary.starts_with("imports ")
        || summary.starts_with("add ")
    {
        return CliOperationKind::Write;
    }

    CliOperationKind::Unknown
}

fn normalize_verb(verb: &str) -> &str {
    verb.trim_start_matches('+')
}

fn build_append_flag_invocation(program: &str, path: &[String]) -> Vec<String> {
    let mut invocation = Vec::with_capacity(path.len() + 2);
    invocation.push(program.to_owned());
    invocation.extend(path.iter().cloned());
    invocation.push("--help".to_owned());
    invocation
}

fn build_help_subcommand_invocation(program: &str, help_subcommand: &str, path: &[String]) -> Vec<String> {
    let mut invocation = Vec::with_capacity(path.len() + 2);
    invocation.push(program.to_owned());
    invocation.push(help_subcommand.to_owned());
    invocation.extend(path.iter().cloned());
    invocation
}

fn run_help_invocation(invocation: &[String]) -> Result<String> {
    let Some((program, args)) = invocation.split_first() else {
        return Err(Error::Execution("empty help invocation".into()));
    };
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|error| Error::Execution(format!("failed to run {}: {error}", invocation.join(" "))))?;
    if !output.status.success() {
        return Err(Error::Execution(format!(
            "{} exited with {}: {}",
            invocation.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    String::from_utf8(output.stdout)
        .map_err(|error| Error::Execution(format!("{} returned invalid UTF-8: {error}", invocation.join(" "))))
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ParsedHelp {
    summary: String,
    usage: Option<String>,
    subcommands: Vec<HelpEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct HelpEntry {
    name: String,
    summary: String,
}

fn parse_help(help: &str) -> ParsedHelp {
    let mut summary = String::new();
    let mut usage = None;
    let mut subcommands = Vec::new();
    let mut in_command_section = false;

    let lines = help.lines().collect::<Vec<_>>();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        let trimmed = line.trim();

        if summary.is_empty() && !trimmed.is_empty() {
            summary = trimmed.to_owned();
        }

        if usage.is_none() {
            if trimmed.eq_ignore_ascii_case("USAGE") {
                usage = lines
                    .iter()
                    .skip(index + 1)
                    .map(|candidate| candidate.trim())
                    .find(|candidate| !candidate.is_empty())
                    .map(str::to_owned);
            } else if let Some(value) = trimmed.strip_prefix("Usage:") {
                let value = value.trim();
                if !value.is_empty() {
                    usage = Some(value.to_owned());
                }
            } else if let Some(value) = trimmed.strip_prefix("USAGE:") {
                let value = value.trim();
                if !value.is_empty() {
                    usage = Some(value.to_owned());
                }
            }
        }

        if is_command_heading(trimmed) {
            in_command_section = true;
            index += 1;
            continue;
        }

        if in_command_section {
            if trimmed.is_empty() {
                index += 1;
                continue;
            }
            if is_section_heading(trimmed) && !is_command_heading(trimmed) {
                in_command_section = false;
                continue;
            }
            if let Some(entry) = parse_help_entry(line) {
                subcommands.push(entry);
                index += 1;
                continue;
            }
        }

        index += 1;
    }

    ParsedHelp {
        summary,
        usage,
        subcommands,
    }
}

fn is_command_heading(line: &str) -> bool {
    let normalized = line.trim_end_matches(':');
    normalized.eq_ignore_ascii_case("commands")
        || normalized.eq_ignore_ascii_case("available commands")
        || normalized.eq_ignore_ascii_case("services")
        || normalized.ends_with(" COMMANDS")
}

fn is_section_heading(line: &str) -> bool {
    let normalized = line.trim_end_matches(':');
    normalized.eq_ignore_ascii_case("options")
        || normalized.eq_ignore_ascii_case("flags")
        || normalized.eq_ignore_ascii_case("inherited flags")
        || normalized.eq_ignore_ascii_case("arguments")
        || normalized.eq_ignore_ascii_case("examples")
        || normalized.eq_ignore_ascii_case("learn more")
        || normalized.eq_ignore_ascii_case("environment")
        || normalized.eq_ignore_ascii_case("exit codes")
        || normalized.eq_ignore_ascii_case("community")
        || normalized.eq_ignore_ascii_case("disclaimer")
        || normalized.eq_ignore_ascii_case("usage")
        || normalized
            .chars()
            .all(|character| character.is_ascii_uppercase() || character == ' ' || character == '-')
}

fn parse_help_entry(line: &str) -> Option<HelpEntry> {
    let trimmed = line.trim_start();
    let indent_width = line.len().saturating_sub(trimmed.len());
    if trimmed.len() == line.len() || trimmed.starts_with('-') {
        return None;
    }

    let name_end = trimmed
        .find(|character: char| character.is_whitespace() || character == ':')
        .unwrap_or(trimmed.len());
    let name = trimmed[..name_end].trim_end_matches(':').to_owned();
    if name.is_empty() || name == "help" {
        return None;
    }

    let remainder = &trimmed[name_end..];
    let summary = if let Some(remainder) = remainder.strip_prefix(':') {
        if indent_width > 4 {
            return None;
        }
        remainder.trim().to_owned()
    } else {
        let whitespace_width = remainder
            .chars()
            .take_while(|character| character.is_whitespace())
            .count();
        if whitespace_width < 2 {
            return None;
        }
        remainder.trim().to_owned()
    };

    Some(HelpEntry { name, summary })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use switchboard_core::ProviderKind;

    use crate::{
        inventory::{CliInventoryNodeKind, CliOperationKind, CliUndoSupport},
        inventory_generator::{
            generate_inventory, parse_help, CliHelpRunner, CliHelpStrategy, CliInventoryTarget, HelpEntry,
        },
    };

    const GH_ROOT_HELP: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/help/gh-root.txt"
    ));
    const GH_PR_HELP: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/help/gh-pr.txt"
    ));
    const GH_PR_VIEW_HELP: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/help/gh-pr-view.txt"
    ));
    const GWS_ROOT_HELP: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/help/gws-root.txt"
    ));
    const GWS_CALENDAR_HELP: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/help/gws-calendar.txt"
    ));
    const GWS_CALENDAR_EVENTS_HELP: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/help/gws-calendar-events.txt"
    ));
    const GWS_CALENDAR_EVENTS_INSERT_HELP: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/help/gws-calendar-events-insert.txt"
    ));
    const GWS_CALENDAR_EVENTS_DELETE_HELP: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/help/gws-calendar-events-delete.txt"
    ));

    #[test]
    fn parses_gh_help_sections() {
        let parsed = parse_help(GH_ROOT_HELP);

        assert_eq!(parsed.summary, "Work seamlessly with GitHub from the command line.");
        assert_eq!(parsed.usage.as_deref(), Some("gh <command> <subcommand> [flags]"));
        assert!(parsed.subcommands.contains(&HelpEntry {
            name: "pr".into(),
            summary: "Manage pull requests".into(),
        }));
        assert!(parsed.subcommands.iter().all(|entry| entry.name != "help"));
    }

    #[test]
    fn parses_clap_help_sections() {
        let parsed = parse_help(GWS_CALENDAR_HELP);

        assert_eq!(parsed.summary, "Manipulates events and other calendar data.");
        assert_eq!(parsed.usage.as_deref(), Some("gws [OPTIONS] <COMMAND>"));
        assert!(parsed.subcommands.contains(&HelpEntry {
            name: "events".into(),
            summary: "Operations on the 'events' resource".into(),
        }));
    }

    #[test]
    fn generates_gh_inventory_tree_with_read_and_write_operations() {
        let target = CliInventoryTarget {
            provider: ProviderKind::GitHub,
            program: "gh".into(),
            output_path: "ignored.json".into(),
            help_strategy: CliHelpStrategy::AppendFlag {
                fallback_subcommand: Some("help".into()),
            },
        };
        let runner = FixtureRunner::new(
            "gh",
            [
                (vec![], GH_ROOT_HELP),
                (vec!["pr"], GH_PR_HELP),
                (vec!["pr", "view"], GH_PR_VIEW_HELP),
            ],
        );

        let inventory = generate_inventory(&target, &runner).expect("inventory should build");
        let pr_group = inventory.command(&["pr"]).expect("pr group should exist");
        assert_eq!(pr_group.node_kind, CliInventoryNodeKind::Group);

        let pr_view = inventory.command(&["pr", "view"]).expect("pr view should exist");
        assert_eq!(pr_view.operation_kind, CliOperationKind::Read);

        let pr_merge = inventory.command(&["pr", "merge"]).expect("pr merge should exist");
        assert_eq!(pr_merge.operation_kind, CliOperationKind::Write);
        assert_eq!(
            pr_merge.undo_support,
            CliUndoSupport::CandidateCommand {
                path: vec!["pr".into(), "revert".into()],
            }
        );
    }

    #[test]
    fn generates_inventory_tree_and_infers_undo_candidates() {
        let target = CliInventoryTarget {
            provider: ProviderKind::GoogleWorkspace,
            program: "gws".into(),
            output_path: "ignored.json".into(),
            help_strategy: CliHelpStrategy::AppendFlag {
                fallback_subcommand: None,
            },
        };
        let runner = FixtureRunner::new(
            "gws",
            [
                (vec![], GWS_ROOT_HELP),
                (vec!["calendar"], GWS_CALENDAR_HELP),
                (vec!["calendar", "events"], GWS_CALENDAR_EVENTS_HELP),
                (vec!["calendar", "events", "insert"], GWS_CALENDAR_EVENTS_INSERT_HELP),
                (vec!["calendar", "events", "delete"], GWS_CALENDAR_EVENTS_DELETE_HELP),
            ],
        );

        let inventory = generate_inventory(&target, &runner).expect("inventory should build");
        let root = inventory.command(&[] as &[&str]).expect("root command should exist");
        assert_eq!(root.node_kind, CliInventoryNodeKind::Group);

        let insert = inventory
            .command(&["calendar", "events", "insert"])
            .expect("insert should exist");
        assert_eq!(insert.node_kind, CliInventoryNodeKind::Operation);
        assert_eq!(insert.operation_kind, CliOperationKind::Write);
        assert_eq!(
            insert.undo_support,
            CliUndoSupport::CandidateCommand {
                path: vec!["calendar".into(), "events".into(), "delete".into()],
            }
        );
    }

    struct FixtureRunner {
        program: String,
        fixtures: BTreeMap<Vec<String>, String>,
    }

    impl FixtureRunner {
        fn new<const N: usize>(program: &str, fixtures: [(Vec<&str>, &str); N]) -> Self {
            Self {
                program: program.to_owned(),
                fixtures: fixtures
                    .into_iter()
                    .map(|(path, help)| (path.into_iter().map(str::to_owned).collect::<Vec<_>>(), help.to_owned()))
                    .collect(),
            }
        }
    }

    impl CliHelpRunner for FixtureRunner {
        fn help(&self, target: &CliInventoryTarget, path: &[String]) -> switchboard_core::Result<String> {
            assert_eq!(target.program, self.program);
            Ok(self
                .fixtures
                .get(path)
                .cloned()
                .unwrap_or_else(|| fallback_leaf_help(&target.program, path)))
        }
    }

    fn fallback_leaf_help(program: &str, path: &[String]) -> String {
        let command = if path.is_empty() {
            program.to_owned()
        } else {
            format!("{program} {}", path.join(" "))
        };
        let summary = path
            .last()
            .map(|segment| format!("Stub help for {segment}."))
            .unwrap_or_else(|| format!("Stub help for {program}."));

        format!("{summary}\n\nUsage: {command} [OPTIONS]\n")
    }
}

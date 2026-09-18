use std::{
    collections::BTreeMap,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Deserializer};
use switchboard_core::{NamespaceId, ToolArgument, ToolName};

use crate::batch::{BatchInput, ReadBatchArgs, ReadItem};

const MAX_INPUT_BYTES: u64 = 8 * 1024 * 1024;

pub(super) fn tool_name(value: &str) -> switchboard_core::Result<ToolName> {
    ToolName::new(value)
}

pub(super) fn namespace_id(value: &str) -> switchboard_core::Result<NamespaceId> {
    NamespaceId::new(value)
}

#[derive(Clone, Debug)]
pub(super) enum ResumeSelection {
    CheckpointOption,
    Path(PathBuf),
}

pub(super) fn resume_selection(value: &str) -> std::result::Result<ResumeSelection, std::convert::Infallible> {
    Ok(if value.is_empty() {
        ResumeSelection::CheckpointOption
    } else {
        ResumeSelection::Path(value.into())
    })
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ArgumentAtom {
    Text(String),
    Number(serde_json::Number),
    Flag(bool),
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ArgumentValue {
    Single(ArgumentAtom),
    Repeated(Vec<ArgumentAtom>),
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Arguments {
    Typed(Vec<ToolArgument>),
    Compact(BTreeMap<String, ArgumentValue>),
}

impl Arguments {
    fn into_arguments(self) -> Result<Vec<ToolArgument>> {
        let arguments = match self {
            Self::Typed(arguments) => return Ok(arguments),
            Self::Compact(arguments) => arguments,
        };
        let mut result = Vec::new();
        for (name, values) in arguments {
            let values = match values {
                ArgumentValue::Single(value) => vec![value],
                ArgumentValue::Repeated(values) => values,
            };
            for value in values {
                match value {
                    ArgumentAtom::Text(value) => result.push(ToolArgument::option(&name, value)?),
                    ArgumentAtom::Number(value) => result.push(ToolArgument::option(&name, value.to_string())?),
                    ArgumentAtom::Flag(true) => result.push(ToolArgument::flag(&name)?),
                    ArgumentAtom::Flag(false) => {
                        // Validate omitted flags too; a typo is still an invalid argument name.
                        ToolArgument::flag(&name)?;
                    }
                }
            }
        }
        Ok(result)
    }
}

pub(super) fn deserialize_arguments<'de, D>(deserializer: D) -> std::result::Result<Vec<ToolArgument>, D::Error>
where
    D: Deserializer<'de>,
{
    Arguments::deserialize(deserializer)?
        .into_arguments()
        .map_err(serde::de::Error::custom)
}

fn read_input(reader: impl Read) -> Result<BatchInput> {
    let mut bytes = Vec::new();
    reader.take(MAX_INPUT_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_INPUT_BYTES {
        bail!("batch input exceeds 8 MiB");
    }
    serde_json::from_slice(&bytes).context("invalid batch input")
}

pub(super) fn request(args: &ReadBatchArgs) -> Result<Option<BatchInput>> {
    if let Some(path) = &args.input {
        return if path == Path::new("-") {
            read_input(io::stdin().lock()).map(Some)
        } else {
            read_input(fs::File::open(path).context("read batch input")?).map(Some)
        };
    }
    if let Some(tool) = &args.tool {
        let namespace = args.namespace.as_ref().ok_or_else(|| anyhow!("--tool requires --ns"))?;
        if args.args_json.is_empty() || args.args_json.len() > 1000 {
            bail!("provide 1..1000 --args-json requests");
        }
        let items = args
            .args_json
            .iter()
            .enumerate()
            .map(|(index, arguments)| {
                let arguments: BTreeMap<String, ArgumentValue> = serde_json::from_str(arguments)
                    .with_context(|| format!("invalid --args-json request {}", index + 1))?;
                Ok(ReadItem {
                    id: (index + 1).to_string(),
                    tool: tool.clone(),
                    namespace: namespace.clone(),
                    args: Arguments::Compact(arguments).into_arguments()?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        return Ok(Some(BatchInput { items }));
    }
    if args.resume.is_none() {
        bail!("provide --input, or --tool with --ns and --args-json, or --resume CHECKPOINT");
    }
    Ok(None)
}

fn absolute(path: &Path) -> Result<PathBuf> {
    Ok(if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    })
}

pub(super) fn checkpoint_path(args: &ReadBatchArgs, config_path: Option<&Path>) -> Result<PathBuf> {
    if let Some(resume) = &args.resume {
        if let ResumeSelection::Path(resume) = resume {
            let path = absolute(resume)?;
            if let Some(checkpoint) = &args.checkpoint {
                if absolute(checkpoint)? != path {
                    bail!("--resume and --checkpoint must identify the same checkpoint");
                }
            }
            return Ok(path);
        }
        return args
            .checkpoint
            .as_deref()
            .ok_or_else(|| anyhow!("bare --resume requires --checkpoint; use --resume CHECKPOINT"))
            .and_then(absolute);
    }
    if let Some(path) = &args.checkpoint {
        return absolute(path);
    }
    let config = crate::resolve_config_path(config_path)?;
    let state = switchboard_store::resolve_operation_store_path(&config);
    let directory = absolute(&state)?
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("batches");
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&directory).context("create private batch directory")?;
    let unique = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    Ok(directory.join(format!("batch-{}-{unique}.json", std::process::id())))
}

pub(super) fn resume_argv(config_path: Option<&Path>, checkpoint: &Path, args: &ReadBatchArgs) -> Result<Vec<String>> {
    let config = absolute(&crate::resolve_config_path(config_path)?)?;
    let mut command = vec![
        "switchboard".into(),
        "--config".into(),
        config.to_string_lossy().into_owned(),
        "read-batch".into(),
        "--resume".into(),
        checkpoint.to_string_lossy().into_owned(),
        "--max-pages".into(),
        args.max_pages.to_string(),
        "--deadline-seconds".into(),
        args.deadline_seconds.to_string(),
        "--concurrency".into(),
        args.concurrency.to_string(),
    ];
    if args.json {
        command.push("--json".into());
    }
    Ok(command)
}

pub(super) fn shell_quote(argument: &str) -> String {
    if !argument.is_empty()
        && argument
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "_./:@+=,-".contains(character))
    {
        argument.to_owned()
    } else {
        format!("'{}'", argument.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_arguments_normalize_to_the_existing_checkpoint_request() {
        let compact = r#"{"items":[{"id":"mail","tool":"google.mail.search","namespace":"google.test","args":{"max":25,"query":"receipt","label":["one","two"],"unread":true,"ignored":false}}]}"#;
        let input: BatchInput = serde_json::from_str(compact).expect("compact batch input");
        let expected = BatchInput {
            items: vec![ReadItem {
                id: "mail".into(),
                tool: ToolName::new("google.mail.search").expect("tool"),
                namespace: NamespaceId::new("google.test").expect("namespace"),
                args: vec![
                    ToolArgument::option("label", "one").expect("argument"),
                    ToolArgument::option("label", "two").expect("argument"),
                    ToolArgument::option("max", "25").expect("argument"),
                    ToolArgument::option("query", "receipt").expect("argument"),
                    ToolArgument::flag("unread").expect("argument"),
                ],
            }],
        };
        assert_eq!(input, expected);
        let typed = serde_json::to_vec(&expected).expect("existing typed checkpoint request");
        assert_eq!(
            serde_json::from_slice::<BatchInput>(&typed).expect("typed input"),
            input
        );
    }

    #[test]
    fn compact_arguments_reject_nested_or_null_values_instead_of_dropping_them() {
        for arguments in [
            r#"{"query":null}"#,
            r#"{"query":{"nested":"value"}}"#,
            r#"{"query":[["value"]]}"#,
        ] {
            assert!(serde_json::from_str::<Arguments>(arguments).is_err(), "{arguments}");
        }
    }
}

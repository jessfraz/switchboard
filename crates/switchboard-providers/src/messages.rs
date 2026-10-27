use std::{collections::BTreeSet, ffi::OsString};

use switchboard_core::{
    Adapter, BackendKind, Error, ExecutionTarget, PlannedAction, PlanningTarget, ProviderKind, ReadCoverage,
    ResolvedCredentials, Result, ToolArgument, ToolArgumentSpec, ToolArgumentTransport, ToolArgumentValueKind,
    ToolDescriptor, ToolKind, ToolOutput, ToolRequest,
};

/// Reads the current macOS user's local history through the same implementation
/// as the standalone CLI. There is no child process, credential materialization,
/// or raw command surface.
pub struct MessagesAdapter {
    tools: Vec<ToolDescriptor>,
}

impl MessagesAdapter {
    pub fn new() -> Result<Self> {
        let mut tools = Vec::new();
        for (name, summary) in [
            (
                "messages.chats.list",
                "List recent conversations in local macOS Messages history",
            ),
            (
                "messages.chats.read",
                "Read one local Messages conversation without marking it read",
            ),
            ("messages.search", "Search message text in local macOS Messages history"),
            (
                "messages.doctor",
                "Check read-only access to the local Messages database",
            ),
            (
                "messages.attachments.read",
                "Read a locally available Messages attachment, including images",
            ),
            (
                "messages.activity",
                "Count sent and received messages in local Messages history",
            ),
        ] {
            tools.push(
                ToolDescriptor::new(name, ToolKind::Read, summary, BackendKind::Local)?
                    .with_arguments(arguments(name)?),
            );
        }
        Ok(Self { tools })
    }
}

impl Adapter for MessagesAdapter {
    fn provider(&self) -> ProviderKind {
        ProviderKind::Messages
    }

    fn tools(&self) -> &[ToolDescriptor] {
        &self.tools
    }

    fn plan(
        &self,
        target: &PlanningTarget,
        request: &ToolRequest,
        descriptor: &ToolDescriptor,
    ) -> Result<PlannedAction> {
        command(request.tool.as_str(), &request.args)?;
        Ok(PlannedAction::new(
            request,
            target,
            ToolKind::Read,
            descriptor.summary.clone(),
            BackendKind::Local,
        ))
    }

    fn execute(&self, target: &ExecutionTarget, action: &PlannedAction) -> Result<ToolOutput> {
        if !matches!(target.credentials, ResolvedCredentials::MessagesLocal) {
            return Err(Error::UnsupportedOperation(
                "Messages requires local Messages credentials".into(),
            ));
        }
        let argv = command(action.tool.as_str(), &action.args)?;
        let response = messages_cli::execute(argv).map_err(|error| Error::Execution(error.to_string()))?;
        let mut coverage = ReadCoverage::page(response.next_cursor().map(str::to_owned));
        if response.is_partial() {
            coverage.status = switchboard_core::CoverageStatus::Unknown;
        }
        let value =
            serde_json::to_value(response).map_err(|_| Error::Execution("could not encode Messages result".into()))?;
        let mut output = ToolOutput::new(action.tool.clone(), action.namespace.clone(), action.summary.clone())
            .with_value_field("response", value);
        if action.tool.as_str() != "messages.doctor" {
            output.coverage = Some(coverage);
            output = output.with_field("history_scope", "local");
        }
        Ok(output)
    }
}

fn arguments(name: &str) -> Result<Vec<ToolArgumentSpec>> {
    let mut arguments = vec![option("db", false, false, ToolArgumentValueKind::String)?];
    if name == "messages.doctor" {
        return Ok(arguments);
    }
    if name == "messages.attachments.read" {
        arguments.push(option("attachment-id", true, false, ToolArgumentValueKind::String)?);
        return Ok(arguments);
    }
    for name in ["since", "until", "cursor"] {
        arguments.push(option(name, false, false, ToolArgumentValueKind::String)?);
    }
    arguments.push(option("limit", false, false, ToolArgumentValueKind::String)?);
    match name {
        "messages.chats.list" => {
            arguments.push(option("with", false, true, ToolArgumentValueKind::String)?);
            arguments.push(ToolArgumentSpec::new(
                "unread",
                ToolArgumentTransport::Flag,
                ToolArgumentValueKind::Boolean,
            )?);
        }
        "messages.chats.read" => {
            arguments.push(option("chat-id", true, false, ToolArgumentValueKind::String)?);
            arguments.push(ToolArgumentSpec::new(
                "unread",
                ToolArgumentTransport::Flag,
                ToolArgumentValueKind::Boolean,
            )?);
        }
        "messages.search" => {
            arguments.push(option("query", true, false, ToolArgumentValueKind::String)?);
            arguments.push(option("chat", false, false, ToolArgumentValueKind::String)?);
            arguments.push(option("with", false, true, ToolArgumentValueKind::String)?);
        }
        "messages.activity" => {
            arguments.push(option("chat-id", false, false, ToolArgumentValueKind::String)?);
            arguments.push(option("interval", false, false, ToolArgumentValueKind::String)?);
        }
        _ => return Err(Error::UnsupportedTool(name.into())),
    }
    Ok(arguments)
}

fn option(name: &str, required: bool, repeated: bool, kind: ToolArgumentValueKind) -> Result<ToolArgumentSpec> {
    Ok(ToolArgumentSpec::new(name, ToolArgumentTransport::Option, kind)?
        .with_required(required)
        .with_repeated(repeated))
}

fn command(name: &str, args: &switchboard_core::ToolArguments) -> Result<Vec<OsString>> {
    let specs = arguments(name)?;
    let mut seen = BTreeSet::new();
    for arg in args.iter() {
        let spec = specs
            .iter()
            .find(|spec| spec.name == arg.name())
            .ok_or_else(|| Error::InvalidArguments(format!("unknown Messages argument: {}", arg.name())))?;
        if !seen.insert(arg.name()) && !spec.repeated {
            return Err(Error::InvalidArguments(format!(
                "duplicate Messages argument: {}",
                arg.name()
            )));
        }
        match arg {
            ToolArgument::Flag { .. } if spec.transport == ToolArgumentTransport::Flag => {}
            ToolArgument::Option { value, .. } if spec.transport == ToolArgumentTransport::Option => {
                if value.trim().is_empty() || value.contains('\0') {
                    return Err(Error::InvalidArguments(format!(
                        "{} must be nonempty and contain no NUL",
                        spec.name
                    )));
                }
                if spec.name == "limit" && !value.parse::<u32>().is_ok_and(|value| (1..=500).contains(&value)) {
                    return Err(Error::InvalidArguments("limit must be between 1 and 500".into()));
                }
            }
            _ => {
                return Err(Error::InvalidArguments(format!(
                    "invalid Messages argument type: {}",
                    spec.name
                )))
            }
        }
    }
    for spec in specs.iter().filter(|spec| spec.required) {
        if !seen.contains(spec.name.as_str()) {
            return Err(Error::InvalidArguments(format!("missing --{}", spec.name)));
        }
    }
    let command = match name {
        "messages.chats.list" => "chats",
        "messages.chats.read" => "read",
        "messages.search" => "search",
        "messages.doctor" => "doctor",
        "messages.attachments.read" => "attachment",
        "messages.activity" => "activity",
        _ => return Err(Error::UnsupportedTool(name.into())),
    };
    let mut argv = vec![OsString::from("messages"), command.into(), "--json".into()];
    for arg in args.iter().filter(|arg| {
        !matches!(arg.name(), "attachment-id" | "query") && (arg.name() != "chat-id" || name == "messages.activity")
    }) {
        argv.push(match arg {
            ToolArgument::Flag { name } => format!("--{name}").into(),
            ToolArgument::Option { name, value } if name == "chat-id" => format!("--chat={value}").into(),
            ToolArgument::Option { name, value } => format!("--{name}={value}").into(),
        });
    }
    // End option parsing before positional text. Searching for "--db=x" must
    // search that literal string rather than changing the selected database.
    let positional = if name == "messages.activity" {
        None
    } else {
        args.value("chat-id")
            .or_else(|| args.value("query"))
            .or_else(|| args.value("attachment-id"))
    };
    if let Some(value) = positional {
        argv.extend(["--".into(), value.into()]);
    }
    Ok(argv)
}

#[cfg(test)]
mod tests {
    use std::{ffi::OsString, fs};

    use switchboard_core::{
        Adapter, AuthSecretRefs, CoverageStatus, ExecutionMode, ExecutionTarget, PlanningTarget, ProviderKind,
        ResolvedAuth, ResolvedCredentials, ResolvedNamespace, ToolArgument, ToolArguments, ToolKind, ToolRequest,
    };

    use crate::messages::{command, MessagesAdapter};

    #[test]
    fn adapter_executes_real_local_reads_and_preserves_source_and_pagination() {
        let directory = tempfile::tempdir().expect("fixture directory");
        let path = directory.path().join("chat.db");
        let connection = rusqlite::Connection::open(&path).expect("fixture database");
        connection
            .execute_batch(include_str!("../../messages-cli/tests/fixtures/schema.sql"))
            .expect("Messages schema");
        connection
            .execute_batch(
                "INSERT INTO chat(ROWID,guid,display_name,service_name) VALUES(1,'chat-test','Example','iMessage');
             INSERT INTO handle(ROWID,id) VALUES(1,'person@example.invalid');
             INSERT INTO chat_handle_join(chat_id,handle_id) VALUES(1,1);
             INSERT INTO message(ROWID,guid,text,date,handle_id,is_from_me,is_read) VALUES
               (1,'message-first','Dinner?',750000000000000000,1,0,0),
               (2,'message-second','Yes',750000001000000000,0,1,1);
             INSERT INTO chat_message_join(chat_id,message_id,message_date) VALUES
               (1,1,750000000000000000),(1,2,750000001000000000);",
            )
            .expect("fixture messages");
        drop(connection);
        let before = fs::read(&path).expect("source bytes");
        let planning = PlanningTarget {
            namespace: ResolvedNamespace::new(
                "messages.local",
                ProviderKind::Messages,
                "local",
                "messages_local",
                true,
                None,
            )
            .expect("namespace"),
            auth: ResolvedAuth::new("messages_local", "local", AuthSecretRefs::MessagesLocal).expect("auth"),
        };
        let target = ExecutionTarget {
            namespace: planning.namespace.clone(),
            auth: planning.auth.clone(),
            credentials: ResolvedCredentials::MessagesLocal,
        };
        let adapter = MessagesAdapter::new().expect("adapter");
        let mut args = vec![
            ToolArgument::option("db", path.to_string_lossy()).expect("database option"),
            ToolArgument::option("chat-id", "chat-test").expect("chat option"),
            ToolArgument::option("limit", "1").expect("limit option"),
        ];
        let mut ids = Vec::new();
        for page in 0..2 {
            let request = ToolRequest::new(
                "messages.chats.read",
                "messages.local",
                ExecutionMode::Auto,
                args.clone(),
            )
            .expect("request");
            let action = adapter
                .plan(&planning, &request, adapter.find_tool(&request.tool).expect("tool"))
                .expect("plan");
            let output = adapter.execute(&target, &action).expect("local execution");
            let response: messages_cli::Response =
                serde_json::from_value(output.fields.get("response").expect("response").clone())
                    .expect("typed standalone response");
            let messages_cli::Response::Messages {
                messages, next_cursor, ..
            } = response
            else {
                panic!("message response expected");
            };
            assert_eq!(messages.len(), 1);
            ids.push(messages[0].id.clone());
            let coverage = output.coverage.expect("read coverage");
            if page == 0 {
                assert_eq!(coverage.status, CoverageStatus::Truncated);
                assert_eq!(coverage.next_cursor, next_cursor);
                args.push(ToolArgument::option("cursor", next_cursor.expect("continuation")).expect("cursor option"));
            } else {
                assert_eq!(coverage.status, CoverageStatus::Complete);
                assert!(next_cursor.is_none());
                assert!(messages[0].is_unread);
            }
        }
        assert_eq!(ids, ["message-second", "message-first"]);
        assert_eq!(fs::read(&path).expect("source bytes after reading"), before);
    }

    #[test]
    fn supported_tools_are_all_curated_reads_with_no_write_or_raw_entrypoint() {
        let adapter = MessagesAdapter::new().expect("adapter");
        assert_eq!(adapter.tools().len(), 6);
        assert!(adapter.tools().iter().all(|tool| tool.kind == ToolKind::Read));
        assert!(adapter
            .tools()
            .iter()
            .all(|tool| tool.surface == switchboard_core::ToolSurface::Curated));
    }

    #[test]
    fn query_and_filter_values_cannot_be_interpreted_as_cli_options() {
        let args = ToolArguments::new(vec![
            ToolArgument::option("query", "--db=/other/database").expect("query"),
            ToolArgument::option("chat", "--unread").expect("chat"),
        ]);
        assert_eq!(
            command("messages.search", &args).expect("command"),
            [
                "messages",
                "search",
                "--json",
                "--chat=--unread",
                "--",
                "--db=/other/database"
            ]
            .map(OsString::from)
        );
    }

    #[test]
    fn invalid_or_write_arguments_cannot_be_planned() {
        for extra in [
            ToolArgument::option("sql", "DELETE FROM message").expect("argument"),
            ToolArgument::flag("send").expect("argument"),
            ToolArgument::option("limit", "501").expect("argument"),
            ToolArgument::option("limit", "0").expect("argument"),
            ToolArgument::option("unread", "true").expect("argument"),
            ToolArgument::flag("db").expect("argument"),
        ] {
            assert!(command("messages.chats.list", &vec![extra].into()).is_err());
        }
        assert!(command("messages.chats.read", &ToolArguments::empty()).is_err());
        assert!(command("messages.search", &ToolArguments::empty()).is_err());
        assert!(command("messages.send", &ToolArguments::empty()).is_err());
        assert!(command(
            "messages.chats.list",
            &vec![
                ToolArgument::option("limit", "5").expect("argument"),
                ToolArgument::option("limit", "10").expect("argument"),
            ]
            .into()
        )
        .is_err());
    }
}

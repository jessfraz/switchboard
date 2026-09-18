use std::{
    ffi::OsString,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context, Result};
use switchboard_core::OperationRequest;

use crate::args::CommandKind;

/// Set the CLI's inherited subprocess deadline before auth starts. Restore it
/// when embedding callers invoke the CLI more than once in the same process.
pub(crate) struct Deadline {
    previous: Option<OsString>,
}

impl Deadline {
    pub(crate) fn configure(command: &CommandKind) -> Result<Self> {
        let guard = Self {
            previous: std::env::var_os("SWITCHBOARD_DEADLINE_UNIX_MS"),
        };
        let budget = match command {
            CommandKind::ReadBatch(args) => Some(args.time_budget()),
            CommandKind::Operation(request) => {
                let (tool, args) = match request {
                    OperationRequest::Single(request) => (&request.tool, &request.args),
                    OperationRequest::AggregateRead(request) => (&request.tool, &request.args),
                };
                if tool.as_str() == "github.ci.status" {
                    let wait = args
                        .value("wait")
                        .unwrap_or("0")
                        .parse::<u64>()
                        .context("--wait must be an integer from 0 through 60")?;
                    if wait > 60 {
                        bail!("--wait must be an integer from 0 through 60");
                    }
                    Some(Duration::from_secs(if wait == 0 { 60 } else { wait }))
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some(budget) = budget {
            let own = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() + budget.as_millis();
            let inherited = guard
                .previous
                .as_ref()
                .map(|value| value.to_string_lossy().parse::<u128>())
                .transpose()
                .context("invalid inherited subprocess deadline")?;
            // CLI initialization is single-threaded; workers only read this inherited contract.
            std::env::set_var(
                "SWITCHBOARD_DEADLINE_UNIX_MS",
                inherited.map_or(own, |previous| previous.min(own)).to_string(),
            );
        }
        Ok(guard)
    }
}

impl Drop for Deadline {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => std::env::set_var("SWITCHBOARD_DEADLINE_UNIX_MS", value),
            None => std::env::remove_var("SWITCHBOARD_DEADLINE_UNIX_MS"),
        }
    }
}

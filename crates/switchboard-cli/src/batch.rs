use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Context, Result};
use clap::Args;
use serde::{Deserialize, Serialize};
use switchboard_core::{
    CoverageStatus, DispatchOutcome, ExecutionMode, Failure, NamespaceId, ReadCoverage, Switchboard, ToolArgument,
    ToolExecutionSupport, ToolKind, ToolName, ToolOutput, ToolRequest,
};

mod input;

#[derive(Debug, Args)]
pub(crate) struct ReadBatchArgs {
    /// JSON object containing an items array; use - to read stdin.
    #[arg(long, conflicts_with = "tool")]
    pub(crate) input: Option<PathBuf>,
    /// Curated read tool shared by each --args-json request.
    #[arg(long, requires_all = ["namespace", "args_json"], value_parser = input::tool_name)]
    tool: Option<ToolName>,
    /// Namespace shared by each --args-json request.
    #[arg(long = "ns", requires = "tool", value_parser = input::namespace_id)]
    namespace: Option<NamespaceId>,
    /// One request's arguments as an object; repeat for multiple queries or IDs.
    #[arg(long, requires = "tool", value_name = "OBJECT")]
    args_json: Vec<String>,
    /// Durable results; successful pages are saved after each bounded wave.
    #[arg(long)]
    pub(crate) checkpoint: Option<PathBuf>,
    /// Resume a checkpoint without the original input; bare --resume uses --checkpoint.
    #[arg(long, num_args = 0..=1, default_missing_value = "", value_name = "CHECKPOINT", value_parser = input::resume_selection)]
    resume: Option<input::ResumeSelection>,
    #[arg(long, default_value_t = 4, value_parser = clap::value_parser!(u16).range(1..=16))]
    concurrency: u16,
    #[arg(long, default_value_t = 120, value_parser = clap::value_parser!(u64).range(1..=3600))]
    deadline_seconds: u64,
    #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u16).range(1..=100))]
    max_pages: u16,
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BatchInput {
    items: Vec<ReadItem>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadItem {
    id: String,
    tool: ToolName,
    namespace: NamespaceId,
    #[serde(default, deserialize_with = "input::deserialize_arguments")]
    args: Vec<ToolArgument>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PageReceipt {
    requested_cursor: Option<String>,
    output: ToolOutput,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ItemProgress {
    pages: Vec<PageReceipt>,
    coverage: ReadCoverage,
    failure: Option<Failure>,
    /// Completed one logical request. Unknown coverage is retained explicitly.
    finished: bool,
}

impl Default for ItemProgress {
    fn default() -> Self {
        Self {
            pages: Vec::new(),
            coverage: ReadCoverage {
                status: CoverageStatus::Unknown,
                next_cursor: None,
            },
            failure: None,
            finished: false,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Checkpoint {
    schema_version: u32,
    request: BatchInput,
    items: BTreeMap<String, ItemProgress>,
}

impl ReadBatchArgs {
    pub(crate) fn time_budget(&self) -> Duration {
        Duration::from_secs(self.deadline_seconds)
    }
}

pub(crate) fn run(
    switchboard: &Switchboard,
    config_path: Option<&Path>,
    args: ReadBatchArgs,
    presentation: &crate::presentation::Presentation,
) -> Result<String> {
    let started = Instant::now();
    let requested = input::request(&args)?;
    let checkpoint_path = input::checkpoint_path(&args, config_path)?;
    let _lock = CheckpointLock::acquire(&checkpoint_path)?;
    let mut checkpoint = if args.resume.is_some() {
        let saved: Checkpoint = serde_json::from_slice(&fs::read(&checkpoint_path).context("read checkpoint")?)?;
        if saved.schema_version != 1 {
            bail!("unsupported checkpoint schema version {}", saved.schema_version);
        }
        if requested.as_ref().is_some_and(|request| *request != saved.request) {
            bail!("checkpoint does not match this exact batch input");
        }
        if saved
            .items
            .keys()
            .any(|id| !saved.request.items.iter().any(|item| &item.id == id))
        {
            bail!("checkpoint contains an unknown item");
        }
        for item in &saved.request.items {
            if let Some(progress) = saved.items.get(&item.id) {
                validate_progress(item, progress)?;
            }
        }
        let mut saved = saved;
        for progress in saved.items.values_mut().filter(|progress| !progress.finished) {
            progress.failure = None;
        }
        saved
    } else {
        if checkpoint_path.exists() {
            bail!("checkpoint exists; use --resume or a new path");
        }
        Checkpoint {
            schema_version: 1,
            request: requested.ok_or_else(|| anyhow!("provide --input, or --tool with --ns and --args-json"))?,
            items: BTreeMap::new(),
        }
    };
    validate(switchboard, &checkpoint.request)?;
    let request = checkpoint.request.clone();
    // Persist the request before authentication so every provider attempt has a resumable owner.
    write_checkpoint(&checkpoint_path, &checkpoint)?;
    let mut blocked = BTreeMap::new();
    let namespaces = request
        .items
        .iter()
        .filter(|item| !checkpoint.items.get(&item.id).is_some_and(|progress| progress.finished))
        .map(|item| item.namespace.clone())
        .collect::<BTreeSet<_>>();
    // Resolve account identity serially before parallel provider reads.
    for namespace in namespaces {
        if let Err(error) = crate::auth::check_namespace(switchboard, &namespace) {
            let failure = error
                .downcast_ref::<switchboard_core::Error>()
                .map(Failure::from_error)
                .unwrap_or_else(|| Failure::from_error(&switchboard_core::Error::Config(error.to_string())));
            blocked.insert(namespace.clone(), failure.with_namespace(namespace));
        }
    }
    // Auth may exhaust the overall deadline. Save its blocker even when no
    // provider-read wave can start, retaining any pages from an earlier run.
    for item in &request.items {
        if let Some(failure) = blocked.get(&item.namespace) {
            let progress = checkpoint.items.entry(item.id.clone()).or_default();
            if !progress.finished {
                progress.failure = Some(failure.clone());
            }
        }
    }
    let mut attempted = BTreeMap::<String, u16>::new();
    loop {
        let pending = request
            .items
            .iter()
            .filter(|item| {
                !checkpoint
                    .items
                    .get(&item.id)
                    .is_some_and(|progress| progress.finished || progress.failure.is_some())
                    && attempted.get(&item.id).copied().unwrap_or(0) < args.max_pages
            })
            .take(usize::from(args.concurrency))
            .cloned()
            .collect::<Vec<_>>();
        if pending.is_empty() {
            break;
        }
        if switchboard_core::process::remaining_timeout(args.time_budget()).is_err() {
            break;
        }
        let results = std::thread::scope(|scope| {
            let handles = pending
                .iter()
                .map(|item| {
                    let progress = checkpoint.items.get(&item.id).cloned().unwrap_or_default();
                    let blocker = blocked.get(&item.namespace).cloned();
                    scope.spawn(move || read_page(switchboard, item, progress, blocker))
                })
                .collect::<Vec<_>>();
            handles
                .into_iter()
                .map(|handle| handle.join().map_err(|_| anyhow!("batch worker failed")))
                .collect::<Result<Vec<_>>>()
        })?;
        for (item, progress) in pending.iter().zip(results) {
            *attempted.entry(item.id.clone()).or_default() += 1;
            if let Some(failure) = &progress.failure {
                if failure.phase == switchboard_core::FailurePhase::Authentication {
                    blocked.insert(item.namespace.clone(), failure.clone());
                }
            }
            checkpoint.items.insert(item.id.clone(), progress);
        }
        write_checkpoint(&checkpoint_path, &checkpoint)?;
    }
    for item in &request.items {
        checkpoint.items.entry(item.id.clone()).or_default();
    }
    write_checkpoint(&checkpoint_path, &checkpoint)?;
    let complete = checkpoint.items.values().all(|progress| {
        progress.finished && progress.failure.is_none() && progress.coverage.status == CoverageStatus::Complete
    });
    let mut resume_argv = input::resume_argv(config_path, &checkpoint_path, &args)?;
    presentation.append_argv(&mut resume_argv);
    #[derive(Serialize)]
    struct Report<'a> {
        status: &'static str,
        elapsed_ms: u128,
        checkpoint: &'a Path,
        resume_argv: &'a [String],
        items: &'a BTreeMap<String, ItemProgress>,
    }
    let text = if args.json {
        presentation.json(&Report {
            status: if complete { "complete" } else { "partial" },
            elapsed_ms: started.elapsed().as_millis(),
            checkpoint: &checkpoint_path,
            resume_argv: &resume_argv,
            items: &checkpoint.items,
        })?
    } else {
        format!(
            "{}: {} items saved to {} in {} ms\nResume: {}\n",
            if complete { "Complete" } else { "Partial" },
            checkpoint.items.len(),
            checkpoint_path.display(),
            started.elapsed().as_millis(),
            resume_argv
                .iter()
                .map(|argument| input::shell_quote(argument))
                .collect::<Vec<_>>()
                .join(" ")
        )
    };
    crate::output::require_complete(text, complete)
}

fn validate(switchboard: &Switchboard, request: &BatchInput) -> Result<()> {
    if request.items.is_empty() || request.items.len() > 1000 {
        bail!("batch requires 1..1000 items");
    }
    let mut ids = BTreeSet::new();
    for item in &request.items {
        if item.id.trim().is_empty() || !ids.insert(&item.id) {
            bail!("batch item IDs must be nonempty and unique");
        }
        let tool = switchboard
            .describe_tool(&item.tool)?
            .ok_or_else(|| anyhow!("unknown tool {}", item.tool))?;
        if tool.kind != ToolKind::Read || tool.execution_support != ToolExecutionSupport::Executable {
            bail!("{} is not an executable read tool", item.tool);
        }
        if tool.surface != switchboard_core::ToolSurface::Curated {
            bail!(
                "{} is an unrestricted passthrough; batches require curated read contracts",
                item.tool
            );
        }
        let namespace = switchboard
            .list_namespaces()
            .into_iter()
            .find(|namespace| namespace.id == item.namespace)
            .ok_or_else(|| anyhow!("unknown namespace {}", item.namespace))?;
        if item.tool.provider()? != namespace.provider {
            bail!("tool and namespace provider differ for {}", item.id);
        }
    }
    Ok(())
}

fn read_page(
    switchboard: &Switchboard,
    item: &ReadItem,
    mut progress: ItemProgress,
    blocker: Option<Failure>,
) -> ItemProgress {
    progress.failure = blocker;
    if progress.failure.is_some() {
        return progress;
    }
    let mut arguments = item.args.clone();
    if let Some(cursor) = request_cursor(item, &progress) {
        arguments.retain(|argument| argument.name() != "cursor");
        arguments.push(ToolArgument::Option {
            name: "cursor".into(),
            value: cursor,
        });
    }
    let mut result = ToolRequest::new(
        item.tool.as_str(),
        item.namespace.as_str(),
        ExecutionMode::Auto,
        arguments.clone(),
    )
    .and_then(|request| switchboard.dispatch(request));
    if let Err(switchboard_core::Error::RateLimited {
        retry_after_seconds, ..
    }) = &result
    {
        let delay = Duration::from_secs(*retry_after_seconds);
        if *retry_after_seconds <= 60
            && switchboard_core::process::remaining_timeout(delay + Duration::from_secs(1))
                .is_ok_and(|remaining| remaining > delay)
        {
            std::thread::sleep(delay);
            result = ToolRequest::new(
                item.tool.as_str(),
                item.namespace.as_str(),
                ExecutionMode::Auto,
                arguments,
            )
            .and_then(|request| switchboard.dispatch(request));
        }
    }
    match result {
        Ok(DispatchOutcome::Executed(output)) => accept_page(item, &mut progress, output),
        Ok(DispatchOutcome::Planned(_)) => {
            progress.failure = Some(
                Failure::from_error(&switchboard_core::Error::PolicyDenied(
                    "read batch did not execute".into(),
                ))
                .with_namespace(item.namespace.clone()),
            )
        }
        Err(error) => progress.failure = Some(Failure::from_error(&error).with_namespace(item.namespace.clone())),
    }
    progress
}

fn accept_page(item: &ReadItem, progress: &mut ItemProgress, output: ToolOutput) {
    let coverage = output.coverage.clone().unwrap_or(ReadCoverage {
        status: CoverageStatus::Unknown,
        next_cursor: None,
    });
    let requested_cursor = request_cursor(item, progress);
    let replacing_partial = has_partial_page(progress);
    let retained = progress.pages.len() - usize::from(replacing_partial);
    let repeated_cursor = coverage.next_cursor.is_some()
        && (coverage.next_cursor == requested_cursor
            || progress.pages[..retained].iter().any(|page| {
                page.output
                    .coverage
                    .as_ref()
                    .is_some_and(|previous| previous.next_cursor == coverage.next_cursor)
            }));
    if repeated_cursor {
        progress.failure = Some(
            Failure::from_error(&switchboard_core::Error::Execution(
                "provider repeated a continuation token".into(),
            ))
            .with_namespace(item.namespace.clone()),
        );
        return;
    }
    let authentication_failure = output
        .fields
        .get("failures")
        .and_then(|value| serde_json::from_value::<Vec<Failure>>(value.clone()).ok())
        .and_then(|failures| {
            failures
                .into_iter()
                .find(|failure| failure.phase == switchboard_core::FailurePhase::Authentication)
        });
    progress.failure = None;
    if coverage.status == CoverageStatus::Unknown && item.tool.as_str() == "google.mail.search" {
        progress.failure = authentication_failure.or_else(|| {
            Some(
                Failure::from_error(&switchboard_core::Error::Execution(
                    "page metadata was incomplete; retry this page after resolving its failures".into(),
                ))
                .with_namespace(item.namespace.clone()),
            )
        });
    }
    // A failed resume must not erase evidence saved by the previous attempt.
    // Replace a partial page only after its original request succeeds fully.
    if replacing_partial && coverage.status == CoverageStatus::Unknown {
        return;
    }
    progress.finished = coverage.next_cursor.is_none() && coverage.status != CoverageStatus::Unknown;
    if item.tool.as_str() != "google.mail.search" {
        progress.finished = true;
    }
    progress.pages.truncate(retained);
    progress.coverage = coverage;
    progress.pages.push(PageReceipt {
        requested_cursor,
        output,
    });
}

fn has_partial_page(progress: &ItemProgress) -> bool {
    progress.pages.last().is_some_and(|page| {
        page.output
            .coverage
            .as_ref()
            .is_some_and(|coverage| coverage.status == CoverageStatus::Unknown)
    })
}

fn request_cursor(item: &ReadItem, progress: &ItemProgress) -> Option<String> {
    if has_partial_page(progress) {
        return progress.pages.last().and_then(|page| page.requested_cursor.clone());
    }
    progress.coverage.next_cursor.clone().or_else(|| {
        if progress.pages.is_empty() {
            initial_cursor(item)
        } else {
            None
        }
    })
}

fn initial_cursor(item: &ReadItem) -> Option<String> {
    item.args
        .iter()
        .filter(|argument| argument.name() == "cursor")
        .filter_map(ToolArgument::value)
        .next_back()
        .map(str::to_owned)
}

fn validate_progress(item: &ReadItem, progress: &ItemProgress) -> Result<()> {
    let mut expected_cursor = initial_cursor(item);
    let mut seen = BTreeSet::new();
    let mut expected_coverage = ReadCoverage::default();
    for (index, page) in progress.pages.iter().enumerate() {
        if page.output.tool != item.tool
            || page.output.namespace != item.namespace
            || page.requested_cursor != expected_cursor
        {
            bail!(
                "checkpoint page does not match request identity or cursor for {}",
                item.id
            );
        }
        let coverage = page.output.coverage.clone().unwrap_or_default();
        if (coverage.status == CoverageStatus::Complete && coverage.next_cursor.is_some())
            || (coverage.status == CoverageStatus::Truncated && coverage.next_cursor.is_none())
        {
            bail!("checkpoint contains contradictory coverage for {}", item.id);
        }
        if coverage
            .next_cursor
            .as_ref()
            .is_some_and(|cursor| Some(cursor) == expected_cursor.as_ref() || !seen.insert(cursor.clone()))
        {
            bail!("checkpoint repeats a continuation for {}", item.id);
        }
        if index + 1 < progress.pages.len() && coverage.status != CoverageStatus::Truncated {
            bail!("checkpoint continued an incomplete or finished page for {}", item.id);
        }
        expected_cursor = coverage.next_cursor.clone();
        expected_coverage = coverage;
    }
    if progress.coverage != expected_coverage {
        bail!("checkpoint summary does not match saved page coverage for {}", item.id);
    }
    let finished = !progress.pages.is_empty()
        && progress.failure.is_none()
        && (item.tool.as_str() != "google.mail.search" || expected_coverage.status == CoverageStatus::Complete);
    if progress.finished != finished {
        bail!("checkpoint completion is not supported by saved pages for {}", item.id);
    }
    Ok(())
}

fn write_checkpoint(path: &Path, checkpoint: &Checkpoint) -> Result<()> {
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary).context("create private checkpoint")?;
    let result = (|| {
        serde_json::to_writer_pretty(&mut file, checkpoint)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

struct CheckpointLock(PathBuf);
impl CheckpointLock {
    fn acquire(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let lock = path.with_extension("lock");
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock)
            .with_context(|| {
                format!(
                    "checkpoint is locked: {}; remove the lock only after its owner has stopped",
                    lock.display()
                )
            })?;
        Ok(Self(lock))
    }
}
impl Drop for CheckpointLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn item() -> ReadItem {
        ReadItem {
            id: "mail".into(),
            tool: ToolName::new("google.mail.search").expect("test setup should succeed"),
            namespace: NamespaceId::new("google.test").expect("test setup should succeed"),
            args: vec![ToolArgument::option("query", "receipt").expect("test setup should succeed")],
        }
    }

    fn page(coverage: ReadCoverage) -> ToolOutput {
        let item = item();
        let mut output = ToolOutput::new(item.tool, item.namespace, "search page");
        output.coverage = Some(coverage);
        output
    }

    #[test]
    fn incomplete_pages_and_repeated_cursors_cannot_finish_a_batch_item() {
        let mut progress = ItemProgress::default();
        accept_page(&item(), &mut progress, page(ReadCoverage::page(Some("page2".into()))));
        assert!(!progress.finished);
        assert_eq!(progress.pages.len(), 1);
        accept_page(&item(), &mut progress, page(ReadCoverage::page(Some("page2".into()))));
        assert!(!progress.finished);
        assert!(progress.failure.is_some());
        assert_eq!(progress.pages.len(), 1);
        let mut incomplete = ItemProgress::default();
        accept_page(&item(), &mut incomplete, page(ReadCoverage::default()));
        assert!(!incomplete.finished);
        assert!(incomplete.failure.is_some());
        assert_eq!(incomplete.pages.len(), 1);
        let mut complete = ItemProgress::default();
        accept_page(&item(), &mut complete, page(ReadCoverage::page(None)));
        assert!(complete.finished);
        assert!(complete.failure.is_none());
    }

    #[test]
    fn checkpoint_roundtrip_preserves_partial_pages_and_exclusive_ownership() {
        let directory = std::env::temp_dir().join(format!(
            "switchboard-checkpoint-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("test setup should succeed")
                .as_nanos()
        ));
        let path = directory.join("checkpoint.json");
        let lock = CheckpointLock::acquire(&path).expect("test setup should succeed");
        assert!(CheckpointLock::acquire(&path).is_err());
        let mut progress = ItemProgress::default();
        accept_page(&item(), &mut progress, page(ReadCoverage::page(Some("next".into()))));
        let checkpoint = Checkpoint {
            schema_version: 1,
            request: BatchInput { items: vec![item()] },
            items: BTreeMap::from([("mail".into(), progress)]),
        };
        write_checkpoint(&path, &checkpoint).expect("test setup should succeed");
        let restored: Checkpoint = serde_json::from_slice(&fs::read(&path).expect("test setup should succeed"))
            .expect("test setup should succeed");
        assert_eq!(restored.request, checkpoint.request);
        let restored = restored.items.get("mail").expect("test setup should succeed");
        assert!(!restored.finished);
        assert_eq!(restored.coverage.next_cursor.as_deref(), Some("next"));
        assert_eq!(restored.pages.len(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path)
                    .expect("test setup should succeed")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        drop(lock);
        assert!(CheckpointLock::acquire(&path).is_ok());
        fs::remove_dir_all(directory).expect("test setup should succeed");
    }

    #[test]
    fn resume_requires_page_evidence_and_matching_identity() {
        let mut forged = ItemProgress {
            finished: true,
            coverage: ReadCoverage::page(None),
            ..Default::default()
        };
        assert!(validate_progress(&item(), &forged).is_err());
        accept_page(&item(), &mut forged, page(ReadCoverage::page(None)));
        assert!(validate_progress(&item(), &forged).is_ok());
        forged.pages[0].output.namespace = NamespaceId::new("google.other").expect("namespace should build");
        assert!(validate_progress(&item(), &forged).is_err());
    }

    #[test]
    fn partial_metadata_keeps_authentication_failure_for_account_stop() {
        let mut output = page(ReadCoverage::default());
        let failure = Failure::from_error(&switchboard_core::Error::AuthenticationRejected {
            reason: "revoked".into(),
        })
        .with_namespace(item().namespace);
        output.fields.insert(
            "failures".into(),
            serde_json::to_value(vec![failure.clone()]).expect("failure should serialize"),
        );
        let mut progress = ItemProgress::default();
        accept_page(&item(), &mut progress, output);
        assert_eq!(progress.failure, Some(failure));
        assert!(!progress.finished);
        assert!(validate_progress(&item(), &progress).is_ok());
    }
}

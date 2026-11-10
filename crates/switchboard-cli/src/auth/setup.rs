use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, bail, Context, Result};
use clap::Args;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use switchboard_core::{
    AuthKind, AuthSecretRefs, AuthStore, DispatchOutcome, ExecutionMode, GoogleDelegation, NamespaceId, NamespaceStore,
    ProviderKind, ToolArgument, ToolRequest,
};
use switchboard_providers::delegation;
use switchboard_store::SwitchboardConfig;

#[derive(Debug, Args)]
pub(super) struct SetupArgs {
    #[arg(long = "ns")]
    namespace: String,
    /// Set up sensitive Gmail settings when a Workspace administrator is required.
    #[arg(long)]
    setup: bool,
    /// Apply the previously displayed Cloud-resource and signer-permission plan.
    #[arg(long)]
    apply: bool,
    /// Optional existing Cloud project. Normally inferred from this account's login.
    #[arg(long)]
    project: Option<String>,
    /// Renew the ordinary OAuth login without changing delegation or its scopes.
    #[arg(long)]
    refresh: bool,
    #[arg(long)]
    pub(super) json: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SetupState {
    version: u32,
    namespace: String,
    config_path: PathBuf,
    original_auth: String,
    delegation: GoogleDelegation,
    client_id: Option<String>,
    ready: bool,
}

#[derive(Serialize)]
struct Receipt<'a> {
    namespace: &'a str,
    status: &'a str,
    account: &'a str,
    project: &'a str,
    service_account: &'a str,
    signer: &'a str,
    scopes: [&'static str; 2],
    client_id: Option<&'a str>,
    admin_url: &'static str,
    next_step: String,
    explanation: &'static str,
}

const EXPLANATION: &str = "Regular Google login stays unchanged. Applying first verifies Google-hosted domain membership; an email suffix alone is not proof. This optional keyless capability manages sensitive Gmail settings. Google grants authority across the Workspace domain; Switchboard fixes it to this account and these settings. A super administrator must authorize the numeric client ID and scopes once, with second-admin approval if required. Consumer Google accounts cannot use this API.";

fn receipt(state: &SetupState, status: &str, next: String, json: bool) -> Result<String> {
    let value = Receipt {
        namespace: &state.namespace,
        status,
        account: &state.delegation.subject,
        project: &state.delegation.project,
        service_account: &state.delegation.service_account,
        signer: &state.delegation.signer_account,
        scopes: GoogleDelegation::SCOPES,
        client_id: state.client_id.as_deref(),
        admin_url: delegation::ADMIN_URL,
        next_step: next,
        explanation: EXPLANATION,
    };
    if json {
        return crate::output::render_json(&value, true);
    }
    if state.ready {
        return Ok(format!(
            "{}: ready\nAccount: {}\n{}\n",
            state.namespace, value.account, value.next_step
        ));
    }
    Ok(format!("{}: {}\nAccount: {}\nCloud project: {}\nService account: {}\nRuntime signer: {}\n\n{}\n\n{}\n{}\n\n{}\n",
        state.namespace, status.replace('_', " "), value.account, value.project, value.service_account, value.signer,
        EXPLANATION,
        state.client_id.as_ref().map(|id| format!("Client ID: {id}\nScopes: {}", GoogleDelegation::SCOPES.join(","))).unwrap_or_else(|| format!(
            "Plan: enable Gmail, IAM and IAM Credentials APIs in the selected existing project; create or reuse only this Switchboard service account; grant this signer Token Creator on that account only. No downloadable private key and no project-wide role grants.\nWorkspace scopes: {}", GoogleDelegation::SCOPES.join(",")
        )),
        if state.client_id.is_some() { format!("Administrator authorization: {}", delegation::ADMIN_URL) } else { String::new() },
        value.next_step))
}

fn save(path: &Path, contents: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or_else(|| anyhow!("setup file has no parent"))?;
    fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(contents)?;
    temp.as_file().sync_all()?;
    temp.persist(path)
        .map_err(|_| anyhow!("could not atomically save setup state"))?;
    Ok(())
}

fn save_state(path: &Path, state: &SetupState) -> Result<()> {
    save(path, &serde_json::to_vec_pretty(state)?)
}

fn infer_project(state_dir: &Path) -> Result<String> {
    #[derive(Deserialize)]
    struct ClientFile {
        installed: Option<ProjectId>,
        web: Option<ProjectId>,
    }
    #[derive(Deserialize)]
    struct ProjectId {
        project_id: Option<String>,
    }
    // This is the OAuth client configuration, not a credential export. Parse
    // only the non-secret project ID and never echo deserialization contents.
    fs::read(state_dir.join("client_secret.json")).ok()
        .and_then(|bytes| serde_json::from_slice::<ClientFile>(&bytes).ok())
        .and_then(|file| file.installed.or(file.web))
        .and_then(|file| file.project_id)
        .ok_or_else(|| anyhow!("The Cloud project could not be inferred from this account's login. Specify --project PROJECT_ID once; normal login needs no change."))
}

fn with_signer_login<T>(path: &Path, namespace: &str, action: impl Fn() -> switchboard_core::Result<T>) -> Result<T> {
    match action() {
        Err(switchboard_core::Error::AuthenticationRejected { .. }) => {
            ordinary_login(path, namespace, false)?;
            Ok(action()?)
        }
        result => Ok(result?),
    }
}

fn ordinary_login(config_path: &Path, namespace: &str, json: bool) -> Result<String> {
    let switchboard = crate::load_switchboard(Some(config_path))?;
    let request = ToolRequest::new(
        "google.cli.write",
        namespace,
        ExecutionMode::Draft,
        vec![ToolArgument::option(
            "argv-json",
            serde_json::to_string(&["auth", "login"])?,
        )?],
    )?;
    // Invoking `auth login` explicitly authorizes this exact provider login;
    // retain normal plan, approval, account verification and audit machinery.
    let output = crate::approve_and_apply(&switchboard, request)?;
    let outcome = DispatchOutcome::Executed(output);
    if json {
        crate::output::render_json_dispatch(&outcome)
    } else {
        Ok(crate::output::render_dispatch_human(&outcome))
    }
}

pub(super) fn check_delegated(config: &GoogleDelegation, namespace: &NamespaceId) -> Result<super::AuthReceipt> {
    delegation::verify(config)?;
    Ok(super::AuthReceipt {
        namespace: namespace.clone(),
        provider: ProviderKind::GoogleWorkspace,
        expected_account: config.subject.clone(),
        authenticated_account: config.subject.clone(),
        identity_verified: true,
    })
}

pub(super) fn run(config_path: Option<&Path>, args: SetupArgs, setup_command: bool) -> Result<String> {
    let path = fs::canonicalize(crate::resolve_config_path(config_path)?)?;
    let config = SwitchboardConfig::from_file(&path)?;
    let (namespaces, auth, _) = config.into_stores();
    let id = NamespaceId::new(args.namespace.clone())?;
    let namespace = namespaces.get(&id).ok_or_else(|| anyhow!("unknown namespace {id}"))?;
    if namespace.provider != ProviderKind::GoogleWorkspace {
        bail!("auth login currently supports Google namespaces");
    }
    let current = auth
        .get(&namespace.auth_ref)
        .ok_or_else(|| anyhow!("namespace has no configured authentication"))?;
    let state_dir = namespace
        .state_dir
        .as_ref()
        .ok_or_else(|| anyhow!("Google namespace requires its isolated state directory"))?;
    let checkpoint = state_dir.join("switchboard-gmail-settings-setup.json");
    let configured = match current.secrets() {
        AuthSecretRefs::GoogleDelegated { delegation } => Some(delegation.clone()),
        _ => None,
    };
    if args.refresh {
        if args.apply || args.setup || setup_command {
            bail!("--refresh cannot be combined with setup or --apply");
        }
        return ordinary_login(&path, &args.namespace, args.json);
    }
    if !setup_command && !args.setup && configured.is_none() && !checkpoint.is_file() {
        if args.apply || args.project.is_some() {
            bail!("--apply and --project apply only to a displayed administrator setup plan; use --setup once when Gmail settings require it");
        }
        return ordinary_login(&path, &args.namespace, args.json);
    }
    if !matches!(current.kind(), AuthKind::GoogleCli | AuthKind::GoogleDelegated) {
        bail!("Guided Workspace settings setup requires a namespace-managed Google login. Existing explicit OAuth credentials were preserved. Run `switchboard auth migration-preview --ns {}` to review a supported migration first.", args.namespace);
    }
    let mut state = if checkpoint.is_file() {
        let state: SetupState = serde_json::from_slice(&fs::read(&checkpoint)?)
            .map_err(|_| anyhow!("saved setup state is invalid; no Cloud resources were changed"))?;
        state.delegation.validate()?;
        if state.version != 1
            || state.namespace != args.namespace
            || state.config_path != path
            || !state.delegation.subject.eq_ignore_ascii_case(current.account_label())
            || state.delegation.signer_state_dir != *state_dir
            || configured.as_ref().is_some_and(|config| config != &state.delegation)
            || args
                .project
                .as_ref()
                .is_some_and(|project| project != &state.delegation.project)
            || (configured.is_none() && state.original_auth != namespace.auth_ref.as_str())
        {
            bail!("saved setup no longer matches this namespace, account or project; no Cloud resources were changed");
        }
        state
    } else if let Some(delegation) = configured {
        SetupState {
            version: 1,
            namespace: args.namespace.clone(),
            config_path: path.clone(),
            original_auth: namespace.auth_ref.to_string(),
            delegation,
            client_id: None,
            ready: true,
        }
    } else {
        let subject = current.account_label().to_owned();
        let domain = subject
            .rsplit_once('@')
            .ok_or_else(|| anyhow!("configure the account's actual email before administrator setup"))?
            .1
            .to_owned();
        let project = args
            .project
            .clone()
            .map(Ok)
            .unwrap_or_else(|| infer_project(state_dir))?;
        let digest: String = Sha256::digest(format!("{}\0{}", args.namespace, subject).as_bytes())
            .iter()
            .take(4)
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let delegation = GoogleDelegation {
            service_account: format!("switchboard-settings-{}@{project}.iam.gserviceaccount.com", digest),
            project,
            domain,
            subject: subject.clone(),
            signer_account: subject,
            signer_state_dir: state_dir.clone(),
        };
        delegation.validate()?;
        let state = SetupState {
            version: 1,
            namespace: args.namespace.clone(),
            config_path: path.clone(),
            original_auth: namespace.auth_ref.to_string(),
            delegation,
            client_id: None,
            ready: false,
        };
        save_state(&checkpoint, &state)?;
        return receipt(
            &state,
            "plan_ready",
            format!(
                "Review this plan, then run: switchboard auth login --ns {} --apply",
                args.namespace
            ),
            args.json,
        );
    };
    if state.client_id.is_none() && !state.ready {
        if !args.apply {
            return receipt(
                &state,
                "plan_ready",
                format!(
                    "Apply this saved plan: switchboard auth login --ns {} --apply",
                    args.namespace
                ),
                args.json,
            );
        }
        let grant = with_signer_login(&path, &args.namespace, || delegation::provision(&state.delegation))?;
        state.client_id = Some(grant.client_id);
        save_state(&checkpoint, &state)?;
        return receipt(&state, "awaiting_administrator", format!("A Workspace super administrator must add the client ID and scopes at the link above. Then run: switchboard auth login --ns {}\nGoogle may require a second administrator and up to 24 hours to propagate. No additional Cloud setup is needed.", args.namespace), args.json);
    }
    with_signer_login(&path, &args.namespace, || delegation::verify(&state.delegation)).with_context(|| format!("Setup is saved and resumable. Confirm the grant at {} and retry `switchboard auth login --ns {}`. If the signer session needs renewal, use --refresh. No Cloud resources were created or permissions broadened by this verification.", delegation::ADMIN_URL, args.namespace))?;
    install_config(&path, &state)?;
    state.ready = true;
    save_state(&checkpoint, &state)?;
    receipt(&state, "ready", "Gmail settings can now use the same namespace. Ordinary mail, Drive, Calendar and login retain user OAuth. Administrator authorization is not repeated on normal login. No private keys or delegated tokens were saved.".into(), args.json)
}

#[derive(Deserialize)]
struct EditConfig {
    namespace: BTreeMap<String, BTreeMap<String, EditNamespace>>,
}
#[derive(Deserialize)]
struct EditNamespace {
    provider: toml::Spanned<String>,
    auth: Option<toml::Spanned<String>>,
}
#[derive(Serialize)]
struct AuthEntry<'a> {
    provider: &'static str,
    kind: &'static str,
    account: &'a str,
    delegation: &'a GoogleDelegation,
}

fn updated_config(source: &str, state: &SetupState) -> Result<String> {
    let parsed = SwitchboardConfig::from_toml_str_at(source, &state.config_path)?;
    let (namespaces, auth, _) = parsed.into_stores();
    let id = NamespaceId::new(state.namespace.clone())?;
    let ns = namespaces
        .get(&id)
        .ok_or_else(|| anyhow!("namespace was removed during setup"))?;
    let current = auth
        .get(&ns.auth_ref)
        .ok_or_else(|| anyhow!("namespace authentication disappeared"))?;
    if let AuthSecretRefs::GoogleDelegated { delegation } = current.secrets() {
        if delegation == &state.delegation {
            return Ok(source.to_owned());
        }
        bail!("another delegation was configured while setup was running");
    }
    if current.kind() != AuthKind::GoogleCli {
        bail!(
            "Existing explicit OAuth credentials were preserved; this setup requires a namespace-managed Google login"
        );
    }
    if ns.auth_ref.as_str() != state.original_auth
        || !current.account_label().eq_ignore_ascii_case(&state.delegation.subject)
        || ns.state_dir.as_ref() != Some(&state.delegation.signer_state_dir)
    {
        bail!(
            "namespace account, credentials or state directory changed during setup; configuration was not overwritten"
        );
    }
    let editing: EditConfig =
        toml::from_str(source).map_err(|_| anyhow!("cannot locate namespace in configuration"))?;
    let alias = state
        .namespace
        .strip_prefix("google.")
        .ok_or_else(|| anyhow!("invalid Google namespace"))?;
    let table = editing
        .namespace
        .get("google")
        .and_then(|table| table.get(alias))
        .ok_or_else(|| anyhow!("cannot locate Google namespace"))?;
    let key = format!("{}_gmail_settings", state.original_auth);
    if auth.list().iter().any(|entry| entry.id().as_str() == key) {
        bail!("the planned delegation auth name is already in use; configuration was not changed");
    }
    let literal = toml::Value::String(key.clone()).to_string();
    let mut result = source.to_owned();
    if let Some(field) = &table.auth {
        result.replace_range(field.span(), &literal);
    } else {
        let start = source[..table.provider.span().start]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        if source[start..table.provider.span().start].contains(['{', '[']) {
            bail!("administrator setup requires this namespace to use a normal TOML table, not an inline table");
        }
        result.insert_str(start, &format!("auth = {literal}\n"));
    }
    let entry = AuthEntry {
        provider: "google",
        kind: "google_delegated",
        account: &state.delegation.subject,
        delegation: &state.delegation,
    };
    let entries = BTreeMap::from([("auth", BTreeMap::from([(key, entry)]))]);
    result.push('\n');
    result.push_str(&toml::to_string_pretty(&entries)?);
    SwitchboardConfig::from_toml_str_at(&result, &state.config_path)?;
    Ok(result)
}

fn install_config(path: &Path, state: &SetupState) -> Result<()> {
    let source = fs::read_to_string(path)?;
    let updated = updated_config(&source, state)?;
    if updated == source {
        return Ok(());
    }
    if fs::read_to_string(path)? != source {
        bail!("configuration changed during verification; resume setup without overwriting it");
    }
    save(path, updated.as_bytes())
}

#[cfg(test)]
mod tests {
    use crate::auth::setup::{install_config, receipt, updated_config, SetupState};
    use std::fs;
    use switchboard_core::{AuthSecretRefs, AuthStore, GoogleDelegation, NamespaceId, NamespaceStore};
    use switchboard_store::SwitchboardConfig;

    fn state(root: &std::path::Path) -> SetupState {
        SetupState {
            version: 1,
            namespace: "google.work".into(),
            config_path: root.join("switchboard.toml"),
            original_auth: "shared".into(),
            delegation: GoogleDelegation {
                project: "example-project".into(),
                service_account: "settings@example-project.iam.gserviceaccount.com".into(),
                domain: "example.com".into(),
                subject: "person@example.com".into(),
                signer_account: "person@example.com".into(),
                signer_state_dir: root.join("login"),
            },
            client_id: Some("12345".into()),
            ready: false,
        }
    }
    const SOURCE: &str = r#"# A comment and existing login must survive
[auth.shared]
provider = "google"
kind = "google_cli"
account = "person@example.com"

[namespace.google.work]
provider = "google"
account = "Work"
auth = "shared" # Keep this comment too
state_dir = "login"

[namespace.google.other]
provider = "google"
account = "Other"
auth = "shared"
state_dir = "other-login"
"#;
    #[test]
    fn config_install_preserves_shared_oauth_comments_relative_paths_and_is_idempotent() {
        let root = tempfile::tempdir().expect("test fixture should succeed");
        let state = state(root.path());
        fs::write(&state.config_path, SOURCE).expect("test fixture should succeed");
        install_config(&state.config_path, &state).expect("test fixture should succeed");
        let installed = fs::read_to_string(&state.config_path).expect("test fixture should succeed");
        assert!(installed.contains("auth = \"shared_gmail_settings\" # Keep this comment too"));
        assert!(installed.starts_with("# A comment and existing login must survive"));
        let (namespaces, auth, _) = SwitchboardConfig::from_file(&state.config_path)
            .expect("test fixture should succeed")
            .into_stores();
        let work = namespaces
            .get(&NamespaceId::new("google.work").expect("test fixture should succeed"))
            .expect("test fixture should succeed");
        let other = namespaces
            .get(&NamespaceId::new("google.other").expect("test fixture should succeed"))
            .expect("test fixture should succeed");
        assert_eq!(work.state_dir.as_ref(), Some(&state.delegation.signer_state_dir));
        assert!(
            matches!(auth.get(&work.auth_ref).expect("test fixture should succeed").secrets(), AuthSecretRefs::GoogleDelegated { delegation } if delegation == &state.delegation)
        );
        assert!(matches!(
            auth.get(&other.auth_ref)
                .expect("test fixture should succeed")
                .secrets(),
            AuthSecretRefs::GoogleCli
        ));
        install_config(&state.config_path, &state).expect("test fixture should succeed");
        assert_eq!(
            fs::read_to_string(&state.config_path).expect("test fixture should succeed"),
            installed
        );
        assert!(!installed.contains("private_key"));
        assert!(!installed.contains("refresh_token"));
    }
    #[test]
    fn config_install_refuses_a_changed_account_or_state_directory() {
        let root = tempfile::tempdir().expect("test fixture should succeed");
        let state = state(root.path());
        for source in [
            SOURCE.replace("person@example.com", "another@example.com"),
            SOURCE.replace("state_dir = \"login\"", "state_dir = \"changed\""),
        ] {
            fs::write(&state.config_path, &source).expect("test fixture should succeed");
            assert!(install_config(&state.config_path, &state).is_err());
            assert_eq!(
                fs::read_to_string(&state.config_path).expect("test fixture should succeed"),
                source
            );
        }
    }
    #[test]
    fn implicit_auth_can_be_upgraded_without_rewriting_the_namespace() {
        let root = tempfile::tempdir().expect("test fixture should succeed");
        let mut state = state(root.path());
        let source =
            "[namespace.google.work]\nprovider = \"google\"\naccount = \"person@example.com\"\nstate_dir = \"login\"\n";
        let (namespaces, _, _) = SwitchboardConfig::from_toml_str_at(source, &state.config_path)
            .expect("test fixture should succeed")
            .into_stores();
        state.original_auth = namespaces
            .get(&NamespaceId::new("google.work").expect("test fixture should succeed"))
            .expect("test fixture should succeed")
            .auth_ref
            .to_string();
        let updated = updated_config(source, &state).expect("test fixture should succeed");
        let (namespaces, auth, _) = SwitchboardConfig::from_toml_str_at(&updated, &state.config_path)
            .expect("test fixture should succeed")
            .into_stores();
        let work = namespaces
            .get(&NamespaceId::new("google.work").expect("test fixture should succeed"))
            .expect("test fixture should succeed");
        assert!(matches!(
            auth.get(&work.auth_ref).expect("test fixture should succeed").secrets(),
            AuthSecretRefs::GoogleDelegated { .. }
        ));
    }
    #[test]
    fn ready_login_does_not_repeat_administrator_instructions() {
        let root = tempfile::tempdir().expect("test fixture should succeed");
        let mut state = state(root.path());
        state.ready = true;
        let rendered = receipt(&state, "ready", "Ready to use.".into(), false).expect("test fixture should succeed");
        assert!(rendered.contains("ready"));
        assert!(!rendered.contains("administrator"));
    }
}

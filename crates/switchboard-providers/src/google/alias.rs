use serde::{de::DeserializeOwned, Deserialize, Serialize};
use switchboard_core::{
    AuthSecretRefs, Error, ExecutionTarget, GoogleDelegation, PlannedAction, PlanningTarget, ResolvedCredentials,
    Result, StoredOperation, ToolArguments, ToolOutput, ToolRequest, VerificationReceipt, VerificationStatus,
};

use crate::{cli::CliStdioMode, google::GoogleWorkspaceAdapter};

const SENDERS: &[&str] = &["gmail", "users", "settings", "sendAs"];

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MailboxParams<'a> {
    user_id: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DirectoryParams<'a> {
    user_key: &'a str,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Senders {
    send_as: Vec<Sender>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Sender {
    send_as_email: String,
    verification_status: Option<SenderVerification>,
    #[serde(default)]
    is_primary: bool,
    #[serde(default)]
    is_default: bool,
    reply_to_address: Option<String>,
    #[serde(default)]
    treat_as_alias: bool,
}

#[derive(Clone, Copy, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum SenderVerification {
    Accepted,
    Pending,
    #[serde(other)]
    Unspecified,
}

#[derive(Clone, Copy, PartialEq)]
enum Readiness {
    Accepted,
    Pending,
}

impl Sender {
    fn readiness(&self) -> Result<Readiness> {
        // Gmail omits verificationStatus on the primary address. The caller
        // validates that identity against the fixed delegated subject first.
        if self.is_primary {
            return Ok(Readiness::Accepted);
        }
        match self.verification_status {
            Some(SenderVerification::Accepted) => Ok(Readiness::Accepted),
            Some(SenderVerification::Pending) => Ok(Readiness::Pending),
            _ => Err(Error::Execution(format!(
                "Gmail sender {} has no known verification status; readiness cannot be established",
                self.send_as_email
            ))),
        }
    }
}

enum AliasApi {
    Directory,
    SendAs,
}

#[derive(Deserialize)]
struct DirectoryAliases {
    #[serde(default)]
    aliases: Vec<DirectoryAlias>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DirectoryAlias {
    alias: String,
    // Insert may omit this; list/readback must still establish ownership.
    primary_email: Option<String>,
}

#[derive(Serialize)]
struct NewDirectoryAlias<'a> {
    alias: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NewSender<'a> {
    send_as_email: &'a str,
    reply_to_address: &'a str,
    treat_as_alias: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_name: Option<&'a str>,
}

pub(super) fn validate_request(target: &PlanningTarget, request: &ToolRequest) -> Result<()> {
    let AuthSecretRefs::GoogleDelegated { delegation } = target.auth.secrets() else {
        return Err(setup_error(target.namespace.id.as_str()));
    };
    email(&request.args, delegation)?;
    Ok(())
}

fn setup_error(namespace: &str) -> Error {
    Error::UnsupportedOperation(format!(
        "This alias workflow requires Workspace administrator authorization, not more ordinary OAuth scopes. Run `switchboard auth login --ns {namespace} --setup` for guided setup. Consumer Google accounts cannot use this API."
    ))
}

fn email<'a>(args: &'a ToolArguments, delegation: &GoogleDelegation) -> Result<&'a str> {
    delegation.validate()?;
    let email = args
        .value("email")
        .ok_or_else(|| Error::InvalidArguments("missing --email".into()))?;
    let (local, domain) = email
        .split_once('@')
        .ok_or_else(|| Error::InvalidArguments("--email must be a same-domain email address".into()))?;
    if local.is_empty()
        || local.starts_with('.')
        || local.ends_with('.')
        || local.contains("..")
        || !local
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b".!#$%&'*+-/=?^_`{|}~".contains(&c))
        || !domain.eq_ignore_ascii_case(&delegation.domain)
    {
        return Err(Error::InvalidArguments(
            "--email must be an unquoted address in the configured Workspace subject's domain".into(),
        ));
    }
    Ok(email)
}

fn delegation(target: &ExecutionTarget) -> Result<&GoogleDelegation> {
    match &target.credentials {
        ResolvedCredentials::GoogleDelegated { delegation } => Ok(delegation),
        _ => Err(setup_error(target.namespace.id.as_str())),
    }
}

impl Senders {
    fn validate_account(&self, account: &str) -> Result<()> {
        let mut primaries = self.send_as.iter().filter(|sender| sender.is_primary);
        if !primaries
            .next()
            .is_some_and(|sender| sender.send_as_email.eq_ignore_ascii_case(account))
            || primaries.next().is_some()
        {
            return Err(Error::Execution(
                "Gmail primary identity does not match the configured delegated subject".into(),
            ));
        }
        Ok(())
    }

    fn find(&self, email: &str) -> Option<&Sender> {
        self.send_as
            .iter()
            .find(|sender| sender.send_as_email.eq_ignore_ascii_case(email))
    }
}

impl DirectoryAliases {
    fn contains(&self, email: &str, account: &str) -> Result<bool> {
        if self.aliases.iter().any(|alias| {
            !alias
                .primary_email
                .as_deref()
                .is_some_and(|owner| owner.eq_ignore_ascii_case(account))
        }) {
            return Err(Error::Execution(
                "Directory alias ownership does not match the configured delegated subject".into(),
            ));
        }
        Ok(self.aliases.iter().any(|alias| alias.alias.eq_ignore_ascii_case(email)))
    }
}

impl GoogleWorkspaceAdapter {
    pub(super) fn ensure_mail_alias(&self, target: &ExecutionTarget, action: &PlannedAction) -> Result<ToolOutput> {
        let delegation = delegation(target)?;
        let email = email(&action.args, delegation)?;
        if action.operation_id.is_none() {
            return Err(Error::InvalidArguments(
                "alias setup requires a durable operation ID".into(),
            ));
        }
        let mut write_attempted = false;
        let result: Result<ToolOutput> = (|| {
            // Exercise the fixed-subject Gmail credential before any Directory write.
            // An existing sender is authoritative, even when catch-all routing supplies mail.
            let senders = self.mail_alias_senders(target, &delegation.subject)?;
            let (sender_status, receiving_status, verification) = if let Some(sender) = senders.find(email) {
                ("existing", "not_checked", sender.readiness()?)
            } else {
                let params = DirectoryParams {
                    user_key: &delegation.subject,
                };
                let aliases: DirectoryAliases = self.alias_call(
                    target,
                    AliasApi::Directory,
                    "list",
                    &params,
                    None::<&NewDirectoryAlias<'_>>,
                )?;
                let receiving_status = if aliases.contains(email, &delegation.subject)? {
                    "existing"
                } else {
                    // Keep the receiving alias on later failure. A new ensure operation
                    // discovers partial completion from provider state and finishes it.
                    write_attempted = true;
                    let _: DirectoryAlias = self.alias_call(
                        target,
                        AliasApi::Directory,
                        "insert",
                        &params,
                        Some(&NewDirectoryAlias { alias: email }),
                    )?;
                    let aliases: DirectoryAliases = self.alias_call(
                        target,
                        AliasApi::Directory,
                        "list",
                        &params,
                        None::<&NewDirectoryAlias<'_>>,
                    )?;
                    if !aliases.contains(email, &delegation.subject)? {
                        return Err(Error::Execution(
                            "receiving alias was not present in Directory readback".into(),
                        ));
                    }
                    "created"
                };
                write_attempted = true;
                let _: Sender = self.alias_call(
                    target,
                    AliasApi::SendAs,
                    "create",
                    &MailboxParams { user_id: "me" },
                    Some(&NewSender {
                        send_as_email: email,
                        reply_to_address: email,
                        treat_as_alias: true,
                        display_name: action.args.value("display-name"),
                    }),
                )?;
                let senders = self.mail_alias_senders(target, &delegation.subject)?;
                let sender = senders
                    .find(email)
                    .ok_or_else(|| Error::Execution("Gmail sender was not present in readback".into()))?;
                if !sender
                    .reply_to_address
                    .as_deref()
                    .is_some_and(|reply| reply.eq_ignore_ascii_case(email))
                    || !sender.treat_as_alias
                    || sender.is_default
                {
                    return Err(Error::Execution(
                        "new Gmail sender settings did not match the requested alias".into(),
                    ));
                }
                ("created", receiving_status, sender.readiness()?)
            };
            let verification_status = match verification {
                Readiness::Accepted => "accepted",
                Readiness::Pending => "pending",
            };
            Ok(ToolOutput::new(
                action.tool.clone(),
                action.namespace.clone(),
                if verification == Readiness::Accepted {
                    format!("Gmail alias {email} is ready to send from {}", delegation.subject)
                } else {
                    format!("Gmail alias {email} is pending email verification; it is not ready to send")
                },
            )
            .with_field("email", email)
            .with_field("account", &delegation.subject)
            .with_field("sender", sender_status)
            .with_field("receiving_alias", receiving_status)
            .with_field("verification_status", verification_status))
        })();
        // A later auth/config failure cannot establish that an earlier write failed.
        result.map_err(|error| {
            if write_attempted {
                Error::Execution(format!("alias setup may be partially applied: {error}"))
            } else {
                error
            }
        })
    }

    pub(super) fn verify_mail_alias(
        &self,
        target: &ExecutionTarget,
        operation: &StoredOperation,
    ) -> Result<VerificationReceipt> {
        let delegation = delegation(target)?;
        let email = email(&operation.args, delegation)?;
        let senders = self.mail_alias_senders(target, &delegation.subject)?;
        let Some(sender) = senders.find(email) else {
            return Ok(VerificationReceipt::new(
                VerificationStatus::Mismatch,
                format!("Gmail sender {email} is absent"),
                vec![],
            ));
        };
        match sender.readiness() {
            Ok(Readiness::Accepted) => Ok(VerificationReceipt::new(
                VerificationStatus::Verified,
                format!("Verified accepted Gmail sender {email} on {}", delegation.subject),
                vec![],
            )),
            Ok(Readiness::Pending) => Ok(VerificationReceipt::unavailable(format!(
                "Gmail sender {email} is pending email verification; it is not ready to send"
            ))),
            Err(error) => Ok(VerificationReceipt::unavailable(error.to_string())),
        }
    }

    fn mail_alias_senders(&self, target: &ExecutionTarget, account: &str) -> Result<Senders> {
        let senders: Senders =
            self.read_json(target, &[SENDERS, &["list"]].concat(), &MailboxParams { user_id: "me" })?;
        senders.validate_account(account)?;
        Ok(senders)
    }

    fn alias_call<P: Serialize, B: Serialize, T: DeserializeOwned>(
        &self,
        target: &ExecutionTarget,
        api: AliasApi,
        method: &str,
        params: &P,
        body: Option<&B>,
    ) -> Result<T> {
        let tool = if body.is_some() {
            "google.cli.write"
        } else {
            "google.cli.read"
        };
        let spec = self
            .catalog
            .find_command(tool)
            .and_then(|command| command.executable.as_ref())
            .ok_or_else(|| Error::UnsupportedTool(tool.into()))?;
        let mut argv = match api {
            AliasApi::Directory => vec![
                "admin-reports".into(),
                "users".into(),
                "aliases".into(),
                method.into(),
                "--api-version".into(),
                "directory_v1".into(),
            ],
            AliasApi::SendAs => SENDERS.iter().copied().chain([method]).map(str::to_owned).collect(),
        };
        argv.extend(["--params".into(), encode(params)?, "--format".into(), "json".into()]);
        if let Some(body) = body {
            argv.extend(["--json".into(), encode(body)?]);
        }
        let response = self.backend.execute_raw(target, spec, argv, CliStdioMode::Capture)?;
        serde_json::from_str(&response.stdout)
            .map_err(|error| Error::Execution(format!("invalid Google alias response: {error}")))
    }
}

fn encode(value: &impl Serialize) -> Result<String> {
    serde_json::to_string(value).map_err(|error| Error::InvalidArguments(error.to_string()))
}

#[cfg(test)]
mod tests;

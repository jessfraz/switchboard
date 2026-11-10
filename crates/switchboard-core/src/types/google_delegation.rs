use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// A fixed Gmail-settings delegation. These are identifiers, never credentials.
/// Google grants domain-wide authority; the fixed subject is a Switchboard
/// boundary, not a claim that IAM restricts which users a signer can impersonate.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GoogleDelegation {
    pub project: String,
    pub service_account: String,
    pub domain: String,
    pub subject: String,
    pub signer_account: String,
    pub signer_state_dir: PathBuf,
}

impl GoogleDelegation {
    pub const SCOPES: [&'static str; 2] = [
        "https://www.googleapis.com/auth/gmail.settings.basic",
        "https://www.googleapis.com/auth/gmail.settings.sharing",
    ];

    pub fn validate(&self) -> Result<()> {
        let identifier = |value: &str| {
            !value.is_empty()
                && value
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        };
        let email = |value: &str| {
            value.split_once('@').is_some_and(|(name, domain)| {
                !name.is_empty()
                    && domain.contains('.')
                    && !value.chars().any(|c| c.is_control() || c.is_whitespace())
                    && !value.contains(['/', '\\', '?', '#', ':', '%'])
            })
        };
        if !identifier(&self.project) || !(6..=30).contains(&self.project.len()) {
            return Err(Error::Config(
                "delegation requires a valid Google Cloud project ID".into(),
            ));
        }
        if !email(&self.subject) || !email(&self.signer_account) || !email(&self.service_account) {
            return Err(Error::Config(
                "delegation requires explicit subject, signer and service-account emails".into(),
            ));
        }
        if !self
            .service_account
            .ends_with(&format!("@{}.iam.gserviceaccount.com", self.project))
        {
            return Err(Error::Config(
                "delegation service account must belong to its configured project".into(),
            ));
        }
        if !self.domain.contains('.')
            || self.domain.contains(['/', '\\', '@', ':', '?', '#', '%'])
            || self.domain.chars().any(|c| c.is_control() || c.is_whitespace())
            || !self
                .subject
                .rsplit_once('@')
                .is_some_and(|(_, domain)| domain.eq_ignore_ascii_case(&self.domain))
            || ["gmail.com", "googlemail.com"]
                .iter()
                .any(|domain| self.domain.eq_ignore_ascii_case(domain))
        {
            return Err(Error::Config("delegation requires a managed Workspace subject in its configured domain; consumer Gmail is unsupported".into()));
        }
        if !self.signer_state_dir.is_absolute() {
            return Err(Error::Config(
                "delegation signer_state_dir must be an absolute namespace credential directory".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::GoogleDelegation;

    #[test]
    fn delegation_rejects_cross_domain_and_consumer_subjects() {
        let valid = GoogleDelegation {
            project: "example-project".into(),
            service_account: "settings@example-project.iam.gserviceaccount.com".into(),
            domain: "example.com".into(),
            subject: "person@example.com".into(),
            signer_account: "signer@example.com".into(),
            signer_state_dir: "/tmp/workspace".into(),
        };
        valid.validate().expect("test fixture should succeed");
        for (domain, subject) in [
            ("example.com", "person@other.example"),
            ("gmail.com", "person@gmail.com"),
        ] {
            let mut invalid = valid.clone();
            invalid.domain = domain.into();
            invalid.subject = subject.into();
            assert!(invalid.validate().is_err());
        }
        let mut invalid = valid;
        invalid.service_account = "settings@another-project.iam.gserviceaccount.com".into();
        assert!(invalid.validate().is_err());
    }
}

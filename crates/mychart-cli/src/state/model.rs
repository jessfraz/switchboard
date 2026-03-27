use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{client::StoredCookie, oauth::DynamicClientState};

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct AccountDiscoveryState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) query: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) brand_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) brand_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) endpoint_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) endpoint_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) managing_organization_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) managing_organization_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) last_synced_at_epoch_seconds: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct MyChartAccountState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) api_base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) portal_base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) client_secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) dynamic_client: Option<DynamicClientState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) redirect_uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) access_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) token_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) patient_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) expires_at_epoch_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pending_oauth_state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pending_code_verifier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) device_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) cookies: Vec<StoredCookie>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) discovery: Option<AccountDiscoveryState>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct MyChartState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) current_account: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) accounts: BTreeMap<String, MyChartAccountState>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) api_base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) portal_base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) legacy_base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) client_secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) redirect_uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) access_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) token_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) patient_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) expires_at_epoch_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pending_oauth_state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pending_code_verifier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) device_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) cookies: Vec<StoredCookie>,
}

impl MyChartState {
    pub(crate) fn migrate_legacy_account(&mut self) {
        if self.portal_base_url.is_none() && self.legacy_base_url.is_some() {
            self.portal_base_url = self.legacy_base_url.clone();
        }

        if self.accounts.is_empty() && self.has_legacy_account_data() {
            self.accounts.insert("default".into(), self.legacy_account_state());
            self.current_account = Some("default".into());
        }

        if self.current_account.is_none() {
            self.current_account = self.accounts.keys().next().cloned();
        }

        if let Some(current_account) = self.current_account.clone() {
            if !self.accounts.contains_key(&current_account) {
                self.current_account = self.accounts.keys().next().cloned();
            }
        }

        self.clear_legacy_fields();
    }

    fn has_legacy_account_data(&self) -> bool {
        self.api_base_url.is_some()
            || self.portal_base_url.is_some()
            || self.client_id.is_some()
            || self.client_secret.is_some()
            || self.redirect_uri.is_some()
            || self.access_token.is_some()
            || self.refresh_token.is_some()
            || self.token_type.is_some()
            || self.scope.is_some()
            || self.patient_id.is_some()
            || self.expires_at_epoch_seconds.is_some()
            || self.pending_oauth_state.is_some()
            || self.pending_code_verifier.is_some()
            || self.username.is_some()
            || self.device_id.is_some()
            || !self.cookies.is_empty()
    }

    fn legacy_account_state(&self) -> MyChartAccountState {
        MyChartAccountState {
            api_base_url: self.api_base_url.clone(),
            portal_base_url: self.portal_base_url.clone(),
            client_id: self.client_id.clone(),
            client_secret: self.client_secret.clone(),
            dynamic_client: None,
            redirect_uri: self.redirect_uri.clone(),
            access_token: self.access_token.clone(),
            refresh_token: self.refresh_token.clone(),
            token_type: self.token_type.clone(),
            scope: self.scope.clone(),
            patient_id: self.patient_id.clone(),
            expires_at_epoch_seconds: self.expires_at_epoch_seconds,
            pending_oauth_state: self.pending_oauth_state.clone(),
            pending_code_verifier: self.pending_code_verifier.clone(),
            username: self.username.clone(),
            device_id: self.device_id.clone(),
            cookies: self.cookies.clone(),
            discovery: None,
        }
    }

    pub(crate) fn clear_legacy_fields(&mut self) {
        self.api_base_url = None;
        self.portal_base_url = None;
        self.legacy_base_url = None;
        self.client_id = None;
        self.client_secret = None;
        self.redirect_uri = None;
        self.access_token = None;
        self.refresh_token = None;
        self.token_type = None;
        self.scope = None;
        self.patient_id = None;
        self.expires_at_epoch_seconds = None;
        self.pending_oauth_state = None;
        self.pending_code_verifier = None;
        self.username = None;
        self.device_id = None;
        self.cookies = Vec::new();
    }
}

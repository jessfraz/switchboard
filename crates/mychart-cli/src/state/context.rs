use std::{
    collections::BTreeMap,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    args::GlobalArgs,
    client::{cookie_names, normalize_api_base_url, StoredCookie},
    oauth::DynamicClientState,
    Error, Result,
};

use super::{store::resolve_state_path, MyChartAccountState, MyChartState, StateStore};

pub(crate) struct ResolvedContext {
    pub(crate) account: String,
    pub(crate) debug_auth: bool,
    pub(crate) api_base_url: Option<String>,
    pub(crate) portal_base_url: Option<String>,
    pub(crate) client_id: Option<String>,
    pub(crate) client_secret: Option<String>,
    pub(crate) dynamic_client: Option<DynamicClientState>,
    pub(crate) redirect_uri: Option<String>,
    pub(crate) access_token: Option<String>,
    pub(crate) refresh_token: Option<String>,
    pub(crate) token_type: Option<String>,
    pub(crate) scope: Option<String>,
    pub(crate) patient_id: Option<String>,
    pub(crate) expires_at_epoch_seconds: Option<u64>,
    pub(crate) pending_oauth_state: Option<String>,
    pub(crate) pending_code_verifier: Option<String>,
    pub(crate) username: Option<String>,
    pub(crate) device_id: String,
    pub(crate) cookies: BTreeMap<String, String>,
    store: StateStore,
    state: MyChartState,
}

pub(crate) struct ApiSessionState {
    pub(crate) base_url: String,
    pub(crate) client_id: String,
    pub(crate) client_secret: Option<String>,
    pub(crate) redirect_uri: String,
    pub(crate) access_token: String,
    pub(crate) refresh_token: Option<String>,
    pub(crate) token_type: Option<String>,
    pub(crate) scope: Option<String>,
    pub(crate) patient_id: Option<String>,
    pub(crate) expires_at_epoch_seconds: Option<u64>,
}

impl ResolvedContext {
    pub(crate) fn from_global(global: &GlobalArgs) -> Result<Self> {
        let path = resolve_state_path(global.config.as_deref())?;
        let store = StateStore::new(path);
        let mut state = store.load()?;
        state.migrate_legacy_account();

        let account = global
            .account
            .clone()
            .or_else(|| state.current_account.clone())
            .or_else(|| state.accounts.keys().next().cloned())
            .unwrap_or_else(|| "default".into());
        let persisted = state.accounts.get(&account).cloned().unwrap_or_default();

        Ok(Self {
            account,
            debug_auth: global.debug_auth,
            api_base_url: pick(global.base_url.clone(), persisted.api_base_url.clone()),
            portal_base_url: pick(global.portal_base_url.clone(), persisted.portal_base_url.clone()),
            client_id: pick(global.client_id.clone(), persisted.client_id.clone()),
            client_secret: pick(global.client_secret.clone(), persisted.client_secret.clone()),
            dynamic_client: persisted.dynamic_client.clone(),
            redirect_uri: pick(global.redirect_uri.clone(), persisted.redirect_uri.clone()),
            access_token: pick(global.access_token.clone(), persisted.access_token.clone()),
            refresh_token: pick(global.refresh_token.clone(), persisted.refresh_token.clone()),
            token_type: persisted.token_type.clone(),
            scope: persisted.scope.clone(),
            patient_id: persisted.patient_id.clone(),
            expires_at_epoch_seconds: persisted.expires_at_epoch_seconds,
            pending_oauth_state: persisted.pending_oauth_state.clone(),
            pending_code_verifier: persisted.pending_code_verifier.clone(),
            username: pick(global.username.clone(), persisted.username.clone()),
            device_id: persisted.device_id.clone().unwrap_or_else(generate_device_id),
            cookies: persisted
                .cookies
                .iter()
                .map(|cookie| (cookie.name.clone(), cookie.value.clone()))
                .collect(),
            store,
            state,
        })
    }

    pub(crate) fn active_account_name(&self) -> Option<&str> {
        self.state.current_account.as_deref()
    }

    pub(crate) fn discovery_cache_path(&self) -> Result<std::path::PathBuf> {
        self.store.sibling_path("brands-cache.json")
    }

    pub(crate) fn list_accounts(&self) -> Vec<(String, MyChartAccountState)> {
        self.state
            .accounts
            .iter()
            .map(|(name, state)| (name.clone(), state.clone()))
            .collect()
    }

    pub(crate) fn describe_account(&self, name: Option<&str>) -> Option<(String, MyChartAccountState)> {
        let name = match name {
            Some(name) => name.to_owned(),
            None => self.account.clone(),
        };
        self.state.accounts.get(&name).cloned().map(|state| (name, state))
    }

    pub(crate) fn set_current_account(&mut self, name: String) -> Result<()> {
        let account_state =
            self.state.accounts.get(&name).cloned().ok_or_else(|| {
                Error::Config(format!("unknown MyChart account {name:?}, run `mychart connect list`"))
            })?;
        self.account = name.clone();
        self.state.current_account = Some(name);
        self.apply_account_state(&account_state);
        self.persist_state()
    }

    pub(crate) fn upsert_account(
        &mut self,
        name: String,
        account_state: MyChartAccountState,
        set_current: bool,
    ) -> Result<()> {
        self.state.accounts.insert(name.clone(), account_state.clone());
        if set_current {
            self.account = name.clone();
            self.state.current_account = Some(name);
            self.apply_account_state(&account_state);
        }
        self.persist_state()
    }

    pub(crate) fn require_api_base_url(&self) -> Result<String> {
        let base_url = self.api_base_url.clone().ok_or_else(|| {
            Error::Config(
                "missing MyChart FHIR base URL, pass --base-url, set MYCHART_BASE_URL, use `mychart connect`, or store it during auth authorize-url"
                    .into(),
            )
        })?;
        normalize_api_base_url(&base_url)
    }

    pub(crate) fn require_portal_base_url(&self) -> Result<String> {
        self.portal_base_url.clone().ok_or_else(|| {
            Error::Config(
                "missing MyChart portal base URL, pass --portal-base-url, set MYCHART_PORTAL_BASE_URL, or store it during portal auth login-password"
                    .into(),
            )
        })
    }

    pub(crate) fn require_client_id(&self) -> Result<String> {
        self.client_id
            .clone()
            .ok_or_else(|| Error::Config("missing client id, pass --client-id or set MYCHART_CLIENT_ID".into()))
    }

    pub(crate) fn dynamic_client(&self) -> Option<&DynamicClientState> {
        self.dynamic_client.as_ref()
    }

    pub(crate) fn require_redirect_uri(&self, explicit: Option<String>) -> Result<String> {
        explicit.or_else(|| self.redirect_uri.clone()).ok_or_else(|| {
            Error::Config("missing redirect URI, pass --redirect-uri or set MYCHART_REDIRECT_URI".into())
        })
    }

    pub(crate) fn require_access_token(&self, explicit: Option<String>) -> Result<String> {
        explicit.or_else(|| self.access_token.clone()).ok_or_else(|| {
            Error::Config("missing access token, run mychart auth exchange-code or pass --access-token".into())
        })
    }

    pub(crate) fn require_refresh_token(&self, explicit: Option<String>) -> Result<String> {
        explicit.or_else(|| self.refresh_token.clone()).ok_or_else(|| {
            Error::Config("missing refresh token, run mychart auth exchange-code or pass --refresh-token".into())
        })
    }

    pub(crate) fn require_code_verifier(&self, explicit: Option<String>) -> Result<String> {
        explicit.or_else(|| self.pending_code_verifier.clone()).ok_or_else(|| {
            Error::Config(
                "missing PKCE code verifier, pass --code-verifier or generate one with mychart auth authorize-url"
                    .into(),
            )
        })
    }

    pub(crate) fn api_authenticated(&self) -> bool {
        self.access_token.is_some()
    }

    pub(crate) fn has_portal_session(&self) -> bool {
        !self.cookies.is_empty()
    }

    pub(crate) fn require_portal_session(&self) -> Result<()> {
        if self.has_portal_session() {
            Ok(())
        } else {
            Err(Error::Config(
                "missing MyChart portal session, run mychart portal auth login-password first".into(),
            ))
        }
    }

    pub(crate) fn require_username(&self, explicit: Option<String>) -> Result<String> {
        explicit
            .or_else(|| self.username.clone())
            .ok_or_else(|| Error::Config("missing username, pass --username or set MYCHART_USERNAME".into()))
    }

    pub(crate) fn store_pending_oauth(
        &mut self,
        base_url: String,
        client_id: String,
        client_secret: Option<String>,
        redirect_uri: String,
        oauth_state: String,
        code_verifier: String,
    ) -> Result<()> {
        self.api_base_url = Some(base_url.clone());
        self.client_id = Some(client_id.clone());
        self.client_secret = client_secret.clone();
        self.redirect_uri = Some(redirect_uri.clone());
        self.pending_oauth_state = Some(oauth_state.clone());
        self.pending_code_verifier = Some(code_verifier.clone());

        let account = self.current_account_state_mut();
        account.api_base_url = Some(base_url);
        account.client_id = Some(client_id);
        account.client_secret = client_secret;
        account.redirect_uri = Some(redirect_uri);
        account.pending_oauth_state = Some(oauth_state);
        account.pending_code_verifier = Some(code_verifier);
        self.persist_state()
    }

    pub(crate) fn store_api_tokens(&mut self, session: ApiSessionState) -> Result<()> {
        self.api_base_url = Some(session.base_url.clone());
        self.client_id = Some(session.client_id.clone());
        self.client_secret = session.client_secret.clone();
        self.redirect_uri = Some(session.redirect_uri.clone());
        self.access_token = Some(session.access_token.clone());
        self.refresh_token = session.refresh_token.clone();
        self.token_type = session.token_type.clone();
        self.scope = session.scope.clone();
        self.patient_id = session.patient_id.clone();
        self.expires_at_epoch_seconds = session.expires_at_epoch_seconds;
        self.pending_oauth_state = None;
        self.pending_code_verifier = None;

        let account = self.current_account_state_mut();
        account.api_base_url = Some(session.base_url);
        account.client_id = Some(session.client_id);
        account.client_secret = session.client_secret;
        account.redirect_uri = Some(session.redirect_uri);
        account.access_token = Some(session.access_token);
        account.refresh_token = session.refresh_token;
        account.token_type = session.token_type;
        account.scope = session.scope;
        account.patient_id = session.patient_id;
        account.expires_at_epoch_seconds = session.expires_at_epoch_seconds;
        account.pending_oauth_state = None;
        account.pending_code_verifier = None;
        self.persist_state()
    }

    pub(crate) fn store_dynamic_client(&mut self, dynamic_client: DynamicClientState) -> Result<()> {
        self.dynamic_client = Some(dynamic_client.clone());
        self.current_account_state_mut().dynamic_client = Some(dynamic_client);
        self.persist_state()
    }

    pub(crate) fn clear_api_session(&mut self) -> Result<()> {
        self.access_token = None;
        self.refresh_token = None;
        self.token_type = None;
        self.scope = None;
        self.patient_id = None;
        self.expires_at_epoch_seconds = None;
        self.pending_oauth_state = None;
        self.pending_code_verifier = None;
        self.dynamic_client = None;

        let account = self.current_account_state_mut();
        account.access_token = None;
        account.refresh_token = None;
        account.token_type = None;
        account.scope = None;
        account.patient_id = None;
        account.expires_at_epoch_seconds = None;
        account.pending_oauth_state = None;
        account.pending_code_verifier = None;
        account.dynamic_client = None;
        self.persist_state()
    }

    pub(crate) fn update_cookies(&mut self, cookies: BTreeMap<String, String>) {
        self.cookies = cookies;
    }

    pub(crate) fn store_portal_session(&mut self, portal_base_url: String, username: Option<String>) -> Result<()> {
        if let Some(username) = username {
            self.username = Some(username);
        }

        self.portal_base_url = Some(portal_base_url.clone());
        let username = self.username.clone();
        let device_id = self.device_id.clone();
        let cookies = self
            .cookies
            .iter()
            .map(|(name, value)| StoredCookie {
                name: name.clone(),
                value: value.clone(),
            })
            .collect();

        let account = self.current_account_state_mut();
        account.portal_base_url = Some(portal_base_url);
        account.username = username;
        account.device_id = Some(device_id);
        account.cookies = cookies;
        self.persist_state()
    }

    pub(crate) fn clear_portal_session(&mut self) -> Result<()> {
        self.cookies.clear();
        let portal_base_url = self.portal_base_url.clone();
        let username = self.username.clone();
        let device_id = self.device_id.clone();

        let account = self.current_account_state_mut();
        account.portal_base_url = portal_base_url;
        account.username = username;
        account.device_id = Some(device_id);
        account.cookies = Vec::new();
        self.persist_state()
    }

    pub(crate) fn cookie_names(&self) -> Vec<String> {
        cookie_names(&self.cookies)
    }

    fn current_account_state_mut(&mut self) -> &mut MyChartAccountState {
        self.state.current_account = Some(self.account.clone());
        self.state.accounts.entry(self.account.clone()).or_default()
    }

    fn apply_account_state(&mut self, account: &MyChartAccountState) {
        self.api_base_url = account.api_base_url.clone();
        self.portal_base_url = account.portal_base_url.clone();
        self.client_id = account.client_id.clone();
        self.client_secret = account.client_secret.clone();
        self.dynamic_client = account.dynamic_client.clone();
        self.redirect_uri = account.redirect_uri.clone();
        self.access_token = account.access_token.clone();
        self.refresh_token = account.refresh_token.clone();
        self.token_type = account.token_type.clone();
        self.scope = account.scope.clone();
        self.patient_id = account.patient_id.clone();
        self.expires_at_epoch_seconds = account.expires_at_epoch_seconds;
        self.pending_oauth_state = account.pending_oauth_state.clone();
        self.pending_code_verifier = account.pending_code_verifier.clone();
        self.username = account.username.clone();
        self.device_id = account.device_id.clone().unwrap_or_else(generate_device_id);
        self.cookies = account
            .cookies
            .iter()
            .map(|cookie| (cookie.name.clone(), cookie.value.clone()))
            .collect();
    }

    fn persist_state(&mut self) -> Result<()> {
        self.state.clear_legacy_fields();
        self.store.save(&self.state)
    }
}

fn pick(explicit: Option<String>, persisted: Option<String>) -> Option<String> {
    explicit.or(persisted)
}

fn generate_device_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("codex-{nanos:x}-{:x}", std::process::id())
}

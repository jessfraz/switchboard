mod context;
mod model;
mod store;

pub(crate) use self::{
    context::{ApiSessionState, ResolvedContext},
    model::{AccountDiscoveryState, MyChartAccountState, MyChartState},
    store::StateStore,
};

pub(crate) const ENV_MYCHART_CONFIG: &str = "MYCHART_CONFIG";
pub(crate) const ENV_MYCHART_ACCOUNT: &str = "MYCHART_ACCOUNT";
pub(crate) const ENV_MYCHART_BASE_URL: &str = "MYCHART_BASE_URL";
pub(crate) const ENV_MYCHART_PORTAL_BASE_URL: &str = "MYCHART_PORTAL_BASE_URL";
pub(crate) const ENV_MYCHART_CLIENT_ID: &str = "MYCHART_CLIENT_ID";
pub(crate) const ENV_MYCHART_CLIENT_SECRET: &str = "MYCHART_CLIENT_SECRET";
pub(crate) const ENV_MYCHART_REDIRECT_URI: &str = "MYCHART_REDIRECT_URI";
pub(crate) const ENV_MYCHART_ACCESS_TOKEN: &str = "MYCHART_ACCESS_TOKEN";
pub(crate) const ENV_MYCHART_REFRESH_TOKEN: &str = "MYCHART_REFRESH_TOKEN";
pub(crate) const ENV_MYCHART_USERNAME: &str = "MYCHART_USERNAME";
pub(crate) const ENV_MYCHART_DEBUG_AUTH: &str = "MYCHART_DEBUG_AUTH";

use crate::discovery::slugify;

pub(crate) const PAGES_REDIRECT_URI: &str = "https://jessfraz.github.io/switchboard/mychart-callback/";
pub(crate) const LOOPBACK_REDIRECT_URI: &str = "http://127.0.0.1:8910/callback";
pub(crate) const UCLA_FHIR_BASE_URL: &str = "https://arrprox.mednet.ucla.edu/FHIRPRD/api/FHIR/R4";
pub(crate) const UCLA_PORTAL_BASE_URL: &str = "https://my.uclahealth.org/MyChart";
pub(crate) const UCLA_PRODUCTION_CLIENT_ID: &str = "6afd07db-4c59-4e7c-8462-08bd63f725cc";
pub(crate) const EPIC_SANDBOX_FHIR_BASE_URL: &str = "https://fhir.epic.com/interconnect-fhir-oauth/api/FHIR/R4";
pub(crate) const EPIC_SANDBOX_CLIENT_ID: &str = "a7869c00-3088-4b23-8ce5-cdef423c438c";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BuiltinPreset {
    pub(crate) account_name: &'static str,
    pub(crate) aliases: &'static [&'static str],
    pub(crate) api_base_url: &'static str,
    pub(crate) portal_base_url: Option<&'static str>,
    pub(crate) client_id: &'static str,
    pub(crate) redirect_uri: &'static str,
}

const UCLA_PRESET: BuiltinPreset = BuiltinPreset {
    account_name: "ucla",
    aliases: &["ucla", "ucla-health", "ucla-medical-center", "myuclahealth"],
    api_base_url: UCLA_FHIR_BASE_URL,
    portal_base_url: Some(UCLA_PORTAL_BASE_URL),
    client_id: UCLA_PRODUCTION_CLIENT_ID,
    redirect_uri: PAGES_REDIRECT_URI,
};

const EPIC_SANDBOX_PRESET: BuiltinPreset = BuiltinPreset {
    account_name: "epic-sandbox",
    aliases: &["epic-sandbox", "sandbox", "epic-sandbox-r4", "epic-fhir-sandbox"],
    api_base_url: EPIC_SANDBOX_FHIR_BASE_URL,
    portal_base_url: None,
    client_id: EPIC_SANDBOX_CLIENT_ID,
    redirect_uri: LOOPBACK_REDIRECT_URI,
};

const BUILTIN_PRESETS: &[BuiltinPreset] = &[UCLA_PRESET, EPIC_SANDBOX_PRESET];

pub(crate) fn resolve_builtin_preset(query: &str) -> Option<&'static BuiltinPreset> {
    let normalized = slugify(query);
    BUILTIN_PRESETS
        .iter()
        .find(|preset| preset.aliases.iter().any(|alias| *alias == normalized))
}

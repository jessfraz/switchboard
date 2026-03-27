use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Error, Result};

const EPIC_BRANDS_BUNDLE_URL: &str = "https://open.epic.com/Endpoints/Brands";
const CACHE_TTL_SECONDS: u64 = 60 * 60 * 24 * 7;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct DiscoveryCatalog {
    pub(crate) source_url: String,
    pub(crate) fetched_at_epoch_seconds: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) bundle_last_updated: Option<String>,
    #[serde(default)]
    pub(crate) brands: Vec<DiscoveryBrand>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct DiscoveryBrand {
    pub(crate) brand_id: String,
    pub(crate) brand_name: String,
    pub(crate) account_slug: String,
    pub(crate) fhir_base_url: String,
    pub(crate) endpoint_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) endpoint_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) managing_organization_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) managing_organization_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) country: Option<String>,
    #[serde(default)]
    pub(crate) facilities: Vec<DiscoveryFacility>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct DiscoveryFacility {
    pub(crate) name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) city: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) state: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct DiscoveryMatch {
    pub(crate) brand: DiscoveryBrand,
    pub(crate) score: u32,
    pub(crate) exact: bool,
}

impl DiscoveryCatalog {
    pub(crate) fn search(&self, query: &str, limit: usize) -> Vec<DiscoveryMatch> {
        let normalized_query = normalize_search_text(query);
        if normalized_query.is_empty() {
            return Vec::new();
        }

        let query_tokens = normalized_query.split_whitespace().collect::<Vec<_>>();
        let mut matches = self
            .brands
            .iter()
            .filter_map(|brand| {
                let score = score_brand_match(brand, &normalized_query, &query_tokens);
                (score > 0).then(|| DiscoveryMatch {
                    brand: brand.clone(),
                    score,
                    exact: is_exact_brand_match(brand, &normalized_query),
                })
            })
            .collect::<Vec<_>>();

        matches.sort_by(|left, right| {
            right
                .score
                .cmp(&left.score)
                .then_with(|| left.brand.brand_name.cmp(&right.brand.brand_name))
                .then_with(|| left.brand.fhir_base_url.cmp(&right.brand.fhir_base_url))
        });
        matches.truncate(limit);
        matches
    }

    pub(crate) fn resolve_unique(&self, query: &str) -> Option<DiscoveryBrand> {
        let matches = self.search(query, 10);
        if matches.is_empty() {
            return None;
        }

        let exact = matches.iter().filter(|candidate| candidate.exact).collect::<Vec<_>>();
        if exact.len() == 1 {
            return Some(exact[0].brand.clone());
        }
        if matches.len() == 1 {
            return Some(matches[0].brand.clone());
        }

        None
    }
}

pub(crate) fn load_catalog(cache_path: &Path, refresh: bool) -> Result<DiscoveryCatalog> {
    if !refresh {
        if let Some(cached) = load_cached_catalog(cache_path)? {
            if catalog_is_fresh(&cached) {
                return Ok(cached);
            }
        }
    }

    let catalog = fetch_catalog()?;
    save_catalog(cache_path, &catalog)?;
    Ok(catalog)
}

pub(crate) fn slugify(input: &str) -> String {
    let mut slug = String::new();
    let mut last_was_separator = false;

    for character in input.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
            last_was_separator = false;
        } else if !last_was_separator && !slug.is_empty() {
            slug.push('-');
            last_was_separator = true;
        }
    }

    while slug.ends_with('-') {
        slug.pop();
    }

    if slug.is_empty() {
        "account".into()
    } else {
        slug
    }
}

fn load_cached_catalog(cache_path: &Path) -> Result<Option<DiscoveryCatalog>> {
    match fs::read_to_string(cache_path) {
        Ok(contents) => {
            let catalog = serde_json::from_str(&contents).map_err(|error| {
                Error::Config(format!(
                    "failed to parse cached Epic brands catalog at {}: {error}",
                    cache_path.display()
                ))
            })?;
            Ok(Some(catalog))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(Error::Io(format!(
            "failed to read cached Epic brands catalog at {}: {error}",
            cache_path.display()
        ))),
    }
}

fn catalog_is_fresh(catalog: &DiscoveryCatalog) -> bool {
    now_epoch_seconds().saturating_sub(catalog.fetched_at_epoch_seconds) <= CACHE_TTL_SECONDS
}

fn save_catalog(cache_path: &Path, catalog: &DiscoveryCatalog) -> Result<()> {
    if let Some(parent) = cache_path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            Error::Io(format!(
                "failed to create Epic brands cache directory {}: {error}",
                parent.display()
            ))
        })?;
    }

    let temp_path = cache_path.with_extension("tmp");
    let contents = serde_json::to_vec_pretty(catalog)
        .map_err(|error| Error::Config(format!("failed to serialize Epic brands catalog: {error}")))?;
    fs::write(&temp_path, contents).map_err(|error| {
        Error::Io(format!(
            "failed to write Epic brands cache file {}: {error}",
            temp_path.display()
        ))
    })?;
    fs::rename(&temp_path, cache_path).map_err(|error| {
        Error::Io(format!(
            "failed to move Epic brands cache into place at {}: {error}",
            cache_path.display()
        ))
    })?;
    Ok(())
}

fn fetch_catalog() -> Result<DiscoveryCatalog> {
    let http = Client::builder()
        .user_agent(format!("mychart-cli/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| Error::Http(format!("failed to build Epic discovery client: {error}")))?;
    let response = http
        .get(EPIC_BRANDS_BUNDLE_URL)
        .send()
        .map_err(|error| Error::Http(format!("failed to download Epic brands bundle: {error}")))?;
    let status = response.status();
    if status.as_u16() >= 400 {
        return Err(Error::Http(format!(
            "Epic brands bundle request failed with status {}",
            status.as_u16()
        )));
    }
    let body = response
        .text()
        .map_err(|error| Error::Http(format!("failed to read Epic brands bundle response: {error}")))?;
    let bundle = parse_bundle(&body)?;
    Ok(bundle_to_catalog(bundle))
}

fn parse_bundle(body: &str) -> Result<BrandsBundle> {
    serde_json::from_str(body)
        .map_err(|error| Error::Config(format!("failed to parse Epic brands bundle JSON: {error}")))
}

fn bundle_to_catalog(bundle: BrandsBundle) -> DiscoveryCatalog {
    let mut endpoints = BTreeMap::new();
    let mut organizations = Vec::new();

    for entry in bundle.entry {
        match entry.resource_type() {
            Some("Organization") => {
                if let Ok(organization) = serde_json::from_value::<BundleOrganization>(entry.resource.clone()) {
                    organizations.push(IdentifiedOrganization {
                        full_url: entry.full_url.clone(),
                        organization,
                    });
                }
            }
            Some("Endpoint") => {
                if let Ok(endpoint) = serde_json::from_value::<BundleEndpoint>(entry.resource.clone()) {
                    for alias in resource_aliases(entry.full_url.as_deref(), &endpoint.id, "Endpoint") {
                        endpoints.insert(alias, endpoint.clone());
                    }
                }
            }
            _ => {}
        }
    }

    let mut facilities_by_parent = BTreeMap::<String, Vec<BundleOrganization>>::new();
    for organization in &organizations {
        if let Some(reference) = organization
            .organization
            .part_of
            .as_ref()
            .and_then(|reference| reference.reference.clone())
        {
            facilities_by_parent
                .entry(reference)
                .or_default()
                .push(organization.organization.clone());
        }
    }

    let mut brands = organizations
        .into_iter()
        .filter_map(|organization| {
            let endpoint = organization
                .organization
                .endpoint
                .iter()
                .filter_map(|reference| reference.reference.as_deref())
                .find_map(|reference| endpoints.get(reference).cloned())?;

            let aliases = resource_aliases(
                organization.full_url.as_deref(),
                &organization.organization.id,
                "Organization",
            );
            let mut facilities = BTreeMap::<String, DiscoveryFacility>::new();
            for alias in aliases {
                if let Some(children) = facilities_by_parent.get(&alias) {
                    for facility in children {
                        let name = facility.name.clone().unwrap_or_else(|| "Unnamed facility".into());
                        let entry = DiscoveryFacility {
                            name: name.clone(),
                            city: facility.address.first().and_then(|address| address.city.clone()),
                            state: facility.address.first().and_then(|address| address.state.clone()),
                        };
                        facilities.insert(name, entry);
                    }
                }
            }

            let brand_name = organization
                .organization
                .name
                .clone()
                .or_else(|| endpoint.name.clone())
                .unwrap_or_else(|| endpoint.address.clone());
            Some(DiscoveryBrand {
                brand_id: organization.organization.id.clone(),
                account_slug: slugify(&brand_name),
                brand_name,
                fhir_base_url: endpoint.address,
                endpoint_id: endpoint.id,
                endpoint_name: endpoint.name,
                managing_organization_id: endpoint
                    .managing_organization
                    .as_ref()
                    .and_then(|organization| organization.identifier.as_ref())
                    .and_then(|identifier| identifier.value.clone()),
                managing_organization_name: endpoint
                    .managing_organization
                    .as_ref()
                    .and_then(|organization| organization.display.clone()),
                state: organization
                    .organization
                    .address
                    .first()
                    .and_then(|address| address.state.clone()),
                country: organization
                    .organization
                    .address
                    .first()
                    .and_then(|address| address.country.clone()),
                facilities: facilities.into_values().collect(),
            })
        })
        .collect::<Vec<_>>();

    brands.sort_by(|left, right| left.brand_name.cmp(&right.brand_name));

    DiscoveryCatalog {
        source_url: EPIC_BRANDS_BUNDLE_URL.into(),
        fetched_at_epoch_seconds: now_epoch_seconds(),
        bundle_last_updated: bundle.meta.and_then(|meta| meta.last_updated),
        brands,
    }
}

fn score_brand_match(brand: &DiscoveryBrand, query: &str, query_tokens: &[&str]) -> u32 {
    let mut score = 0;
    let name = normalize_search_text(&brand.brand_name);
    let endpoint_name = brand
        .endpoint_name
        .as_deref()
        .map(normalize_search_text)
        .unwrap_or_default();
    let org_name = brand
        .managing_organization_name
        .as_deref()
        .map(normalize_search_text)
        .unwrap_or_default();
    let base_url = normalize_search_text(&brand.fhir_base_url);

    score += score_text_match(&name, query, 1200, 900, 600);
    score += score_text_match(&endpoint_name, query, 1000, 700, 500);
    score += score_text_match(&brand.account_slug, query, 1000, 900, 700);
    score += score_text_match(&org_name, query, 500, 300, 200);
    score += score_text_match(&base_url, query, 350, 200, 150);

    for token in query_tokens {
        if name.contains(token) {
            score += 75;
        }
        if endpoint_name.contains(token) {
            score += 50;
        }
        if org_name.contains(token) {
            score += 30;
        }
        if brand
            .facilities
            .iter()
            .any(|facility| normalize_search_text(&facility.name).contains(token))
        {
            score += 20;
        }
    }

    score
}

fn score_text_match(haystack: &str, needle: &str, exact_score: u32, prefix_score: u32, contains_score: u32) -> u32 {
    if haystack.is_empty() {
        return 0;
    }
    if haystack == needle {
        return exact_score;
    }
    if haystack.starts_with(needle) {
        return prefix_score;
    }
    if haystack.contains(needle) {
        return contains_score;
    }
    0
}

fn is_exact_brand_match(brand: &DiscoveryBrand, query: &str) -> bool {
    normalize_search_text(&brand.brand_name) == query
        || normalize_search_text(&brand.account_slug) == query
        || brand
            .endpoint_name
            .as_deref()
            .map(normalize_search_text)
            .is_some_and(|value| value == query)
}

fn normalize_search_text(input: &str) -> String {
    input
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn resource_aliases(full_url: Option<&str>, id: &str, resource_type: &str) -> Vec<String> {
    let mut aliases = BTreeSet::new();
    aliases.insert(id.to_owned());
    aliases.insert(format!("{resource_type}/{id}"));
    aliases.insert(format!("urn:uuid:{id}"));
    if let Some(full_url) = full_url {
        aliases.insert(full_url.to_owned());
    }
    aliases.into_iter().collect()
}

fn now_epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[derive(Clone, Debug, Deserialize)]
struct BrandsBundle {
    meta: Option<BundleMeta>,
    #[serde(default)]
    entry: Vec<BundleEntry>,
}

#[derive(Clone, Debug, Deserialize)]
struct BundleMeta {
    #[serde(rename = "lastUpdated")]
    last_updated: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct BundleEntry {
    #[serde(rename = "fullUrl")]
    full_url: Option<String>,
    resource: Value,
}

impl BundleEntry {
    fn resource_type(&self) -> Option<&str> {
        self.resource.get("resourceType").and_then(Value::as_str)
    }
}

#[derive(Clone, Debug)]
struct IdentifiedOrganization {
    full_url: Option<String>,
    organization: BundleOrganization,
}

#[derive(Clone, Debug, Deserialize)]
struct BundleOrganization {
    id: String,
    name: Option<String>,
    #[serde(default)]
    endpoint: Vec<ReferenceValue>,
    #[serde(default, rename = "partOf")]
    part_of: Option<ReferenceValue>,
    #[serde(default)]
    address: Vec<AddressValue>,
}

#[derive(Clone, Debug, Deserialize)]
struct BundleEndpoint {
    id: String,
    address: String,
    name: Option<String>,
    #[serde(default, rename = "managingOrganization")]
    managing_organization: Option<ManagingOrganizationValue>,
}

#[derive(Clone, Debug, Deserialize)]
struct ReferenceValue {
    reference: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct AddressValue {
    city: Option<String>,
    state: Option<String>,
    country: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct ManagingOrganizationValue {
    identifier: Option<IdentifierValue>,
    display: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct IdentifierValue {
    value: Option<String>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{bundle_to_catalog, slugify, BrandsBundle};

    #[test]
    fn bundle_parser_keeps_primary_brand_and_facilities() {
        let bundle: BrandsBundle = serde_json::from_value(json!({
            "meta": {"lastUpdated": "2026-03-27T03:00:03Z"},
            "entry": [
                {
                    "fullUrl": "urn:uuid:brand-1",
                    "resource": {
                        "resourceType": "Organization",
                        "id": "brand-1",
                        "name": "UCLA Medical Center",
                        "endpoint": [{"reference": "urn:uuid:endpoint-1"}],
                        "address": [{"state": "CA", "country": "USA"}]
                    }
                },
                {
                    "fullUrl": "urn:uuid:facility-1",
                    "resource": {
                        "resourceType": "Organization",
                        "id": "facility-1",
                        "name": "UCLA Santa Monica",
                        "partOf": {"reference": "urn:uuid:brand-1"},
                        "address": [{"city": "Santa Monica", "state": "CA", "country": "USA"}]
                    }
                },
                {
                    "fullUrl": "urn:uuid:endpoint-1",
                    "resource": {
                        "resourceType": "Endpoint",
                        "id": "endpoint-1",
                        "address": "https://example.org/FHIR/R4",
                        "name": "UCLA Medical Center",
                        "managingOrganization": {
                            "identifier": {"value": "341"},
                            "display": "UCLA Health"
                        }
                    }
                }
            ]
        }))
        .expect("bundle should parse");

        let catalog = bundle_to_catalog(bundle);
        assert_eq!(catalog.brands.len(), 1);
        assert_eq!(catalog.brands[0].brand_name, "UCLA Medical Center");
        assert_eq!(catalog.brands[0].facilities.len(), 1);
        assert_eq!(catalog.brands[0].facilities[0].name, "UCLA Santa Monica");
    }

    #[test]
    fn search_prefers_exact_brand_names() {
        let bundle: BrandsBundle = serde_json::from_value(json!({
            "entry": [
                {
                    "fullUrl": "urn:uuid:brand-1",
                    "resource": {
                        "resourceType": "Organization",
                        "id": "brand-1",
                        "name": "UCLA Medical Center",
                        "endpoint": [{"reference": "urn:uuid:endpoint-1"}]
                    }
                },
                {
                    "fullUrl": "urn:uuid:brand-2",
                    "resource": {
                        "resourceType": "Organization",
                        "id": "brand-2",
                        "name": "UCLA Health Medicare Advantage Plan",
                        "endpoint": [{"reference": "urn:uuid:endpoint-2"}]
                    }
                },
                {
                    "fullUrl": "urn:uuid:endpoint-1",
                    "resource": {
                        "resourceType": "Endpoint",
                        "id": "endpoint-1",
                        "address": "https://example.org/medical/FHIR/R4",
                        "name": "UCLA Medical Center"
                    }
                },
                {
                    "fullUrl": "urn:uuid:endpoint-2",
                    "resource": {
                        "resourceType": "Endpoint",
                        "id": "endpoint-2",
                        "address": "https://example.org/plan/FHIR/R4",
                        "name": "UCLA Health Medicare Advantage Plan"
                    }
                }
            ]
        }))
        .expect("bundle should parse");

        let catalog = bundle_to_catalog(bundle);
        let resolved = catalog
            .resolve_unique("ucla medical center")
            .expect("exact match should resolve");
        assert_eq!(resolved.endpoint_id, "endpoint-1");
        assert!(catalog.resolve_unique("ucla").is_none());
    }

    #[test]
    fn slugify_normalizes_account_names() {
        assert_eq!(slugify("UCLA Medical Center"), "ucla-medical-center");
    }
}

use std::time::{SystemTime, UNIX_EPOCH};

use rsa::{
    pkcs1v15::SigningKey,
    pkcs8::{DecodePrivateKey, EncodePrivateKey, LineEnding},
    signature::{SignatureEncoding, Signer},
    traits::PublicKeyParts,
    RsaPrivateKey, RsaPublicKey,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::Sha256;

use crate::{base64_url_encode, generate_nonce, Error, Result};

const DYNAMIC_CLIENT_RSA_BITS: usize = 2048;
pub(crate) const JWT_BEARER_GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:jwt-bearer";

#[derive(Clone, Debug)]
pub(crate) struct DynamicClientKeyMaterial {
    pub(crate) private_key_pem: String,
    pub(crate) jwks: DynamicJwkSet,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct DynamicClientRegistrationRequest {
    pub(crate) software_id: String,
    pub(crate) jwks: DynamicJwkSet,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct DynamicClientRegistrationResponse {
    pub(crate) client_id: String,
    #[serde(default)]
    pub(crate) client_id_issued_at: Option<u64>,
    #[serde(default)]
    pub(crate) token_endpoint_auth_method: Option<String>,
    #[serde(default)]
    pub(crate) grant_types: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct DynamicClientState {
    pub(crate) client_id: String,
    pub(crate) private_key_pem: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) registration_endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) client_id_issued_at_epoch_seconds: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct DynamicJwkSet {
    pub(crate) keys: Vec<DynamicJwk>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct DynamicJwk {
    pub(crate) kty: String,
    pub(crate) n: String,
    pub(crate) e: String,
}

#[derive(Debug, Serialize)]
struct JwtHeader<'a> {
    alg: &'a str,
    typ: &'a str,
}

#[derive(Debug, Serialize)]
struct JwtBearerClaims<'a> {
    sub: &'a str,
    aud: &'a str,
    jti: String,
    nbf: u64,
    exp: u64,
    iat: u64,
    iss: &'a str,
}

pub(crate) fn generate_dynamic_client_key_material() -> Result<DynamicClientKeyMaterial> {
    let mut rng = rsa::rand_core::OsRng;
    let private_key = RsaPrivateKey::new(&mut rng, DYNAMIC_CLIENT_RSA_BITS).map_err(|error| Error::Auth {
        message: "failed to generate an RSA keypair for Epic dynamic client registration".into(),
        details: json!({
            "error": error.to_string(),
            "bits": DYNAMIC_CLIENT_RSA_BITS,
        }),
    })?;
    let public_key = RsaPublicKey::from(&private_key);
    let private_key_pem = private_key
        .to_pkcs8_pem(LineEnding::LF)
        .map_err(|error| Error::Auth {
            message: "failed to serialize the Epic dynamic client private key".into(),
            details: json!({
                "error": error.to_string(),
            }),
        })?
        .to_string();

    Ok(DynamicClientKeyMaterial {
        private_key_pem,
        jwks: DynamicJwkSet {
            keys: vec![DynamicJwk {
                kty: "RSA".into(),
                n: base64_url_encode(&public_key.n().to_bytes_be()),
                e: base64_url_encode(&public_key.e().to_bytes_be()),
            }],
        },
    })
}

pub(crate) fn dynamic_client_registration_request(
    software_id: String,
    jwks: DynamicJwkSet,
) -> DynamicClientRegistrationRequest {
    DynamicClientRegistrationRequest {
        software_id,
        jwks,
    }
}

pub(crate) fn sign_dynamic_client_assertion(
    dynamic_client_id: &str,
    token_endpoint: &str,
    private_key_pem: &str,
) -> Result<String> {
    let private_key = RsaPrivateKey::from_pkcs8_pem(private_key_pem).map_err(|error| Error::Auth {
        message: "failed to parse the stored Epic dynamic client private key".into(),
        details: json!({
            "error": error.to_string(),
        }),
    })?;
    let issued_at = current_epoch_seconds()?;
    let claims = JwtBearerClaims {
        sub: dynamic_client_id,
        aud: token_endpoint,
        jti: generate_nonce(24)?,
        nbf: issued_at,
        exp: issued_at.saturating_add(300),
        iat: issued_at,
        iss: dynamic_client_id,
    };
    let header = JwtHeader {
        alg: "RS256",
        typ: "JWT",
    };
    let header_segment = encode_jwt_component(&header)?;
    let claims_segment = encode_jwt_component(&claims)?;
    let signing_input = format!("{header_segment}.{claims_segment}");
    let signing_key = SigningKey::<Sha256>::new(private_key);
    let signature = signing_key.sign(signing_input.as_bytes());
    Ok(format!(
        "{signing_input}.{}",
        base64_url_encode(signature.to_bytes().as_ref())
    ))
}

fn current_epoch_seconds() -> Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|error| Error::Auth {
            message: "failed to determine the current time for JWT creation".into(),
            details: json!({
                "error": error.to_string(),
            }),
        })
}

fn encode_jwt_component<T>(value: &T) -> Result<String>
where
    T: Serialize,
{
    let serialized = serde_json::to_vec(value).map_err(|error| Error::Auth {
        message: "failed to serialize a JWT component for Epic dynamic client authentication".into(),
        details: json!({
            "error": error.to_string(),
        }),
    })?;
    Ok(base64_url_encode(&serialized))
}

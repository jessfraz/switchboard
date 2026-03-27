use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use clap::{Args, Subcommand};
use reqwest::Method;
use serde_json::{json, Map, Value};

use crate::{execute, execute_json, Error, MindbodyClient, ResolvedContext, Result};

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
const BASE64_ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

#[derive(Debug, Args)]
pub(crate) struct LiabilityWaiverCommand {
    #[command(subcommand)]
    pub(crate) command: LiabilityWaiverSubcommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum LiabilityWaiverSubcommand {
    Get(LocationIdArgs),
    Sign(SignLiabilityWaiverArgs),
}

#[derive(Debug, Args)]
pub(crate) struct LocationIdArgs {
    #[arg(value_name = "LOCATION_ID")]
    location_id: u64,
}

#[derive(Debug, Args)]
pub(crate) struct SignLiabilityWaiverArgs {
    #[arg(long = "booking-id")]
    booking_id: String,

    #[arg(long = "liability-waiver-hashed-text")]
    liability_waiver_hashed_text: String,

    #[arg(long = "signature-png-file", value_name = "PATH")]
    signature_png_file: PathBuf,
}

impl SignLiabilityWaiverArgs {
    fn build_body(&self) -> Result<Value> {
        let mut body = Map::new();
        body.insert("bookingId".into(), json!(self.booking_id));
        body.insert(
            "liabilityWaiverHashedText".into(),
            json!(self.liability_waiver_hashed_text),
        );
        body.insert(
            "pngBase64UserSignaturePicture".into(),
            json!(read_and_encode_signature_png(&self.signature_png_file)?),
        );
        Ok(Value::Object(body))
    }
}

pub(crate) fn run_liability_waivers(
    command: LiabilityWaiverSubcommand,
    client: &MindbodyClient,
    context: &ResolvedContext,
) -> Result<Value> {
    match command {
        LiabilityWaiverSubcommand::Get(args) => execute(
            client,
            context,
            Method::GET,
            &format!("/locations/{}/liabilitywaivers", args.location_id),
            Vec::new(),
        ),
        LiabilityWaiverSubcommand::Sign(args) => execute_json(
            client,
            context,
            Method::POST,
            "/signedliabilitywaivers",
            Vec::new(),
            args.build_body()?,
            None,
        ),
    }
}

fn read_and_encode_signature_png(path: &Path) -> Result<String> {
    let bytes = if path.as_os_str() == "-" {
        let mut bytes = Vec::new();
        std::io::stdin()
            .read_to_end(&mut bytes)
            .map_err(|error| Error::Io(format!("failed to read PNG signature from stdin: {error}")))?;
        bytes
    } else {
        fs::read(path)
            .map_err(|error| Error::Io(format!("failed to read PNG signature from {}: {error}", path.display())))?
    };

    if !bytes.starts_with(PNG_SIGNATURE) {
        return Err(Error::Arguments(
            "liability waiver signatures must be provided as PNG bytes via --signature-png-file".into(),
        ));
    }

    Ok(encode_base64(&bytes))
}

fn encode_base64(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);

    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = *chunk.get(1).unwrap_or(&0);
        let third = *chunk.get(2).unwrap_or(&0);
        let triple = (u32::from(first) << 16) | (u32::from(second) << 8) | u32::from(third);

        encoded.push(BASE64_ALPHABET[((triple >> 18) & 0x3F) as usize] as char);
        encoded.push(BASE64_ALPHABET[((triple >> 12) & 0x3F) as usize] as char);

        if chunk.len() > 1 {
            encoded.push(BASE64_ALPHABET[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            encoded.push('=');
        }

        if chunk.len() > 2 {
            encoded.push(BASE64_ALPHABET[(triple & 0x3F) as usize] as char);
        } else {
            encoded.push('=');
        }
    }

    encoded
}

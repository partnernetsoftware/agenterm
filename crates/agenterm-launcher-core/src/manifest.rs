//! MiniCon `candidate-manifest.json` (schema 1). Unknown schema or kind is
//! refused rather than guessed.

use serde::Deserialize;

use crate::{ACCEPTED_MANIFEST_SCHEMAS, MANIFEST_KIND, MANIFEST_MAX_BYTES};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Sidecar {
    pub name: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Asset {
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
    pub sidecar: Option<Sidecar>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Manifest {
    pub schema: u64,
    pub kind: String,
    pub version: String,
    pub expected_tag: Option<String>,
    pub assets: Vec<Asset>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestError {
    TooLarge(u64),
    Malformed(String),
    UnknownSchema(u64),
    WrongKind(String),
    TagMismatch { version: String, tag: String },
}

pub fn parse(body: &[u8]) -> Result<Manifest, ManifestError> {
    if body.len() as u64 > MANIFEST_MAX_BYTES {
        return Err(ManifestError::TooLarge(body.len() as u64));
    }
    let m: Manifest =
        serde_json::from_slice(body).map_err(|e| ManifestError::Malformed(e.to_string()))?;
    if !ACCEPTED_MANIFEST_SCHEMAS.contains(&m.schema) {
        return Err(ManifestError::UnknownSchema(m.schema));
    }
    if m.kind != MANIFEST_KIND {
        return Err(ManifestError::WrongKind(m.kind));
    }
    if let Some(tag) = &m.expected_tag
        && tag.strip_prefix('v') != Some(m.version.as_str())
    {
        return Err(ManifestError::TagMismatch { version: m.version.clone(), tag: tag.clone() });
    }
    Ok(m)
}

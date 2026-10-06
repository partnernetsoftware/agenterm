//! Streamed byte verification against a manifest asset, plus the signer
//! identity seam. Platform signer checks (WinVerifyTrust signer, macOS
//! TeamIdentifier) are implemented outside this crate behind `SignerCheck`.

use sha2::{Digest, Sha256};
use std::io::Read;

use crate::manifest::Asset;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyError {
    Io(String),
    /// More bytes than the manifest declares: aborted early.
    Oversize { limit: u64 },
    SizeMismatch { want: u64, got: u64 },
    HashMismatch,
    Signer(String),
}

/// Read `src` to the end, refusing to go past `asset.bytes`.
pub fn verify_stream<R: Read>(mut src: R, asset: &Asset) -> Result<(), VerifyError> {
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = src.read(&mut buf).map_err(|e| VerifyError::Io(e.to_string()))?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > asset.bytes {
            return Err(VerifyError::Oversize { limit: asset.bytes });
        }
        hasher.update(&buf[..n]);
    }
    if total != asset.bytes {
        return Err(VerifyError::SizeMismatch { want: asset.bytes, got: total });
    }
    let got: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if !got.eq_ignore_ascii_case(&asset.sha256) {
        return Err(VerifyError::HashMismatch);
    }
    Ok(())
}

/// Expected publisher identity. Values are filled at "开工" from the signing
/// skills, never guessed here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpectedSigner {
    /// Authenticode leaf certificate SHA-1 thumbprint or exact subject.
    Windows { subject: String },
    /// macOS `codesign -dv` TeamIdentifier.
    MacOs { team_id: String },
    /// Linux / APE: TLS + sha256 only (documented v1 limit).
    None,
}

pub trait SignerCheck {
    /// Must fail closed when the binary is validly signed by anyone else.
    fn check(&self, path: &std::path::Path, expected: &ExpectedSigner) -> Result<(), VerifyError>;
}

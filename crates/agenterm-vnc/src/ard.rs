//! Apple Remote Management authentication (RFB security type 30).
//!
//! This is what macOS Screen Sharing offers when it authenticates against a
//! real macOS account, and it is the reason this crate authenticates at all
//! rather than delegating to `vnc-rs`, which does not implement it.
//!
//! The exchange, as the server drives it:
//!
//! 1. Server sends `generator` (2 bytes), `key_length` (2 bytes), then a
//!    `key_length`-byte prime modulus and its `key_length`-byte public key.
//! 2. Both sides perform Diffie-Hellman over that group.
//! 3. `MD5(shared_secret)` becomes a 128-bit AES key.
//! 4. The client builds a 128-byte plaintext holding the username at offset 0
//!    and the password at offset 64, each NUL-terminated, padded with random
//!    bytes, and encrypts it with AES-128-ECB.
//! 5. The client sends the 128-byte ciphertext followed by its own public key.
//!
//! A standard `SecurityResult` word follows, which the caller validates.

use aes::Aes128;
use cipher::{Array, BlockCipherEncrypt, KeyInit};
use md5::{Digest, Md5};
use num_bigint::BigUint;
use rand::TryRngCore;

/// The fixed size of the credentials block, per the protocol.
const CREDENTIALS_LEN: usize = 128;
/// Username and password each get a 64-byte field, NUL-terminated, so the
/// longest credential that fits is 63 bytes.
const FIELD_LEN: usize = 64;
const MAX_CREDENTIAL_LEN: usize = FIELD_LEN - 1;

/// What the client must send back to complete the exchange.
pub struct ArdResponse {
    /// AES-encrypted credentials, always [`CREDENTIALS_LEN`] bytes.
    pub ciphertext: Vec<u8>,
    /// The client's Diffie-Hellman public key, `key_length` bytes.
    pub public_key: Vec<u8>,
}

/// Deliberately hand-written: the ciphertext is derived from the user's
/// password, so it must never reach a log line or an assertion message.
impl std::fmt::Debug for ArdResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArdResponse")
            .field("ciphertext", &format_args!("<{} bytes redacted>", self.ciphertext.len()))
            .field("public_key", &format_args!("<{} bytes>", self.public_key.len()))
            .finish()
    }
}

/// Why an ARD exchange could not be completed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArdError {
    /// The username or password exceeds the 63-byte field the protocol allows.
    CredentialTooLong,
    /// The server proposed parameters this client will not use.
    BadParameters(&'static str),
}

/// Compute the client's reply to an ARD challenge.
///
/// `private_key` is injected rather than generated so tests can pin it; the
/// live path uses [`random_private_key`].
pub fn respond(
    generator: &[u8],
    prime: &[u8],
    server_public_key: &[u8],
    private_key: &[u8],
    username: &str,
    password: &str,
    padding: [u8; CREDENTIALS_LEN],
) -> Result<ArdResponse, ArdError> {
    let key_length = prime.len();
    if key_length == 0 {
        return Err(ArdError::BadParameters("the server sent an empty prime modulus"));
    }
    if server_public_key.len() != key_length {
        return Err(ArdError::BadParameters(
            "the server's public key length does not match the prime",
        ));
    }
    if username.len() > MAX_CREDENTIAL_LEN || password.len() > MAX_CREDENTIAL_LEN {
        return Err(ArdError::CredentialTooLong);
    }

    let generator = BigUint::from_bytes_be(generator);
    let prime_value = BigUint::from_bytes_be(prime);
    let private = BigUint::from_bytes_be(private_key);

    // Our public key, and the shared secret from the server's.
    let public = generator.modpow(&private, &prime_value);
    let shared = BigUint::from_bytes_be(server_public_key).modpow(&private, &prime_value);

    // Both values travel as fixed-width, left-zero-padded big-endian numbers:
    // a short secret must still hash as `key_length` bytes or the derived key
    // silently differs from the server's.
    let shared = left_pad(&shared.to_bytes_be(), key_length);
    let public = left_pad(&public.to_bytes_be(), key_length);

    let aes_key = Md5::digest(&shared);
    let credentials = build_credentials(username, password, padding);

    let cipher = Aes128::new(&Array(aes_key.into()));
    let mut ciphertext = credentials;
    // ECB: every 16-byte block is encrypted independently, which is what the
    // protocol specifies here regardless of ECB's general reputation.
    for chunk in ciphertext.chunks_exact_mut(16) {
        let mut block = Array(<[u8; 16]>::try_from(&*chunk).expect("a sixteen byte block"));
        cipher.encrypt_block(&mut block);
        chunk.copy_from_slice(&block.0);
    }

    Ok(ArdResponse { ciphertext: ciphertext.to_vec(), public_key: public })
}

/// Lay the credentials into the fixed 128-byte block.
fn build_credentials(
    username: &str,
    password: &str,
    padding: [u8; CREDENTIALS_LEN],
) -> [u8; CREDENTIALS_LEN] {
    // Starting from random padding means the unused tail of each field carries
    // no information about the credential's length.
    let mut block = padding;
    block[..username.len()].copy_from_slice(username.as_bytes());
    block[username.len()] = 0;
    block[FIELD_LEN..FIELD_LEN + password.len()].copy_from_slice(password.as_bytes());
    block[FIELD_LEN + password.len()] = 0;
    block
}

/// Left-pad a big-endian number to an exact width.
fn left_pad(value: &[u8], width: usize) -> Vec<u8> {
    if value.len() >= width {
        // A value wider than the modulus cannot occur for a reduced result;
        // taking the low bytes keeps this total rather than panicking.
        return value[value.len() - width..].to_vec();
    }
    let mut padded = vec![0u8; width];
    padded[width - value.len()..].copy_from_slice(value);
    padded
}

/// A fresh Diffie-Hellman private key of the negotiated width.
pub(crate) fn random_private_key(key_length: usize) -> Vec<u8> {
    let mut key = vec![0u8; key_length];
    rand::rngs::OsRng
        .try_fill_bytes(&mut key)
        .expect("the operating system random source is available");
    key
}

/// Random padding for the credentials block.
pub(crate) fn random_padding() -> [u8; CREDENTIALS_LEN] {
    let mut padding = [0u8; CREDENTIALS_LEN];
    rand::rngs::OsRng
        .try_fill_bytes(&mut padding)
        .expect("the operating system random source is available");
    padding
}

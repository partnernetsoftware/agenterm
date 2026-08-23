//! Apple Remote Management conformance.
//!
//! The expected values come from an independent Python implementation
//! (`hashlib` MD5 + pycryptodome AES + plain integer `pow`) that shares no
//! code with this crate, so agreement is evidence about the protocol rather
//! than about a single implementation agreeing with itself.

use agenterm_vnc::testing::{ArdError, respond};

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("hex"))
        .collect()
}

const PRIME: &str = "0c037c37588b4329887e61c2da3324b1ba4b81a63f9748fed2d8a410c2fc21b1232f0d3bfa024276cfd88448197aae486a63bfca7b8bf7754dfb327c7201f6fd";
const GENERATOR: &str = "0002";
const PRIVATE: &str = "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f40";
const SERVER_PUBLIC: &str = "0762ca00083afc01a18c33c045a405da4ea2d3b07c5f60b6fca27a9e40777f22c33e99e3408b74025c0876344dc8c7931dd4401bea821b4967c72c525bf8b499";
const PADDING: &str = "030a11181f262d343b424950575e656c737a81888f969da4abb2b9c0c7ced5dce3eaf1f8ff060d141b222930373e454c535a61686f767d848b9299a0a7aeb5bcc3cad1d8dfe6edf4fb020910171e252c333a41484f565d646b727980878e959ca3aab1b8bfc6cdd4dbe2e9f0f7fe050c131a21282f363d444b525960676e757c";
const EXPECTED_CIPHERTEXT: &str = "373179e77103dd6f0cedc4a771946909ab758d85f0dc6d5693dd0c38d020279d2b3c3ee96797bb95a1bbd97a84da72a53fdc9a0b3519360c3a65f9ad39b4149ef20f4ad7901db1603e7a0b810542592f3b4352c13b75f171b9a47f853c89a4b5a0c4c792b79ddb2253f6e5b97a36213dd2e633d2e4cd00aba548ebf2c3fa68a1";
const EXPECTED_PUBLIC: &str = "07f4593adf128e952e3fd4b6430b7fc38d3f93d8a5576322e5caf0406362066974bb6b3e7dc575490dc96d1c08c6f1a6d8df954ad8029fcfaded5ff968e2986b";

fn fixed_padding() -> [u8; 128] {
    unhex(PADDING).try_into().expect("128 bytes")
}

#[test]
fn matches_an_independent_implementation() {
    let response = respond(
        &unhex(GENERATOR),
        &unhex(PRIME),
        &unhex(SERVER_PUBLIC),
        &unhex(PRIVATE),
        "admin",
        "hunter2",
        fixed_padding(),
    )
    .expect("a well-formed exchange");

    assert_eq!(response.ciphertext, unhex(EXPECTED_CIPHERTEXT), "ciphertext");
    assert_eq!(response.public_key, unhex(EXPECTED_PUBLIC), "client public key");
}

#[test]
fn the_public_key_is_padded_to_the_prime_width() {
    let response = respond(
        &unhex(GENERATOR),
        &unhex(PRIME),
        &unhex(SERVER_PUBLIC),
        &unhex(PRIVATE),
        "admin",
        "hunter2",
        fixed_padding(),
    )
    .expect("a well-formed exchange");
    // A short public key must still occupy the full width, or the server
    // reads it misaligned and authentication fails for no visible reason.
    assert_eq!(response.public_key.len(), unhex(PRIME).len());
    assert_eq!(response.ciphertext.len(), 128);
}

#[test]
fn an_over_long_credential_is_refused_rather_than_truncated() {
    let long = "x".repeat(64);
    let result = respond(
        &unhex(GENERATOR),
        &unhex(PRIME),
        &unhex(SERVER_PUBLIC),
        &unhex(PRIVATE),
        &long,
        "hunter2",
        fixed_padding(),
    );
    // Silently truncating would send a credential the user never typed.
    assert_eq!(result.err(), Some(ArdError::CredentialTooLong));
}

#[test]
fn a_mismatched_server_key_length_is_refused() {
    let result = respond(
        &unhex(GENERATOR),
        &unhex(PRIME),
        &unhex(SERVER_PUBLIC)[..8],
        &unhex(PRIVATE),
        "admin",
        "hunter2",
        fixed_padding(),
    );
    assert!(matches!(result, Err(ArdError::BadParameters(_))), "got {result:?}");
}

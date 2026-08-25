//! DES conformance against published vectors.
//!
//! The preflight authenticates for real, so this cipher has to be exactly
//! right: a wrong result reads as "bad password" against every VNC server.

// The test reaches the private module through the crate's test-only export.
use agenterm_vnc::testing::{encrypt_challenge, key_from_password};

/// Encrypt one block by feeding it as the first half of a 16-byte challenge.
fn des_block(block: [u8; 8], key: [u8; 8]) -> [u8; 8] {
    let mut challenge = [0u8; 16];
    challenge[..8].copy_from_slice(&block);
    let out = encrypt_challenge(&challenge, &key);
    out[..8].try_into().expect("eight bytes")
}

#[test]
fn matches_the_fips_known_answer_vector() {
    // FIPS 81 / the classic DES worked example.
    let key = [0x13, 0x34, 0x57, 0x79, 0x9b, 0xbc, 0xdf, 0xf1];
    let plaintext = [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef];
    assert_eq!(
        des_block(plaintext, key),
        [0x85, 0xe8, 0x13, 0x54, 0x0f, 0x0a, 0xb4, 0x05]
    );
}

#[test]
fn matches_the_all_zero_vector() {
    assert_eq!(
        des_block([0; 8], [0; 8]),
        [0x8c, 0xa6, 0x4d, 0xe9, 0xc1, 0xb1, 0x23, 0xa7]
    );
}

#[test]
fn matches_the_all_ones_vector() {
    assert_eq!(
        des_block([0xff; 8], [0xff; 8]),
        [0x73, 0x59, 0xb2, 0x16, 0x3e, 0x4e, 0xdc, 0x58]
    );
}

#[test]
fn a_password_key_reverses_each_byte_and_zero_pads() {
    // 'a' is 0x61 = 0b0110_0001; reversed it is 0b1000_0110 = 0x86.
    assert_eq!(key_from_password("a"), [0x86, 0, 0, 0, 0, 0, 0, 0]);
    // Longer than eight characters truncates rather than wrapping.
    assert_eq!(key_from_password("123456789")[..8].len(), 8);
}

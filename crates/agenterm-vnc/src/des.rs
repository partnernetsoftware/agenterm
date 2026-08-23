//! The DES variant RFB uses for VNC Authentication.
//!
//! This exists because the preflight has to complete a real authentication
//! exchange (see [`crate::preflight`]) and `vnc-rs` keeps its own DES private.
//! It is standard DES/ECB on a single 8-byte block; RFB's only quirk is that
//! the password bytes are bit-reversed before becoming the key, which the
//! caller does when building the key.

/// An 8-byte DES key.
pub type Key = [u8; 8];

const PC1: [u8; 56] = [
    57, 49, 41, 33, 25, 17, 9, 1, 58, 50, 42, 34, 26, 18, 10, 2, 59, 51, 43, 35, 27, 19, 11, 3, 60,
    52, 44, 36, 63, 55, 47, 39, 31, 23, 15, 7, 62, 54, 46, 38, 30, 22, 14, 6, 61, 53, 45, 37, 29,
    21, 13, 5, 28, 20, 12, 4,
];

const PC2: [u8; 48] = [
    14, 17, 11, 24, 1, 5, 3, 28, 15, 6, 21, 10, 23, 19, 12, 4, 26, 8, 16, 7, 27, 20, 13, 2, 41, 52,
    31, 37, 47, 55, 30, 40, 51, 45, 33, 48, 44, 49, 39, 56, 34, 53, 46, 42, 50, 36, 29, 32,
];

const IP: [u8; 64] = [
    58, 50, 42, 34, 26, 18, 10, 2, 60, 52, 44, 36, 28, 20, 12, 4, 62, 54, 46, 38, 30, 22, 14, 6,
    64, 56, 48, 40, 32, 24, 16, 8, 57, 49, 41, 33, 25, 17, 9, 1, 59, 51, 43, 35, 27, 19, 11, 3, 61,
    53, 45, 37, 29, 21, 13, 5, 63, 55, 47, 39, 31, 23, 15, 7,
];

const FP: [u8; 64] = [
    40, 8, 48, 16, 56, 24, 64, 32, 39, 7, 47, 15, 55, 23, 63, 31, 38, 6, 46, 14, 54, 22, 62, 30,
    37, 5, 45, 13, 53, 21, 61, 29, 36, 4, 44, 12, 52, 20, 60, 28, 35, 3, 43, 11, 51, 19, 59, 27,
    34, 2, 42, 10, 50, 18, 58, 26, 33, 1, 41, 9, 49, 17, 57, 25,
];

const E: [u8; 48] = [
    32, 1, 2, 3, 4, 5, 4, 5, 6, 7, 8, 9, 8, 9, 10, 11, 12, 13, 12, 13, 14, 15, 16, 17, 16, 17, 18,
    19, 20, 21, 20, 21, 22, 23, 24, 25, 24, 25, 26, 27, 28, 29, 28, 29, 30, 31, 32, 1,
];

const P: [u8; 32] = [
    16, 7, 20, 21, 29, 12, 28, 17, 1, 15, 23, 26, 5, 18, 31, 10, 2, 8, 24, 14, 32, 27, 3, 9, 19,
    13, 30, 6, 22, 11, 4, 25,
];

const SHIFTS: [u32; 16] = [1, 1, 2, 2, 2, 2, 2, 2, 1, 2, 2, 2, 2, 2, 2, 1];

const S: [[u8; 64]; 8] = [
    [
        14, 4, 13, 1, 2, 15, 11, 8, 3, 10, 6, 12, 5, 9, 0, 7, 0, 15, 7, 4, 14, 2, 13, 1, 10, 6, 12,
        11, 9, 5, 3, 8, 4, 1, 14, 8, 13, 6, 2, 11, 15, 12, 9, 7, 3, 10, 5, 0, 15, 12, 8, 2, 4, 9,
        1, 7, 5, 11, 3, 14, 10, 0, 6, 13,
    ],
    [
        15, 1, 8, 14, 6, 11, 3, 4, 9, 7, 2, 13, 12, 0, 5, 10, 3, 13, 4, 7, 15, 2, 8, 14, 12, 0, 1,
        10, 6, 9, 11, 5, 0, 14, 7, 11, 10, 4, 13, 1, 5, 8, 12, 6, 9, 3, 2, 15, 13, 8, 10, 1, 3, 15,
        4, 2, 11, 6, 7, 12, 0, 5, 14, 9,
    ],
    [
        10, 0, 9, 14, 6, 3, 15, 5, 1, 13, 12, 7, 11, 4, 2, 8, 13, 7, 0, 9, 3, 4, 6, 10, 2, 8, 5,
        14, 12, 11, 15, 1, 13, 6, 4, 9, 8, 15, 3, 0, 11, 1, 2, 12, 5, 10, 14, 7, 1, 10, 13, 0, 6,
        9, 8, 7, 4, 15, 14, 3, 11, 5, 2, 12,
    ],
    [
        7, 13, 14, 3, 0, 6, 9, 10, 1, 2, 8, 5, 11, 12, 4, 15, 13, 8, 11, 5, 6, 15, 0, 3, 4, 7, 2,
        12, 1, 10, 14, 9, 10, 6, 9, 0, 12, 11, 7, 13, 15, 1, 3, 14, 5, 2, 8, 4, 3, 15, 0, 6, 10, 1,
        13, 8, 9, 4, 5, 11, 12, 7, 2, 14,
    ],
    [
        2, 12, 4, 1, 7, 10, 11, 6, 8, 5, 3, 15, 13, 0, 14, 9, 14, 11, 2, 12, 4, 7, 13, 1, 5, 0, 15,
        10, 3, 9, 8, 6, 4, 2, 1, 11, 10, 13, 7, 8, 15, 9, 12, 5, 6, 3, 0, 14, 11, 8, 12, 7, 1, 14,
        2, 13, 6, 15, 0, 9, 10, 4, 5, 3,
    ],
    [
        12, 1, 10, 15, 9, 2, 6, 8, 0, 13, 3, 4, 14, 7, 5, 11, 10, 15, 4, 2, 7, 12, 9, 5, 6, 1, 13,
        14, 0, 11, 3, 8, 9, 14, 15, 5, 2, 8, 12, 3, 7, 0, 4, 10, 1, 13, 11, 6, 4, 3, 2, 12, 9, 5,
        15, 10, 11, 14, 1, 7, 6, 0, 8, 13,
    ],
    [
        4, 11, 2, 14, 15, 0, 8, 13, 3, 12, 9, 7, 5, 10, 6, 1, 13, 0, 11, 7, 4, 9, 1, 10, 14, 3, 5,
        12, 2, 15, 8, 6, 1, 4, 11, 13, 12, 3, 7, 14, 10, 15, 6, 8, 0, 5, 9, 2, 6, 11, 13, 8, 1, 4,
        10, 7, 9, 5, 0, 15, 14, 2, 3, 12,
    ],
    [
        13, 2, 8, 4, 6, 15, 11, 1, 10, 9, 3, 14, 5, 0, 12, 7, 1, 15, 13, 8, 10, 3, 7, 4, 12, 5, 6,
        11, 0, 14, 9, 2, 7, 11, 4, 1, 9, 12, 14, 2, 0, 6, 10, 13, 15, 3, 5, 8, 2, 1, 14, 7, 4, 10,
        8, 13, 15, 12, 9, 0, 3, 5, 6, 11,
    ],
];

/// Read bit `position` (1-based, MSB first) out of a 64-bit block.
fn bit(block: u64, position: u8) -> u64 {
    (block >> (64 - position)) & 1
}

/// Apply a bit permutation table, producing an `output_bits`-wide value in the
/// high bits of a u64 (the representation every stage here shares).
fn permute(block: u64, table: &[u8], output_bits: u32) -> u64 {
    let mut result = 0u64;
    for (index, &source) in table.iter().enumerate() {
        result |= bit(block, source) << (63 - index as u32);
    }
    let _ = output_bits;
    result
}

/// Derive the sixteen 48-bit round subkeys from a key.
fn subkeys(key: &Key) -> [u64; 16] {
    let key_block = u64::from_be_bytes(*key);
    let permuted = permute(key_block, &PC1, 56);
    // C and D are the two 28-bit halves, held in the low bits for rotation.
    let mut c = (permuted >> 36) & 0x0fff_ffff;
    let mut d = (permuted >> 8) & 0x0fff_ffff;

    let mut result = [0u64; 16];
    for (round, shift) in SHIFTS.iter().enumerate() {
        c = ((c << shift) | (c >> (28 - shift))) & 0x0fff_ffff;
        d = ((d << shift) | (d >> (28 - shift))) & 0x0fff_ffff;
        // Rejoin into the top 56 bits so PC2's 1-based indices line up.
        let combined = ((c << 28) | d) << 8;
        result[round] = permute(combined, &PC2, 48);
    }
    result
}

/// The DES round function: expand, mix with the subkey, substitute, permute.
fn feistel(right: u32, subkey: u64) -> u32 {
    let expanded = permute(u64::from(right) << 32, &E, 48) ^ subkey;
    let mut substituted = 0u64;
    for box_index in 0..8 {
        // Each S-box consumes six bits and yields four.
        let six = (expanded >> (58 - box_index * 6)) & 0x3f;
        let row = ((six & 0x20) >> 4) | (six & 1);
        let column = (six >> 1) & 0x0f;
        let value = u64::from(S[box_index as usize][(row * 16 + column) as usize]);
        substituted |= value << (60 - box_index * 4);
    }
    (permute(substituted, &P, 32) >> 32) as u32
}

/// Encrypt one 8-byte block with DES/ECB.
fn encrypt_block(block: [u8; 8], key: &Key) -> [u8; 8] {
    let keys = subkeys(key);
    let permuted = permute(u64::from_be_bytes(block), &IP, 64);
    let mut left = (permuted >> 32) as u32;
    let mut right = permuted as u32;

    for subkey in keys {
        let next = left ^ feistel(right, subkey);
        left = right;
        right = next;
    }

    // The halves are swapped once more before the final permutation.
    let preoutput = (u64::from(right) << 32) | u64::from(left);
    permute(preoutput, &FP, 64).to_be_bytes()
}

/// Encrypt a 16-byte RFB challenge as two independent ECB blocks.
pub fn encrypt_challenge(challenge: &[u8; 16], key: &Key) -> [u8; 16] {
    let mut result = [0u8; 16];
    for (chunk, out) in challenge.chunks_exact(8).zip(result.chunks_exact_mut(8)) {
        let mut block = [0u8; 8];
        block.copy_from_slice(chunk);
        out.copy_from_slice(&encrypt_block(block, key));
    }
    result
}

/// Turn a password into an RFB DES key.
///
/// RFB truncates or zero-pads to eight bytes and reverses the bits of each,
/// a quirk of the original implementation that every server reproduces.
pub fn key_from_password(password: &str) -> Key {
    let bytes = password.as_bytes();
    let mut key = [0u8; 8];
    for (index, slot) in key.iter_mut().enumerate() {
        *slot = bytes.get(index).copied().unwrap_or(0).reverse_bits();
    }
    key
}

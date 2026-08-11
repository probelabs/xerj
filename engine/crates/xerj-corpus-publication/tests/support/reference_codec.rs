use base64::{engine::general_purpose::STANDARD, Engine as _};
use sha2::{Digest, Sha256};

pub fn u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_be_bytes());
}

pub fn u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_be_bytes());
}

pub fn s(out: &mut Vec<u8>, value: &[u8]) {
    u64(out, value.len() as u64);
    out.extend_from_slice(value);
}

pub fn a(out: &mut Vec<u8>, values: &[Vec<u8>]) {
    u64(out, values.len() as u64);
    for value in values {
        out.extend_from_slice(value);
    }
}

pub fn rendered(prefix: &str, bytes: &[u8]) -> String {
    format!("{prefix}{:x}", Sha256::digest(bytes))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn padded_base64(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

pub fn decode_padded_base64(encoded: &str) -> Vec<u8> {
    let decoded = STANDARD
        .decode(encoded)
        .expect("valid padded standard base64");
    assert_eq!(
        STANDARD.encode(&decoded),
        encoded,
        "base64 must re-encode exactly"
    );
    decoded
}

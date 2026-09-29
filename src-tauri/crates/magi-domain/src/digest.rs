use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::{DomainError, ValidationIssue};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Digest(String);

impl Digest {
    pub fn from_bytes(bytes: &[u8]) -> Self {
        let hash = Sha256::digest(bytes);
        Self(encode_hex(&hash))
    }

    pub fn from_hex(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(DomainError::Validation(vec![ValidationIssue::new(
                "digest",
                "digest_format",
                "SHA-256 digest must contain exactly 64 hexadecimal characters",
            )]));
        }
        Ok(Self(value.to_ascii_lowercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_valid(&self) -> bool {
        self.0.len() == 64
            && self
                .0
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }
}

impl std::fmt::Display for Digest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

pub fn canonical_json<T: Serialize>(value: &T) -> Result<Vec<u8>, DomainError> {
    let value = serde_json::to_value(value)?;
    let mut output = Vec::new();
    write_canonical(&value, &mut output)?;
    Ok(output)
}

fn write_canonical(value: &serde_json::Value, output: &mut Vec<u8>) -> Result<(), DomainError> {
    match value {
        serde_json::Value::Null => output.extend_from_slice(b"null"),
        serde_json::Value::Bool(value) => {
            output.extend_from_slice(if *value { &b"true"[..] } else { &b"false"[..] })
        }
        serde_json::Value::Number(value) => {
            if value.is_i64() || value.is_u64() {
                output.extend_from_slice(value.to_string().as_bytes());
            } else {
                return Err(DomainError::Validation(vec![ValidationIssue::new(
                    "canonical_json",
                    "unsupported_number",
                    "hashed MAGI contracts use integer numeric values only",
                )]));
            }
        }
        serde_json::Value::String(value) => {
            output.extend_from_slice(serde_json::to_string(value)?.as_bytes())
        }
        serde_json::Value::Array(values) => {
            output.push(b'[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output.push(b',');
                }
                write_canonical(value, output)?;
            }
            output.push(b']');
        }
        serde_json::Value::Object(values) => {
            let mut entries: Vec<_> = values.iter().collect();
            entries.sort_by(|(left, _), (right, _)| left.encode_utf16().cmp(right.encode_utf16()));
            output.push(b'{');
            for (index, (key, value)) in entries.into_iter().enumerate() {
                if index > 0 {
                    output.push(b',');
                }
                output.extend_from_slice(serde_json::to_string(key)?.as_bytes());
                output.push(b':');
                write_canonical(value, output)?;
            }
            output.push(b'}');
        }
    }
    Ok(())
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

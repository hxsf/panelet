// SPDX-License-Identifier: MPL-2.0

//! Fixed Panelet container metadata for a precompiled Rune `Unit<ArrayUnit>`.
//!
//! The format is intentionally narrow and versioned. It is an integrity
//! envelope for the footprint experiment, not a production authorization
//! format: production must add a signature, trusted-key rotation and
//! anti-rollback policy before accepting packages uploaded over HTTP.

#![no_std]

use core::fmt;

use sha2::{Digest, Sha256};

/// `N4CRUNE\0`, making accidental interpretation as a bare encoded Unit blob less
/// likely.
pub const MAGIC: [u8; 8] = *b"N4CRUNE\0";
pub const CONTAINER_FORMAT_VERSION: u16 = 1;
pub const RUNE_VERSION: [u8; 3] = [0, 14, 2];
pub const ARRAY_UNIT_STORAGE: u8 = 1;
/// CBOR supports the unknown-length maps emitted by Rune's 0.14.2 `Unit`
/// serializer. Bincode 1.3.3 rejects that serializer at runtime.
pub const CBOR_CODEC: u8 = 1;
pub const RUNTIME_ABI_VERSION: u16 = 1;
/// `core + clone + cmp + vec`, with no Panelet native host functions yet.
pub const MODULE_SET_ID: [u8; 16] = *b"N4C-CMCV-ABI-01!";
pub const HEADER_LEN: usize = 72;
/// A deliberately conservative PoC transport/storage ceiling. This guards the
/// container before Serde sees its input; the Rune allocator limit adds a
/// separate bound while building the decoded `Unit`.
pub const MAX_UNIT_PAYLOAD_BYTES: usize = 64 * 1024;
pub const MAX_UNIT_BLOB_BYTES: usize = HEADER_LEN + MAX_UNIT_PAYLOAD_BYTES;

const VERSION_RANGE: core::ops::Range<usize> = 8..10;
const RUNE_VERSION_RANGE: core::ops::Range<usize> = 10..13;
const STORAGE_OFFSET: usize = 13;
const RUNTIME_ABI_RANGE: core::ops::Range<usize> = 14..16;
const MODULE_SET_RANGE: core::ops::Range<usize> = 16..32;
const PAYLOAD_LEN_RANGE: core::ops::Range<usize> = 32..36;
const PAYLOAD_SHA256_RANGE: core::ops::Range<usize> = 36..68;
const CODEC_OFFSET: usize = 68;
const RESERVED_RANGE: core::ops::Range<usize> = 69..72;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Header {
    payload_len: u32,
    payload_sha256: [u8; 32],
}

impl Header {
    pub fn for_payload(payload: &[u8]) -> Result<Self, HeaderError> {
        if payload.len() > MAX_UNIT_PAYLOAD_BYTES {
            return Err(HeaderError::PayloadTooLarge {
                actual: payload.len(),
            });
        }

        let payload_len =
            u32::try_from(payload.len()).map_err(|_| HeaderError::PayloadTooLarge {
                actual: payload.len(),
            })?;

        Ok(Self {
            payload_len,
            payload_sha256: sha256(payload),
        })
    }

    pub fn encode(self) -> [u8; HEADER_LEN] {
        let mut bytes = [0; HEADER_LEN];
        bytes[..MAGIC.len()].copy_from_slice(&MAGIC);
        bytes[VERSION_RANGE].copy_from_slice(&CONTAINER_FORMAT_VERSION.to_le_bytes());
        bytes[RUNE_VERSION_RANGE].copy_from_slice(&RUNE_VERSION);
        bytes[STORAGE_OFFSET] = ARRAY_UNIT_STORAGE;
        bytes[RUNTIME_ABI_RANGE].copy_from_slice(&RUNTIME_ABI_VERSION.to_le_bytes());
        bytes[MODULE_SET_RANGE].copy_from_slice(&MODULE_SET_ID);
        bytes[PAYLOAD_LEN_RANGE].copy_from_slice(&self.payload_len.to_le_bytes());
        bytes[PAYLOAD_SHA256_RANGE].copy_from_slice(&self.payload_sha256);
        bytes[CODEC_OFFSET] = CBOR_CODEC;
        bytes
    }

    /// Validate fixed metadata, the advertised length and the payload digest;
    /// return the exact encoded payload only after all checks pass.
    pub fn validate_blob<'a>(blob: &'a [u8]) -> Result<&'a [u8], HeaderError> {
        if blob.len() > MAX_UNIT_BLOB_BYTES {
            return Err(HeaderError::BlobTooLarge { actual: blob.len() });
        }

        if blob.len() < HEADER_LEN {
            return Err(HeaderError::TooShort { actual: blob.len() });
        }

        if blob[..MAGIC.len()] != MAGIC {
            return Err(HeaderError::Magic);
        }

        let format_version = u16::from_le_bytes(blob[VERSION_RANGE].try_into().unwrap());
        if format_version != CONTAINER_FORMAT_VERSION {
            return Err(HeaderError::ContainerVersion { format_version });
        }

        if blob[RUNE_VERSION_RANGE] != RUNE_VERSION {
            return Err(HeaderError::RuneVersion);
        }

        if blob[STORAGE_OFFSET] != ARRAY_UNIT_STORAGE {
            return Err(HeaderError::Storage);
        }

        if blob[CODEC_OFFSET] != CBOR_CODEC {
            return Err(HeaderError::Codec);
        }

        let runtime_abi = u16::from_le_bytes(blob[RUNTIME_ABI_RANGE].try_into().unwrap());
        if runtime_abi != RUNTIME_ABI_VERSION {
            return Err(HeaderError::RuntimeAbi { runtime_abi });
        }

        if blob[MODULE_SET_RANGE] != MODULE_SET_ID {
            return Err(HeaderError::ModuleSet);
        }

        if blob[RESERVED_RANGE].iter().any(|byte| *byte != 0) {
            return Err(HeaderError::Reserved);
        }

        let payload_len = u32::from_le_bytes(blob[PAYLOAD_LEN_RANGE].try_into().unwrap()) as usize;
        if payload_len > MAX_UNIT_PAYLOAD_BYTES {
            return Err(HeaderError::PayloadTooLarge {
                actual: payload_len,
            });
        }

        let expected_total =
            HEADER_LEN
                .checked_add(payload_len)
                .ok_or(HeaderError::PayloadTooLarge {
                    actual: payload_len,
                })?;

        if blob.len() != expected_total {
            return Err(HeaderError::LengthMismatch {
                expected: expected_total,
                actual: blob.len(),
            });
        }

        let payload = &blob[HEADER_LEN..];
        let expected_sha256: [u8; 32] = blob[PAYLOAD_SHA256_RANGE].try_into().unwrap();
        if sha256(payload) != expected_sha256 {
            return Err(HeaderError::Hash);
        }

        Ok(payload)
    }

    pub fn payload_sha256_from_blob(blob: &[u8]) -> Result<[u8; 32], HeaderError> {
        let payload = Self::validate_blob(blob)?;
        Ok(sha256(payload))
    }
}

/// SHA-256 is an integrity checksum here. It does not authenticate an uploader.
pub fn sha256(payload: &[u8]) -> [u8; 32] {
    let digest = Sha256::digest(payload);
    let mut bytes = [0; 32];
    bytes.copy_from_slice(&digest);
    bytes
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HeaderError {
    BlobTooLarge { actual: usize },
    TooShort { actual: usize },
    Magic,
    ContainerVersion { format_version: u16 },
    RuneVersion,
    Storage,
    Codec,
    RuntimeAbi { runtime_abi: u16 },
    ModuleSet,
    Reserved,
    PayloadTooLarge { actual: usize },
    LengthMismatch { expected: usize, actual: usize },
    Hash,
}

impl fmt::Display for HeaderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BlobTooLarge { actual } => {
                write!(formatter, "container is too large: {actual} bytes")
            }
            Self::TooShort { actual } => {
                write!(formatter, "container is too short: {actual} bytes")
            }
            Self::Magic => formatter.write_str("container magic does not match"),
            Self::ContainerVersion { format_version } => {
                write!(
                    formatter,
                    "unsupported container format version: {format_version}"
                )
            }
            Self::RuneVersion => formatter.write_str("Rune version does not match"),
            Self::Storage => formatter.write_str("Unit storage kind does not match ArrayUnit"),
            Self::Codec => formatter.write_str("payload codec does not match CBOR"),
            Self::RuntimeAbi { runtime_abi } => {
                write!(formatter, "runtime ABI does not match: {runtime_abi}")
            }
            Self::ModuleSet => formatter.write_str("minimal module set does not match"),
            Self::Reserved => formatter.write_str("reserved header bytes are non-zero"),
            Self::PayloadTooLarge { actual } => {
                write!(formatter, "payload is too large: {actual} bytes")
            }
            Self::LengthMismatch { expected, actual } => {
                write!(
                    formatter,
                    "container length mismatch: expected {expected}, got {actual}"
                )
            }
            Self::Hash => formatter.write_str("payload SHA-256 does not match"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoded_header_accepts_its_exact_payload() {
        let payload = b"unit";
        let header = Header::for_payload(payload).unwrap().encode();
        let mut blob = header.to_vec();
        blob.extend_from_slice(payload);
        assert_eq!(Header::validate_blob(&blob).unwrap(), payload);
    }
}

//! Encoding and length validation for persisted records.
//!
//! Schema v1 uses JSON for structured records because it is inspectable and
//! straightforward to migrate. Raw bodies stay in separate tables as bytes;
//! they are never embedded in JSON.

use serde::Serialize;
use serde::de::DeserializeOwned;

use super::{ProjectError, Result};

/// A single structured record is not expected to exceed this size. The limit
/// keeps a corrupt or hostile file from causing an unbounded allocation.
pub const MAX_RECORD_BYTES: usize = 32 * 1024 * 1024;

pub fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > MAX_RECORD_BYTES {
        return Err(ProjectError::RecordTooLarge {
            size: bytes.len(),
            limit: MAX_RECORD_BYTES,
        });
    }
    Ok(bytes)
}

pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    if bytes.len() > MAX_RECORD_BYTES {
        return Err(ProjectError::RecordTooLarge {
            size: bytes.len(),
            limit: MAX_RECORD_BYTES,
        });
    }
    Ok(serde_json::from_slice(bytes)?)
}

/// Encode a value that is split into bounded chunks before storage.
///
/// Only the workspace uses this today: its retained result bodies can add up
/// to more than [`MAX_RECORD_BYTES`], so the persistence layer stores the JSON
/// across several entries and applies its own read cap.
pub fn encode_large<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec(value)?)
}

/// Decode a value assembled from bounded chunks. See [`encode_large`].
pub fn decode_large<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    Ok(serde_json::from_slice(bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    struct Sample {
        value: u64,
    }

    #[test]
    fn round_trips_and_rejects_oversized_values() {
        let sample = Sample { value: 7 };
        let bytes = encode(&sample).expect("encode");
        let decoded: Sample = decode(&bytes).expect("decode");
        assert_eq!(sample, decoded);

        let huge = vec![b' '; MAX_RECORD_BYTES + 1];
        assert!(decode::<Sample>(&huge).is_err());
    }
}

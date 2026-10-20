//! Raw-byte names in JSON.
//!
//! A JSON string cannot hold an arbitrary filename, so byte fields carry
//! base64 and paths carry base64 of their `OsStr` bytes. Nothing here
//! is lossily converted.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Deserializer, Serializer};

/// Base64 of raw name bytes, for callers that shape their own JSON.
pub fn encode(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

pub(crate) fn decode(text: &str) -> Result<Vec<u8>, base64::DecodeError> {
    STANDARD.decode(text)
}

// serde's `with` contract fixes the field type, so `&Vec<u8>` is not a
// needless borrow here.
#[allow(clippy::ptr_arg)]
pub(crate) fn serialize<S: Serializer>(bytes: &Vec<u8>, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&encode(bytes))
}

pub(crate) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
    let text = String::deserialize(deserializer)?;
    decode(&text).map_err(serde::de::Error::custom)
}

pub(crate) mod path {
    use super::{decode, encode};
    use serde::{Deserialize, Deserializer, Serializer};
    use std::ffi::OsString;
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::path::PathBuf;

    #[allow(clippy::ptr_arg)]
    pub(crate) fn serialize<S: Serializer>(
        path: &PathBuf,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&encode(path.as_os_str().as_bytes()))
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<PathBuf, D::Error> {
        let text = String::deserialize(deserializer)?;
        let bytes = decode(&text).map_err(serde::de::Error::custom)?;
        Ok(PathBuf::from(OsString::from_vec(bytes)))
    }
}

// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use desert_rust::{BinaryDeserializer, BinarySerializer};
use std::fmt::{Display, Formatter};

/// Why versioned serialized data could not be decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    MissingVersion,
    UnknownVersion(u8),
    UnsupportedVersion(u8),
    InvalidData(String),
}

impl Display for DecodeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingVersion => f.write_str("missing serialization version"),
            Self::UnknownVersion(version) => write!(f, "unknown serialization version: {version}"),
            Self::UnsupportedVersion(version) => {
                write!(f, "serialization version {version} is no longer supported")
            }
            Self::InvalidData(error) => f.write_str(error),
        }
    }
}

impl std::error::Error for DecodeError {}

/// serde_json - no longer supported
pub const SERIALIZATION_VERSION_V1: u8 = 1u8;

/// bincode 2 with bincode::config::standard()
pub const SERIALIZATION_VERSION_V2: u8 = 2u8;

/// desert
pub const SERIALIZATION_VERSION_V3: u8 = 3u8;
const DEFAULT_SERIALIZATION_CAPACITY: usize = 128;

pub fn serialize_with_version<T: BinarySerializer>(
    value: &T,
    version: u8,
) -> Result<Vec<u8>, String> {
    let mut result = Vec::with_capacity(DEFAULT_SERIALIZATION_CAPACITY + 1);
    result.push(version);
    desert_rust::serialize(value, result).map_err(|e| format!("Failed to serialize value: {e}"))
}

pub fn serialize<T: BinarySerializer>(value: &T) -> Result<Vec<u8>, String> {
    serialize_with_version(value, SERIALIZATION_VERSION_V3)
}

pub fn deserialize<T: BinaryDeserializer>(bytes: &[u8]) -> Result<T, String> {
    let (version, data) = bytes.split_at(1);
    deserialize_with_version(data, version[0])
}

pub fn try_deserialize<T: BinaryDeserializer>(bytes: &[u8]) -> Result<Option<T>, String> {
    match decode(bytes) {
        Ok(value) => Ok(Some(value)),
        Err(DecodeError::MissingVersion | DecodeError::UnknownVersion(_)) => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

/// Decodes versioned data without panicking or discarding the reason it does not decode.
pub fn decode<T: BinaryDeserializer>(bytes: &[u8]) -> Result<T, DecodeError> {
    let (&version, data) = bytes.split_first().ok_or(DecodeError::MissingVersion)?;
    decode_with_version(data, version)
}

pub fn decode_with_version<T: BinaryDeserializer>(
    data: &[u8],
    version: u8,
) -> Result<T, DecodeError> {
    match version {
        SERIALIZATION_VERSION_V1 | SERIALIZATION_VERSION_V2 => {
            Err(DecodeError::UnsupportedVersion(version))
        }
        SERIALIZATION_VERSION_V3 => desert_rust::deserialize(data)
            .map_err(|error| DecodeError::InvalidData(error.to_string())),
        _ => Err(DecodeError::UnknownVersion(version)),
    }
}

pub fn deserialize_with_version<T: BinaryDeserializer>(
    data: &[u8],
    version: u8,
) -> Result<T, String> {
    match try_deserialize_with_version(data, version)? {
        Some(value) => Ok(value),
        None => {
            tracing::error!(
                version,
                data = format!("{:?}", data),
                "invalid serialization version"
            );
            panic!("invalid serialization version: {version}")
        }
    }
}

pub fn try_deserialize_with_version<T: BinaryDeserializer>(
    data: &[u8],
    version: u8,
) -> Result<Option<T>, String> {
    match decode_with_version(data, version) {
        Ok(value) => Ok(Some(value)),
        Err(DecodeError::UnknownVersion(_)) => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use crate::model::component::ComponentId;
    use crate::serialization::SERIALIZATION_VERSION_V3;
    use desert_rust::BinaryCodec;
    use rand::Rng;
    use rand::distr::Alphanumeric;
    use serde::{Deserialize, Serialize};
    use test_r::test;
    use tracing::info;

    #[derive(Debug, PartialEq, Eq, Serialize, Deserialize, BinaryCodec)]
    enum Example {
        First(String),
        Second { x: i64, y: bool, z: Box<Example> },
    }

    impl Example {
        pub fn random(rng: &mut impl Rng) -> Example {
            match rng.random_range(0..2) {
                0 => Example::First(
                    rng.sample_iter(Alphanumeric)
                        .take(7)
                        .map(char::from)
                        .collect(),
                ),
                1 => Example::Second {
                    x: rng.random::<i64>(),
                    y: rng.random::<bool>(),
                    z: Box::new(Example::random(rng)),
                },
                _ => unreachable!(),
            }
        }
    }

    #[test]
    pub fn roundtrip_component_id() {
        let example = Some(ComponentId::new());
        info!("example: {example:?}");
        let serialized = super::serialize(&example).unwrap();
        assert_eq!(serialized[0], SERIALIZATION_VERSION_V3);
        let deserialized = super::deserialize(&serialized).unwrap();
        assert_eq!(example, deserialized);
    }

    #[test]
    pub fn roundtrip() {
        let mut rng = rand::rng();
        for _ in 0..1000 {
            let example = Example::random(&mut rng);
            let serialized = super::serialize(&example).unwrap();
            let deserialized = super::deserialize(&serialized).unwrap();
            assert_eq!(example, deserialized);
        }
    }

    #[test]
    pub fn try_deserialize_without_version() {
        let mut rng = rand::rng();
        for _ in 0..1000 {
            let example = Example::random(&mut rng);
            let serialized = serde_json::to_vec(&example).unwrap();
            let result: Option<Example> = super::try_deserialize(&serialized).unwrap();
            assert_eq!(result, None);
        }
    }

    #[test]
    pub fn dropped_versions_are_an_error() {
        for version in [
            super::SERIALIZATION_VERSION_V1,
            super::SERIALIZATION_VERSION_V2,
        ] {
            let bytes = [version, 0u8, 0u8, 0u8, 0u8];
            let result: Result<Option<Example>, String> = super::try_deserialize(&bytes);
            let err = result.expect_err("a dropped version must not deserialize");
            assert_eq!(
                err,
                format!("serialization version {version} is no longer supported")
            );
        }
    }

    #[test]
    fn decode_distinguishes_version_errors_from_invalid_data() {
        use super::{DecodeError, decode, decode_with_version, try_deserialize};

        assert_eq!(decode::<String>(&[]), Err(DecodeError::MissingVersion));
        assert_eq!(try_deserialize::<String>(&[]), Ok(None));
        for version in [0, 255] {
            assert_eq!(
                decode::<String>(&[version]),
                Err(DecodeError::UnknownVersion(version))
            );
            assert_eq!(try_deserialize::<String>(&[version]), Ok(None));
        }
        for version in [1, 2] {
            assert_eq!(
                decode::<String>(&[version]),
                Err(DecodeError::UnsupportedVersion(version))
            );
            assert!(try_deserialize::<String>(&[version]).is_err());
        }
        assert!(matches!(
            decode::<String>(&[SERIALIZATION_VERSION_V3]),
            Err(DecodeError::InvalidData(_))
        ));
        assert!(try_deserialize::<String>(&[SERIALIZATION_VERSION_V3]).is_err());

        let bytes = super::serialize(&"archived entry".to_string()).unwrap();
        assert_eq!(decode::<String>(&bytes), Ok("archived entry".to_string()));
        assert_eq!(
            decode_with_version::<String>(&bytes[1..], SERIALIZATION_VERSION_V3),
            Ok("archived entry".to_string())
        );
        assert_eq!(
            try_deserialize::<String>(&bytes),
            Ok(Some("archived entry".to_string()))
        );
    }
}

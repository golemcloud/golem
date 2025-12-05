// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

/// serde_json - no longer supported
pub const SERIALIZATION_VERSION_V1: u8 = 1u8;

/// bincode 2 with bincode::config::standard()
pub const SERIALIZATION_VERSION_V2: u8 = 2u8;

/// desert
pub const SERIALIZATION_VERSION_V3: u8 = 3u8;

pub fn serialize_with_version<T: BinarySerializer>(
    value: &T,
    version: u8,
) -> Result<Vec<u8>, String> {
    let data = desert_rust::serialize_to_byte_vec(value)
        .map_err(|e| format!("Failed to serialize value: {e}"))?;
    let mut result = Vec::with_capacity(data.len() + 1);
    result.push(version);
    result.extend(data);
    Ok(result)
}

pub fn serialize<T: BinarySerializer>(value: &T) -> Result<Vec<u8>, String> {
    serialize_with_version(value, SERIALIZATION_VERSION_V3)
}

pub fn deserialize<T: BinaryDeserializer>(bytes: &[u8]) -> Result<T, String> {
    let (version, data) = bytes.split_at(1);
    deserialize_with_version(data, version[0])
}

pub fn try_deserialize<T: BinaryDeserializer>(bytes: &[u8]) -> Result<Option<T>, String> {
    if bytes.is_empty() {
        Ok(None)
    } else {
        let (version, data) = bytes.split_at(1);
        try_deserialize_with_version(data, version[0])
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
    match version {
        SERIALIZATION_VERSION_V1 => {
            panic!("Support for v1 serialization format has been dropped");
        }
        SERIALIZATION_VERSION_V2 => {
            panic!("Support for v2 serialization format has been dropped");
        }
        SERIALIZATION_VERSION_V3 => desert_rust::deserialize(data)
            .map_err(|err| err.to_string())
            .map(Some),
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use crate::model::component::ComponentId;
    use desert_rust::BinaryCodec;
    use rand::distr::Alphanumeric;
    use rand::Rng;
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
}

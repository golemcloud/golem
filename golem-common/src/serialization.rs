// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use bincode::{Decode, Encode};
use bytes::{BufMut, Bytes, BytesMut};
use tracing::error;

/// serde_json - no longer supported
pub const SERIALIZATION_VERSION_V1: u8 = 1u8;

/// bincode 2 with bincode::config::standard()
pub const SERIALIZATION_VERSION_V2: u8 = 2u8;

pub fn serialize_with_version<T: Encode>(value: &T, version: u8) -> Result<Bytes, String> {
    let data = bincode::encode_to_vec(value, bincode::config::standard())
        .map_err(|e| format!("Failed to serialize value: {e}"))?;
    let mut bytes = BytesMut::new();
    bytes.put_u8(version);
    bytes.extend_from_slice(&data);
    Ok(bytes.freeze())
}

pub fn serialize<T: Encode>(value: &T) -> Result<Bytes, String> {
    serialize_with_version(value, SERIALIZATION_VERSION_V2)
}

pub fn deserialize<T: Decode>(bytes: &[u8]) -> Result<T, String> {
    let (version, data) = bytes.split_at(1);
    deserialize_with_version(data, version[0])
}

pub fn try_deserialize<T: Decode>(bytes: &[u8]) -> Result<Option<T>, String> {
    if bytes.is_empty() {
        Ok(None)
    } else {
        let (version, data) = bytes.split_at(1);
        try_deserialize_with_version(data, version[0])
    }
}

pub fn deserialize_with_version<T: Decode>(data: &[u8], version: u8) -> Result<T, String> {
    match try_deserialize_with_version(data, version)? {
        Some(value) => Ok(value),
        None => {
            error!(
                "invalid serialization version: {}, full data set: {:?}",
                version, data
            );
            panic!("invalid serialization version: {}", version)
        }
    }
}

pub fn try_deserialize_with_version<T: Decode>(
    data: &[u8],
    version: u8,
) -> Result<Option<T>, String> {
    match version {
        SERIALIZATION_VERSION_V1 => {
            panic!("Support for v1 serialization format has been dropped");
        }
        SERIALIZATION_VERSION_V2 => {
            let (entry, _) = bincode::decode_from_slice(data, bincode::config::standard())
                .map_err(|e| format!("Failed to deserialize value: {e}"))?;
            Ok(Some(entry))
        }
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use bincode::{Decode, Encode};
    use rand::distributions::Alphanumeric;
    use rand::Rng;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, PartialEq, Eq, Serialize, Deserialize, Encode, Decode)]
    enum Example {
        First(String),
        Second { x: i64, y: bool, z: Box<Example> },
    }

    impl Example {
        pub fn random(rng: &mut impl Rng) -> Example {
            match rng.gen_range(0..2) {
                0 => Example::First(
                    rng.sample_iter(Alphanumeric)
                        .take(7)
                        .map(char::from)
                        .collect(),
                ),
                1 => Example::Second {
                    x: rng.gen::<i64>(),
                    y: rng.gen::<bool>(),
                    z: Box::new(Example::random(rng)),
                },
                _ => unreachable!(),
            }
        }
    }

    #[test]
    pub fn roundtrip() {
        let mut rng = rand::thread_rng();
        for _ in 0..1000 {
            let example = Example::random(&mut rng);
            let serialized = super::serialize(&example).unwrap();
            let deserialized = super::deserialize(&serialized).unwrap();
            assert_eq!(example, deserialized);
        }
    }

    #[test]
    pub fn try_deserialize_without_version() {
        let mut rng = rand::thread_rng();
        for _ in 0..1000 {
            let example = Example::random(&mut rng);
            let serialized = serde_json::to_vec(&example).unwrap();
            let result: Option<Example> = super::try_deserialize(&serialized).unwrap();
            assert_eq!(result, None);
        }
    }
}

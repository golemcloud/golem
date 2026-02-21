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

use blake3::HexError;
use serde::de::Visitor;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::fmt::{Display, Formatter};
use std::str::FromStr;

#[derive(Clone, Copy, std::hash::Hash, PartialEq, Eq, Debug)]
pub struct Hash(pub(crate) blake3::Hash);

impl Hash {
    pub fn new(hash: blake3::Hash) -> Self {
        Self(hash)
    }

    pub fn empty() -> Self {
        Self(blake3::hash(&[]))
    }

    pub fn as_blake3_hash(&self) -> &blake3::Hash {
        &self.0
    }

    pub fn into_blake3(self) -> blake3::Hash {
        self.0
    }
}

impl Display for Hash {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0.to_hex().as_str())
    }
}

impl From<Hash> for blake3::Hash {
    fn from(hash: Hash) -> Self {
        hash.0
    }
}

impl From<blake3::Hash> for Hash {
    fn from(hash: blake3::Hash) -> Self {
        Self::new(hash)
    }
}

impl TryFrom<&str> for Hash {
    type Error = HexError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        blake3::Hash::from_hex(value).map(Hash)
    }
}

impl FromStr for Hash {
    type Err = HexError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s)
    }
}

impl Serialize for Hash {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.0.to_hex().as_str())
    }
}

impl<'de> Deserialize<'de> for Hash {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct HashVisitor;

        impl<'de> Visitor<'de> for HashVisitor {
            type Value = Hash;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a hex string representing a hash")
            }

            fn visit_str<E>(self, v: &str) -> Result<Hash, E>
            where
                E: serde::de::Error,
            {
                blake3::Hash::from_hex(v)
                    .map(Hash)
                    .map_err(|e| E::custom(format!("invalid BLAKE3 hash: {}", e)))
            }
        }

        deserializer.deserialize_str(HashVisitor)
    }
}

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

use crate::model::diff::ser::{to_json_with_mode, SerializeMode, ToSerializableWithMode};
use crate::model::diff::Diffable;
use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};
use std::fmt::{Display, Formatter};
use std::sync::OnceLock;

#[derive(Clone, Copy, std::hash::Hash, PartialEq, Eq, Debug)]
pub struct Hash {
    hash: blake3::Hash,
}

impl Hash {
    pub fn new(hash: blake3::Hash) -> Self {
        Self { hash }
    }

    pub fn empty() -> Self {
        Self {
            // TODO: const?
            hash: blake3::hash(&[]),
        }
    }

    pub fn as_blake3_hash(&self) -> &blake3::Hash {
        &self.hash
    }

    pub fn into_blake3(self) -> blake3::Hash {
        self.hash
    }
}

impl Display for Hash {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.hash.to_hex().as_str())
    }
}

impl From<Hash> for blake3::Hash {
    fn from(hash: Hash) -> Self {
        hash.hash
    }
}

impl From<blake3::Hash> for Hash {
    fn from(hash: blake3::Hash) -> Self {
        Self::new(hash)
    }
}

impl Serialize for Hash {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.hash.to_hex().as_str())
    }
}

pub trait Hashable {
    fn hash(&self) -> Hash;
}

#[derive(Debug, Clone)]
pub enum HashOfKind<V> {
    Precalculated(Hash),
    FromValue { value: V, lazy_hash: OnceLock<Hash> },
}

#[derive(Debug, Clone)]
pub struct HashOf<V>(HashOfKind<V>);

impl<V> HashOf<V> {
    pub fn from_hash(hash: Hash) -> Self {
        Self(HashOfKind::Precalculated(hash))
    }

    pub fn from_blake3_hash(hash: blake3::Hash) -> Self {
        Self(HashOfKind::Precalculated(hash.into()))
    }

    pub fn form_value(value: V) -> Self {
        Self(HashOfKind::FromValue {
            value,
            lazy_hash: OnceLock::new(),
        })
    }

    pub fn as_value(&self) -> Option<&V> {
        match &self.0 {
            HashOfKind::Precalculated(_) => None,
            HashOfKind::FromValue { value, .. } => Some(value),
        }
    }
}

impl<V: Hashable> Hashable for HashOf<V> {
    fn hash(&self) -> Hash {
        match &self.0 {
            HashOfKind::Precalculated(hash) => *hash,
            HashOfKind::FromValue { value, lazy_hash } => *lazy_hash.get_or_init(|| value.hash()),
        }
    }
}

impl<V: Hashable> PartialEq for HashOf<V> {
    fn eq(&self, other: &Self) -> bool {
        self.hash() == other.hash()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum DiffForHashOf<V: Diffable> {
    HashDiff { local_hash: Hash, remote_hash: Hash },
    ValueDiff { diff: V::DiffResult },
}

impl<V: Hashable + Diffable> Diffable for HashOf<V> {
    type DiffResult = DiffForHashOf<V>;

    fn diff(local: &Self, remote: &Self) -> Option<Self::DiffResult> {
        if local == remote {
            return None;
        }

        let local_hash = local.hash();
        let remote_hash = remote.hash();

        let diff = match (local.as_value(), remote.as_value()) {
            (Some(local), Some(remote)) => local.diff_with_server(remote),
            _ => None,
        };

        match diff {
            Some(diff) => Some(DiffForHashOf::ValueDiff { diff }),
            None => Some(DiffForHashOf::HashDiff {
                local_hash,
                remote_hash,
            }),
        }
    }
}

impl<V: Diffable> Serialize for DiffForHashOf<V> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            DiffForHashOf::HashDiff {
                local_hash,
                remote_hash,
            } => {
                let mut s = serializer.serialize_struct("DiffForHashOfByHashes", 2)?;
                s.serialize_field("localHash", local_hash)?;
                s.serialize_field("remoteHash", remote_hash)?;
                s.end()
            }
            DiffForHashOf::ValueDiff { diff } => diff.serialize(serializer),
        }
    }
}

impl<V: Hashable> From<V> for HashOf<V> {
    fn from(value: V) -> Self {
        Self::form_value(value)
    }
}

impl<V: Hashable> From<Hash> for HashOf<V> {
    fn from(value: Hash) -> Self {
        Self::from_hash(value)
    }
}

impl<V: Hashable> From<blake3::Hash> for HashOf<V> {
    fn from(value: blake3::Hash) -> Self {
        Self::from_hash(value.into())
    }
}

impl<V: Hashable + Serialize> ToSerializableWithMode for HashOf<V> {
    fn to_serializable(&self, mode: SerializeMode) -> serde_json::Value {
        match mode {
            SerializeMode::HashOnly => {
                serde_json::Value::String(self.hash().hash.to_hex().to_string())
            }
            SerializeMode::ValueIfAvailable => match &self.0 {
                HashOfKind::Precalculated(hash) => {
                    serde_json::Value::String(hash.hash.to_hex().to_string())
                }
                HashOfKind::FromValue {
                    value,
                    lazy_hash: _,
                } => serde_json::to_value(value)
                    .expect("failed to convert value to JSON for hashing"),
            },
        }
    }
}

pub fn hash_from_serialized_value<T: Serialize>(value: &T) -> Hash {
    blake3::hash(
        to_json_with_mode(value, SerializeMode::HashOnly)
            .expect("failed to serialize as JSON for hashing")
            .as_bytes(),
    )
    .into()
}

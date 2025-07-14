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

// TODO: move to golem-common (or some other common place)

use golem_common::model::{ComponentFilePermissions, ComponentType};
use serde::{Serialize, Serializer};
use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};
use std::marker::PhantomData;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SerializeTargetKind {
    Hashing,
    Diffing,
}

pub trait SerializeTarget: Serialize {
    fn serialize_target() -> SerializeTargetKind;
}

#[derive(Clone)]
pub struct SerializeForHashing;

impl SerializeTarget for SerializeForHashing {
    fn serialize_target() -> SerializeTargetKind {
        SerializeTargetKind::Hashing
    }
}

impl Serialize for SerializeForHashing {
    fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        panic!("SerializeForHashing should not be serialized, use serde(skip)")
    }
}

#[derive(Clone)]
pub struct SerializeForDiffing;

impl SerializeTarget for SerializeForDiffing {
    fn serialize_target() -> SerializeTargetKind {
        SerializeTargetKind::Diffing
    }
}

impl Serialize for SerializeForDiffing {
    fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        panic!("SerializeForDiffing should not be serialized, use serde(skip)")
    }
}

#[derive(Clone, Copy, std::hash::Hash, PartialEq, Eq, Debug)]
pub struct Hash {
    pub hash: blake3::Hash,
}

impl Hash {
    pub fn new(hash: blake3::Hash) -> Self {
        Self { hash }
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
pub enum HashOf<V, ST: SerializeTarget> {
    Precalculated(Hash),
    // TODO: wrap value and lazy_hash in private Struct?
    FromValue {
        value: V,
        lazy_hash: OnceLock<Hash>,
        _serialize_target: PhantomData<ST>,
    },
}

impl<V, ST: SerializeTarget> HashOf<V, ST> {
    fn from_hash(hash: Hash) -> Self {
        Self::Precalculated(hash)
    }

    fn form_value(value: V) -> Self {
        Self::FromValue {
            value,
            lazy_hash: OnceLock::new(),
            _serialize_target: PhantomData,
        }
    }

    fn with_serialize_target<NST: SerializeTarget>(self) -> HashOf<V, NST> {
        match self {
            HashOf::Precalculated(hash) => HashOf::Precalculated(hash),
            HashOf::FromValue {
                value,
                lazy_hash,
                _serialize_target: _,
            } => HashOf::FromValue {
                value,
                lazy_hash,
                _serialize_target: PhantomData,
            },
        }
    }
}

impl<V: Hashable, ST: SerializeTarget> Hashable for HashOf<V, ST> {
    fn hash(&self) -> Hash {
        match self {
            HashOf::Precalculated(hash) => *hash,
            HashOf::FromValue {
                value,
                lazy_hash,
                _serialize_target: _,
            } => *lazy_hash.get_or_init(|| value.hash()),
        }
    }
}

impl<V: Hashable + Serialize, ST: SerializeTarget> Serialize for HashOf<V, ST> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            HashOf::Precalculated(hash) => serializer.serialize_str(hash.hash.to_hex().as_str()),
            HashOf::FromValue {
                value,
                lazy_hash: _,
                _serialize_target: _,
            } => match ST::serialize_target() {
                SerializeTargetKind::Hashing => {
                    serializer.serialize_str(self.hash().hash.to_hex().as_str())
                }
                SerializeTargetKind::Diffing => value.serialize(serializer),
            },
        }
    }
}

impl<V: Hashable + Serialize, ST: SerializeTarget> PartialEq for HashOf<V, ST> {
    fn eq(&self, other: &Self) -> bool {
        self.hash() == other.hash()
    }
}

impl<V: Hashable, ST: SerializeTarget> From<V> for HashOf<V, ST> {
    fn from(value: V) -> Self {
        Self::form_value(value)
    }
}

impl<V: Hashable, ST: SerializeTarget> From<Hash> for HashOf<V, ST> {
    fn from(value: Hash) -> Self {
        Self::from_hash(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentFile {
    pub hash: Hash,
    pub permissions: ComponentFilePermissions,
}

impl Hashable for ComponentFile {
    fn hash(&self) -> Hash {
        hash_from_json(self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Component<ST: SerializeTarget> {
    pub binary_hash: Hash,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub component_type: ComponentType,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub dynamic_linking_wasm_rpc: BTreeMap<String, BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub files: BTreeMap<String, HashOf<ComponentFile, ST>>,

    #[serde(skip)]
    pub _serialize_target: PhantomData<ST>,
}

impl<ST: SerializeTarget> Hashable for Component<ST> {
    fn hash(&self) -> Hash {
        hash_from_json(self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Deployment<ST: SerializeTarget> {
    pub components: BTreeMap<String, HashOf<Component<ST>, ST>>,

    #[serde(skip)]
    pub _serialize_target: PhantomData<ST>,
}

impl Hashable for Deployment<SerializeForHashing> {
    fn hash(&self) -> Hash {
        hash_from_json(self)
    }
}

impl<ST: SerializeTarget> Deployment<ST> {
    fn with_serialize_target<NST: SerializeTarget>(self) -> Deployment<NST> {
        Deployment {
            _serialize_target: PhantomData,
            components: self
                .components
                .into_iter()
                .map(|(k, v)| (k, v.with_serialize_target::<NST>()))
                .collect(),
        }
    }
}

fn hash_from_json<V: Serialize>(value: &V) -> Hash {
    // TODO: should we propagate serialization errors?
    //       if yes, then we should change the type of Hashable::hash()
    blake3::hash(
        serde_json::to_string(&value)
            .expect("failed to serialize as JSON for hashing")
            .as_bytes(),
    )
    .into()
}

#[cfg(test)]
mod test {
    use crate::model::diffable::{
        Component, Deployment, Hashable, SerializeForDiffing, SerializeForHashing, SerializeTarget,
    };
    use golem_common::model::ComponentType;
    use std::collections::BTreeMap;
    use std::marker::PhantomData;
    use test_r::test;

    #[test]
    fn test() {
        fn new_component<ST: SerializeTarget>(name: &str) -> Component<ST> {
            Component {
                binary_hash: blake3::hash(name.as_bytes()).into(),
                version: Some("1.0.0".to_string()),
                component_type: ComponentType::Durable,
                env: BTreeMap::from([
                    ("LOL".to_string(), "LOL".to_string()),
                    ("X".to_string(), "Y".to_string()),
                ]),
                dynamic_linking_wasm_rpc: Default::default(),
                files: Default::default(),

                _serialize_target: PhantomData,
            }
        }

        let deployment_for_diffing: Deployment<SerializeForDiffing> = Deployment {
            components: BTreeMap::from([
                ("comp1".to_string(), new_component("comp1").into()),
                ("comp2".to_string(), new_component("comp2").into()),
            ]),

            _serialize_target: PhantomData,
        };

        let deployment_for_hashing = deployment_for_diffing
            .clone()
            .with_serialize_target::<SerializeForHashing>();

        println!(
            "{}",
            serde_yaml::to_string(&deployment_for_diffing).unwrap()
        );
        println!(
            "{}",
            serde_yaml::to_string(&deployment_for_hashing).unwrap()
        );
        println!("{}", deployment_for_hashing.hash())
    }
}

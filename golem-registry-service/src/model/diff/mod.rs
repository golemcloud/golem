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

mod component;
mod deployment;
mod hash;
mod ser;

pub use component::*;
pub use deployment::*;
pub use hash::*;
pub use ser::*;

use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};
use similar::TextDiff;
use std::collections::{BTreeMap, BTreeSet};

pub trait Diffable {
    type DiffResult: Serialize;

    fn diff_with_local(&self, local: &Self) -> Option<Self::DiffResult> {
        Self::diff(local, self)
    }

    fn diff_with_remote(&self, server: &Self) -> Option<Self::DiffResult> {
        Self::diff(self, server)
    }

    fn diff(local: &Self, server: &Self) -> Option<Self::DiffResult>;

    fn unified_yaml_diff_with_local(&self, local: &Self, mode: SerializeMode) -> String
    where
        Self: Serialize,
    {
        Self::unified_yaml_diff(local, self, mode)
    }

    fn unified_yaml_diff_with_server(&self, server: &Self, mode: SerializeMode) -> String
    where
        Self: Serialize,
    {
        Self::unified_yaml_diff(self, server, mode)
    }

    fn unified_yaml_diff(local: &Self, server: &Self, mode: SerializeMode) -> String
    where
        Self: Serialize,
    {
        TextDiff::from_lines(
            &to_yaml_with_mode(&server, mode).expect("failed to serialize server"),
            &to_yaml_with_mode(&local, mode).expect("failed to serialize server"),
        )
        .unified_diff()
        .context_radius(4)
        .header("server", "local")
        .to_string()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum BTreeDiffValue<ValueDiff> {
    Add,
    Remove,
    Update(Option<ValueDiff>),
}

impl<ValueDiff: Serialize> Serialize for BTreeDiffValue<ValueDiff> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            BTreeDiffValue::Add => serializer.serialize_str("add"),
            BTreeDiffValue::Remove => serializer.serialize_str("remove"),
            BTreeDiffValue::Update(diff) => match diff {
                Some(diff) => {
                    let mut s = serializer.serialize_struct("BTreeDiffValueUpdate", 1)?;
                    s.serialize_field("update", diff)?;
                    s.end()
                }
                None => serializer.serialize_str("update"),
            },
        }
    }
}

pub type BTreeDiff<K, V: Diffable> = BTreeMap<K, BTreeDiffValue<V::DiffResult>>;

impl<K, V> Diffable for BTreeMap<K, V>
where
    K: Ord + Clone + Serialize,
    V: Diffable,
    V::DiffResult: Serialize,
{
    type DiffResult = BTreeDiff<K, V>;

    fn diff(local: &Self, server: &Self) -> Option<Self::DiffResult> {
        let mut diff = BTreeMap::new();

        let keys = local.keys().chain(server.keys()).collect::<BTreeSet<_>>();

        for key in keys {
            match (local.get(key), server.get(key)) {
                (Some(local), Some(server)) => {
                    if let Some(value_diff) = local.diff_with_remote(server) {
                        diff.insert(key.clone(), BTreeDiffValue::Update(Some(value_diff)));
                    }
                }
                (Some(_), None) => {
                    diff.insert(key.clone(), BTreeDiffValue::Add);
                }
                (None, Some(_)) => {
                    diff.insert(key.clone(), BTreeDiffValue::Remove);
                }
                (None, None) => {
                    panic!("unreachable");
                }
            }
        }

        if diff.is_empty() {
            None
        } else {
            Some(diff)
        }
    }
}

#[cfg(test)]
mod test {
    use crate::model::diff::component::{Component, ComponentFile};
    use crate::model::diff::deployment::Deployment;
    use crate::model::diff::hash::Hashable;
    use crate::model::diff::ser::{
        to_json_pretty_with_mode, to_json_with_mode, to_yaml_with_mode, SerializeMode,
        ToSerializableWithModeExt,
    };
    use crate::model::diff::{ComponentMetadata, Diffable};
    use golem_common::model::{ComponentFilePermissions, ComponentType};
    use std::collections::BTreeMap;
    use test_r::test;

    // TODO: proper test
    #[test]
    fn test() {
        fn new_component(name: &str) -> Component {
            Component {
                metadata: ComponentMetadata {
                    version: Some("1.0.0".to_string()),
                    component_type: ComponentType::Durable,
                    env: BTreeMap::from([
                        ("LOL".to_string(), "LOL".to_string()),
                        ("X".to_string(), "Y".to_string()),
                    ]),
                    dynamic_linking_wasm_rpc: Default::default(),
                }
                .into(),
                binary_hash: blake3::hash(name.as_bytes()).into(),
                files: BTreeMap::from([
                    (
                        "lol".to_string(),
                        ComponentFile {
                            hash: blake3::hash("xycyxc".as_bytes()).into(),
                            permissions: ComponentFilePermissions::ReadOnly,
                        }
                        .into(),
                    ),
                    (
                        "lol-2".to_string(),
                        ComponentFile {
                            hash: blake3::hash("xycyxc-sdsd".as_bytes()).into(),
                            permissions: ComponentFilePermissions::ReadOnly,
                        }
                        .into(),
                    ),
                ]),
            }
        }

        let deployment = Deployment {
            components: BTreeMap::from([
                ("comp1".to_string(), new_component("comp1").into()),
                ("comp2".to_string(), new_component("comp2").into()),
            ]),
        };

        println!("{}", deployment.hash());

        println!(
            "{}",
            to_json_with_mode(&deployment, SerializeMode::HashOnly).unwrap()
        );
        println!(
            "{}",
            to_yaml_with_mode(&deployment, SerializeMode::HashOnly).unwrap()
        );

        println!(
            "{}",
            to_json_pretty_with_mode(&deployment, SerializeMode::ValueIfAvailable).unwrap()
        );
        println!(
            "{}",
            to_yaml_with_mode(&deployment, SerializeMode::ValueIfAvailable).unwrap()
        );

        for component in deployment.components.values() {
            println!("----");
            println!("{}", component.hash());
            println!(
                "{}",
                component
                    .to_json_with_mode(SerializeMode::HashOnly)
                    .unwrap()
            );
            println!(
                "{}",
                component
                    .to_pretty_json_with_mode(SerializeMode::HashOnly)
                    .unwrap()
            );
            println!(
                "{}",
                component
                    .to_yaml_with_mode(SerializeMode::HashOnly)
                    .unwrap()
            );
            println!(
                "{}",
                component
                    .to_pretty_json_with_mode(SerializeMode::ValueIfAvailable)
                    .unwrap()
            );
            println!(
                "{}",
                component
                    .to_yaml_with_mode(SerializeMode::ValueIfAvailable)
                    .unwrap()
            );
        }

        let server_deployment = {
            let mut deployment = deployment.clone();
            deployment
                .components
                .insert("comp3".to_string(), new_component("comp3").into());
            deployment.components.remove("comp1");
            if let Some(comp) = deployment.components.get("comp2") {
                if let Some(comp) = comp.as_value() {
                    let mut comp = comp.clone();
                    comp.files.insert(
                        "new_file".to_string(),
                        ComponentFile {
                            hash: blake3::hash("xxx".as_bytes()).into(),
                            permissions: ComponentFilePermissions::ReadOnly,
                        }
                        .into(),
                    );
                    deployment
                        .components
                        .insert("comp2".to_string(), comp.into());
                }
            }
            deployment
        };

        println!(
            "{}",
            serde_yaml::to_string(&deployment.diff_with_remote(&server_deployment)).unwrap()
        );

        println!(
            "{}",
            serde_json::to_string_pretty(&deployment.diff_with_remote(&server_deployment)).unwrap()
        );

        println!(
            "{}",
            deployment.unified_yaml_diff_with_server(&server_deployment, SerializeMode::HashOnly)
        );

        println!(
            "{}",
            deployment
                .unified_yaml_diff_with_server(&server_deployment, SerializeMode::ValueIfAvailable)
        );
    }
}

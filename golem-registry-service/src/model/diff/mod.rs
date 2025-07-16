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

use crate::model::diff::hash::{Hash, HashOf, Hashable};
use golem_common::model::{ComponentFilePermissions, ComponentType};
use ser::serialize_with_mode;
use serde::{Serialize, Serializer};
use std::any::Any;
use std::collections::BTreeMap;
use std::fmt::Display;

pub mod component;
pub mod deployment;
pub mod hash;
pub mod ser;

#[cfg(test)]
mod test {
    use crate::model::diff::component::{Component, ComponentFile};
    use crate::model::diff::deployment::Deployment;
    use crate::model::diff::hash::Hashable;
    use crate::model::diff::ser::{
        to_json_with_mode, to_pretty_json_with_mode, to_yaml_with_mode, SerializeMode,
    };
    use golem_common::model::{ComponentFilePermissions, ComponentType};
    use std::collections::BTreeMap;
    use test_r::test;

    #[test]
    fn test() {
        fn new_component(name: &str) -> Component {
            Component {
                binary_hash: blake3::hash(name.as_bytes()).into(),
                version: Some("1.0.0".to_string()),
                component_type: ComponentType::Durable,
                env: BTreeMap::from([
                    ("LOL".to_string(), "LOL".to_string()),
                    ("X".to_string(), "Y".to_string()),
                ]),
                dynamic_linking_wasm_rpc: Default::default(),
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
            to_pretty_json_with_mode(&deployment, SerializeMode::ValueIfAvailable).unwrap()
        );
        println!(
            "{}",
            to_yaml_with_mode(&deployment, SerializeMode::ValueIfAvailable).unwrap()
        );

        /*let mut writer = Vec::with_capacity(128);
        let mut json_serializer = serde_json::Serializer::new(&mut writer);
        let serializer = TargetAwareSerializer {
            inner: &mut json_serializer,
            target: SerializeMode::Values,
        };
        deployment.serialize(serializer).unwrap();
        let serialized = String::from_utf8(writer).unwrap();
        println!("{serialized}");
        println!("{}", deployment.hash())*/
    }
}

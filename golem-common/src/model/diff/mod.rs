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

mod component;
mod deployment;
mod environment;
mod hash;
mod http_api_definition;
mod http_api_deployment;
mod ser;

pub use component::*;
pub use deployment::*;
pub use environment::*;
pub use hash::*;
pub use http_api_definition::*;
pub use http_api_deployment::*;
pub use ser::*;

use serde::{Serialize, Serializer};
use similar::TextDiff;
use std::collections::{BTreeMap, BTreeSet};

pub trait Diffable {
    type DiffResult: Serialize;

    fn diff_with_local(&self, local: &Self) -> Option<Self::DiffResult> {
        Self::diff(local, self)
    }

    fn diff_with_server(&self, server: &Self) -> Option<Self::DiffResult> {
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
pub enum BTreeMapDiffValue<ValueDiff> {
    Add,
    Delete,
    Update(Option<ValueDiff>),
}

impl<ValueDiff: Serialize> Serialize for BTreeMapDiffValue<ValueDiff> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            BTreeMapDiffValue::Add => serializer.serialize_str("add"),
            BTreeMapDiffValue::Delete => serializer.serialize_str("delete"),
            BTreeMapDiffValue::Update(diff) => match diff {
                Some(diff) => diff.serialize(serializer),
                None => serializer.serialize_str("update"),
            },
        }
    }
}

pub type BTreeMapDiff<K, V> = BTreeMap<K, BTreeMapDiffValue<<V as Diffable>::DiffResult>>;

impl<K, V> Diffable for BTreeMap<K, V>
where
    K: Ord + Clone + Serialize,
    V: Diffable,
    V::DiffResult: Serialize,
{
    type DiffResult = BTreeMapDiff<K, V>;

    fn diff(local: &Self, server: &Self) -> Option<Self::DiffResult> {
        let mut diff = BTreeMap::new();

        let keys = local.keys().chain(server.keys()).collect::<BTreeSet<_>>();

        for key in keys {
            match (local.get(key), server.get(key)) {
                (Some(local), Some(server)) => {
                    if let Some(value_diff) = local.diff_with_server(server) {
                        diff.insert(key.clone(), BTreeMapDiffValue::Update(Some(value_diff)));
                    }
                }
                (Some(_), None) => {
                    diff.insert(key.clone(), BTreeMapDiffValue::Add);
                }
                (None, Some(_)) => {
                    diff.insert(key.clone(), BTreeMapDiffValue::Delete);
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BTreeSetDiffValue {
    Add,
    Delete,
}

pub type BTreeSetDiff<K> = BTreeMap<K, BTreeSetDiffValue>;

impl<K> Diffable for BTreeSet<K>
where
    K: Ord + Clone + Serialize,
{
    type DiffResult = BTreeSetDiff<K>;

    fn diff(local: &Self, server: &Self) -> Option<Self::DiffResult> {
        let mut diff = BTreeMap::new();

        let keys = local.iter().chain(server.iter()).collect::<BTreeSet<_>>();

        for key in keys {
            match (local.contains(key), server.contains(key)) {
                (true, true) => {
                    // NOP, same
                }
                (true, false) => {
                    diff.insert(key.clone(), BTreeSetDiffValue::Add);
                }
                (false, true) => {
                    diff.insert(key.clone(), BTreeSetDiffValue::Delete);
                }
                (false, false) => {
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
    use crate::model::component::ComponentType;
    use crate::model::diff::component::{Component, ComponentFile};
    use crate::model::diff::deployment::Deployment;
    use crate::model::diff::hash::Hashable;
    use crate::model::diff::http_api_definition::{
        HttpApiDefinition, HttpApiDefinitionBinding, HttpApiRoute,
    };
    use crate::model::diff::http_api_deployment::{HttpApiDeployment, NO_SUBDOMAIN};
    use crate::model::diff::ser::{
        to_json_pretty_with_mode, to_json_with_mode, to_yaml_with_mode, SerializeMode,
        ToSerializableWithModeExt,
    };
    use crate::model::diff::{ComponentMetadata, Diffable};
    use crate::model::ComponentFilePermissions;
    use std::collections::{BTreeMap, BTreeSet};
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
                files_by_path: BTreeMap::from([
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
                plugins_by_priority: Default::default(),
            }
        }

        let deployment = Deployment {
            components: BTreeMap::from([
                ("comp1".to_string(), new_component("comp1").into()),
                ("comp2".to_string(), new_component("comp2").into()),
            ]),
            http_api_definitions: BTreeMap::from([
                (
                    "main-api".to_string(),
                    HttpApiDefinition {
                        routes: BTreeMap::from([
                            (
                                ("GET", "/users").into(),
                                HttpApiRoute {
                                    binding: HttpApiDefinitionBinding {
                                        binding_type: None,
                                        component_name: None,
                                        worker_name: None,
                                        idempotency_key: None,
                                        response: Some("fake rib".to_string()),
                                    },
                                    security: None,
                                },
                            ),
                            (
                                ("GET", "/posts").into(),
                                HttpApiRoute {
                                    binding: HttpApiDefinitionBinding {
                                        binding_type: None,
                                        component_name: None,
                                        worker_name: None,
                                        idempotency_key: None,
                                        response: None,
                                    },
                                    security: None,
                                },
                            ),
                            (
                                ("POST", "/users").into(),
                                HttpApiRoute {
                                    binding: HttpApiDefinitionBinding {
                                        binding_type: None,
                                        component_name: None,
                                        worker_name: None,
                                        idempotency_key: None,
                                        response: None,
                                    },
                                    security: None,
                                },
                            ),
                        ]),
                        version: "1.0.0".to_string(),
                    }
                    .into(),
                ),
                (
                    "admin-api".to_string(),
                    HttpApiDefinition {
                        routes: BTreeMap::default(),
                        version: "1.0.2".to_string(),
                    }
                    .into(),
                ),
            ]),
            http_api_deployments: BTreeMap::from([
                (
                    (NO_SUBDOMAIN, "localhost").into(),
                    HttpApiDeployment {
                        apis: BTreeSet::from(["main-api".to_string()]),
                    }
                    .into(),
                ),
                (
                    (NO_SUBDOMAIN, "app.com").into(),
                    HttpApiDeployment {
                        apis: BTreeSet::from(["main-api".to_string()]),
                    }
                    .into(),
                ),
                (
                    (Some("api"), "app.com").into(),
                    HttpApiDeployment {
                        apis: BTreeSet::from(["main-api".to_string()]),
                    }
                    .into(),
                ),
                (
                    (Some("admin"), "app.com").into(),
                    HttpApiDeployment {
                        apis: BTreeSet::from(["main-api".to_string()]),
                    }
                    .into(),
                ),
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

            {
                let comp = deployment
                    .components
                    .get("comp2")
                    .unwrap()
                    .as_value()
                    .unwrap();
                let mut comp = comp.clone();
                comp.files_by_path.insert(
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

            deployment.http_api_definitions.remove("admin-api");

            {
                let mut api = deployment
                    .http_api_definitions
                    .get("main-api")
                    .unwrap()
                    .as_value()
                    .unwrap()
                    .clone();
                api.routes.insert(
                    ("POST", "/posts").into(),
                    HttpApiRoute {
                        binding: HttpApiDefinitionBinding {
                            binding_type: None,
                            component_name: None,
                            worker_name: None,
                            idempotency_key: None,
                            response: None,
                        },
                        security: Some("lol".to_string()),
                    },
                );
                api.routes.insert(
                    ("GET", "/users").into(),
                    HttpApiRoute {
                        binding: HttpApiDefinitionBinding {
                            binding_type: None,
                            component_name: Some("comp3".to_string()),
                            worker_name: None,
                            idempotency_key: None,
                            response: None,
                        },
                        security: Some("xxx".to_string()),
                    },
                );

                deployment
                    .http_api_definitions
                    .insert("main-api".to_string(), api.into());
            }

            deployment.http_api_deployments = BTreeMap::from([
                (
                    (NO_SUBDOMAIN, "localhost").into(),
                    HttpApiDeployment {
                        apis: BTreeSet::from(["other-api".to_string()]),
                    }
                    .into(),
                ),
                (
                    (NO_SUBDOMAIN, "app.com").into(),
                    HttpApiDeployment {
                        apis: BTreeSet::default(),
                    }
                    .into(),
                ),
                (
                    (Some("api"), "app.com").into(),
                    HttpApiDeployment {
                        apis: BTreeSet::from(["main-api".to_string(), "other-api".to_string()]),
                    }
                    .into(),
                ),
                (
                    (Some("admin"), "app.com").into(),
                    HttpApiDeployment {
                        apis: BTreeSet::from(["main-api".to_string()]),
                    }
                    .into(),
                ),
            ]);

            deployment
        };

        println!(
            "{}",
            serde_yaml::to_string(&deployment.diff_with_server(&server_deployment)).unwrap()
        );

        println!(
            "{}",
            serde_json::to_string_pretty(&deployment.diff_with_server(&server_deployment)).unwrap()
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

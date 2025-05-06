// Copyright 2024-2025 Golem Cloud
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

use crate::model::app::AppComponentName;
use crate::model::component::Component;
use crate::model::deploy_diff::DiffSerialize;
use crate::model::ComponentName;
use golem_client::model::{DynamicLinkedInstance, DynamicLinking};
use golem_common::model::{ComponentFilePermissions, ComponentType};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DiffableComponentFile {
    pub hash: String,
    pub permissions: ComponentFilePermissions,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DiffableComponent {
    pub component_name: ComponentName,
    pub component_hash: String,
    pub component_type: ComponentType,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub files: BTreeMap<String, DiffableComponentFile>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dynamic_linking: BTreeMap<String, BTreeMap<String, String>>,
}

impl DiffableComponent {
    pub fn from_server(
        component: &Component,
        component_hash: String,
        files: BTreeMap<String, DiffableComponentFile>,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            component_name: component.component_name.clone(),
            component_hash,
            component_type: component.component_type,
            files,
            dynamic_linking: component
                .metadata
                .dynamic_linking
                .iter()
                .map(|(name, link)| {
                    (
                        name.clone(),
                        match link {
                            golem_common::model::component_metadata::DynamicLinkedInstance::WasmRpc(links) => links
                                .targets
                                .iter()
                                .map(|(resource, target)| {
                                    (resource.clone(), target.interface_name.clone())
                                })
                                .collect::<BTreeMap<String, String>>(),
                        },
                    )
                })
                .collect(),
        })
    }

    pub fn from_manifest(
        component_name: &AppComponentName,
        component_hash: String,
        component_type: ComponentType,
        files: BTreeMap<String, DiffableComponentFile>,
        dynamic_linking: Option<&DynamicLinking>,
    ) -> anyhow::Result<Self> {
        Ok(DiffableComponent {
            component_name: component_name.as_str().into(),
            component_hash,
            component_type,
            files,
            dynamic_linking: dynamic_linking
                .iter()
                .flat_map(|dl| {
                    dl.dynamic_linking.iter().map(|(name, instance)| {
                        (
                            name.clone(),
                            match instance {
                                DynamicLinkedInstance::WasmRpc(links) => links
                                    .targets
                                    .iter()
                                    .map(|(resource, target)| {
                                        (resource.clone(), target.interface_name.clone())
                                    })
                                    .collect::<BTreeMap<_, _>>(),
                            },
                        )
                    })
                })
                .collect(),
        })
    }
}

impl DiffSerialize for DiffableComponent {
    fn to_diffable_string(&self) -> anyhow::Result<String> {
        Ok(serde_yaml::to_string(&self)?)
    }
}

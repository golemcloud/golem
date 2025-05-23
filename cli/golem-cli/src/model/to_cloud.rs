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

use std::collections::HashMap;

pub trait ToCloud<T> {
    fn to_cloud(self) -> T;
}

impl<A: ToCloud<B>, B> ToCloud<Box<B>> for Box<A> {
    fn to_cloud(self) -> Box<B> {
        Box::new((*self).to_cloud())
    }
}

impl<A: ToCloud<B>, B> ToCloud<Option<B>> for Option<A> {
    fn to_cloud(self) -> Option<B> {
        self.map(|v| v.to_cloud())
    }
}

impl<A: ToCloud<B>, B> ToCloud<Vec<B>> for Vec<A> {
    fn to_cloud(self) -> Vec<B> {
        self.into_iter().map(|v| v.to_cloud()).collect()
    }
}

impl ToCloud<golem_cloud_client::model::ComponentType> for golem_client::model::ComponentType {
    fn to_cloud(self) -> golem_cloud_client::model::ComponentType {
        match self {
            golem_client::model::ComponentType::Durable => {
                golem_cloud_client::model::ComponentType::Durable
            }
            golem_client::model::ComponentType::Ephemeral => {
                golem_cloud_client::model::ComponentType::Ephemeral
            }
        }
    }
}

impl ToCloud<golem_cloud_client::model::ScanCursor> for golem_client::model::ScanCursor {
    fn to_cloud(self) -> golem_cloud_client::model::ScanCursor {
        golem_cloud_client::model::ScanCursor {
            cursor: self.cursor,
            layer: self.layer,
        }
    }
}

impl ToCloud<golem_cloud_client::model::InvokeParameters>
    for golem_client::model::InvokeParameters
{
    fn to_cloud(self) -> golem_cloud_client::model::InvokeParameters {
        golem_cloud_client::model::InvokeParameters {
            params: self.params,
        }
    }
}

impl ToCloud<golem_cloud_client::model::DynamicLinking> for golem_client::model::DynamicLinking {
    fn to_cloud(self) -> golem_cloud_client::model::DynamicLinking {
        golem_cloud_client::model::DynamicLinking {
            dynamic_linking: self
                .dynamic_linking
                .iter()
                .map(|(key, oss_instance)| {
                    (
                        key.clone(),
                        golem_cloud_client::model::DynamicLinkedInstance::WasmRpc(
                            golem_cloud_client::model::DynamicLinkedWasmRpc {
                                targets: match oss_instance {
                                    golem_client::model::DynamicLinkedInstance::WasmRpc(rpc) => rpc
                                        .targets
                                        .iter()
                                        .map(|(name, target)| {
                                            (
                                                name.clone(),
                                                golem_cloud_client::model::WasmRpcTarget {
                                                    interface_name: target.interface_name.clone(),
                                                    component_name: target.component_name.clone(),
                                                    component_type: target
                                                        .component_type
                                                        .to_cloud(),
                                                },
                                            )
                                        })
                                        .collect(),
                                },
                            },
                        ),
                    )
                })
                .collect::<HashMap<_, _>>(),
        }
    }
}

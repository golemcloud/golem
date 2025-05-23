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

// NOTE: This module contains normalized entities for doing diffs before deployment.
//       This solution is intended to be a naive and temporary one until environments
//       and atomic deployments will be developed.

pub mod api_definition;
pub mod api_deployment;
pub mod component;

use serde::Serialize;

pub trait DiffSerialize {
    fn to_diffable_string(&self) -> anyhow::Result<String>;
}

pub trait ToYamlValueWithoutNulls {
    fn to_yaml_value_without_nulls(&self) -> serde_yaml::Result<serde_yaml::Value>;
}

impl<T: Serialize> ToYamlValueWithoutNulls for T {
    fn to_yaml_value_without_nulls(&self) -> serde_yaml::Result<serde_yaml::Value> {
        Ok(yaml_value_without_nulls(serde_yaml::to_value(self)?))
    }
}

fn yaml_value_without_nulls(value: serde_yaml::Value) -> serde_yaml::Value {
    match value {
        serde_yaml::Value::Mapping(mapping) => serde_yaml::Value::Mapping(
            mapping
                .into_iter()
                .filter_map(|(key, value)| {
                    if value == serde_yaml::Value::Null {
                        None
                    } else {
                        Some((key, yaml_value_without_nulls(value)))
                    }
                })
                .collect(),
        ),
        serde_yaml::Value::Sequence(sequence) => serde_yaml::Value::Sequence(
            sequence.into_iter().map(yaml_value_without_nulls).collect(),
        ),
        _ => value,
    }
}

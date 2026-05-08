// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use crate::model::diff::{DiffError, Diffable};
use serde::Serialize;
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginInstallation {
    pub priority: i32,
    pub name: String,
    pub version: String,
    pub grant_id: Uuid,
    pub parameters: BTreeMap<String, String>,
}

impl Diffable for PluginInstallation {
    type DiffResult = PluginInstallationDiff;

    fn diff(new: &Self, current: &Self) -> Result<Option<Self::DiffResult>, DiffError> {
        let priority_changed = new.priority != current.priority;
        let parameters_changed = new.parameters != current.parameters;

        Ok(if priority_changed || parameters_changed {
            Some(PluginInstallationDiff {
                priority_changed,
                parameters_changed,
            })
        } else {
            None
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginInstallationDiff {
    pub priority_changed: bool,
    pub parameters_changed: bool,
}

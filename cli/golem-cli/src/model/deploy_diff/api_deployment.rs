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

use crate::model::api::ApiDeployment;
use crate::model::app_raw::HttpApiDeployment;
use crate::model::deploy_diff::DiffSerialize;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Sub;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiffableHttpApiDeployment {
    pub host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subdomain: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub definitions: BTreeSet<String>,
}

impl DiffableHttpApiDeployment {
    pub fn from_server(api_deployment: ApiDeployment) -> anyhow::Result<Self> {
        Ok(DiffableHttpApiDeployment {
            host: api_deployment.site.host,
            subdomain: api_deployment.site.subdomain,
            definitions: api_deployment
                .api_definitions
                .iter()
                .map(|api_def| format!("{}@{}", api_def.id, api_def.version))
                .collect(),
        })
    }

    pub fn from_manifest(
        api_deployment: &HttpApiDeployment,
        latest_api_definition_versions: &BTreeMap<String, String>,
    ) -> anyhow::Result<Self> {
        Ok(DiffableHttpApiDeployment {
            host: api_deployment.host.clone(),
            subdomain: api_deployment.subdomain.clone(),
            definitions: api_deployment
                .definitions
                .iter()
                .map(|api_def| {
                    if api_def.contains("@") {
                        api_def.clone()
                    } else {
                        format!(
                            "{}@{}",
                            api_def,
                            latest_api_definition_versions
                                .get(api_def)
                                .expect("Missing latest version for HTTP API definition")
                        )
                    }
                })
                .collect(),
        })
    }

    pub fn definitions(&self) -> impl Iterator<Item = (&str, &str)> {
        self.definitions.iter().map(to_name_and_version)
    }

    pub fn plan(&self, new: &Self) -> HttpApiDeploymentUpdatePlan {
        HttpApiDeploymentUpdatePlan {
            delete: self
                .definitions
                .sub(&new.definitions)
                .iter()
                .map(to_name_and_version_owned)
                .collect(),
            add: new
                .definitions
                .sub(&self.definitions)
                .iter()
                .map(to_name_and_version_owned)
                .collect(),
        }
    }
}

impl DiffSerialize for DiffableHttpApiDeployment {
    fn to_diffable_string(&self) -> anyhow::Result<String> {
        Ok(serde_yaml::to_string(&self)?)
    }
}

pub struct HttpApiDeploymentUpdatePlan {
    pub delete: BTreeSet<(String, String)>,
    pub add: BTreeSet<(String, String)>,
}

fn to_name_and_version<S: AsRef<str>>(api_def_with_version: &S) -> (&str, &str) {
    let mut parts = api_def_with_version.as_ref().split('@');
    (
        parts.next().expect("missing API name"),
        parts.next().expect("missing version"),
    )
}

fn to_name_and_version_owned<S: AsRef<str>>(api_def_with_version: &S) -> (String, String) {
    let (name, version) = to_name_and_version(api_def_with_version);
    (name.to_owned(), version.to_owned())
}

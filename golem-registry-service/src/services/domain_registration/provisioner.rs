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

use super::aws_provisioner::{AwsDomainProvisioner, AwsDomainProvisionerConfig};
use async_trait::async_trait;
use golem_common::SafeDisplay;
use golem_common::model::Empty;
use golem_common::model::domain_registration::Domain;
use serde::{Deserialize, Serialize};
use std::fmt::Write;
use std::sync::Arc;

#[async_trait]
pub trait DomainProvisioner: Send + Sync {
    fn domain_available_to_provision(&self, domain: &Domain) -> bool;

    async fn provision_domain(&self, domain: &Domain) -> anyhow::Result<()>;

    async fn remove_domain(&self, domain: &Domain) -> anyhow::Result<()>;
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum DomainProvisionerConfig {
    Aws(AwsDomainProvisionerConfig),
    NoOp(Empty),
}

impl Default for DomainProvisionerConfig {
    fn default() -> DomainProvisionerConfig {
        DomainProvisionerConfig::NoOp(Empty {})
    }
}

impl SafeDisplay for DomainProvisionerConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        match self {
            DomainProvisionerConfig::Aws(inner) => {
                let _ = writeln!(&mut result, "AWS:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
            DomainProvisionerConfig::NoOp(_) => {
                let _ = writeln!(&mut result, "noop");
            }
        }
        result
    }
}

pub async fn configured(
    environment: &str,
    workspace: &str,
    config: &DomainProvisionerConfig,
) -> anyhow::Result<Arc<dyn DomainProvisioner>> {
    match config {
        DomainProvisionerConfig::NoOp(_) => Ok(Arc::new(NoopDomainProvisioner)),
        DomainProvisionerConfig::Aws(config) => {
            let provisioner = AwsDomainProvisioner::new(environment, workspace, config).await?;
            Ok(Arc::new(provisioner))
        }
    }
}

pub struct NoopDomainProvisioner;

#[async_trait]
impl DomainProvisioner for NoopDomainProvisioner {
    fn domain_available_to_provision(&self, _domain: &Domain) -> bool {
        true
    }

    async fn provision_domain(&self, _domain: &Domain) -> anyhow::Result<()> {
        Ok(())
    }

    async fn remove_domain(&self, _domain: &Domain) -> anyhow::Result<()> {
        Ok(())
    }
}

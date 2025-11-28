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

use super::aws_config::AwsConfig;
use super::aws_load_balancer::AwsLoadBalancer;
use super::provisioner::DomainProvisioner;
use anyhow::anyhow;
use async_trait::async_trait;
use golem_common::SafeDisplay;
use golem_common::model::domain_registration::Domain;
use rusoto_route53::{
    AliasTarget, Change, ChangeBatch, ChangeResourceRecordSetsRequest, ListHostedZonesRequest,
    ResourceRecordSet, Route53, Route53Client,
};
use serde::{Deserialize, Serialize};
use std::fmt::Write;

pub struct AwsDomainProvisioner {
    domain_suffix: String,
    hosted_zone: AwsRoute53HostedZone,
    client: Route53Client,
    load_balancer: AwsLoadBalancer,
}

impl AwsDomainProvisioner {
    pub async fn new(
        environment: &str,
        workspace: &str,
        config: &AwsDomainProvisionerConfig,
    ) -> anyhow::Result<Self> {
        let aws_config = AwsConfig::from_k8s_env();
        Self::with_aws_config(environment, workspace, aws_config, config).await
    }

    async fn with_aws_config(
        environment: &str,
        workspace: &str,
        aws_config: AwsConfig,
        config: &AwsDomainProvisionerConfig,
    ) -> anyhow::Result<Self> {
        let load_balancer = AwsLoadBalancer::new(environment, workspace, &aws_config).await?;
        let client: Route53Client = aws_config.clone().try_into()?;
        let hosted_zone =
            AwsRoute53HostedZone::with_client(&client, &config.managed_domain).await?;

        Ok(Self {
            domain_suffix: format!(".{}", config.managed_domain),
            hosted_zone,
            client,
            load_balancer,
        })
    }
}

#[async_trait]
impl DomainProvisioner for AwsDomainProvisioner {
    fn domain_available_to_provision(&self, domain: &Domain) -> bool {
        domain.0.ends_with(&self.domain_suffix)
    }

    async fn provision_domain(&self, domain: &Domain) -> anyhow::Result<()> {
        let change_batch = ChangeBatch {
            changes: vec![Change {
                action: "UPSERT".to_string(),
                resource_record_set: ResourceRecordSet {
                    name: domain.0.clone(),
                    type_: "A".to_string(),
                    alias_target: Some(AliasTarget {
                        dns_name: self.load_balancer.dns_name.clone(),
                        evaluate_target_health: false,
                        hosted_zone_id: self.load_balancer.hosted_zone.clone(),
                    }),
                    ..Default::default()
                },
            }],
            ..Default::default()
        };

        let request = ChangeResourceRecordSetsRequest {
            hosted_zone_id: self.hosted_zone.id.clone(),
            change_batch,
        };

        self.client.change_resource_record_sets(request).await?;

        Ok(())
    }

    async fn remove_domain(&self, domain: &Domain) -> anyhow::Result<()> {
        let change_batch = ChangeBatch {
            changes: vec![Change {
                action: "DELETE".to_string(),
                resource_record_set: ResourceRecordSet {
                    name: domain.0.clone(),
                    type_: "A".to_string(),
                    alias_target: Some(AliasTarget {
                        dns_name: self.load_balancer.dns_name.clone(),
                        evaluate_target_health: false,
                        hosted_zone_id: self.load_balancer.hosted_zone.clone(),
                    }),
                    ..Default::default()
                },
            }],
            ..Default::default()
        };

        let request = ChangeResourceRecordSetsRequest {
            hosted_zone_id: self.hosted_zone.id.clone(),
            change_batch,
        };

        self.client.change_resource_record_sets(request).await?;

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AwsRoute53HostedZone {
    pub id: String,
    pub name: String,
}

impl AwsRoute53HostedZone {
    pub async fn with_client(
        client: &Route53Client,
        domain: &str,
    ) -> anyhow::Result<AwsRoute53HostedZone> {
        let zones = client
            .list_hosted_zones(ListHostedZonesRequest::default())
            .await?;

        let target_zone_name = format!("{domain}."); // appends a dot

        let zone = zones
            .hosted_zones
            .iter()
            .find(|x| x.name.clone() == target_zone_name)
            .map(move |x| AwsRoute53HostedZone {
                id: x
                    .id
                    .clone()
                    .strip_prefix("/hostedzone/")
                    .unwrap()
                    .to_string(),
                name: target_zone_name,
            });

        zone.ok_or(anyhow!("hosted zone not found"))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AwsDomainProvisionerConfig {
    managed_domain: String,
}

impl Default for AwsDomainProvisionerConfig {
    fn default() -> Self {
        Self {
            // TODO: separate domain for custom apis
            managed_domain: "dev-api.golem.cloud".to_string(),
        }
    }
}

impl SafeDisplay for AwsDomainProvisionerConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "managed_domain:");
        let _ = writeln!(&mut result, "{}", self.managed_domain);
        result
    }
}

#[cfg(test)]
mod tests {
    use crate::services::domain_registration::aws_config::AwsConfig;
    use crate::services::domain_registration::aws_provisioner::{
        AwsDomainProvisioner, AwsDomainProvisionerConfig,
    };
    use crate::services::domain_registration::provisioner::DomainProvisioner;
    use golem_common::model::domain_registration::Domain;
    use test_r::test;

    async fn provisioner() -> anyhow::Result<AwsDomainProvisioner> {
        let provisioner = AwsDomainProvisioner::with_aws_config(
            "dev",
            "release",
            AwsConfig::new("TOKEN", "ARN"),
            &AwsDomainProvisionerConfig {
                managed_domain: "dev-api.golem.cloud".to_string(),
            },
        )
        .await?;

        Ok(provisioner)
    }

    #[test]
    #[ignore]
    pub async fn test_provision() -> anyhow::Result<()> {
        let provisioner = provisioner().await?;
        let result = provisioner
            .provision_domain(&Domain(
                "aws-provisioner-test.dev-api.golem.cloud".to_string(),
            ))
            .await;
        println!("result: {result:?}");
        assert!(result.is_ok());
        Ok(())
    }

    #[test]
    #[ignore]
    pub async fn test_remove() -> anyhow::Result<()> {
        let provisioner = provisioner().await?;
        let result = provisioner
            .provision_domain(&Domain(
                "aws-provisioner-test.dev-api.golem.cloud".to_string(),
            ))
            .await;
        println!("result: {result:?}");
        assert!(result.is_ok());
        Ok(())
    }
}

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
use anyhow::anyhow;
use aws_sdk_elasticloadbalancingv2::types::Tag;

#[derive(Debug, Clone, PartialEq)]
pub struct AwsLoadBalancerListener {
    pub arn: String,
    pub protocol: String,
    pub port: i32,
}

impl AwsLoadBalancerListener {
    pub fn is_https(&self) -> bool {
        self.protocol == "HTTPS"
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AwsLoadBalancer {
    pub arn: String,
    pub name: String,
    pub dns_name: String,
    pub hosted_zone: String,
    pub listeners: Vec<AwsLoadBalancerListener>,
}

impl AwsLoadBalancer {
    pub async fn new(
        environment: &str,
        workspace: &str,
        config: &AwsConfig,
    ) -> anyhow::Result<AwsLoadBalancer> {
        let cluster_tag = Tag::builder()
            .key("elbv2.k8s.aws/cluster")
            .value(format!("golem-eks-cluster-{environment}"))
            .build();
        let stack_tag = Tag::builder()
            .key("ingress.k8s.aws/stack")
            .value(format!("{environment}-{workspace}/ingress-api-gateway"))
            .build();
        let client = config.elb_client();

        let balancers = client.describe_load_balancers().send().await?;

        let resource_arns: Vec<String> = balancers
            .load_balancers()
            .iter()
            .filter_map(|balancer| balancer.load_balancer_arn().map(|s| s.to_string()))
            .collect();

        let balancers_tags = client
            .describe_tags()
            .set_resource_arns(Some(resource_arns))
            .send()
            .await?;

        let mut balancer_arn: Option<String> = None;

        for tags in balancers_tags.tag_descriptions() {
            let mut has_cluster = false;
            let mut has_stack = false;

            for tag in tags.tags() {
                if tag.key() == cluster_tag.key() && tag.value() == cluster_tag.value() {
                    has_cluster = true;
                } else if tag.key() == stack_tag.key() && tag.value() == stack_tag.value() {
                    has_stack = true;
                }

                if has_cluster && has_stack {
                    break;
                }
            }

            if has_cluster
                && has_stack
                && let Some(resource_arn) = tags.resource_arn()
            {
                balancer_arn = balancers
                    .load_balancers()
                    .iter()
                    .find(|b| b.load_balancer_arn() == Some(resource_arn))
                    .and_then(|b| b.load_balancer_arn().map(|s| s.to_string()));
                if balancer_arn.is_some() {
                    break;
                }
            }
        }

        match balancer_arn {
            Some(ref arn) => {
                let balancer = balancers
                    .load_balancers()
                    .iter()
                    .find(|b| b.load_balancer_arn() == Some(arn.as_str()))
                    .ok_or_else(|| anyhow!("load balancer not found"))?;

                let balancer_listeners = client
                    .describe_listeners()
                    .load_balancer_arn(arn)
                    .send()
                    .await?;

                let listeners = balancer_listeners
                    .listeners()
                    .iter()
                    .map(|listener| AwsLoadBalancerListener {
                        arn: listener.listener_arn().unwrap_or_default().to_string(),
                        protocol: listener
                            .protocol()
                            .map(|p| p.as_str().to_string())
                            .unwrap_or_default(),
                        port: listener.port().unwrap_or_default(),
                    })
                    .collect();

                Ok(AwsLoadBalancer {
                    arn: arn.clone(),
                    name: balancer
                        .load_balancer_name()
                        .unwrap_or_default()
                        .to_string(),
                    dns_name: balancer.dns_name().unwrap_or_default().to_string(),
                    hosted_zone: balancer
                        .canonical_hosted_zone_id()
                        .unwrap_or_default()
                        .to_string(),
                    listeners,
                })
            }
            None => Err(anyhow!("load balancer not found")),
        }
    }

    pub fn get_https_listener(&self) -> Option<AwsLoadBalancerListener> {
        self.listeners
            .iter()
            .find(|listener| listener.is_https())
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use crate::services::domain_registration::aws_config::AwsConfig;
    use crate::services::domain_registration::aws_load_balancer::AwsLoadBalancer;
    use test_r::test;

    async fn aws_config() -> AwsConfig {
        AwsConfig::new().await
    }

    #[test]
    #[ignore]
    pub async fn test_aws_load_balancer() {
        let config = aws_config().await;
        let result = AwsLoadBalancer::new("dev", "release", &config).await;

        println!("result: {result:?}");
        assert!(result.is_ok());
    }
}

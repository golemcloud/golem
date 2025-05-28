use rusoto_elbv2::{
    DescribeListenersInput, DescribeLoadBalancersInput, DescribeTagsInput, Elb, ElbClient,
    LoadBalancer,
};

use crate::aws_config::*;

#[derive(Debug, Clone, PartialEq)]
pub struct AwsLoadBalancerListener {
    pub arn: String,
    pub protocol: String,
    pub port: i64,
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
    ) -> Result<AwsLoadBalancer, Box<dyn std::error::Error>> {
        let cluster_tag = rusoto_elbv2::Tag {
            key: "elbv2.k8s.aws/cluster".to_string(),
            value: Some(format!("golem-eks-cluster-{}", environment)),
        };
        let stack_tag = rusoto_elbv2::Tag {
            key: "ingress.k8s.aws/stack".to_string(),
            value: Some(format!("{}-{}/ingress-api-gateway", environment, workspace)),
        };
        let client: ElbClient = config.clone().try_into()?;

        let balancers = client
            .describe_load_balancers(DescribeLoadBalancersInput::default())
            .await?;

        let resource_arns = balancers
            .load_balancers
            .iter()
            .flatten()
            .map(|balancer| balancer.load_balancer_arn.clone().unwrap())
            .collect();

        let balancers_tags = client
            .describe_tags(DescribeTagsInput { resource_arns })
            .await?;

        let mut balancer: Option<&LoadBalancer> = None;

        for tags in balancers_tags.tag_descriptions.iter().flatten() {
            let mut has_cluster = false;
            let mut has_stack = false;

            for tag in tags.tags.iter().flatten() {
                let t = tag.clone();
                if t == cluster_tag {
                    has_cluster = true;
                } else if t == stack_tag {
                    has_stack = true;
                }

                if has_cluster && has_stack {
                    break;
                }
            }

            if has_cluster && has_stack {
                balancer = balancers.load_balancers.iter().flatten().find(|balancer| {
                    balancer.load_balancer_arn.is_some()
                        && balancer.load_balancer_arn == tags.resource_arn
                });

                if balancer.is_some() {
                    break;
                }
            }
        }

        match balancer {
            Some(balancer) => {
                let balancer_listeners = client
                    .describe_listeners(DescribeListenersInput {
                        load_balancer_arn: balancer.load_balancer_arn.clone(),
                        ..Default::default()
                    })
                    .await?;

                let listeners = balancer_listeners
                    .listeners
                    .iter()
                    .flatten()
                    .map(|listener| AwsLoadBalancerListener {
                        arn: listener.listener_arn.clone().unwrap(),
                        protocol: listener.protocol.clone().unwrap(),
                        port: listener.port.unwrap(),
                    })
                    .collect();

                Ok(AwsLoadBalancer {
                    arn: balancer.load_balancer_arn.clone().unwrap(),
                    name: balancer.load_balancer_name.clone().unwrap(),
                    dns_name: balancer.dns_name.clone().unwrap(),
                    hosted_zone: balancer.canonical_hosted_zone_id.clone().unwrap(),
                    listeners,
                })
            }
            None => Err("Not found".to_string().into()),
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
    use test_r::test;

    use crate::aws_config::AwsConfig;
    use crate::aws_load_balancer::AwsLoadBalancer;

    fn aws_config() -> AwsConfig {
        AwsConfig::new("TOKEN", "ARN")
    }

    #[test]
    #[ignore]
    pub async fn test_aws_load_balancer() {
        let config = aws_config();
        let result = AwsLoadBalancer::new("dev", "release", &config).await;

        println!("result: {:?}", result);
        assert!(result.is_ok());
    }
}

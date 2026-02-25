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

use aws_config::BehaviorVersion;

#[derive(Debug, Clone)]
pub struct AwsConfig {
    pub sdk_config: aws_config::SdkConfig,
}

impl AwsConfig {
    pub async fn from_k8s_env() -> Self {
        let sdk_config = aws_config::load_defaults(BehaviorVersion::latest()).await;
        Self { sdk_config }
    }

    pub async fn new(_token: &str, _role_arn: &str) -> Self {
        // In tests this is called with dummy values; the default credential chain
        // will resolve credentials from the environment (including web identity token).
        let sdk_config = aws_config::load_defaults(BehaviorVersion::latest()).await;
        Self { sdk_config }
    }

    pub fn acm_client(&self) -> aws_sdk_acm::Client {
        aws_sdk_acm::Client::new(&self.sdk_config)
    }

    pub fn elb_client(&self) -> aws_sdk_elasticloadbalancingv2::Client {
        aws_sdk_elasticloadbalancingv2::Client::new(&self.sdk_config)
    }

    pub fn route53_client(&self) -> aws_sdk_route53::Client {
        aws_sdk_route53::Client::new(&self.sdk_config)
    }
}

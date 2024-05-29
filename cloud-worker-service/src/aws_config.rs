use rusoto_acm::AcmClient;
use rusoto_core::{HttpClient, Region};
use rusoto_credential::{Secret, Variable};
use rusoto_elbv2::ElbClient;
use rusoto_route53::Route53Client;

#[derive(Debug, Clone)]
pub struct AwsConfig {
    pub credentials_provider: rusoto_sts::WebIdentityProvider,
    pub region: Region,
}

impl AwsConfig {
    pub fn from_k8s_env() -> Self {
        Self {
            credentials_provider: rusoto_sts::WebIdentityProvider::from_k8s_env(),
            region: Region::default(),
        }
    }

    pub fn new(token: &str, role_arn: &str) -> Self {
        let credentials_provider = rusoto_sts::WebIdentityProvider {
            web_identity_token: Variable::with_value(Secret::from(token.to_string())),
            role_arn: Variable::with_value(role_arn.to_string()),
            role_session_name: None,
            duration_seconds: None,
            policy: None,
            policy_arns: None,
        };

        AwsConfig {
            credentials_provider,
            region: Region::default(),
        }
    }
}

impl TryInto<AcmClient> for AwsConfig {
    type Error = Box<dyn std::error::Error>;

    fn try_into(self) -> Result<AcmClient, Self::Error> {
        let http_client = HttpClient::new()?;

        Ok(AcmClient::new_with(
            http_client,
            self.credentials_provider.clone(),
            self.region.clone(),
        ))
    }
}

impl TryInto<ElbClient> for AwsConfig {
    type Error = Box<dyn std::error::Error>;

    fn try_into(self) -> Result<ElbClient, Self::Error> {
        let http_client = HttpClient::new()?;

        Ok(ElbClient::new_with(
            http_client,
            self.credentials_provider.clone(),
            self.region.clone(),
        ))
    }
}

impl TryInto<Route53Client> for AwsConfig {
    type Error = Box<dyn std::error::Error>;

    fn try_into(self) -> Result<Route53Client, Self::Error> {
        let http_client = HttpClient::new()?;

        Ok(Route53Client::new_with(
            http_client,
            self.credentials_provider.clone(),
            self.region.clone(),
        ))
    }
}

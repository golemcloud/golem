use std::fmt::Debug;
use std::fmt::Display;

use bincode::{Decode, Encode};
use poem_openapi::NewType;
use serde::{Deserialize, Serialize};

use crate::worker_binding::GolemWorkerBinding;

// Common to API definitions regardless of different protocols
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize, Encode, Decode, NewType)]
pub struct ApiDefinitionId(pub String);

impl Display for ApiDefinitionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize, Encode, Decode, NewType)]
pub struct ApiVersion(pub String);

impl Display for ApiVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

// Constraints applicable to any type of API Definition
pub trait HasApiDefinitionId {
    fn get_api_definition_id(&self) -> ApiDefinitionId;
}

pub trait HasVersion {
    fn get_version(&self) -> ApiVersion;
}

pub trait HasGolemWorkerBindings {
    fn get_golem_worker_bindings(&self) -> Vec<GolemWorkerBinding>;
}

#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize, Encode, Decode, NewType)]
pub struct Domain(pub String);

#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize, Encode, Decode, NewType)]
pub struct SubDomain(pub String);

pub struct Host {
    pub domain: Domain,
    pub sub_domain: SubDomain,
}

pub trait HasHost {
    fn get_host(&self) -> Host;
}

impl Host {
    pub fn new(domain: Domain, sub_domain: SubDomain) -> Host {
        Host { domain, sub_domain }
    }

    pub fn from_string(host: &str) -> Host {
        let parts: Vec<&str> = host.split(".").collect();
        let domain = Domain(parts[1..].join("."));
        let sub_domain = SubDomain(parts[0].to_string());
        Host { domain, sub_domain }
    }
}

impl Display for Host {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.sub_domain.0, self.domain.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn test_roundtrip_host() {
        let host = Host::from_string("subdomain.domain.com");
        let host_str = host.to_string();
        let output = Host::from_string(&host_str);
        assert_eq!(
            (output, output.domain, output.sub_domain),
            (
                host,
                Domain("domain.com".to_string()),
                SubDomain("subdomain".to_string())
            )
        );
    }
}

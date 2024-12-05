pub mod account;
pub mod api_definition;
pub mod api_deployment;
pub mod api_security;
pub mod certificate;
pub mod component;
pub mod domain;
pub mod errors;
pub mod grant;
pub mod health_check;
pub mod login;
pub mod plugin;
pub mod policy;
pub mod project;
pub mod project_grant;
pub mod token;
pub mod worker;

use golem_cli::cloud::AccountId;
use golem_cloud_client::model::{TokenSecret, UnsafeToken};

pub fn token_header(secret: &TokenSecret) -> String {
    format!("bearer {}", secret.value)
}

#[derive(Clone, PartialEq, Debug)]
pub struct CloudAuthentication(pub UnsafeToken);

impl CloudAuthentication {
    pub fn header(&self) -> String {
        let CloudAuthentication(value) = self;

        token_header(&value.secret)
    }

    pub fn account_id(&self) -> AccountId {
        let CloudAuthentication(value) = self;

        AccountId {
            id: value.data.account_id.clone(),
        }
    }
}

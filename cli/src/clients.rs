use golem_client::model::{TokenSecret, UnsafeToken};
use crate::model::AccountId;

pub mod login;
pub mod account;
pub mod token;

pub fn token_header(secret: &TokenSecret) -> String {
    format!("bearer {}", secret.value)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CloudAuthentication(pub UnsafeToken);

impl CloudAuthentication {
    pub fn header(&self) -> String {
        let CloudAuthentication(value) = self;

        token_header(&value.secret)
    }

    pub fn account_id(&self) -> AccountId {
        let CloudAuthentication(value) = self;

        AccountId {id: value.data.account_id.clone()}
    }
}

use crate::model::{AccountId, ProjectAction};
use golem_client::model::TokenSecret;
use golem_client::model::UnsafeToken;

pub mod account;
pub mod errors;
pub mod gateway;
pub mod grant;
pub mod login;
pub mod policy;
pub mod project;
pub mod project_grant;
pub mod template;
pub mod token;
pub mod worker;

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

pub fn action_cli_to_api(action: ProjectAction) -> golem_client::model::ProjectAction {
    match action {
        ProjectAction::ViewTemplate => golem_client::model::ProjectAction::ViewTemplate {},
        ProjectAction::CreateTemplate => golem_client::model::ProjectAction::CreateTemplate {},
        ProjectAction::UpdateTemplate => golem_client::model::ProjectAction::UpdateTemplate {},
        ProjectAction::DeleteTemplate => golem_client::model::ProjectAction::DeleteTemplate {},
        ProjectAction::ViewWorker => golem_client::model::ProjectAction::ViewWorker {},
        ProjectAction::CreateWorker => golem_client::model::ProjectAction::CreateWorker {},
        ProjectAction::UpdateWorker => golem_client::model::ProjectAction::UpdateWorker {},
        ProjectAction::DeleteWorker => golem_client::model::ProjectAction::DeleteWorker {},
        ProjectAction::ViewProjectGrants => {
            golem_client::model::ProjectAction::ViewProjectGrants {}
        }
        ProjectAction::CreateProjectGrants => {
            golem_client::model::ProjectAction::CreateProjectGrants {}
        }
        ProjectAction::DeleteProjectGrants => {
            golem_client::model::ProjectAction::DeleteProjectGrants {}
        }
    }
}

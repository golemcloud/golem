pub mod account;
pub mod api_definition;
pub mod api_deployment;
pub mod certificate;
pub mod component;
pub mod domain;
pub mod errors;
pub mod grant;
pub mod health_check;
pub mod login;
pub mod policy;
pub mod project;
pub mod project_grant;
pub mod token;
pub mod worker;

use crate::cloud::model::ProjectAction;
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

pub fn action_cli_to_api(action: ProjectAction) -> golem_cloud_client::model::ProjectAction {
    match action {
        ProjectAction::ViewComponent => golem_cloud_client::model::ProjectAction::ViewComponent {},
        ProjectAction::CreateComponent => {
            golem_cloud_client::model::ProjectAction::CreateComponent {}
        }
        ProjectAction::UpdateComponent => {
            golem_cloud_client::model::ProjectAction::UpdateComponent {}
        }
        ProjectAction::DeleteComponent => {
            golem_cloud_client::model::ProjectAction::DeleteComponent {}
        }
        ProjectAction::ViewWorker => golem_cloud_client::model::ProjectAction::ViewWorker {},
        ProjectAction::CreateWorker => golem_cloud_client::model::ProjectAction::CreateWorker {},
        ProjectAction::UpdateWorker => golem_cloud_client::model::ProjectAction::UpdateWorker {},
        ProjectAction::DeleteWorker => golem_cloud_client::model::ProjectAction::DeleteWorker {},
        ProjectAction::ViewProjectGrants => {
            golem_cloud_client::model::ProjectAction::ViewProjectGrants {}
        }
        ProjectAction::CreateProjectGrants => {
            golem_cloud_client::model::ProjectAction::CreateProjectGrants {}
        }
        ProjectAction::DeleteProjectGrants => {
            golem_cloud_client::model::ProjectAction::DeleteProjectGrants {}
        }
    }
}

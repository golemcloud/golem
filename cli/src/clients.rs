use golem_client::model::{TokenSecret, UnsafeToken};
use crate::model::{AccountId, ProjectAction};

pub mod login;
pub mod account;
pub mod token;
pub mod component;
pub mod project;
pub mod grant;
pub mod policy;
pub mod project_grant;
pub mod instance;

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

pub fn action_cli_to_api(action: ProjectAction) -> golem_client::model::ProjectAction {
    match action {
        ProjectAction::ViewComponent => golem_client::model::ProjectAction::ViewComponent {},
        ProjectAction::CreateComponent => golem_client::model::ProjectAction::CreateComponent {},
        ProjectAction::UpdateComponent => golem_client::model::ProjectAction::UpdateComponent {},
        ProjectAction::DeleteComponent => golem_client::model::ProjectAction::DeleteComponent {},
        ProjectAction::ViewInstance => golem_client::model::ProjectAction::ViewInstance {},
        ProjectAction::CreateInstance => golem_client::model::ProjectAction::CreateInstance {},
        ProjectAction::UpdateInstance => golem_client::model::ProjectAction::UpdateInstance {},
        ProjectAction::DeleteInstance => golem_client::model::ProjectAction::DeleteInstance {},
        ProjectAction::ViewProjectGrants => golem_client::model::ProjectAction::ViewProjectGrants {},
        ProjectAction::CreateProjectGrants => golem_client::model::ProjectAction::CreateProjectGrants {},
        ProjectAction::DeleteProjectGrants => golem_client::model::ProjectAction::DeleteProjectGrants {},
    }
}
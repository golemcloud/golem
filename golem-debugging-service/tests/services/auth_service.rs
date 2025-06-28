use async_trait::async_trait;
use golem_common::model::auth::ProjectAction;
use golem_common::model::auth::{AuthCtx, Namespace};
use golem_common::model::{AccountId, ComponentId, ProjectId};
use golem_debugging_service::auth::AuthService;
use golem_service_base::clients::auth::AuthServiceError;

// This will be used by debugging service in tests
pub struct TestAuthService;

#[async_trait]
impl AuthService for TestAuthService {
    async fn get_account(&self, ctx: &AuthCtx) -> Result<AccountId, AuthServiceError> {
        Ok(AccountId::from(ctx.token_secret.value.to_string().as_str()))
    }

    async fn authorize_project_action(
        &self,
        project_id: &ProjectId,
        _permission: ProjectAction,
        ctx: &AuthCtx,
    ) -> Result<Namespace, AuthServiceError> {
        Ok(Namespace::new(
            project_id.clone(),
            AccountId::from(ctx.token_secret.value.to_string().as_str()),
        ))
    }

    async fn is_authorized_by_component(
        &self,
        component_id: &ComponentId,
        _permission: ProjectAction,
        ctx: &AuthCtx,
    ) -> Result<Namespace, AuthServiceError> {
        Ok(Namespace::new(
            ProjectId(component_id.0),
            AccountId::from(ctx.token_secret.value.to_string().as_str()),
        ))
    }
}

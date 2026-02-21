use async_trait::async_trait;
use golem_common::model::auth::TokenSecret;
use golem_common::model::environment::EnvironmentId;
use golem_debugging_service::services::auth::{AuthService, AuthServiceError};
use golem_service_base::model::auth::{AuthCtx, UserAuthCtx};
use golem_worker_executor_test_utils::TestContext;

pub struct TestAuthService {
    test_ctx: TestContext,
}

impl TestAuthService {
    pub fn new(test_ctx: TestContext) -> Self {
        Self { test_ctx }
    }
}

#[async_trait]
impl AuthService for TestAuthService {
    async fn authenticate_token(&self, token: TokenSecret) -> Result<AuthCtx, AuthServiceError> {
        if token != self.test_ctx.account_token {
            return Err(AuthServiceError::CouldNotAuthenticate);
        }
        Ok(AuthCtx::User(UserAuthCtx {
            account_id: self.test_ctx.account_id,
            account_plan_id: self.test_ctx.account_plan_id,
            account_roles: self.test_ctx.account_roles.clone(),
        }))
    }
    async fn check_user_allowed_to_debug_in_environment(
        &self,
        _environment_id: EnvironmentId,
        _auth_ctx: &AuthCtx,
    ) -> Result<(), AuthServiceError> {
        Ok(())
    }
}

use async_trait::async_trait;
use golem_common::model::auth::TokenSecret;
use golem_debugging_service::services::auth::{AuthService, AuthServiceError};
use golem_service_base::model::auth::{AuthCtx, UserAuthCtx};
use golem_worker_executor_test_utils::TestContext;

// This will be used by debugging service in tests
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
}

use async_trait::async_trait;
use golem_common::model::auth::TokenSecret;
use golem_common::model::card::owner::AgentOwnerPattern;
use golem_common::model::card::{
    AgentResourcePattern, AgentVerb, ClassPermissionTarget, ConfigResourcePattern, ConfigVerb,
    EffectiveSurface, EnvResourcePattern, EnvVerb, FilesystemResourcePattern, FilesystemVerb,
    GrantSurface, OplogResourcePattern, OplogVerb, PermissionTarget,
};
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
        let owner = AgentOwnerPattern::AccountAgents {
            account: golem_common::model::account::AccountEmail::new("test@golem"),
        };
        Ok(AuthCtx::User(UserAuthCtx {
            account_id: self.test_ctx.account_id,
            account_email: golem_common::model::account::AccountEmail::new("test@golem"),
            account_plan_id: self.test_ctx.account_plan_id,
            account_roles: self.test_ctx.account_roles.clone(),
            effective_surface: EffectiveSurface {
                source_card_ids: Vec::new(),
                lower: vec![GrantSurface {
                    positive: vec![
                        PermissionTarget::Oplog(ClassPermissionTarget {
                            verb: Some(OplogVerb::Read),
                            owner: owner.clone(),
                            resource: OplogResourcePattern::Any,
                        }),
                        PermissionTarget::Agent(ClassPermissionTarget {
                            verb: Some(AgentVerb::View),
                            owner: owner.clone(),
                            resource: AgentResourcePattern::Empty,
                        }),
                        PermissionTarget::Filesystem(ClassPermissionTarget {
                            verb: Some(FilesystemVerb::Read),
                            owner: owner.clone(),
                            resource: FilesystemResourcePattern::any(),
                        }),
                        PermissionTarget::Env(ClassPermissionTarget {
                            verb: Some(EnvVerb::Read),
                            owner: owner.clone(),
                            resource: EnvResourcePattern::Any,
                        }),
                        PermissionTarget::Config(ClassPermissionTarget {
                            verb: Some(ConfigVerb::Read),
                            owner: owner.clone(),
                            resource: ConfigResourcePattern::Any,
                        }),
                        PermissionTarget::Agent(ClassPermissionTarget {
                            verb: Some(AgentVerb::Fork),
                            owner: owner.clone(),
                            resource: AgentResourcePattern::Empty,
                        }),
                        PermissionTarget::Agent(ClassPermissionTarget {
                            verb: Some(AgentVerb::Interrupt),
                            owner: owner.clone(),
                            resource: AgentResourcePattern::Empty,
                        }),
                        PermissionTarget::Agent(ClassPermissionTarget {
                            verb: Some(AgentVerb::Resume),
                            owner,
                            resource: AgentResourcePattern::Empty,
                        }),
                    ],
                    negative: Vec::new(),
                }],
                upper: Vec::new(),
            },
            delegation_surface: None,
        }))
    }
}

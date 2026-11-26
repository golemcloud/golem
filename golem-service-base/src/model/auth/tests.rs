// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::*;
use assert2::assert;
use golem_common::model::card::owner::{AccountOwnerPattern, EmptyOwnerPattern};
use golem_common::model::card::recipient::RecipientPattern;
use golem_common::model::card::{
    AccountResourcePattern, AccountTokenResourcePattern, AccountTokenVerb, AccountVerb, CardId,
    ClassPermissionPattern, ClassPermissionTarget, DelegationCard, PermissionPattern,
    PermissionTarget, SystemResourcePattern, SystemVerb,
};
use test_r::test;

fn account_email(account_id: AccountId) -> golem_common::model::account::AccountEmail {
    golem_common::model::account::AccountEmail::new(format!("{account_id}@test"))
}

fn mk_user_ctx(roles: &[AccountRole], plan_id: PlanId, account_id: AccountId) -> AuthCtx {
    AuthCtx::User(UserAuthCtx {
        account_id,
        account_email: account_email(account_id),
        account_plan_id: plan_id,
        account_roles: roles.iter().cloned().collect(),
        effective_surface: empty_effective_surface(),
        delegation_surface: None,
        authenticated_by: None,
    })
}

fn empty_effective_surface() -> EffectiveSurface {
    EffectiveSurface {
        source_card_ids: Vec::new(),
        lower: Vec::new(),
        upper: Vec::new(),
    }
}

fn report_grant(recipient: RecipientPattern) -> PermissionPattern {
    PermissionPattern::System(ClassPermissionPattern {
        verb: Some(SystemVerb::ViewAccountSummariesReport),
        owner: EmptyOwnerPattern,
        recipient,
        resource: SystemResourcePattern,
    })
}

fn report_target() -> PermissionTarget {
    PermissionTarget::System(ClassPermissionTarget {
        verb: Some(SystemVerb::ViewAccountSummariesReport),
        owner: EmptyOwnerPattern,
        resource: SystemResourcePattern,
    })
}

fn account_token_grant(account_id: AccountId, recipient: RecipientPattern) -> PermissionPattern {
    PermissionPattern::AccountToken(ClassPermissionPattern {
        verb: Some(AccountTokenVerb::Create),
        owner: AccountOwnerPattern::Account {
            account: account_email(account_id),
        },
        recipient,
        resource: AccountTokenResourcePattern::Any,
    })
}

fn account_token_target(account_id: AccountId) -> PermissionTarget {
    PermissionTarget::AccountToken(ClassPermissionTarget {
        verb: Some(AccountTokenVerb::Create),
        owner: AccountOwnerPattern::Account {
            account: account_email(account_id),
        },
        resource: AccountTokenResourcePattern::Any,
    })
}

fn account_grant(account_id: AccountId, recipient: RecipientPattern) -> PermissionPattern {
    PermissionPattern::Account(ClassPermissionPattern {
        verb: Some(AccountVerb::Update),
        owner: AccountOwnerPattern::Account {
            account: account_email(account_id),
        },
        recipient,
        resource: AccountResourcePattern,
    })
}

fn account_target(account_id: AccountId) -> PermissionTarget {
    PermissionTarget::Account(ClassPermissionTarget {
        verb: Some(AccountVerb::Update),
        owner: AccountOwnerPattern::Account {
            account: account_email(account_id),
        },
        resource: AccountResourcePattern,
    })
}

fn effective_surface_for_account(
    account_id: AccountId,
    lower_positive: Vec<PermissionPattern>,
) -> EffectiveSurface {
    let recipient = RecipientPattern::Account {
        account: account_email(account_id),
    };
    EffectiveSurface::from_grants(&lower_positive, &[], &[], &[], &recipient).unwrap()
}

#[test]
fn system_authorization_bypasses_roles_and_effective_surface() {
    let ctx = AuthCtx::System;
    assert!(!ctx.account_roles().contains(&AccountRole::Admin));

    assert!(ctx.authorize_permission(&report_target()).is_ok());
    assert!(ctx.authorize_permission(&report_target()).is_ok());
}

#[test]
fn system_auth_cannot_validate_card_derivation() {
    let ctx = AuthCtx::System;

    assert!(matches!(
        ctx.delegation_surface_for_card_derivation("test"),
        Err(super::AuthorizationError::AuthContextHasNoCards(_))
    ));
}

#[test]
fn auth_without_delegation_surface_cannot_validate_card_derivation() {
    let ctx = mk_user_ctx(&[], PlanId::new(), AccountId::new());

    assert!(matches!(
        ctx.delegation_surface_for_card_derivation("test"),
        Err(super::AuthorizationError::AuthContextHasNoCards(_))
    ));

    let encoded: golem_api_grpc::proto::golem::auth::AuthCtx = ctx.clone().into();
    let decoded = AuthCtx::try_from(encoded).unwrap();
    assert!(decoded == ctx);
}

#[test]
fn auth_context_protobuf_preserves_delegation_surface() {
    let account_id = AccountId::new();
    let delegation_surface = DelegationSurface {
        cards: vec![DelegationCard {
            source_card_id: Some(CardId::new()),
            lower_positive: vec![report_grant(RecipientPattern::Any)],
            lower_negative: Vec::new(),
            upper_positive: Vec::new(),
            upper_negative: Vec::new(),
        }],
    };
    let ctx = AuthCtx::User(UserAuthCtx {
        account_id,
        account_email: account_email(account_id),
        account_plan_id: PlanId::new(),
        account_roles: BTreeSet::new(),
        effective_surface: empty_effective_surface(),
        delegation_surface: Some(delegation_surface),
        authenticated_by: None,
    });

    let encoded: golem_api_grpc::proto::golem::auth::AuthCtx = ctx.clone().into();
    let decoded = AuthCtx::try_from(encoded).unwrap();

    assert!(decoded == ctx);
}

#[test]
fn auth_context_protobuf_preserves_the_authenticating_token() {
    use golem_common::model::auth::TokenId;

    let account_id = AccountId::new();
    let token = TokenId::new();
    let user = AuthCtx::User(UserAuthCtx {
        account_id,
        account_email: account_email(account_id),
        account_plan_id: PlanId::new(),
        account_roles: BTreeSet::new(),
        effective_surface: empty_effective_surface(),
        delegation_surface: None,
        authenticated_by: Some(token),
    });
    let impersonation = AuthCtx::AdminImpersonation(AdminImpersonationAuthCtx {
        admin_account_id: AccountId::new(),
        target_account_id: account_id,
        target_account_email: account_email(account_id),
        target_account_roles: BTreeSet::new(),
        target_account_plan_id: PlanId::new(),
        effective_surface: empty_effective_surface(),
        delegation_surface: None,
        authenticated_by: Some(token),
    });

    for ctx in [user, impersonation] {
        let encoded: golem_api_grpc::proto::golem::auth::AuthCtx = ctx.clone().into();
        let decoded = AuthCtx::try_from(encoded).unwrap();
        assert!(decoded.authenticated_by() == Some(token));
        assert!(decoded == ctx);
    }
    assert!(AuthCtx::System.authenticated_by() == None);
}

#[test]
fn user_with_effective_surface_can_authorize_permission() {
    let account_id = AccountId::new();
    let grant = report_grant(RecipientPattern::Account {
        account: account_email(account_id),
    });
    let target = report_target();
    let ctx = AuthCtx::User(UserAuthCtx {
        account_id,
        account_email: account_email(account_id),
        account_plan_id: PlanId::new(),
        account_roles: BTreeSet::new(),
        effective_surface: effective_surface_for_account(account_id, vec![grant]),
        delegation_surface: None,
        authenticated_by: None,
    });

    assert!(ctx.authorize_permission(&target).is_ok());
}

#[test]
fn user_with_empty_effective_surface_cannot_authorize_permission() {
    let permission = report_target();
    let ctx = mk_user_ctx(&[], PlanId::new(), AccountId::new());

    assert!(ctx.authorize_permission(&permission).is_err());
}

#[test]
fn user_with_effective_surface_can_authorize_account_token_permission() {
    let account_id = AccountId::new();
    let recipient = RecipientPattern::Account {
        account: account_email(account_id),
    };
    let grant = account_token_grant(account_id, recipient);
    let target = account_token_target(account_id);
    let ctx = AuthCtx::User(UserAuthCtx {
        account_id,
        account_email: account_email(account_id),
        account_plan_id: PlanId::new(),
        account_roles: BTreeSet::new(),
        effective_surface: effective_surface_for_account(account_id, vec![grant]),
        delegation_surface: None,
        authenticated_by: None,
    });

    assert!(ctx.authorize_permission(&target).is_ok());
}

#[test]
fn effective_surface_account_token_grant_for_different_holder_does_not_authorize_target() {
    let account_id = AccountId::new();
    let other_account_id = AccountId::new();
    let grant = account_token_grant(
        account_id,
        RecipientPattern::Account {
            account: account_email(other_account_id),
        },
    );
    let target = account_token_target(account_id);
    let ctx = AuthCtx::User(UserAuthCtx {
        account_id,
        account_email: account_email(account_id),
        account_plan_id: PlanId::new(),
        account_roles: BTreeSet::new(),
        effective_surface: effective_surface_for_account(account_id, vec![grant]),
        delegation_surface: None,
        authenticated_by: None,
    });

    assert!(ctx.authorize_permission(&target).is_err());
}

#[test]
fn effective_surface_account_token_target_ignores_recipient_after_holder_filtering() {
    let account_id = AccountId::new();
    let grant = account_token_grant(
        account_id,
        RecipientPattern::Account {
            account: account_email(account_id),
        },
    );
    let target = account_token_target(account_id);
    let ctx = AuthCtx::User(UserAuthCtx {
        account_id,
        account_email: account_email(account_id),
        account_plan_id: PlanId::new(),
        account_roles: BTreeSet::new(),
        effective_surface: effective_surface_for_account(account_id, vec![grant]),
        delegation_surface: None,
        authenticated_by: None,
    });

    assert!(ctx.authorize_permission(&target).is_ok());
}

#[test]
fn effective_surface_account_token_grant_does_not_authorize_different_owner_target() {
    let account_id = AccountId::new();
    let grant = account_token_grant(
        account_id,
        RecipientPattern::Account {
            account: account_email(account_id),
        },
    );
    let ctx = AuthCtx::User(UserAuthCtx {
        account_id,
        account_email: account_email(account_id),
        account_plan_id: PlanId::new(),
        account_roles: BTreeSet::new(),
        effective_surface: effective_surface_for_account(account_id, vec![grant]),
        delegation_surface: None,
        authenticated_by: None,
    });
    let requested = account_token_target(AccountId::new());

    assert!(ctx.authorize_permission(&requested).is_err());
}

#[test]
fn user_with_empty_effective_surface_cannot_authorize_account_token_permission() {
    let account_id = AccountId::new();
    let permission = account_token_target(account_id);
    let ctx = mk_user_ctx(&[], PlanId::new(), account_id);

    assert!(ctx.authorize_permission(&permission).is_err());
}

#[test]
fn user_with_effective_surface_can_authorize_account_permission() {
    let account_id = AccountId::new();
    let account_email = account_email(account_id);
    let recipient = RecipientPattern::Account {
        account: account_email.clone(),
    };
    let grant = account_grant(account_id, recipient);
    let target = account_target(account_id);
    let ctx = AuthCtx::User(UserAuthCtx {
        account_id,
        account_email,
        account_plan_id: PlanId::new(),
        account_roles: BTreeSet::new(),
        effective_surface: effective_surface_for_account(account_id, vec![grant]),
        delegation_surface: None,
        authenticated_by: None,
    });

    assert!(ctx.authorize_permission(&target).is_ok());
}

#[test]
fn user_with_empty_effective_surface_cannot_authorize_account_permission() {
    let account_id = AccountId::new();
    let permission = account_target(account_id);
    let ctx = mk_user_ctx(&[], PlanId::new(), account_id);

    assert!(ctx.authorize_permission(&permission).is_err());
}

mod authorizer {
    use super::super::Authorizer;
    use assert2::assert;
    use golem_common::model::account::AccountId;
    use golem_common::model::agent::AgentTypeName;
    use golem_common::model::application::ApplicationId;
    use golem_common::model::authority::{
        AgentTarget, AuthorizationRequest, CatalogOperation, Decision, Denial, DesignatedKind,
        EnvironmentTarget, Operation, Resource, Scope, Target,
    };
    use golem_common::model::caller::Caller;
    use golem_common::model::environment::EnvironmentId;
    use golem_common::model::{AgentFingerprint, AgentId};
    use std::sync::Arc;
    use test_r::test;
    use uuid::Uuid;

    /// Answers every decision the same way and shows only the targets it was given.
    struct FixedAuthorizer {
        decision: Decision,
        visible: Vec<Target>,
    }

    #[async_trait::async_trait]
    impl Authorizer for FixedAuthorizer {
        async fn decide(&self, _request: &AuthorizationRequest) -> Decision {
            self.decision.clone()
        }

        async fn filter(
            &self,
            _request: &AuthorizationRequest,
            targets: Vec<Target>,
        ) -> Vec<Target> {
            targets
                .into_iter()
                .filter(|target| self.visible.contains(target))
                .collect()
        }

        async fn scope(&self, _request: &AuthorizationRequest) -> Scope {
            Scope {
                whole_environment: false,
                agent_types: vec![AgentTypeName("Report".to_string())],
                tools: vec![],
                agents: vec![],
            }
        }
    }

    fn agent(h: u128) -> Target {
        Target::Agent(AgentTarget {
            environment_id: EnvironmentId(Uuid::from_u128(0xe0)),
            agent_type: AgentTypeName("Report".to_string()),
            agent_id: AgentId::from_parts(7, h),
            fingerprint: Some(AgentFingerprint(Uuid::from_u128(0xf1))),
        })
    }

    #[test]
    async fn an_authorizer_is_shared_as_a_trait_object_and_answers_decide_filter_and_scope() {
        let denial = Decision::Deny(Denial::LooksUnknown {
            kind: DesignatedKind::Agent,
        });
        let authorizer: Arc<dyn Authorizer> = Arc::new(FixedAuthorizer {
            decision: denial.clone(),
            visible: vec![agent(1)],
        });
        let listing = AuthorizationRequest::new(
            Caller::anonymous(),
            Operation::single(
                CatalogOperation::ListAgents,
                Resource::None,
                Target::Environment(EnvironmentTarget {
                    account_id: AccountId(Uuid::from_u128(0xacc0)),
                    application_id: ApplicationId(Uuid::from_u128(0xa0)),
                    environment_id: EnvironmentId(Uuid::from_u128(0xe0)),
                }),
            )
            .unwrap(),
        );

        assert!(authorizer.decide(&listing).await == denial);
        assert!(authorizer.filter(&listing, vec![agent(1), agent(2)]).await == vec![agent(1)]);
        assert!(
            authorizer.scope(&listing).await.agent_types
                == vec![AgentTypeName("Report".to_string())]
        );
    }
}

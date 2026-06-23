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
    AccountResourcePattern, AccountTokenResourcePattern, AccountTokenVerb, AccountVerb,
    ClassPermissionPattern, ClassPermissionTarget, PermissionPattern, PermissionTarget,
    SystemResourcePattern, SystemVerb,
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
        account: account_id.to_string(),
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
fn user_with_effective_surface_can_authorize_permission() {
    let account_id = AccountId::new();
    let grant = report_grant(RecipientPattern::Account {
        account: account_id.to_string(),
    });
    let target = report_target();
    let ctx = AuthCtx::User(UserAuthCtx {
        account_id,
        account_email: account_email(account_id),
        account_plan_id: PlanId::new(),
        account_roles: BTreeSet::new(),
        effective_surface: effective_surface_for_account(account_id, vec![grant]),
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
        account: account_id.to_string(),
    };
    let grant = account_token_grant(account_id, recipient);
    let target = account_token_target(account_id);
    let ctx = AuthCtx::User(UserAuthCtx {
        account_id,
        account_email: account_email(account_id),
        account_plan_id: PlanId::new(),
        account_roles: BTreeSet::new(),
        effective_surface: effective_surface_for_account(account_id, vec![grant]),
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
            account: other_account_id.to_string(),
        },
    );
    let target = account_token_target(account_id);
    let ctx = AuthCtx::User(UserAuthCtx {
        account_id,
        account_email: account_email(account_id),
        account_plan_id: PlanId::new(),
        account_roles: BTreeSet::new(),
        effective_surface: effective_surface_for_account(account_id, vec![grant]),
    });

    assert!(ctx.authorize_permission(&target).is_err());
}

#[test]
fn effective_surface_account_token_target_ignores_recipient_after_holder_filtering() {
    let account_id = AccountId::new();
    let grant = account_token_grant(
        account_id,
        RecipientPattern::Account {
            account: account_id.to_string(),
        },
    );
    let target = account_token_target(account_id);
    let ctx = AuthCtx::User(UserAuthCtx {
        account_id,
        account_email: account_email(account_id),
        account_plan_id: PlanId::new(),
        account_roles: BTreeSet::new(),
        effective_surface: effective_surface_for_account(account_id, vec![grant]),
    });

    assert!(ctx.authorize_permission(&target).is_ok());
}

#[test]
fn effective_surface_account_token_grant_does_not_authorize_different_owner_target() {
    let account_id = AccountId::new();
    let grant = account_token_grant(
        account_id,
        RecipientPattern::Account {
            account: account_id.to_string(),
        },
    );
    let ctx = AuthCtx::User(UserAuthCtx {
        account_id,
        account_email: account_email(account_id),
        account_plan_id: PlanId::new(),
        account_roles: BTreeSet::new(),
        effective_surface: effective_surface_for_account(account_id, vec![grant]),
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
    let recipient = RecipientPattern::Account {
        account: account_id.to_string(),
    };
    let grant = account_grant(account_id, recipient);
    let target = account_target(account_id);
    let ctx = AuthCtx::User(UserAuthCtx {
        account_id,
        account_email: account_email(account_id),
        account_plan_id: PlanId::new(),
        account_roles: BTreeSet::new(),
        effective_surface: effective_surface_for_account(account_id, vec![grant]),
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

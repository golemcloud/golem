// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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
use test_r::test;

fn mk_user_ctx(roles: &[AccountRole], plan_id: PlanId, account_id: AccountId) -> AuthCtx {
    AuthCtx::User(UserAuthCtx {
        account_id,
        account_plan_id: plan_id,
        account_roles: roles.iter().cloned().collect(),
    })
}

fn mk_impersonated(id: AccountId) -> AuthCtx {
    AuthCtx::ImpersonatedUser(ImpersonatedUserAuthCtx { account_id: id })
}

fn make_env_roles(roles: &[EnvironmentRole]) -> BTreeSet<EnvironmentRole> {
    roles.iter().copied().collect()
}

#[test]
fn system_can_do_global_actions() {
    let ctx = AuthCtx::System;
    assert!(
        ctx.authorize_global_action(GlobalAction::CreateAccount)
            .is_ok()
    );
    assert!(
        ctx.authorize_global_action(GlobalAction::GetDefaultPlan)
            .is_ok()
    );
    assert!(
        ctx.authorize_global_action(GlobalAction::GetReports)
            .is_ok()
    );
}

#[test]
fn user_without_roles_cannot_do_global_actions() {
    let ctx = mk_user_ctx(&[], PlanId::new(), AccountId::new());
    assert!(
        ctx.authorize_global_action(GlobalAction::CreateAccount)
            .is_err()
    );
    assert!(
        ctx.authorize_global_action(GlobalAction::GetDefaultPlan)
            .is_err()
    );
    assert!(
        ctx.authorize_global_action(GlobalAction::GetReports)
            .is_err()
    );
}

#[test]
fn marketing_admin_can_get_reports() {
    let ctx = mk_user_ctx(
        &[AccountRole::MarketingAdmin],
        PlanId::new(),
        AccountId::new(),
    );
    assert!(
        ctx.authorize_global_action(GlobalAction::GetReports)
            .is_ok()
    );
    assert!(
        ctx.authorize_global_action(GlobalAction::CreateAccount)
            .is_err()
    );
}

#[test]
fn impersonated_cannot_do_global_actions() {
    let ctx = mk_impersonated(AccountId::new());
    assert!(
        ctx.authorize_global_action(GlobalAction::CreateAccount)
            .is_err()
    );
    assert!(
        ctx.authorize_global_action(GlobalAction::GetReports)
            .is_err()
    );
}

#[test]
fn user_can_view_own_plan() {
    let plan_id = PlanId::new();
    let ctx = mk_user_ctx(&[], plan_id, AccountId::new());
    assert!(
        ctx.authorize_plan_action(&plan_id, PlanAction::ViewPlan)
            .is_ok()
    );
}

#[test]
fn user_cannot_view_other_plan() {
    let ctx = mk_user_ctx(&[], PlanId::new(), AccountId::new());
    assert!(
        ctx.authorize_plan_action(&PlanId::new(), PlanAction::ViewPlan)
            .is_err()
    );
}

#[test]
fn admin_can_view_any_plan() {
    let ctx = mk_user_ctx(&[AccountRole::Admin], PlanId::new(), AccountId::new());
    assert!(
        ctx.authorize_plan_action(&PlanId::new(), PlanAction::ViewPlan)
            .is_ok()
    );
}

#[test]
fn impersonated_cannot_view_any_plan() {
    let ctx = mk_impersonated(AccountId::new());
    assert!(
        ctx.authorize_plan_action(&PlanId::new(), PlanAction::ViewPlan)
            .is_err()
    );
}

#[test]
fn user_can_modify_own_account_basic_fields() {
    let account_id = AccountId::new();
    let ctx = mk_user_ctx(&[], PlanId::new(), account_id);
    assert!(
        ctx.authorize_account_action(account_id, AccountAction::UpdateAccount)
            .is_ok()
    );
}

#[test]
fn user_cannot_modify_other_account() {
    let ctx = mk_user_ctx(&[], PlanId::new(), AccountId::new());
    assert!(
        ctx.authorize_account_action(AccountId::new(), AccountAction::UpdateAccount)
            .is_err()
    );
}

#[test]
fn admin_can_modify_any_account() {
    let ctx = mk_user_ctx(&[AccountRole::Admin], PlanId::new(), AccountId::new());
    assert!(
        ctx.authorize_account_action(AccountId::new(), AccountAction::SetPlan)
            .is_ok()
    );
}

#[test]
fn impersonated_cannot_modify_account() {
    let account_id = AccountId::new();
    let ctx = mk_impersonated(account_id);
    assert!(
        ctx.authorize_account_action(account_id, AccountAction::UpdateAccount)
            .is_ok()
    );

    assert!(
        ctx.authorize_account_action(account_id, AccountAction::SetPlan)
            .is_err()
    );
    assert!(
        ctx.authorize_account_action(account_id, AccountAction::SetRoles)
            .is_err()
    );
}

#[test]
fn owner_can_do_everything() {
    let account_id = AccountId::new();
    let ctx = mk_user_ctx(&[], PlanId::new(), account_id);

    assert!(
        ctx.authorize_environment_action(
            account_id,
            &make_env_roles(&[]),
            EnvironmentAction::DeleteEnvironment,
        )
        .is_ok()
    );
}

#[test]
fn non_owner_with_viewer_role_can_view_env() {
    let ctx = mk_user_ctx(&[], PlanId::new(), AccountId::new());
    let env_owner = AccountId::new();

    assert!(
        ctx.authorize_environment_action(
            env_owner,
            &make_env_roles(&[EnvironmentRole::Viewer]),
            EnvironmentAction::ViewEnvironment,
        )
        .is_ok()
    );
}

#[test]
fn non_owner_cannot_update_env_without_admin_role() {
    let ctx = mk_user_ctx(&[], PlanId::new(), AccountId::new());
    let env_owner = AccountId::new();

    assert!(
        ctx.authorize_environment_action(
            env_owner,
            &make_env_roles(&[EnvironmentRole::Viewer]),
            EnvironmentAction::UpdateEnvironment,
        )
        .is_err()
    );
}

#[test]
fn impersonated_with_deployer_role_can_deploy() {
    let ctx = mk_impersonated(AccountId::new());

    assert!(
        ctx.authorize_environment_action(
            AccountId::new(),
            &make_env_roles(&[EnvironmentRole::Deployer]),
            EnvironmentAction::DeployEnvironment,
        )
        .is_ok()
    );
}

#[test]
fn impersonated_cannot_do_admin_operations_without_admin_role() {
    let ctx = mk_impersonated(AccountId::new());
    let env_owner = AccountId::new();

    assert!(
        ctx.authorize_environment_action(
            env_owner,
            &make_env_roles(&[EnvironmentRole::Viewer]),
            EnvironmentAction::DeleteEnvironment,
        )
        .is_err()
    );
}

#[test]
fn admin_user_can_do_any_environment_action() {
    let ctx = mk_user_ctx(&[AccountRole::Admin], PlanId::new(), AccountId::new());
    let env_owner = AccountId::new();

    assert!(
        ctx.authorize_environment_action(
            env_owner,
            &make_env_roles(&[]),
            EnvironmentAction::DeleteEnvironment
        )
        .is_ok()
    );
}

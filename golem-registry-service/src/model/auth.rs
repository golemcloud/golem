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

use golem_common::model::account::AccountId;
use golem_common::model::auth::{
    AccountAction, AccountRole, EnvironmentAction, EnvironmentRole, GlobalAction, PlanAction,
};
use std::collections::HashSet;
use std::hash::Hash;
use uuid::uuid;
use golem_common::SafeDisplay;
use golem_common::model::PlanId;

pub const SYSTEM_ACCOUNT_ID: AccountId = AccountId(uuid!("00000000-0000-0000-0000-000000000000"));

#[derive(Debug, thiserror::Error)]
pub enum AuthorizationError {
    #[error("The global action {0} is not allowed")]
    GlobalActionNotAllowed(GlobalAction),
    #[error("The plan action {0} is not allowed")]
    PlanActionNotAllowed(PlanAction),
    #[error("The account action {0} is not allowed")]
    AccountActionNotAllowed(AccountAction),
    #[error("The environment action {0} is not allowed")]
    EnvironmentActionNotAllowed(EnvironmentAction),
}

impl SafeDisplay for AuthorizationError {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

#[derive(Debug)]
pub struct AuthCtx {
    pub account_id: AccountId,
    pub account_plan_id: Option<PlanId>,
    pub account_roles: HashSet<AccountRole>,
}

// Note: Basic visibility of resources is enforced in the repo. Use this to check permissions to modify resource / access restricted resources.
// To support defense-in-depth everything in here should be cheap -- avoid async / fetching data.
impl AuthCtx {
    /// Get the sytem AuthCtx for system initiated action
    pub fn system() -> AuthCtx {
        AuthCtx {
            account_id: SYSTEM_ACCOUNT_ID.clone(),
            account_plan_id: None,
            account_roles: HashSet::from([AccountRole::Admin]),
        }
    }

    /// Whether storage level visibility rules (e.g. does this account have any shares for this environment)
    /// should be disabled for this user.
    pub fn should_override_storage_visibility_rules(&self) -> bool {
        has_any_role(&self.account_roles, &[AccountRole::Admin])
    }

    pub fn authorize_global_action(&self, action: GlobalAction) -> Result<(), AuthorizationError> {
        let is_allowed = match action {
            GlobalAction::CreateAccount => has_any_role(
                &self.account_roles,
                &[AccountRole::Admin]
            ),
            GlobalAction::GetDefaultPlan => has_any_role(
                &self.account_roles,
                &[AccountRole::Admin]
            ),
        };

        if !is_allowed {
            Err(AuthorizationError::GlobalActionNotAllowed(action))?
        }

        Ok(())
    }

    pub fn authorize_plan_action(&self, plan_id: &PlanId, action: PlanAction) -> Result<(), AuthorizationError> {
        match action {
            PlanAction::ViewPlan => {
                // Users are allowed to see their own plan
                if let Some(account_plan_id) = &self.account_plan_id && account_plan_id == plan_id {
                    return Ok(())
                }

                // admins are allowed to see all plans
                if has_any_role(&self.account_roles, &[AccountRole::Admin]) {
                    return Ok(())
                }

                Err(AuthorizationError::PlanActionNotAllowed(action))
            }
            PlanAction::CreateOrUpdatePlan => {
                // Only admins can change plan details
                if has_any_role(&self.account_roles, &[AccountRole::Admin]) {
                    Ok(())
                } else {
                    Err(AuthorizationError::PlanActionNotAllowed(action))

                }
            }
        }
    }

    pub fn authorize_account_action(
        &self,
        target_account_id: &AccountId,
        action: AccountAction,
    ) -> Result<(), AuthorizationError> {
        // Accounts owners are allowed to do everything with their accounts
        if self.account_id == *target_account_id {
            return Ok(());
        };

        let is_allowed = match action {
            AccountAction::ViewAccount => has_any_role(
                &self.account_roles,
                &[AccountRole::Admin]
            ),
            AccountAction::UpdateAccount =>  has_any_role(
                &self.account_roles,
                &[AccountRole::Admin]
            ),
            AccountAction::CreateApplication =>  has_any_role(
                &self.account_roles,
                &[AccountRole::Admin]
            ),
            AccountAction::SetRoles =>  has_any_role(
                &self.account_roles,
                &[AccountRole::Admin]
            ),
            AccountAction::ViewToken =>  has_any_role(
                &self.account_roles,
                &[AccountRole::Admin]
            ),
            AccountAction::CreateToken =>  has_any_role(
                &self.account_roles,
                &[AccountRole::Admin]
            ),
            AccountAction::DeleteToken =>  has_any_role(
                &self.account_roles,
                &[AccountRole::Admin]
            ),
            AccountAction::CreateKnownSecret =>  has_any_role(
                &self.account_roles,
                &[AccountRole::Admin]
            ),
            AccountAction::UpdateUsage =>  has_any_role(
                &self.account_roles,
                &[AccountRole::Admin]
            ),
        };

        if !is_allowed {
            Err(AuthorizationError::AccountActionNotAllowed(action))?
        }

        Ok(())
    }

    pub fn authorize_environment_action(
        &self,
        account_owning_enviroment: &AccountId,
        roles_from_shares: &HashSet<EnvironmentRole>,
        action: EnvironmentAction,
    ) -> Result<(), AuthorizationError> {
        // Environment owners are allowed to do everything with their environments
        if self.account_id == *account_owning_enviroment {
            return Ok(());
        };

        let is_allowed = match action {
            EnvironmentAction::ViewEnvironment => has_any_role(
                roles_from_shares,
                &[EnvironmentRole::Admin, EnvironmentRole::Deployer, EnvironmentRole::Viewer],
            ),
            EnvironmentAction::CreateComponent => has_any_role(
                roles_from_shares,
                &[EnvironmentRole::Admin, EnvironmentRole::Deployer],
            ),
            EnvironmentAction::UpdateComponent => has_any_role(
                roles_from_shares,
                &[EnvironmentRole::Admin, EnvironmentRole::Deployer],
            ),
            EnvironmentAction::ViewComponent => has_any_role(
                roles_from_shares,
                &[EnvironmentRole::Admin, EnvironmentRole::Deployer, EnvironmentRole::Viewer],
            ),
            EnvironmentAction::ViewShares => has_any_role(
                roles_from_shares,
                &[EnvironmentRole::Admin],
            ),
            EnvironmentAction::UpdateShare => has_any_role(
                roles_from_shares,
                &[EnvironmentRole::Admin],
            ),
            EnvironmentAction::CreateShare => has_any_role(
                roles_from_shares,
                &[EnvironmentRole::Admin],
            ),
            EnvironmentAction::DeleteShare => has_any_role(
                roles_from_shares,
                &[EnvironmentRole::Admin],
            ),
        };

        if !is_allowed {
            Err(AuthorizationError::EnvironmentActionNotAllowed(action))?
        };

        Ok(())
    }
}

fn has_any_role<T: Eq + Hash>(roles: &HashSet<T>, allowed: &[T]) -> bool {
    allowed.iter().any(|r| roles.contains(r))
}

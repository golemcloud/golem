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

use super::audit::{AuditContext, DecisionAudit};
use super::surface::Surface;
use super::{AuthCtx, AuthorizationError, Authorizer};
use async_trait::async_trait;
use golem_common::model::account::AccountId;
use golem_common::model::agent::AgentTypeName;
use golem_common::model::authority::{
    AuthorizationRequest, Decision, Grantors, Operation, Scope, Target,
};
use golem_common::model::caller::{AccountAuthentication, AccountPrincipal, Caller, Principal};
use golem_common::model::card::{
    EffectiveSurface, PermissionTarget, check_belongs_to_part, checks_allow, decide_with_checks,
};
use std::collections::HashMap;
use std::sync::Arc;

/// Answers `decide` from the card checks that the call site makes today, for one authenticated
/// request, until the switch to the new card model replaces it. Build one for each request: the
/// checks name today's owners and resources, which the decision request does not hold.
pub struct CompatibilityAuthorizer {
    auth: AuthCtx,
    /// Today's checks for each part of the request's operation, in the order of the parts.
    checks: Vec<Vec<PermissionTarget>>,
    /// Today's check of each row that a listing may return.
    row_checks: HashMap<Target, Vec<PermissionTarget>>,
    /// Today's check of each agent type that a listing may scan.
    type_checks: Vec<(AgentTypeName, Vec<PermissionTarget>)>,
}

impl CompatibilityAuthorizer {
    pub fn new(auth: AuthCtx, checks: Vec<Vec<PermissionTarget>>) -> Self {
        Self {
            auth,
            checks,
            row_checks: HashMap::new(),
            type_checks: Vec::new(),
        }
    }

    /// For `filter`: today's check of each row. A row without a check is dropped.
    pub fn with_row_checks(
        mut self,
        rows: impl IntoIterator<Item = (Target, Vec<PermissionTarget>)>,
    ) -> Self {
        self.row_checks.extend(rows);
        self
    }

    /// For `scope`: today's check of each agent type that the listing may scan.
    pub fn with_type_checks(
        mut self,
        types: impl IntoIterator<Item = (AgentTypeName, Vec<PermissionTarget>)>,
    ) -> Self {
        self.type_checks.extend(types);
        self
    }

    /// The error that today's code gives for a request that `decide` denies: the error of the
    /// first check that the caller's card surface refuses, or a check that does not belong to its
    /// part. Call sites return it, so that a denial keeps today's response.
    pub fn todays_error(&self, request: &AuthorizationRequest) -> AuthorizationError {
        for (part, checks) in request.operation.parts().iter().zip(&self.checks) {
            for check in checks {
                if !check_belongs_to_part(part.verb, check) {
                    return AuthorizationError::PermissionNotAllowed(Box::new(check.clone()));
                }
                if let Err(error) = self.auth.authorize_permission(check) {
                    return error;
                }
            }
        }
        // Reached only when the checks do not match the parts, which no call site does.
        AuthorizationError::SystemOnlyActionNotAllowed(request.operation.name().to_string())
    }

    /// The caller's card surface, or `None` for the system context, which today's checks allow.
    fn surface(&self) -> Option<&EffectiveSurface> {
        match &self.auth {
            AuthCtx::System => None,
            AuthCtx::User(user) => Some(&user.effective_surface),
            AuthCtx::Agent(agent) => Some(&agent.effective_surface),
            AuthCtx::AdminImpersonation(ctx) => Some(&ctx.effective_surface),
        }
    }
}

#[async_trait]
impl Authorizer for CompatibilityAuthorizer {
    async fn decide(&self, request: &AuthorizationRequest) -> Decision {
        match self.surface() {
            None => Decision::Allow(Grantors::default()),
            Some(surface) => decide_with_checks(surface, &request.operation, &self.checks),
        }
    }

    async fn filter(&self, request: &AuthorizationRequest, targets: Vec<Target>) -> Vec<Target> {
        let Some(surface) = self.surface() else {
            return targets;
        };
        let [part] = request.operation.parts() else {
            return Vec::new();
        };
        targets
            .into_iter()
            .filter(|target| {
                self.row_checks
                    .get(target)
                    .is_some_and(|checks| checks_allow(surface, part.verb, checks))
            })
            .collect()
    }

    async fn scope(&self, request: &AuthorizationRequest) -> Scope {
        let Some(surface) = self.surface() else {
            return Scope {
                whole_environment: true,
                ..Scope::default()
            };
        };
        let [part] = request.operation.parts() else {
            return Scope::default();
        };
        Scope {
            agent_types: self
                .type_checks
                .iter()
                .filter(|(_, checks)| checks_allow(surface, part.verb, checks))
                .map(|(agent_type, _)| agent_type.clone())
                .collect(),
            ..Scope::default()
        }
    }
}

/// Decides `request` from the card checks that the call site makes today, one list for each part.
/// A denial is the error that today's checks give, so the response stays the same.
/// `AuthCtx::System` skips the decision, as today. Every call site that has moved to `decide` in
/// the worker service and the registry decides through this function.
///
/// The decision is audited: its record names `surface`, and an allowed operation whose record
/// cannot be written fails with [`AuthorizationError::AuditUnavailable`] and must not run.
pub async fn decide_with_todays_checks(
    auth: &AuthCtx,
    request: AuthorizationRequest,
    checks: Vec<Vec<PermissionTarget>>,
    audit: &DecisionAudit,
    surface: Surface,
) -> Result<Grantors, AuthorizationError> {
    if auth.is_system() {
        return Ok(Grantors::default());
    }
    let authorizer = Arc::new(CompatibilityAuthorizer::new(auth.clone(), checks));
    let context = AuditContext::for_request(auth, surface);
    match audit
        .authorizer(authorizer.clone())
        .decide(&request, &context)
        .await
    {
        Ok(Decision::Allow(grantors)) => Ok(grantors),
        Ok(Decision::Deny(_)) => Err(authorizer.todays_error(&request)),
        Err(unavailable) => Err(AuthorizationError::AuditUnavailable(unavailable)),
    }
}

impl AuthCtx {
    /// The caller of a request that an account token authenticated: the access account in both
    /// roles. The request passes this caller on.
    pub fn entry_caller(&self, issuer: impl Into<String>) -> Caller {
        Caller::entry(Principal::Account(account_principal(
            issuer,
            self.account_id(),
        )))
    }

    /// The decision request of `operation` for a request that an account token authenticated:
    /// the acting account asks, on the authority of the access account. Under admin impersonation
    /// the admin asks and the target account's cards decide; otherwise both are the same account.
    pub fn entry_request(
        &self,
        issuer: impl Into<String>,
        operation: Operation,
    ) -> AuthorizationRequest {
        let issuer = issuer.into();
        let acting = account_principal(issuer.clone(), self.actor_account_id());
        AuthorizationRequest::new(Caller::entry(Principal::Account(acting)), operation)
            .on_authority_of(Principal::Account(account_principal(
                issuer,
                self.account_id(),
            )))
    }
}

fn account_principal(issuer: impl Into<String>, account_id: AccountId) -> AccountPrincipal {
    AccountPrincipal {
        issuer: issuer.into(),
        account_id,
        authentication: AccountAuthentication::ApiToken,
    }
}

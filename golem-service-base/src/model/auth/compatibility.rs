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

use super::{AuthCtx, Authorizer};
use async_trait::async_trait;
use golem_common::model::agent::AgentTypeName;
use golem_common::model::authority::{AuthorizationRequest, Decision, Grantors, Scope, Target};
use golem_common::model::caller::{AccountAuthentication, AccountPrincipal, Caller, Principal};
use golem_common::model::card::{
    EffectiveSurface, PermissionTarget, checks_allow, decide_with_checks,
};
use std::collections::HashMap;

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

impl AuthCtx {
    /// The caller of a request that an account token authenticated: the access account in both
    /// roles.
    pub fn entry_caller(&self, issuer: impl Into<String>) -> Caller {
        Caller::entry(Principal::Account(AccountPrincipal {
            issuer: issuer.into(),
            account_id: self.account_id(),
            authentication: AccountAuthentication::ApiToken,
        }))
    }
}

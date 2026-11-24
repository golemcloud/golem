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

use golem_common::model::authority::{
    AccountTarget, ApplicationTarget, EnvironmentTarget, Grantors, Operation, OperationShapeError,
    Target,
};
use golem_common::model::card::PermissionTarget;
use golem_common::model::golem_issuer::GolemIssuer;
use golem_service_base::model::auth::{AuthCtx, AuthorizationError, decide_with_todays_checks};
use tracing::error;

/// Decides the registry's operations that the decision catalog names, from the card check that
/// each call site makes today, so that the move to `decide` changes no answer.
#[derive(Clone, Debug)]
pub struct Decisions {
    golem_issuer: GolemIssuer,
}

impl Decisions {
    pub fn new(golem_issuer: GolemIssuer) -> Self {
        Self { golem_issuer }
    }

    /// Decides `operation` for the caller of `auth` from today's `check`. A denial is the error
    /// that today's check gives. An operation that the catalog refuses is a defect of its call
    /// site: it is logged, and today's check answers.
    pub async fn decide(
        &self,
        auth: &AuthCtx,
        operation: Result<Operation, OperationShapeError>,
        check: PermissionTarget,
    ) -> Result<Grantors, AuthorizationError> {
        match operation {
            Ok(operation) => {
                let caller = auth.entry_caller(self.golem_issuer.to_string());
                decide_with_todays_checks(auth, caller, operation, vec![vec![check]]).await
            }
            Err(shape) => {
                error!("Registry operation has no valid decision request: {shape}");
                auth.authorize_permission(&check)?;
                Ok(Grantors::default())
            }
        }
    }
}

pub fn account_target(account_id: golem_common::model::account::AccountId) -> Target {
    Target::Account(AccountTarget { account_id })
}

pub fn application_target(
    account_id: golem_common::model::account::AccountId,
    application_id: golem_common::model::application::ApplicationId,
) -> Target {
    Target::Application(ApplicationTarget {
        account_id,
        application_id,
    })
}

pub fn environment_target(
    account_id: golem_common::model::account::AccountId,
    application_id: golem_common::model::application::ApplicationId,
    environment_id: golem_common::model::environment::EnvironmentId,
) -> Target {
    Target::Environment(EnvironmentTarget {
        account_id,
        application_id,
        environment_id,
    })
}

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

mod deploy_validation_error;
mod deployment_context;
mod http_parameter_conversion;
mod mcp;
mod mirror;
mod read;
mod route_compilation;
mod routes;
mod write;

pub use self::deploy_validation_error::DeployValidationError;
pub use self::mcp::{DeployedMcpError, DeployedMcpService};
pub use self::mirror::{DeployedAgentTypeMirror, ResolvedAgentTypeMirror};
pub use self::read::{DeploymentError, DeploymentService};
pub use self::routes::{DeployedRoutesError, DeployedRoutesService};
pub use self::write::{DeploymentWriteError, DeploymentWriteService};
use golem_common::model::card::owner::ApplicationOwnerPattern;
use golem_common::model::card::{
    ClassPermissionTarget, EnvironmentResourcePattern,
    EnvironmentVerb, PermissionTarget,
};
pub use golem_common::model::deployment::DeployValidationWarning;
use golem_common::model::environment::Environment;
use golem_service_base::model::auth::{AuthCtx, AuthorizationError};

macro_rules! ok_or_continue {
    ($expr:expr, $errors:ident) => {{
        match ($expr) {
            Ok(v) => v,
            Err(e) => {
                $errors.push(e);
                continue;
            }
        }
    }};
}
use ok_or_continue;

fn authorize_environment_permission(
    auth: &AuthCtx,
    environment: &Environment,
    verb: EnvironmentVerb,
) -> Result<(), AuthorizationError> {
    auth.authorize_permission(&PermissionTarget::Environment(ClassPermissionTarget {
        verb: Some(verb),
        owner: ApplicationOwnerPattern::Application {
            account: environment.owner_account_email.clone(),
            application: environment.application_name.clone(),
        },
        resource: EnvironmentResourcePattern::Environment(CardEnvironmentName(
            environment.name.0.clone(),
        )),
    }))
}

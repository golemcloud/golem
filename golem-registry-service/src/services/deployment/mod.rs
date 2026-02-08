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

mod deployment_context;
mod http_parameter_conversion;
mod read;
mod routes;
mod route_compilation;
mod write;

pub use self::read::{DeploymentError, DeploymentService};
pub use self::routes::{DeployedRoutesError, DeployedRoutesService};
pub use self::write::{DeploymentWriteError, DeploymentWriteService};

use crate::repo::deployment::DeploymentRepo;
use crate::repo::model::deployment::DeployRepoError;
use crate::services::application::{ApplicationError, ApplicationService};
use crate::services::environment::{EnvironmentError, EnvironmentService};
use golem_common::model::account::AccountId;
use golem_common::model::agent::{AgentTypeName};
use golem_common::model::application::ApplicationName;
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::deployment::{
    DeploymentPlan, DeploymentRevision, DeploymentSummary, DeploymentVersion,
};
use golem_common::model::environment::{Environment, EnvironmentName};
use golem_common::{
    SafeDisplay, error_forwarding,
    model::{deployment::Deployment, environment::EnvironmentId},
};
use golem_service_base::model::auth::EnvironmentAction;
use golem_service_base::model::auth::{AuthCtx, AuthorizationError};
use golem_service_base::repo::RepoError;
use std::sync::Arc;
use golem_common::model::agent::DeployedRegisteredAgentType;

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
pub(self) use ok_or_continue;

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

pub mod caching;
pub mod error;
pub mod eventual;
pub mod eventual_batch;
pub mod types;

use crate::durable_host::DurableWorkerCtx;
use crate::workerctx::WorkerCtx;
use golem_common::model::card::owner::EnvironmentOwnerPattern;

pub(crate) const PERMISSION_DENIED: &str = "key-value permission denied";

pub(crate) fn environment_owner<Ctx: WorkerCtx>(
    ctx: &DurableWorkerCtx<Ctx>,
) -> EnvironmentOwnerPattern {
    EnvironmentOwnerPattern::Environment {
        account: ctx.state.component_metadata.account_email.clone(),
        application: ctx.state.component_metadata.application_name.clone(),
        environment: ctx.state.component_metadata.environment_name.clone(),
    }
}

pub(crate) fn denial(error: impl std::fmt::Display) -> String {
    format!("{PERMISSION_DENIED}: {error}")
}

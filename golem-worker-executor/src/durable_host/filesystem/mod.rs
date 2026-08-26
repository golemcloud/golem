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

pub mod preopens;
pub mod types;

use crate::durable_host::DurableWorkerCtx;
use crate::durable_host::authorization::targets::{CanonicalGuestPath, filesystem_target};
use crate::workerctx::WorkerCtx;
use golem_common::model::card::FilesystemVerb;
use golem_common::model::card::owner::{AgentOwnerLeafPattern, AgentOwnerPattern};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use wasmtime_wasi::p2::FsError;
use wasmtime_wasi::p2::bindings::filesystem::types::ErrorCode;

static GUEST_PATHS: OnceLock<Mutex<HashMap<(String, u32), CanonicalGuestPath>>> = OnceLock::new();

fn key<Ctx: WorkerCtx>(ctx: &DurableWorkerCtx<Ctx>, rep: u32) -> (String, u32) {
    (ctx.owned_agent_id().to_string(), rep)
}

pub(super) fn remember_path<Ctx: WorkerCtx>(
    ctx: &DurableWorkerCtx<Ctx>,
    rep: u32,
    path: CanonicalGuestPath,
) {
    GUEST_PATHS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .insert(key(ctx, rep), path);
}

pub(super) fn forget_path<Ctx: WorkerCtx>(ctx: &DurableWorkerCtx<Ctx>, rep: u32) {
    if let Some(paths) = GUEST_PATHS.get() {
        paths.lock().unwrap().remove(&key(ctx, rep));
    }
}

pub(super) fn descriptor_path<Ctx: WorkerCtx>(
    ctx: &DurableWorkerCtx<Ctx>,
    rep: u32,
    relative: &str,
) -> Result<CanonicalGuestPath, FsError> {
    GUEST_PATHS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .get(&key(ctx, rep))
        .cloned()
        .ok_or_else(|| FsError::from(ErrorCode::NotPermitted))?
        .resolve(relative)
        .map_err(|_| ErrorCode::NotPermitted.into())
}

pub(super) async fn authorize_paths<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    paths: &[(FilesystemVerb, CanonicalGuestPath)],
) -> Result<(), FsError> {
    if !ctx.state.is_live() {
        return Ok(());
    }
    let component = ctx.component_metadata();
    let owner = AgentOwnerPattern::Agent {
        account: component.account_email.clone(),
        application: component.application_name.clone(),
        environment: component.environment_name.clone(),
        component: component.component_name.clone(),
        agent: AgentOwnerLeafPattern::Agent(ctx.agent_id().agent_id.clone()),
    };
    let targets = paths
        .iter()
        .map(|(verb, path)| filesystem_target(owner.clone(), *verb, path))
        .collect::<Vec<_>>();
    match ctx.authorize_live_permissions(&targets).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(_)) | Err(_) => Err(ErrorCode::NotPermitted.into()),
    }
}

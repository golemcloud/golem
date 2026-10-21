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

use super::{
    CardResourcePattern, CardVerb, DurabilityHost, DurableWorkerCtx,
    GolemPermissionsInspectDecision, PermissionCardHandleRep, Resource, WorkerCtx, card_view,
    ensure_card_permission, handle_card_id, parse_pattern_grant, permission_class_metadata,
    permissions_inspect, permissions_kernel, permissions_types, record_card_decision,
    resolve_permission_card_handle, worker_error_to_permission_error,
};

impl<Ctx: WorkerCtx> permissions_inspect::Host for DurableWorkerCtx<Ctx> {
    async fn inspect_card(
        &mut self,
        c: Resource<PermissionCardHandleRep>,
    ) -> anyhow::Result<Result<permissions_types::CardView, permissions_types::PermissionError>>
    {
        DurabilityHost::observe_function_call(self, "golem::permissions::inspect", "inspect-card");
        let card_ids = handle_card_id(self, &c).into_iter().collect();
        if let Err(error) = record_card_decision::<_, GolemPermissionsInspectDecision, (), _>(
            self,
            card_ids,
            async |ctx| {
                Ok(ctx
                    .with_agent_authority_at_boundary(|ctx| {
                        ensure_card_permission(ctx, CardVerb::Inspect, CardResourcePattern::Any)
                    })
                    .await
                    .map_err(worker_error_to_permission_error)
                    .and_then(|authorized| authorized))
            },
        )
        .await?
        {
            return Ok(Err(error));
        }
        Ok(resolve_permission_card_handle(self, &c)
            .await?
            .and_then(|card| card_view(&card)))
    }
}

impl<Ctx: WorkerCtx> permissions_kernel::Host for DurableWorkerCtx<Ctx> {
    async fn list_modules(&mut self) -> anyhow::Result<Vec<permissions_kernel::KernelModule>> {
        DurabilityHost::observe_function_call(
            self,
            "golem::permissions::kernel-introspection",
            "list-modules",
        );
        Ok(permission_class_metadata()
            .into_iter()
            .map(|metadata| permissions_kernel::KernelModule {
                class_name: metadata.class_name.to_string(),
                verbs: metadata
                    .verbs
                    .iter()
                    .map(|verb| (*verb).to_string())
                    .collect(),
            })
            .collect())
    }

    async fn validate_grant(
        &mut self,
        g: permissions_types::PatternGrant,
    ) -> anyhow::Result<Result<(), permissions_types::PermissionError>> {
        DurabilityHost::observe_function_call(
            self,
            "golem::permissions::kernel-introspection",
            "validate-grant",
        );
        Ok(parse_pattern_grant(&g).map(|_| ()))
    }
}

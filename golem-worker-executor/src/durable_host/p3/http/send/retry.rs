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

use super::*;

pub(in crate::durable_host::p3::http) async fn make_p3_http_retry_task_context<Ctx, U>(
    store: &Accessor<U, DurableP3<Ctx>>,
    retry_point: OplogIndex,
    mut retry_properties: RetryProperties,
) -> TaskRetryContext<Ctx>
where
    Ctx: WorkerCtx,
    U: Send + 'static,
{
    let (
        environment_state_service,
        environment_id,
        default_retry_policy,
        agent_config_retry_policies,
        runtime_retry_policy_mutations,
        max_in_function_retry_delay,
        entity_parent_start_index,
        worker,
    ) = store.with(|mut access| {
        let ctx = durable_worker_ctx::<Ctx, U>(access.data_mut());
        ctx.state.enrich_retry_properties(&mut retry_properties);
        (
            ctx.state.environment_state_service.clone(),
            ctx.state.owned_agent_id.environment_id,
            NamedRetryPolicy::default_from_config(&ctx.state.config.retry),
            ctx.state.agent_config_retry_policies(),
            ctx.state.runtime_retry_policy_mutations.clone(),
            ctx.state.config.max_in_function_retry_delay,
            ctx.entity_parent_start_index(),
            ctx.public_state.worker(),
        )
    });
    let current_retry_policy_state = worker
        .get_last_known_status()
        .await
        .current_retry_state
        .get(&retry_point)
        .cloned();

    TaskRetryContext {
        retry_point,
        entity_parent_start_index,
        environment_state_service,
        environment_id,
        default_retry_policy,
        agent_config_retry_policies,
        runtime_retry_policy_mutations,
        max_in_function_retry_delay,
        current_retry_policy_state,
        retry_properties,
        worker,
    }
}

pub(in crate::durable_host::p3::http) fn http_retry_properties<Ctx, U>(
    _store: &Accessor<U, DurableP3<Ctx>>,
    method: &str,
    uri: &str,
    status_code: Option<u16>,
    error_type: &str,
    _serialized_method: &SerializableHttpMethod,
) -> RetryProperties
where
    Ctx: WorkerCtx,
    U: Send + 'static,
{
    RetryContext::http_with_response(method, uri, status_code, error_type)
}

pub(in crate::durable_host::p3::http) fn inline_retry_eligible_for_method<Ctx, U>(
    store: &Accessor<U, DurableP3<Ctx>>,
    _method: &SerializableHttpMethod,
) -> bool
where
    Ctx: WorkerCtx,
    U: Send + 'static,
{
    store.with(|mut access| {
        let ctx = durable_worker_ctx::<Ctx, U>(access.data_mut());
        http_worker_state_allows_retry(&ctx.durable_execution_state()).is_ok()
    })
}

pub(in crate::durable_host::p3::http) async fn matching_status_retry_policy<Ctx, U>(
    store: &Accessor<U, DurableP3<Ctx>>,
    retry_ctx: &mut TaskRetryContext<Ctx>,
    method: &str,
    uri: &str,
    status_code: u16,
    serialized_method: &SerializableHttpMethod,
) -> Option<NamedRetryPolicy>
where
    Ctx: WorkerCtx,
    U: Send + 'static,
{
    let properties = http_retry_properties(
        store,
        method,
        uri,
        Some(status_code),
        "http-status",
        serialized_method,
    );
    let status_policies: Vec<NamedRetryPolicy> = retry_ctx
        .named_retry_policies()
        .await
        .into_iter()
        .filter(|policy| {
            policy.predicate.references_property("status-code")
                || policy.policy.references_property("status-code")
        })
        .collect();
    match NamedRetryPolicy::resolve_applicable_treating_missing_properties_as_no_match(
        &status_policies,
        &properties,
    ) {
        Ok(policy) => policy.cloned(),
        Err(error) => {
            tracing::warn!(?error, "Failed resolving p3 HTTP status retry policy");
            None
        }
    }
}

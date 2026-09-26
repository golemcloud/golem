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

use crate::durable_host::concurrent::ResolvedCall;
use crate::durable_host::{
    CallReplayOutcome, DurabilityHost, DurableCallSession, DurableFunctionType, DurableWorkerCtx,
    NotCancellable,
};
use crate::preview2::golem_api_1_x::context::{
    Attribute, AttributeChain, AttributeValue, Datetime, Host, HostInvocationContext, HostSpan,
    SpanId, TraceId,
};
use crate::workerctx::{InvocationContextManagement, WorkerCtx};
use anyhow::anyhow;
use golem_common::model::invocation_context::InvocationContextSpan;
use golem_common::model::oplog::host_functions::{
    GolemContextSpanDrop, GolemContextSpanFinish, GolemContextSpanSetAttributes,
    GolemContextStartSpan,
};
use golem_common::model::oplog::{
    AttributeMap, HostPayloadPair, HostRequestGolemContextSpanAttributes,
    HostRequestGolemContextSpanResource, HostRequestNoInput, HostResponseGolemApiUnit,
    SpanAttributes, SpanFinished, SpanKind, SpanOutcome, SpanStarted,
};
use golem_common::model::{OplogIndex, Timestamp};
use golem_service_base::headers::TraceContextHeaders;
use std::collections::HashMap;
use std::sync::Arc;
use wasmtime::component::Resource;

impl<Ctx: WorkerCtx> HostSpan for DurableWorkerCtx<Ctx> {
    async fn started_at(&mut self, self_: Resource<SpanEntry>) -> anyhow::Result<Datetime> {
        self.observe_function_call("golem::api::context::span", "started-at");

        let entry = self.table().get(&self_)?;
        let span_id = entry.span_id.clone();

        let span = self
            .state
            .invocation_context
            .get(&span_id)
            .map_err(|err| anyhow!(err))?;
        Ok(span
            .start()
            .ok_or_else(|| anyhow!("Span has no start timestamp"))?
            .into())
    }

    async fn set_attribute(
        &mut self,
        self_: Resource<SpanEntry>,
        name: String,
        value: AttributeValue,
    ) -> anyhow::Result<()> {
        let entry = self.table().get(&self_)?;
        let span_id = entry.span_id.clone();
        let creation_index = entry.creation_index;
        set_guest_span_attributes(self, creation_index, &span_id, vec![(name, value.into())])
            .await?;
        Ok(())
    }

    async fn set_attributes(
        &mut self,
        self_: Resource<SpanEntry>,
        attributes: Vec<Attribute>,
    ) -> anyhow::Result<()> {
        let entry = self.table().get(&self_)?;
        let span_id = entry.span_id.clone();
        let creation_index = entry.creation_index;
        let attributes = attributes
            .into_iter()
            .map(|attribute| (attribute.key, attribute.value.into()))
            .collect();
        set_guest_span_attributes(self, creation_index, &span_id, attributes).await?;
        Ok(())
    }

    async fn finish(&mut self, self_: Resource<SpanEntry>) -> anyhow::Result<()> {
        let entry = self.table().get(&self_)?;
        if entry.finished {
            return Ok(());
        }
        let span_id = entry.span_id.clone();
        let creation_index = entry.creation_index;
        finish_guest_span::<Ctx, GolemContextSpanFinish>(self, creation_index, &span_id).await?;
        self.table().get_mut(&self_)?.finished = true;
        Ok(())
    }

    async fn drop(&mut self, rep: Resource<SpanEntry>) -> anyhow::Result<()> {
        let entry = self.table().delete(rep)?;

        if !entry.finished {
            finish_guest_span::<Ctx, GolemContextSpanDrop>(
                self,
                entry.creation_index,
                &entry.span_id,
            )
            .await?;
        }

        Ok(())
    }
}

impl<Ctx: WorkerCtx> HostInvocationContext for DurableWorkerCtx<Ctx> {
    async fn trace_id(
        &mut self,
        self_: Resource<InvocationContextEntry>,
    ) -> anyhow::Result<TraceId> {
        self.observe_function_call("golem::api::context::invocation-context", "trace-id");

        let entry = self.table().get(&self_)?;
        Ok(entry.trace_id.to_string())
    }

    async fn span_id(&mut self, self_: Resource<InvocationContextEntry>) -> anyhow::Result<SpanId> {
        self.observe_function_call("golem::api::context::invocation-context", "span-id");

        let entry = self.table().get(&self_)?;
        Ok(entry.span.span_id().to_string())
    }

    async fn parent(
        &mut self,
        self_: Resource<InvocationContextEntry>,
    ) -> anyhow::Result<Option<Resource<InvocationContextEntry>>> {
        self.observe_function_call("golem::api::context::invocation-context", "parent");

        let entry = self.table().get(&self_)?;
        if let Some(parent) = entry.span.parent() {
            let parent_entry = InvocationContextEntry {
                trace_id: entry.trace_id.clone(),
                span: parent.clone(),
            };
            let result = self.table().push(parent_entry)?;
            Ok(Some(result))
        } else {
            Ok(None)
        }
    }

    async fn get_attribute(
        &mut self,
        self_: Resource<InvocationContextEntry>,
        key: String,
        inherited: bool,
    ) -> anyhow::Result<Option<AttributeValue>> {
        self.observe_function_call("golem::api::context::invocation-context", "get-attribute");

        let entry = self.table().get(&self_)?;
        let span_id = entry.span.span_id().clone();

        let attribute = self
            .state
            .invocation_context
            .get_attribute(&span_id, &key, inherited)
            .map_err(|err| anyhow!(err))?;
        Ok(attribute.map(|value| value.into()))
    }

    async fn get_attributes(
        &mut self,
        self_: Resource<InvocationContextEntry>,
        inherited: bool,
    ) -> anyhow::Result<Vec<Attribute>> {
        self.observe_function_call("golem::api::context::invocation-context", "get-attributes");

        let entry = self.table().get(&self_)?;
        let span_id = entry.span.span_id().clone();

        let attributes = self
            .state
            .invocation_context
            .get_attributes(&span_id, inherited)
            .map_err(|err| anyhow!(err))?;
        let result = attributes
            .into_iter()
            .filter_map(|(key, values)| {
                values.into_iter().next().map(|value| Attribute {
                    key,
                    value: value.into(),
                })
            })
            .collect();
        Ok(result)
    }

    async fn get_attribute_chain(
        &mut self,
        self_: Resource<InvocationContextEntry>,
        key: String,
    ) -> anyhow::Result<Vec<AttributeValue>> {
        self.observe_function_call(
            "golem::api::context::invocation-context",
            "get-attribute-chain",
        );

        let entry = self.table().get(&self_)?;
        let span_id = entry.span.span_id().clone();

        let chain = self
            .state
            .invocation_context
            .get_attribute_chain(&span_id, &key)
            .map_err(|err| anyhow!(err))?
            .unwrap_or_default();
        Ok(chain.into_iter().map(|value| value.into()).collect())
    }

    async fn get_attribute_chains(
        &mut self,
        self_: Resource<InvocationContextEntry>,
    ) -> anyhow::Result<Vec<AttributeChain>> {
        self.observe_function_call(
            "golem::api::context::invocation-context",
            "get-attribute-chains",
        );

        let entry = self.table().get(&self_)?;
        let span_id = entry.span.span_id().clone();

        let attributes = self
            .state
            .invocation_context
            .get_attributes(&span_id, true)
            .map_err(|err| anyhow!(err))?;
        let result = attributes
            .into_iter()
            .map(|(key, values)| AttributeChain {
                key,
                values: values.into_iter().map(|value| value.into()).collect(),
            })
            .collect();
        Ok(result)
    }

    async fn trace_context_headers(
        &mut self,
        self_: Resource<InvocationContextEntry>,
    ) -> anyhow::Result<Vec<(String, String)>> {
        self.observe_function_call(
            "golem::api::context::invocation-context",
            "trace-context-headers",
        );

        let entry = self.table().get(&self_)?;
        let span_id = entry.span.span_id().clone();

        let stack = self
            .state
            .invocation_context
            .get_stack(&span_id)
            .map_err(|err| anyhow!(err))?;
        let trace_context_headers = TraceContextHeaders::from_invocation_context(stack);
        Ok(trace_context_headers.to_raw_headers_map())
    }

    async fn drop(&mut self, rep: Resource<InvocationContextEntry>) -> anyhow::Result<()> {
        self.observe_function_call("golem::api::context::invocation-context", "drop");

        self.table().delete(rep)?;
        Ok(())
    }
}

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn start_span(&mut self, name: String) -> anyhow::Result<Resource<SpanEntry>> {
        let attributes = HashMap::from([(
            "name".to_string(),
            golem_common::model::invocation_context::AttributeValue::String(name),
        )]);
        let begun = DurableCallSession::<GolemContextStartSpan, NotCancellable>::begin(
            self,
            DurableFunctionType::ReadLocal,
        )
        .await?;
        let handle;
        let started;
        match begun
            .matching_request(HostRequestNoInput {})
            .resolve(self)
            .await?
        {
            ResolvedCall::Live(begun) => {
                started = guest_span_started(self, attributes);
                handle = begun
                    .start_live_with_span(self, HostRequestNoInput {}, started.clone())
                    .await?;
            }
            ResolvedCall::Replay(replay) => {
                started = replay.recorded_span_started(self).await?.ok_or_else(|| {
                    anyhow!("guest span creation Start has no span_started transition")
                })?;
                handle = replay;
            }
        }
        let creation_index = (!self.state.snapshotting_mode).then(|| handle.start_index());
        let span = install_guest_span(self, &started)?;
        if handle.is_live() {
            handle
                .complete(self, HostResponseGolemApiUnit { result: Ok(()) })
                .await?;
        } else if let CallReplayOutcome::Incomplete(live) = handle.replay(self).await? {
            live.complete(self, HostResponseGolemApiUnit { result: Ok(()) })
                .await?;
        }
        self.state.current_span_id = span.span_id().clone();
        let entry = SpanEntry {
            span_id: span.span_id().clone(),
            creation_index,
            finished: false,
        };
        let result = self.table().push(entry)?;
        Ok(result)
    }

    async fn current_context(&mut self) -> anyhow::Result<Resource<InvocationContextEntry>> {
        self.observe_function_call("golem::api::context", "current-context");

        let trace_id = self.state.invocation_context.trace_id.to_string();
        let span = self
            .state
            .invocation_context
            .get(&self.state.current_span_id)
            .map_err(|err| anyhow!(err))?;
        let entry = InvocationContextEntry { trace_id, span };
        let result = self.table().push(entry)?;
        Ok(result)
    }

    async fn allow_forwarding_trace_context_headers(
        &mut self,
        allow: bool,
    ) -> anyhow::Result<bool> {
        self.observe_function_call(
            "golem::api::context",
            "allow-forwarding-trace-context-headers",
        );

        let result = self.state.forward_trace_context_headers;
        self.state.forward_trace_context_headers = allow;
        Ok(result)
    }
}

pub struct SpanEntry {
    span_id: golem_common::model::invocation_context::SpanId,
    creation_index: Option<OplogIndex>,
    finished: bool,
}

fn guest_span_started<Ctx: WorkerCtx>(
    ctx: &DurableWorkerCtx<Ctx>,
    attributes: HashMap<String, golem_common::model::invocation_context::AttributeValue>,
) -> SpanStarted {
    SpanStarted {
        span_id: golem_common::model::invocation_context::SpanId::generate(),
        trace_id: ctx.state.invocation_context.trace_id.clone(),
        trace_states: ctx.state.invocation_context.trace_states.clone(),
        parent_span_id: Some(ctx.state.current_span_id.clone()),
        links: Vec::new(),
        started_at: Timestamp::now_utc(),
        attributes: AttributeMap(attributes),
        kind: SpanKind::Internal,
    }
}

fn install_guest_span<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    started: &SpanStarted,
) -> Result<Arc<InvocationContextSpan>, anyhow::Error> {
    let parent = started
        .parent_span_id
        .as_ref()
        .map(|id| ctx.state.invocation_context.get(id))
        .transpose()
        .map_err(|err| anyhow!(err))?;
    let mut builder = InvocationContextSpan::local()
        .with_span_id(started.span_id.clone())
        .with_start(started.started_at)
        .with_attributes(started.attributes.0.clone());
    if let Some(parent) = parent {
        builder = builder.with_parent(parent);
    }
    let span = builder.build();
    ctx.state.invocation_context.add_span_with_origin(
        span.clone(),
        started.trace_id.clone(),
        started.trace_states.clone(),
    );
    Ok(span)
}

async fn finish_guest_span<Ctx, Pair>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    creation_index: Option<OplogIndex>,
    span_id: &golem_common::model::invocation_context::SpanId,
) -> anyhow::Result<()>
where
    Ctx: WorkerCtx,
    Pair:
        HostPayloadPair<Req = HostRequestGolemContextSpanResource, Resp = HostResponseGolemApiUnit>,
{
    let request = HostRequestGolemContextSpanResource { creation_index };
    let begun =
        DurableCallSession::<Pair, NotCancellable>::begin(ctx, DurableFunctionType::ReadLocal)
            .await?;
    let mut handle = match begun.matching_request(request.clone()).resolve(ctx).await? {
        ResolvedCall::Live(begun) => begun.start_live(ctx, request).await?,
        ResolvedCall::Replay(handle) => handle,
    };
    if handle.is_live() {
        handle
            .complete_with_span(
                ctx,
                HostResponseGolemApiUnit { result: Ok(()) },
                SpanFinished {
                    span_id: span_id.clone(),
                    finished_at: Timestamp::now_utc(),
                    outcome: SpanOutcome::Completed,
                },
            )
            .await?;
    } else if let CallReplayOutcome::Incomplete(live) = handle.replay(ctx).await? {
        handle = live;
        handle
            .complete_with_span(
                ctx,
                HostResponseGolemApiUnit { result: Ok(()) },
                SpanFinished {
                    span_id: span_id.clone(),
                    finished_at: Timestamp::now_utc(),
                    outcome: SpanOutcome::Completed,
                },
            )
            .await?;
    }
    InvocationContextManagement::remove_span(ctx, span_id)?;
    Ok(())
}

async fn set_guest_span_attributes<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    creation_index: Option<OplogIndex>,
    span_id: &golem_common::model::invocation_context::SpanId,
    attributes: Vec<(
        String,
        golem_common::model::invocation_context::AttributeValue,
    )>,
) -> anyhow::Result<()> {
    if attributes.is_empty() {
        return Ok(());
    }
    let span = ctx
        .state
        .invocation_context
        .get(span_id)
        .map_err(|err| anyhow!(err))?;
    let request = HostRequestGolemContextSpanAttributes {
        creation_index,
        attributes: attributes.clone(),
    };
    let begun = DurableCallSession::<GolemContextSpanSetAttributes, NotCancellable>::begin(
        ctx,
        DurableFunctionType::ReadLocal,
    )
    .await?;
    let mut handle = match begun.matching_request(request.clone()).resolve(ctx).await? {
        ResolvedCall::Live(begun) => begun.start_live(ctx, request).await?,
        ResolvedCall::Replay(handle) => handle,
    };
    let mut applied = HashMap::new();
    for (key, value) in attributes {
        span.set_attribute(key.clone(), value.clone());
        applied.insert(key, value);
    }
    if handle.is_live() {
        handle
            .complete_with_span_attributes(
                ctx,
                HostResponseGolemApiUnit { result: Ok(()) },
                SpanAttributes {
                    span_id: span_id.clone(),
                    attributes: AttributeMap(applied),
                },
            )
            .await?;
    } else if let CallReplayOutcome::Incomplete(live) = handle.replay(ctx).await? {
        handle = live;
        handle
            .complete_with_span_attributes(
                ctx,
                HostResponseGolemApiUnit { result: Ok(()) },
                SpanAttributes {
                    span_id: span_id.clone(),
                    attributes: AttributeMap(applied),
                },
            )
            .await?;
    }
    Ok(())
}

pub struct InvocationContextEntry {
    trace_id: TraceId,
    span: Arc<InvocationContextSpan>,
}

impl From<golem_common::model::invocation_context::AttributeValue> for AttributeValue {
    fn from(value: golem_common::model::invocation_context::AttributeValue) -> Self {
        match value {
            golem_common::model::invocation_context::AttributeValue::String(value) => {
                AttributeValue::String(value)
            }
        }
    }
}

impl From<AttributeValue> for golem_common::model::invocation_context::AttributeValue {
    fn from(value: AttributeValue) -> Self {
        match value {
            AttributeValue::String(value) => {
                golem_common::model::invocation_context::AttributeValue::String(value)
            }
        }
    }
}

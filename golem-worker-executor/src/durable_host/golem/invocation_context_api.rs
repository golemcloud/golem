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

use crate::durable_host::{DurabilityHost, DurableWorkerCtx};
use crate::preview2::golem_api_1_x::context::{
    Attribute, AttributeChain, AttributeValue, Datetime, Host, HostInvocationContext, HostSpan,
    SpanId, TraceId,
};
use crate::workerctx::{InvocationContextManagement, WorkerCtx};
use anyhow::anyhow;
use golem_common::model::invocation_context::InvocationContextSpan;
use golem_service_base::headers::TraceContextHeaders;
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
        self.observe_function_call("golem::api::context::span", "set_attribute");

        let entry = self.table().get(&self_)?;
        let span_id = entry.span_id.clone();

        self.set_span_attribute(&span_id, &name, value.into())
            .await?;
        Ok(())
    }

    async fn set_attributes(
        &mut self,
        self_: Resource<SpanEntry>,
        attributes: Vec<Attribute>,
    ) -> anyhow::Result<()> {
        self.observe_function_call("golem::api::context::span", "set_attributes");

        let entry = self.table().get(&self_)?;
        let span_id = entry.span_id.clone();

        for attribute in attributes {
            self.set_span_attribute(&span_id, &attribute.key, attribute.value.into())
                .await?;
        }
        Ok(())
    }

    async fn finish(&mut self, self_: Resource<SpanEntry>) -> anyhow::Result<()> {
        self.observe_function_call("golem::api::context::span", "finish");

        let entry = self.table().get(&self_)?;
        let span_id = entry.span_id.clone();

        self.finish_span(&span_id)
            .await
            .map_err(|err| anyhow!(err))?;
        Ok(())
    }

    async fn drop(&mut self, rep: Resource<SpanEntry>) -> anyhow::Result<()> {
        self.observe_function_call("golem::api::context::span", "drop");

        let entry = self.table().delete(rep)?;

        self.finish_span(&entry.span_id)
            .await
            .map_err(|err| anyhow!(err))?;

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
        self.observe_function_call("golem::api::context", "start");

        let span = InvocationContextManagement::start_span(
            self,
            &[(
                "name".to_string(),
                golem_common::model::invocation_context::AttributeValue::String(name),
            )],
        )
        .await?;
        let entry = SpanEntry {
            span_id: span.span_id().clone(),
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

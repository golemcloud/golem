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

use golem_common::model::invocation_context::{
    AttributeValue, InvocationContextSpan, InvocationContextStack, TraceId,
};
use golem_service_base::headers::TraceContextHeaders;
use std::collections::HashMap;

pub fn extract_request_attributes(request: &poem::Request) -> HashMap<String, AttributeValue> {
    let mut result = HashMap::new();

    result.insert(
        "request.method".to_string(),
        AttributeValue::String(request.method().to_string()),
    );
    result.insert(
        "request.uri".to_string(),
        AttributeValue::String(request.uri().to_string()),
    );
    result.insert(
        "request.remote_addr".to_string(),
        AttributeValue::String(request.remote_addr().to_string()),
    );

    result
}

pub fn invocation_context_from_request(request: &poem::Request) -> InvocationContextStack {
    let trace_context_headers = TraceContextHeaders::parse(request.headers());
    let request_attributes = extract_request_attributes(request);

    match trace_context_headers {
        Some(ctx) => {
            // Trace context found in headers, starting a new span
            let mut ctx = InvocationContextStack::new(
                ctx.trace_id,
                InvocationContextSpan::external_parent(ctx.parent_id),
                ctx.trace_states,
            );
            ctx.push(
                InvocationContextSpan::local()
                    .with_attributes(request_attributes)
                    .with_parent(ctx.spans.first().clone())
                    .build(),
            );
            ctx
        }
        None => {
            // No trace context in headers, starting a new trace
            InvocationContextStack::new(
                TraceId::generate(),
                InvocationContextSpan::local()
                    .with_attributes(request_attributes)
                    .build(),
                Vec::new(),
            )
        }
    }
}

pub fn grpc_invocation_context_from_request(
    request: &poem::Request,
) -> golem_api_grpc::proto::golem::worker::InvocationContext {
    let invocation_context = invocation_context_from_request(request);
    let grpc_tracing_invocation_context = invocation_context.into();
    golem_api_grpc::proto::golem::worker::InvocationContext {
        parent: None,
        args: Vec::new(),
        env: HashMap::new(),
        tracing: Some(grpc_tracing_invocation_context),
    }
}

// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

use crate::model::cli_output::StructuredOutput;
use crate::model::text_format::{TextOutput, logln};
use golem_client::model::NativeToolInvocationResponse;
use golem_common::model::IdempotencyKey;
use golem_common::model::invocation_session_public::{
    PublicInvocationResult, PublicNativeToolTarget,
};
use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolInvokeView {
    pub response: NativeToolInvocationResponse,
}

impl StructuredOutput for ToolInvokeView {
    const KIND: &'static str = "tool.invoke";
}

impl TextOutput for ToolInvokeView {
    fn log(&self) {
        logln(serde_json::to_string_pretty(&self.response).expect("serializable tool response"));
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolInvocationSessionView {
    pub target: PublicNativeToolTarget,
    pub idempotency_key: IdempotencyKey,
    pub result: PublicInvocationResult,
}

impl StructuredOutput for ToolInvocationSessionView {
    const KIND: &'static str = "tool.invoke-session";
}

impl TextOutput for ToolInvocationSessionView {
    fn log(&self) {
        logln(serde_json::to_string_pretty(self).expect("serializable tool session result"));
    }
}

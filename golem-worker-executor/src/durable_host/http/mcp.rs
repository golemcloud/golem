// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.

use crate::durable_host::authorization::targets::http_target;
use crate::durable_host::concurrent::{
    SharedDurableContext, authorize_live_permissions_at_serialized_access,
};
use crate::workerctx::WorkerCtx;
use golem_mcp_import::transport::TransportError;
use golem_mcp_import::transport::sender::HttpPolicy;
use golem_service_base::error::worker_executor::WorkerExecutorError;

impl<Ctx: WorkerCtx> HttpPolicy for SharedDurableContext<Ctx> {
    type Error = anyhow::Error;

    async fn admit(&mut self, target: &http::Uri) -> anyhow::Result<()> {
        // The owning durable call must select its live arm before constructing
        // a sender. In particular, replay must not consult current authority.
        if !self.with_ctx(|ctx| ctx.state.is_live()) {
            return Err(WorkerExecutorError::runtime("MCP HTTP dispatch during replay").into());
        }
        let target = http_target(&target.to_string()).map_err(|_| TransportError::Denied)?;
        let _permit = authorize_live_permissions_at_serialized_access(
            self,
            |ctx| ctx.durable_ctx_mut(),
            std::slice::from_ref(&target.permission),
        )
        .await?
        .map_err(|_| TransportError::Denied)?;
        // The owning McpToolCall Start reconciles this charge on replay.
        self.with_ctx(|ctx| {
            ctx.state.check_and_increment_http_call_count()?;
            ctx.record_monthly_http_call()?;
            Ok(())
        })
    }
}

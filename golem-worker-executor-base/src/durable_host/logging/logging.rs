// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::services::worker_event::LogLevel;
use async_trait::async_trait;

use crate::durable_host::DurableWorkerCtx;
use crate::metrics::wasm::record_host_function_call;
use crate::model::PersistenceLevel;
use crate::preview2::wasi::logging::logging::{Host, Level};
use crate::workerctx::WorkerCtx;

#[async_trait]
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn log(&mut self, level: Level, context: String, message: String) -> anyhow::Result<()> {
        let _permit = self.begin_async_host_function().await?;
        record_host_function_call("logging::handler", "log");
        if self.state.is_live() || self.state.persistence_level == PersistenceLevel::PersistNothing
        {
            let event_service = &self.public_state.event_service;
            let log_level = match level {
                Level::Critical => LogLevel::Critical,
                Level::Error => LogLevel::Error,
                Level::Warn => LogLevel::Warn,
                Level::Info => LogLevel::Info,
                Level::Debug => LogLevel::Debug,
                Level::Trace => LogLevel::Trace,
            };
            event_service.emit_log(log_level, &context, &message);

            Host::log(&mut self.as_wasi_view(), level, context, message).await
        } else {
            Ok(())
        }
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> Host for &mut DurableWorkerCtx<Ctx> {
    async fn log(&mut self, level: Level, context: String, message: String) -> anyhow::Result<()> {
        (*self).log(level, context, message).await
    }
}

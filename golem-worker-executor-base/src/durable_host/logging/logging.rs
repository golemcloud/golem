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
use crate::preview2::wasi::logging::logging::{Host, Level};
use crate::workerctx::WorkerCtx;
use golem_common::model::{LogLevel, WorkerEvent};

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn log(&mut self, level: Level, context: String, message: String) -> anyhow::Result<()> {
        self.observe_function_call("logging::handler", "log");

        let log_level = match level {
            Level::Critical => LogLevel::Critical,
            Level::Error => LogLevel::Error,
            Level::Warn => LogLevel::Warn,
            Level::Info => LogLevel::Info,
            Level::Debug => LogLevel::Debug,
            Level::Trace => LogLevel::Trace,
        };
        let event = WorkerEvent::log(log_level, &context, &message);
        self.emit_log_event(event).await;
        Ok(())
    }
}

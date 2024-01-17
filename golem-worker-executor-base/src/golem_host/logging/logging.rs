use async_trait::async_trait;
use crate::services::worker_event::LogLevel;

use crate::golem_host::GolemCtx;
use crate::metrics::wasm::record_host_function_call;
use crate::preview2::wasi::logging::logging::{Host, Level};
use crate::workerctx::WorkerCtx;

#[async_trait]
impl<Ctx: WorkerCtx> Host for GolemCtx<Ctx> {
    async fn log(&mut self, level: Level, context: String, message: String) -> anyhow::Result<()> {
        record_host_function_call("logging::handler", "log");
        if self.is_live() {
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

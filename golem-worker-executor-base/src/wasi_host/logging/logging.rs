use async_trait::async_trait;
use tracing::debug;
use wasmtime_wasi::preview2::WasiView;

use crate::preview2::wasi::logging::logging::{Host, Level};

#[async_trait]
impl<T: WasiView> Host for T {
    async fn log(&mut self, level: Level, context: String, message: String) -> anyhow::Result<()> {
        debug!(
            "logging::logging::log called: {:?} [{}] {}",
            level, context, message
        );
        Ok(())
    }
}

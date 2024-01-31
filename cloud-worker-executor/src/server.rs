use cloud_worker_executor::run;
use golem_worker_executor_base::metrics as base_metrics;
use golem_worker_executor_base::services::golem_config::GolemConfig;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

use cloud_worker_executor::services::config::AdditionalGolemConfig;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let prometheus = base_metrics::register_all();
    let config = GolemConfig::new();
    let additional_config = AdditionalGolemConfig::new();

    if config.enable_tracing_console {
        // NOTE: also requires RUSTFLAGS="--cfg tokio_unstable" cargo build
        console_subscriber::init();
    } else if config.enable_json_log {
        tracing_subscriber::fmt()
            .json()
            .flatten_event(true)
            // .with_span_events(FmtSpan::FULL) // NOTE: enable to see span events
            .with_env_filter(EnvFilter::from_default_env())
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::from_default_env())
            .with_ansi(true)
            .init();
    }

    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap(),
    );
    runtime.block_on(run(
        config,
        Arc::new(additional_config),
        prometheus,
        runtime.handle().clone(),
    ))
}

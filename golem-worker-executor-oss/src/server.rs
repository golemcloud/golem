use std::sync::Arc;

use golem_worker_executor_base::metrics;
use golem_worker_executor_base::services::golem_config::GolemConfig;
use golem_worker_executor_oss::run;
use golem_worker_executor_oss::services::config::AdditionalGolemConfig;
use tracing_subscriber::EnvFilter;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let prometheus = metrics::register_all();
    let config = GolemConfig::new();
    let additional_golem_config = Arc::new(AdditionalGolemConfig::new());

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
        prometheus,
        runtime.handle().clone(),
        additional_golem_config,
    ))
}

use std::sync::Arc;

use golem_common::tracing::init_tracing_with_default_env_filter;
use golem_worker_executor_base::metrics as base_metrics;

use cloud_worker_executor::run;
use cloud_worker_executor::services::config::load_or_dump_config;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    match load_or_dump_config() {
        Some((config, additional_config)) => {
            init_tracing_with_default_env_filter(&config.tracing);

            let prometheus = base_metrics::register_all();

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
        None => Ok(()),
    }
}

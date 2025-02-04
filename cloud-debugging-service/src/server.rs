use std::sync::Arc;

use cloud_debugging_service::config::load_or_dump_config;
use cloud_debugging_service::run;
use golem_common::tracing::init_tracing_with_default_env_filter;
use golem_worker_executor_base::metrics as base_metrics;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    match load_or_dump_config() {
        Some((golem_debug_config, additional_config)) => {
            init_tracing_with_default_env_filter(&golem_debug_config.golem_config.tracing);

            let prometheus = base_metrics::register_all();

            let runtime = Arc::new(
                tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .unwrap(),
            );

            runtime.block_on(run(
                golem_debug_config,
                additional_config,
                prometheus,
                runtime.handle().clone(),
            ))
        }
        None => Ok(()),
    }
}

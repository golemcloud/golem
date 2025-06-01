use std::sync::Arc;

use cloud_debugging_service::config::make_debug_config_loader;
use cloud_debugging_service::run;
use golem_common::tracing::init_tracing_with_default_env_filter;
use golem_worker_executor::metrics as base_metrics;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    match make_debug_config_loader().load_or_dump_config() {
        Some(debug_config) => {
            init_tracing_with_default_env_filter(&debug_config.tracing);

            let prometheus = base_metrics::register_all();

            let runtime = Arc::new(
                tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .unwrap(),
            );

            runtime.block_on(run(debug_config, prometheus, runtime.handle().clone()))
        }
        None => Ok(()),
    }
}

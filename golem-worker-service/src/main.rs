use golem_common::tracing::init_tracing_with_default_env_filter;
use golem_worker_service::config::make_config_loader;
use golem_worker_service::WorkerService;
use golem_worker_service_base::app_config::WorkerServiceBaseConfig;
use golem_worker_service_base::metrics;
use opentelemetry::global;
use prometheus::Registry;
use std::path::Path;

fn main() -> Result<(), anyhow::Error> {
    if std::env::args().any(|arg| arg == "--dump-openapi-yaml") {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?
            .block_on(dump_openapi_yaml())
    } else if let Some(config) = make_config_loader().load_or_dump_config() {
        init_tracing_with_default_env_filter(&config.tracing);

        let prometheus = metrics::register_all();

        let exporter = opentelemetry_prometheus::exporter()
            .with_registry(prometheus.clone())
            .build()?;

        global::set_meter_provider(
            opentelemetry_sdk::metrics::MeterProviderBuilder::default()
                .with_reader(exporter)
                .build(),
        );

        Ok(tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?
            .block_on(run(config, prometheus))?)
    } else {
        Ok(())
    }
}

async fn run(config: WorkerServiceBaseConfig, prometheus: Registry) -> Result<(), anyhow::Error> {
    let server = WorkerService::new(config, prometheus, Path::new("./db/migration")).await?;
    server.run().await
}

async fn dump_openapi_yaml() -> Result<(), anyhow::Error> {
    let config = WorkerServiceBaseConfig::default();
    let service =
        WorkerService::new(config, Registry::default(), Path::new("./db/migration")).await?;
    let yaml = service.http_service().spec_yaml();
    println!("{yaml}");
    Ok(())
}

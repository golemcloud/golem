use cloud_server_oss::api;
use cloud_server_oss::config::CloudServiceConfig;
use cloud_server_oss::db;
use cloud_server_oss::service::Services;
use poem::listener::TcpListener;
use tokio::select;
use tracing::error;

fn main() -> Result<(), std::io::Error> {
    let config = CloudServiceConfig::new();

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async_main(&config))
}

async fn async_main(config: &CloudServiceConfig) -> Result<(), std::io::Error> {
    let grpc_port = config.grpc_port;
    let http_port = config.http_port;

    dbg!(
        "Starting cloud server on ports: http: {}, grpc: {}",
        http_port,
        grpc_port
    );

    db::sqlite_migrate(&config.db).await.map_err(|e| {
        error!("DB - init error: {}", e);
        std::io::Error::new(std::io::ErrorKind::Other, "Init error")
    })?;

    let services = Services::new(config).await.map_err(|e| {
        error!("Services - init error: {}", e);
        std::io::Error::new(std::io::ErrorKind::Other, e)
    })?;

    let http_services = services.clone();

    let http_server = tokio::spawn(async move {
        let app = api::combined_routes(&http_services);

        poem::Server::new(TcpListener::bind(format!("0.0.0.0:{}", http_port)))
            .run(app)
            .await
            .expect("HTTP server failed");
    });

    select! {
        _ = http_server => {},
    }

    Ok(())
}

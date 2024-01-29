use cloud_server_oss::config::CloudServiceConfig;
use cloud_server_oss::db;
use cloud_server_oss::service::Services;
use cloud_server_oss::{api, grpcapi};
use poem::listener::TcpListener;
use std::net::{Ipv4Addr, SocketAddrV4};
use tokio::select;
use tracing::{error, info};
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::EnvFilter;

fn main() -> Result<(), std::io::Error> {
    let config = CloudServiceConfig::new();

    if config.enable_tracing_console {
        // NOTE: also requires RUSTFLAGS="--cfg tokio_unstable" cargo build
        console_subscriber::init();
    } else if config.enable_json_log {
        tracing_subscriber::fmt()
            .json()
            .flatten_event(true)
            .with_span_events(FmtSpan::FULL) // NOTE: enable to see span events
            .with_env_filter(EnvFilter::from_default_env())
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::from_default_env())
            .with_ansi(true)
            .init();
    }

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async_main(&config))
}

async fn async_main(config: &CloudServiceConfig) -> Result<(), std::io::Error> {
    let grpc_port = config.grpc_port;
    let http_port = config.http_port;

    info!(
        "Starting cloud server on ports: http: {}, grpc: {}",
        http_port, grpc_port
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
    let grpc_services = services.clone();

    let http_server = tokio::spawn(async move {
        let app = api::combined_routes(&http_services);

        poem::Server::new(TcpListener::bind(format!("0.0.0.0:{}", http_port)))
            .run(app)
            .await
            .expect("HTTP server failed");
    });

    let grpc_server = tokio::spawn(async move {
        grpcapi::start_grpc_server(
            SocketAddrV4::new(Ipv4Addr::new(0, 0, 0, 0), grpc_port).into(),
            &grpc_services,
        )
        .await
        .expect("gRPC server failed");
    });

    select! {
        _ = http_server => {},
        _ = grpc_server => {},
    }

    Ok(())
}

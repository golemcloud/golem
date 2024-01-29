use cloud_server_oss::config::{CloudServiceConfig, DbConfig};
use cloud_server_oss::db;
use cloud_server_oss::service::Services;
use cloud_server_oss::{api, grpcapi};
use poem::listener::TcpListener;
use std::net::{Ipv4Addr, SocketAddrV4};
use tokio::select;
use tracing::{debug, error};

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

    debug!(
        "Starting cloud server on ports: http: {}, grpc: {}",
        http_port, grpc_port
    );

    match config.db.clone() {
        DbConfig::Postgres(c) => {
            db::postgres_migrate(&c, &config.workspace)
                .await
                .map_err(|e| {
                    error!("DB - init error: {}", e);
                    std::io::Error::new(std::io::ErrorKind::Other, "Init error")
                })?;
        }
        DbConfig::Sqlite(c) => {
            db::sqlite_migrate(&c).await.map_err(|e| {
                error!("DB - init error: {}", e);
                std::io::Error::new(std::io::ErrorKind::Other, "Init error")
            })?;
        }
    };


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

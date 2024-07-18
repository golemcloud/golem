use async_trait::async_trait;
use golem_test_framework::components::rdb::{DbInfo, PostgresInfo};
use std::collections::HashMap;
use tracing::info;

#[async_trait]
pub trait CloudDbInfo {
    async fn cloud_env(&self, app_name: &str) -> HashMap<String, String>;
}

#[async_trait]
impl CloudDbInfo for DbInfo {
    async fn cloud_env(&self, app_name: &str) -> HashMap<String, String> {
        match self {
            DbInfo::Postgres(pg) => pg_env(pg, app_name).await,
            DbInfo::Sqlite(db_path) => [
                ("GOLEM__DB__TYPE".to_string(), "Sqlite".to_string()),
                (
                    "GOLEM__DB__CONFIG__DATABASE".to_string(),
                    (db_path.join(app_name))
                        .to_str()
                        .expect("Invalid Sqlite database path")
                        .to_string(),
                ),
                (
                    "GOLEM__DB__CONFIG__MAX_CONNECTIONS".to_string(),
                    "10".to_string(),
                ),
            ]
            .into(),
        }
    }
}

async fn pg_env(pg_info: &PostgresInfo, app_name: &str) -> HashMap<String, String> {
    create_db(&pg_info.host, pg_info.port, app_name, &pg_info.username)
        .await
        .expect("DB creation");

    HashMap::from([
        ("DB_HOST".to_string(), pg_info.host.clone()),
        ("DB_PORT".to_string(), pg_info.port.to_string()),
        ("DB_NAME".to_string(), app_name.to_string()),
        ("DB_USERNAME".to_string(), pg_info.username.clone()),
        ("DB_PASSWORD".to_string(), pg_info.password.clone()),
        ("COMPONENT_REPOSITORY_TYPE".to_string(), "jdbc".to_string()),
        ("GOLEM__DB__TYPE".to_string(), "Postgres".to_string()),
        (
            "GOLEM__DB__CONFIG__MAX_CONNECTIONS".to_string(),
            "10".to_string(),
        ),
        ("GOLEM__DB__CONFIG__HOST".to_string(), pg_info.host.clone()),
        (
            "GOLEM__DB__CONFIG__PORT".to_string(),
            pg_info.port.to_string(),
        ),
        (
            "GOLEM__DB__CONFIG__DATABASE".to_string(),
            app_name.to_string(),
        ),
        (
            "GOLEM__DB__CONFIG__USERNAME".to_string(),
            pg_info.username.clone(),
        ),
        (
            "GOLEM__DB__CONFIG__PASSWORD".to_string(),
            pg_info.password.clone(),
        ),
    ])
}

fn connection_string(host: &str, port: u16) -> String {
    format!("postgres://postgres:postgres@{host}:{port}/postgres?connect_timeout=3")
}

async fn create_db(
    host: &str,
    port: u16,
    db_name: &str,
    user: &str,
) -> Result<(), ::tokio_postgres::Error> {
    let (client, connection) =
        ::tokio_postgres::connect(&connection_string(host, port), ::tokio_postgres::NoTls).await?;

    let connection_fiber = tokio::spawn(async move {
        if let Err(e) = connection.await {
            eprintln!("connection error: {}", e);
        }
    });

    let r = client
        .execute(&format!("create database {db_name} OWNER {user}"), &[])
        .await;

    info!("DB creation returned with {r:?}");
    connection_fiber.abort();
    Ok(())
}

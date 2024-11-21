// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use async_trait::async_trait;
use clap::Args;
use sqlx::mysql::MySqlConnectOptions;
use sqlx::postgres::PgConnectOptions;
use sqlx::ConnectOptions;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tracing::{error, info};

pub mod docker_mysql;
pub mod docker_postgres;
pub mod k8s_postgres;
pub mod provided_postgres;
pub mod sqlite;

#[async_trait]
pub trait Rdb {
    fn info(&self) -> DbInfo;
    async fn kill(&self);
}

#[derive(Debug)]
pub enum DbInfo {
    Sqlite(PathBuf),
    Postgres(PostgresInfo),
    Mysql(MysqlInfo),
}

impl DbInfo {
    pub fn env(&self, service_namespace: &str) -> HashMap<String, String> {
        match self {
            DbInfo::Postgres(pg) => pg.env(service_namespace),
            DbInfo::Mysql(m) => m.env(service_namespace),
            DbInfo::Sqlite(db_path) => [
                ("GOLEM__DB__TYPE".to_string(), "Sqlite".to_string()),
                (
                    "GOLEM__DB__CONFIG__DATABASE".to_string(),
                    db_path
                        .join(service_namespace)
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

pub trait RdbConnection {
    fn connection_string(&self) -> String;

    fn host_connection_string(&self) -> String;
}

pub trait RdbsConnections {
    fn host_connection_strings(&self) -> Vec<String>;
}

#[derive(Debug, Clone, Args)]
pub struct PostgresInfo {
    #[arg(long = "postgres-host", default_value = "localhost")]
    pub host: String,
    #[arg(long = "postgres-port", default_value = "5432")]
    pub port: u16,
    #[arg(long = "postgres-host-port", default_value = "5432")]
    pub host_port: u16,
    #[arg(long = "postgres-db-name", default_value = "postgres")]
    pub database_name: String,
    #[arg(long = "postgres-username", default_value = "postgres")]
    pub username: String,
    #[arg(long = "postgres-password", default_value = "postgres")]
    pub password: String,
}

impl PostgresInfo {
    pub fn env(&self, service_namespace: &str) -> HashMap<String, String> {
        HashMap::from([
            ("DB_HOST".to_string(), self.host.clone()),
            ("DB_PORT".to_string(), self.port.to_string()),
            ("DB_NAME".to_string(), self.database_name.clone()),
            ("DB_USERNAME".to_string(), self.username.clone()),
            ("DB_PASSWORD".to_string(), self.password.clone()),
            ("COMPONENT_REPOSITORY_TYPE".to_string(), "jdbc".to_string()),
            ("GOLEM__DB__TYPE".to_string(), "Postgres".to_string()),
            (
                "GOLEM__DB__CONFIG__MAX_CONNECTIONS".to_string(),
                "10".to_string(),
            ),
            ("GOLEM__DB__CONFIG__HOST".to_string(), self.host.clone()),
            ("GOLEM__DB__CONFIG__PORT".to_string(), self.port.to_string()),
            (
                "GOLEM__DB__CONFIG__SCHEMA".to_string(),
                service_namespace.to_string(),
            ),
            (
                "GOLEM__DB__CONFIG__DATABASE".to_string(),
                self.database_name.clone(),
            ),
            (
                "GOLEM__DB__CONFIG__USERNAME".to_string(),
                self.username.clone(),
            ),
            (
                "GOLEM__DB__CONFIG__PASSWORD".to_string(),
                self.password.clone(),
            ),
        ])
    }
}

impl RdbConnection for PostgresInfo {
    fn connection_string(&self) -> String {
        format!(
            "postgres://{}:{}@{}:{}/{}",
            self.username, self.password, self.host, self.port, self.database_name
        )
    }

    fn host_connection_string(&self) -> String {
        format!(
            "postgres://{}:{}@{}:{}/{}",
            self.username, self.password, self.host, self.host_port, self.database_name
        )
    }
}

async fn postgres_check_if_running(info: &PostgresInfo) -> Result<(), sqlx::Error> {
    use sqlx::Executor;
    let connection_options = PgConnectOptions::new()
        .username(info.username.as_str())
        .password(info.password.as_str())
        .database(info.database_name.as_str())
        .host(info.host.as_str())
        .port(info.host_port);

    let mut conn = connection_options.connect().await?;

    let r = conn.execute(sqlx::query("SELECT 1;")).await;
    if let Err(e) = r {
        eprintln!("Postgres connection error: {}", e);
    }

    Ok(())
}

async fn postgres_wait_for_startup(info: &PostgresInfo, timeout: Duration) {
    info!(
        "Waiting for Postgres start on host {}:{}, timeout: {}s",
        info.host,
        info.host_port,
        timeout.as_secs()
    );
    let start = Instant::now();
    loop {
        let running = postgres_check_if_running(info).await;

        match running {
            Ok(_) => break,
            Err(e) => {
                if start.elapsed() > timeout {
                    error!(
                        "Failed to verify that Postgres host {}:{} is running: {}",
                        info.host, info.host_port, e
                    );
                    std::panic!(
                        "Failed to verify that Postgres host {}:{} is running",
                        info.host,
                        info.host_port
                    );
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

#[derive(Debug, Clone, Args)]
pub struct MysqlInfo {
    #[arg(long = "mysql-host", default_value = "localhost")]
    pub host: String,
    #[arg(long = "mysql-port", default_value = "3306")]
    pub port: u16,
    #[arg(long = "mysql-host-port", default_value = "3306")]
    pub host_port: u16,
    #[arg(long = "mysql-db-name", default_value = "mysql")]
    pub database_name: String,
    #[arg(long = "mysql-username", default_value = "mysql")]
    pub username: String,
    #[arg(long = "mysql-password", default_value = "mysql")]
    pub password: String,
}

impl MysqlInfo {
    pub fn env(&self, service_namespace: &str) -> HashMap<String, String> {
        HashMap::from([
            ("DB_HOST".to_string(), self.host.clone()),
            ("DB_PORT".to_string(), self.port.to_string()),
            ("DB_NAME".to_string(), self.database_name.clone()),
            ("DB_USERNAME".to_string(), self.username.clone()),
            ("DB_PASSWORD".to_string(), self.password.clone()),
            ("COMPONENT_REPOSITORY_TYPE".to_string(), "jdbc".to_string()),
            ("GOLEM__DB__TYPE".to_string(), "Mysql".to_string()),
            (
                "GOLEM__DB__CONFIG__MAX_CONNECTIONS".to_string(),
                "10".to_string(),
            ),
            ("GOLEM__DB__CONFIG__HOST".to_string(), self.host.clone()),
            ("GOLEM__DB__CONFIG__PORT".to_string(), self.port.to_string()),
            (
                "GOLEM__DB__CONFIG__SCHEMA".to_string(),
                service_namespace.to_string(),
            ),
            (
                "GOLEM__DB__CONFIG__DATABASE".to_string(),
                self.database_name.clone(),
            ),
            (
                "GOLEM__DB__CONFIG__USERNAME".to_string(),
                self.username.clone(),
            ),
            (
                "GOLEM__DB__CONFIG__PASSWORD".to_string(),
                self.password.clone(),
            ),
        ])
    }
}

async fn mysql_check_if_running(info: &MysqlInfo) -> Result<(), sqlx::Error> {
    use sqlx::Executor;
    let connection_options = MySqlConnectOptions::new()
        .username(info.username.as_str())
        .password(info.password.as_str())
        .database(info.database_name.as_str())
        .host(info.host.as_str())
        .port(info.host_port);

    let mut conn = connection_options.connect().await?;

    let r = conn.execute(sqlx::query("SELECT 1;")).await;
    if let Err(e) = r {
        eprintln!("Mysql connection error: {}", e);
    }

    Ok(())
}

async fn mysql_wait_for_startup(info: &MysqlInfo, timeout: Duration) {
    info!(
        "Waiting for Mysql start on host {}:{}, timeout: {}s",
        info.host,
        info.host_port,
        timeout.as_secs()
    );
    let start = Instant::now();
    loop {
        let running = mysql_check_if_running(info).await;

        match running {
            Ok(_) => break,
            Err(e) => {
                if start.elapsed() > timeout {
                    error!(
                        "Failed to verify that Mysql host {}:{} is running: {}",
                        info.host, info.host_port, e
                    );
                    std::panic!(
                        "Failed to verify that Mysql host {}:{} is running",
                        info.host,
                        info.host_port
                    );
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

impl RdbConnection for MysqlInfo {
    fn connection_string(&self) -> String {
        format!(
            "mysql://{}:{}@{}:{}/{}",
            self.username, self.password, self.host, self.port, self.database_name
        )
    }

    fn host_connection_string(&self) -> String {
        format!(
            "mysql://{}:{}@{}:{}/{}",
            self.username, self.password, self.host, self.host_port, self.database_name
        )
    }
}

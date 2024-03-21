use crate::context::{
    DbType, EnvConfig, K8sNamespace, K8sRoutingType, ManagedPod, ManagedService, Runtime, NETWORK,
};
use anyhow::Result;
use k8s_openapi::api::core::v1::{Pod, Service};
use kube::api::PostParams;
use kube::{Api, Client};
use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use testcontainers::{clients, Container, RunnableImage};

pub struct Db<'docker_client> {
    inner: DbInner<'docker_client>,
}

pub enum DbInner<'docker_client> {
    Sqlite(PathBuf),
    Postgres {
        host: String,
        port: u16,
        _node: Container<'docker_client, testcontainers_modules::postgres::Postgres>,
    },
    K8S {
        host: String,
        port: u16,
        _pod: ManagedPod,
        _service: ManagedService,
    },
}

impl<'docker_client> Db<'docker_client> {
    async fn test_started_unsafe(local_host: &str, local_port: u16) -> Result<()> {
        let connection_string = Db::connection_string(local_host, local_port);
        println!("Connecting to {connection_string}");

        let (client, connection) =
            ::tokio_postgres::connect(&connection_string, ::tokio_postgres::NoTls).await?;

        tokio::spawn(async move {
            if let Err(e) = connection.await {
                eprintln!("connection error: {}", e);
            }
        });

        let _ = client.query("SELECT version()", &[]).await?;
        Ok(())
    }

    async fn test_started(local_host: &str, local_port: u16) -> Result<()> {
        let mut count = 0;

        loop {
            match Self::test_started_unsafe(local_host, local_port).await {
                Ok(res) => return Ok(res),
                Err(e) => {
                    if count < 20 {
                        // TODO: configurable
                        count += 1;
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    } else {
                        return Err(e);
                    }
                }
            }
        }
    }

    pub async fn start(
        docker: &'docker_client clients::Cli,
        env_config: &EnvConfig,
    ) -> Result<Db<'docker_client>> {
        match &env_config.db_type {
            DbType::Sqlite => Db::prepare_sqlite(),
            DbType::Postgres => Db::start_postgres(docker, env_config).await,
        }
    }

    fn prepare_sqlite() -> Result<Db<'docker_client>> {
        let path = PathBuf::from("../target/golem_test_db");

        if path.exists() {
            std::fs::remove_file(&path)?
        }

        Ok(Db {
            inner: DbInner::Sqlite(path),
        })
    }

    async fn start_postgres(
        docker: &'docker_client clients::Cli,
        env_config: &EnvConfig,
    ) -> Result<Db<'docker_client>> {
        match &env_config.runtime {
            Runtime::Local => Self::start_postgres_docker(docker, true).await,
            Runtime::Docker => Self::start_postgres_docker(docker, false).await,
            Runtime::K8S {
                namespace,
                routing: _,
            } => Self::start_postgres_k8s(namespace).await,
        }
    }

    async fn start_postgres_k8s(namespace: &K8sNamespace) -> Result<Db<'docker_client>> {
        println!("Creating Postgres pod");

        let pods: Api<Pod> = Api::namespaced(Client::try_default().await?, &namespace.0);
        let services: Api<Service> = Api::namespaced(Client::try_default().await?, &namespace.0);

        let pod: Pod = serde_json::from_value(json!({
            "apiVersion": "v1",
            "kind": "Pod",
            "metadata": {
                "name": "golem-postgres",
                "labels": {
                    "app": "golem-postgres",
                    "app-group": "golem"
                },
            },
            "spec": {
                "ports": [{
                    "port": 5432,
                    "protocol": "TCP"
                }],
                "containers": [{
                    "name": "postgres",
                    "image": "postgres:12",
                    "env": [
                        {"name": "POSTGRES_DB", "value": "postgres"},
                        {"name": "POSTGRES_USER", "value": "postgres"},
                        {"name": "POSTGRES_PASSWORD", "value": "postgres"}
                    ]
                }]
            }
        }))?;

        let pp = PostParams::default();

        let res_pod = pods.create(&pp, &pod).await?;

        let managed_pod = ManagedPod::new("golem-postgres", namespace);

        let service: Service = serde_json::from_value(json!({
            "apiVersion": "v1",
            "kind": "Service",
            "metadata": {
                "name": "golem-postgres",
                "labels": {
                    "app": "golem-postgres",
                    "app-group": "golem"
                },
            },
            "spec": {
                "ports": [{
                    "port": 5432,
                    "protocol": "TCP"
                }],
                "selector": { "app": "golem-postgres" },
                "type": "LoadBalancer"
            }
        }))?;

        let res_srv = services.create(&pp, &service).await?;

        let managed_service = ManagedService::new("golem-postgres", namespace);

        println!("Test Postgres started");

        Ok(Db {
            inner: DbInner::K8S {
                host: format!("golem-postgres.{}.svc.cluster.local", &namespace.0),
                port: 5432,
                _pod: managed_pod,
                _service: managed_service,
            },
        })
    }

    async fn start_postgres_docker(
        docker: &'docker_client clients::Cli,
        local_env: bool,
    ) -> Result<Db<'docker_client>> {
        println!("Starting Postgres in docker");

        let name = "golem_postgres";
        let image = RunnableImage::from(testcontainers_modules::postgres::Postgres::default())
            .with_tag("12");

        let image = if local_env {
            image
        } else {
            image.with_container_name(name).with_network(NETWORK)
        };

        let node = docker.run(image);

        let host = if local_env { "localhost" } else { name };

        let port = if local_env {
            node.get_host_port_ipv4(5432)
        } else {
            5432
        };

        let local_port = node.get_host_port_ipv4(5432);

        let res = Db {
            inner: DbInner::Postgres {
                host: host.to_string(),
                port,
                _node: node,
            },
        };

        println!("Test Postgres started");
        Db::test_started("127.0.0.1", local_port).await?;

        Ok(res)
    }

    fn connection_string(local_host: &str, local_port: u16) -> String {
        format!("postgres://postgres:postgres@{local_host}:{local_port}/postgres")
    }

    pub fn info(&self) -> DbInfo {
        match &self.inner {
            DbInner::Sqlite(path) => DbInfo::Sqlite(path.clone()),
            DbInner::Postgres {
                host,
                port,
                _node: _,
            } => DbInfo::Postgres(PostgresInfo {
                host: host.clone(),
                port: *port,
                database_name: "postgres".to_owned(),
                username: "postgres".to_owned(),
                password: "postgres".to_owned(),
            }),
            DbInner::K8S { host, port, .. } => DbInfo::Postgres(PostgresInfo {
                host: host.clone(),
                port: *port,
                database_name: "postgres".to_owned(),
                username: "postgres".to_owned(),
                password: "postgres".to_owned(),
            }),
        }
    }
}

#[derive(Debug, Clone)]
pub enum DbInfo {
    Sqlite(PathBuf),
    Postgres(PostgresInfo),
}

impl DbInfo {
    pub fn env(&self) -> HashMap<String, String> {
        match self {
            DbInfo::Postgres(pg) => pg.env(),
            DbInfo::Sqlite(db_path) => [
                ("GOLEM__DB__TYPE".to_string(), "Sqlite".to_string()),
                (
                    "GOLEM__DB__CONFIG__DATABASE".to_string(),
                    db_path
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

#[derive(Debug, Clone)]
pub struct PostgresInfo {
    pub host: String,
    pub port: u16,
    pub database_name: String,
    pub username: String,
    pub password: String,
}

impl PostgresInfo {
    pub fn connection_string(&self) -> String {
        format!(
            "postgres://{}:{}@{}:{}/{}",
            self.username, self.password, self.host, self.port, self.database_name
        )
    }

    pub fn env(&self) -> HashMap<String, String> {
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

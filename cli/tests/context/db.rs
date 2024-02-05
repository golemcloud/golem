use crate::context::{DbType, EnvConfig, NETWORK};
use libtest_mimic::Failed;
use std::collections::HashMap;
use std::path::PathBuf;
use testcontainers::{clients, Container, RunnableImage};

pub struct Db<'docker_client> {
    inner: DbInner<'docker_client>,
}

pub enum DbInner<'docker_client> {
    Sqlite(PathBuf),
    Postgres {
        host: String,
        port: u16,
        local_port: u16,
        _node: Container<'docker_client, testcontainers_modules::postgres::Postgres>,
    },
}

impl<'docker_client> Db<'docker_client> {
    fn test_started(local_port: u16) -> Result<(), Failed> {
        let mut conn =
            ::postgres::Client::connect(&Db::connection_string(local_port), ::postgres::NoTls)
                .unwrap();

        let _ = conn.query("SELECT version()", &[])?;
        Ok(())
    }

    pub fn start(
        docker: &'docker_client clients::Cli,
        env_config: &EnvConfig,
    ) -> Result<Db<'docker_client>, Failed> {
        match &env_config.db_type {
            DbType::Sqlite => Db::prepare_sqlite(),
            DbType::Postgres => Db::start_postgres(docker, env_config),
        }
    }

    fn prepare_sqlite() -> Result<Db<'docker_client>, Failed> {
        let path = PathBuf::from("../target/golem_test_db");

        if path.exists() {
            std::fs::remove_file(&path)?
        }

        Ok(Db {
            inner: DbInner::Sqlite(path),
        })
    }

    fn start_postgres(
        docker: &'docker_client clients::Cli,
        env_config: &EnvConfig,
    ) -> Result<Db<'docker_client>, Failed> {
        println!("Starting Postgres in docker");

        let name = "golem_postgres";
        let image = RunnableImage::from(testcontainers_modules::postgres::Postgres::default())
            .with_tag("12");

        let image = if env_config.local_golem {
            image
        } else {
            image.with_container_name(name).with_network(NETWORK)
        };

        let node = docker.run(image);

        let host = if env_config.local_golem {
            "localhost"
        } else {
            name
        };

        let port = if env_config.local_golem {
            node.get_host_port_ipv4(5432)
        } else {
            5432
        };

        let local_port = node.get_host_port_ipv4(5432);

        let res = Db {
            inner: DbInner::Postgres {
                host: host.to_string(),
                port,
                local_port,
                _node: node,
            },
        };

        Db::test_started(local_port)?;

        Ok(res)
    }

    fn connection_string(local_port: u16) -> String {
        format!(
            "postgres://postgres:postgres@127.0.0.1:{}/postgres",
            local_port,
        )
    }

    pub fn info(&self) -> DbInfo {
        match &self.inner {
            DbInner::Sqlite(path) => DbInfo::Sqlite(path.clone()),
            DbInner::Postgres {
                host,
                port,
                local_port,
                _node: _,
            } => DbInfo::Postgres(PostgresInfo {
                host: host.clone(),
                port: port.clone(),
                local_port: local_port.clone(),
                database_name: "postgres".to_owned(),
                username: "postgres".to_owned(),
                password: "postgres".to_owned(),
            }),
        }
    }
}

#[derive(Debug)]
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
            ]
            .into(),
        }
    }
}

#[derive(Debug)]
pub struct PostgresInfo {
    pub host: String,
    pub port: u16,
    pub local_port: u16,
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

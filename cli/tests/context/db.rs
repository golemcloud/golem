use crate::context::{EnvConfig, NETWORK};
use libtest_mimic::Failed;
use std::collections::HashMap;
use testcontainers::{clients, Container, RunnableImage};

pub struct Postgres<'docker_client> {
    host: String,
    port: u16,
    local_port: u16,
    _node: Container<'docker_client, testcontainers_modules::postgres::Postgres>,
}

impl<'docker_client> Postgres<'docker_client> {
    fn test_started(&self) -> Result<(), Failed> {
        let mut conn =
            ::postgres::Client::connect(&self.connection_string(), ::postgres::NoTls).unwrap();

        let _ = conn.query("SELECT version()", &[])?;
        Ok(())
    }

    pub fn start(
        docker: &'docker_client clients::Cli,
        env_config: &EnvConfig,
    ) -> Result<Postgres<'docker_client>, Failed> {
        let name = "golem_postgres";
        let image = RunnableImage::from(testcontainers_modules::postgres::Postgres::default())
            .with_tag("12")
            .with_container_name(name)
            .with_network(NETWORK);
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

        let res = Postgres {
            host: host.to_string(),
            port,
            local_port: node.get_host_port_ipv4(5432),
            _node: node,
        };

        res.test_started()?;

        Ok(res)
    }

    pub fn connection_string(&self) -> String {
        format!(
            "postgres://postgres:postgres@127.0.0.1:{}/postgres",
            self.local_port,
        )
    }

    pub fn info(&self) -> PostgresInfo {
        PostgresInfo {
            host: self.host.clone(),
            port: self.port,
            local_port: self.local_port,
            database_name: "postgres".to_owned(),
            username: "postgres".to_owned(),
            password: "postgres".to_owned(),
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

pub trait DbInfo {
    fn env(&self) -> HashMap<String, String>;
}

impl DbInfo for PostgresInfo {
    fn env(&self) -> HashMap<String, String> {
        self.env()
    }
}

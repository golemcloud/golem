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

use std::collections::HashMap;
use std::path::PathBuf;

pub mod postgres;
pub mod sqlite;

pub trait Rdb {
    fn info(&self) -> DbInfo;
    fn kill(&self);
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
                (
                    "GOLEM__DB__CONFIG__MAX_CONNECTIONS".to_string(),
                    "10".to_string(),
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
    pub host_port: u16,
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

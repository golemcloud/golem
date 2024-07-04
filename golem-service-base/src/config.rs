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

use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum ComponentStoreConfig {
    S3(ComponentStoreS3Config),
    Local(ComponentStoreLocalConfig),
}

impl Default for ComponentStoreConfig {
    fn default() -> Self {
        ComponentStoreConfig::Local(ComponentStoreLocalConfig {
            root_path: "/tmp".to_string(),
            object_prefix: "".to_string(),
        })
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct ComponentStoreS3Config {
    pub bucket_name: String,
    pub object_prefix: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ComponentStoreLocalConfig {
    pub root_path: String,
    pub object_prefix: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum DbConfig {
    Postgres(DbPostgresConfig),
    Sqlite(DbSqliteConfig),
}

impl Default for DbConfig {
    fn default() -> Self {
        DbConfig::Sqlite(DbSqliteConfig {
            database: "golem_service.db".to_string(),
            max_connections: 10,
        })
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct DbSqliteConfig {
    pub database: String,
    pub max_connections: u32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct DbPostgresConfig {
    pub host: String,
    pub database: String,
    pub username: String,
    pub password: String,
    pub port: u16,
    pub max_connections: u32,
    pub schema: Option<String>,
}

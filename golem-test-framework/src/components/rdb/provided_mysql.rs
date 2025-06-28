// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::components::rdb::{DbInfo, MysqlInfo, Rdb, RdbConnection};
use async_trait::async_trait;
use tracing::info;

pub struct ProvidedMysqlRdb {
    info: MysqlInfo,
}

impl ProvidedMysqlRdb {
    pub fn new(info: MysqlInfo) -> Self {
        info!("Using provided Mysql database");

        Self { info }
    }

    pub fn public_connection_string(&self) -> String {
        self.info.public_connection_string()
    }

    pub fn public_connection_string_to_db(&self, db_name: &str) -> String {
        let db_info = MysqlInfo {
            database_name: db_name.to_string(),
            ..self.info.clone()
        };

        db_info.public_connection_string()
    }

    pub fn private_connection_string(&self) -> String {
        panic!("Unsupported")
    }
}

#[async_trait]
impl Rdb for ProvidedMysqlRdb {
    fn info(&self) -> DbInfo {
        DbInfo::Mysql(self.info.clone())
    }

    async fn kill(&self) {}
}

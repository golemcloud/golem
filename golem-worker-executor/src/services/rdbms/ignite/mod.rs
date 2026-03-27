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

mod client_rdbms;
pub mod types;

use crate::services::golem_config::RdbmsConfig;
use crate::services::rdbms::{Rdbms, RdbmsType};
use desert_rust::BinaryCodec;
use std::fmt::Display;
use std::sync::Arc;

pub(crate) const IGNITE: &str = "ignite";

#[derive(Debug, Clone, Default, PartialEq, BinaryCodec)]
pub struct IgniteType;

impl IgniteType {
    pub fn new_rdbms(config: RdbmsConfig) -> Arc<dyn Rdbms<IgniteType> + Send + Sync> {
        Arc::new(client_rdbms::IgniteRdbms::new(config))
    }
}

impl RdbmsType for IgniteType {
    type DbColumn = types::DbColumn;
    type DbValue = types::DbValue;

    fn durability_connection_interface() -> &'static str {
        "rdbms::ignite2::db-connection"
    }

    fn durability_transaction_interface() -> &'static str {
        "rdbms::ignite2::db-transaction"
    }

    fn durability_result_stream_interface() -> &'static str {
        "rdbms::ignite2::db-result-stream"
    }
}

impl Display for IgniteType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{IGNITE}")
    }
}

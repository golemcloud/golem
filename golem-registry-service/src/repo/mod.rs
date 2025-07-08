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

use golem_service_base::db::postgres::PostgresLabelledTransaction;
use golem_service_base::db::sqlite::SqliteLabelledTransaction;
use golem_service_base::db::LabelledPoolTransaction;

// Repos
pub mod account;
pub mod application;
pub mod component;
pub mod deployment;
pub mod environment;
pub mod plan;

// Model for SQL records and fields
pub mod model;

trait ForUpdateSupport: LabelledPoolTransaction {
    fn requires_and_supports_for_update(&self) -> bool {
        true
    }
}

impl ForUpdateSupport for PostgresLabelledTransaction {}

// NOTE: Sqlite does not support FOR UPDATE, but only one transaction can write at a time
impl ForUpdateSupport for SqliteLabelledTransaction {
    fn requires_and_supports_for_update(&self) -> bool {
        false
    }
}

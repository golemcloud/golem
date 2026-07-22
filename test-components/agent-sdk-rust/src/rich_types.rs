// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

//! Echo agent exercising the rich semantic schema types end-to-end
//! (Path, Url, Datetime, Duration, Quantity).

use chrono::{DateTime, Utc};
use golem_rust::schema::TypeId;
use golem_rust::{Quantity, QuantityUnit, agent_definition, agent_implementation};
use std::path::PathBuf;
use std::time::Duration;
use url::Url;

/// Unit marker for the quantity echo method: a byte count with base unit `B`.
pub struct Bytes;

impl QuantityUnit for Bytes {
    fn type_id() -> TypeId {
        TypeId::new("golem.it.agent_sdk_rust.Bytes")
    }
    fn base_unit() -> &'static str {
        "B"
    }
}

pub type ByteQuantity = Quantity<Bytes>;

#[agent_definition]
pub trait RichTypesAgent {
    fn new(name: String) -> Self;

    fn echo_path(&self, value: PathBuf) -> PathBuf;
    fn echo_url(&self, value: Url) -> Url;
    fn echo_datetime(&self, value: DateTime<Utc>) -> DateTime<Utc>;
    fn echo_duration(&self, value: Duration) -> Duration;
    fn echo_quantity(&self, value: ByteQuantity) -> ByteQuantity;
}

pub struct RichTypesAgentImpl {
    _name: String,
}

#[agent_implementation]
impl RichTypesAgent for RichTypesAgentImpl {
    fn new(name: String) -> Self {
        Self { _name: name }
    }

    fn echo_path(&self, value: PathBuf) -> PathBuf {
        value
    }

    fn echo_url(&self, value: Url) -> Url {
        value
    }

    fn echo_datetime(&self, value: DateTime<Utc>) -> DateTime<Utc> {
        value
    }

    fn echo_duration(&self, value: Duration) -> Duration {
        value
    }

    fn echo_quantity(&self, value: ByteQuantity) -> ByteQuantity {
        value
    }
}

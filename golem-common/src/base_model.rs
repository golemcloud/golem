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

use crate::newtype_uuid;
use bincode::{Decode, Encode};
use std::collections::HashSet;
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use uuid::Uuid;

newtype_uuid!(
    ComponentId,
    golem_api_grpc::proto::golem::component::ComponentId
);

newtype_uuid!(ProjectId, golem_api_grpc::proto::golem::common::ProjectId);

newtype_uuid!(PluginId, golem_api_grpc::proto::golem::component::PluginId);

newtype_uuid!(
    PluginInstallationId,
    golem_api_grpc::proto::golem::common::PluginInstallationId
);

newtype_uuid!(PlanId, golem_api_grpc::proto::golem::account::PlanId);
newtype_uuid!(ProjectGrantId);
newtype_uuid!(ProjectPolicyId);
newtype_uuid!(TokenId, golem_api_grpc::proto::golem::token::TokenId);

#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord, Hash, Encode, Decode)]
#[cfg_attr(feature = "model", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[cfg_attr(feature = "model", serde(rename_all = "camelCase"))]
pub struct ShardId {
    pub(crate) value: i64,
}

impl ShardId {
    pub fn new(value: i64) -> Self {
        Self { value }
    }

    pub fn from_worker_id(worker_id: &WorkerId, number_of_shards: usize) -> Self {
        let hash = Self::hash_worker_id(worker_id);
        let value = hash.abs() % number_of_shards as i64;
        Self { value }
    }

    pub fn hash_worker_id(worker_id: &WorkerId) -> i64 {
        let (high_bits, low_bits) = (
            (worker_id.component_id.0.as_u128() >> 64) as i64,
            worker_id.component_id.0.as_u128() as i64,
        );
        let high = Self::hash_string(&high_bits.to_string());
        let worker_name = &worker_id.worker_name;
        let component_worker_name = format!("{low_bits}{worker_name}");
        let low = Self::hash_string(&component_worker_name);
        ((high as i64) << 32) | ((low as i64) & 0xFFFFFFFF)
    }

    fn hash_string(string: &str) -> i32 {
        let mut hash = 0;
        if hash == 0 && !string.is_empty() {
            for val in &mut string.bytes() {
                hash = 31_i32.wrapping_mul(hash).wrapping_add(val as i32);
            }
        }
        hash
    }

    pub fn is_left_neighbor(&self, other: &ShardId) -> bool {
        other.value == self.value + 1
    }
}

impl Display for ShardId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "<{}>", self.value)
    }
}

#[cfg(feature = "model")]
impl golem_wasm_rpc::IntoValue for ShardId {
    fn into_value(self) -> golem_wasm_rpc::Value {
        golem_wasm_rpc::Value::S64(self.value)
    }

    fn get_type() -> golem_wasm_ast::analysis::AnalysedType {
        golem_wasm_ast::analysis::analysed_type::s64()
    }
}

pub type ComponentVersion = u64;

#[derive(Clone, Debug, Eq, PartialEq, Hash, Encode, Decode)]
#[cfg_attr(feature = "model", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[cfg_attr(feature = "model", serde(rename_all = "camelCase"))]
pub struct WorkerId {
    pub component_id: ComponentId,
    pub worker_name: String,
}

impl WorkerId {
    pub fn to_redis_key(&self) -> String {
        format!("{}:{}", self.component_id.0, self.worker_name)
    }

    pub fn to_worker_urn(&self) -> String {
        format!("urn:worker:{}/{}", self.component_id, self.worker_name)
    }

    /// The dual of `TargetWorkerId::into_worker_id`
    pub fn into_target_worker_id(self) -> TargetWorkerId {
        TargetWorkerId {
            component_id: self.component_id,
            worker_name: Some(self.worker_name),
        }
    }

    pub fn validate_worker_name(name: &str) -> Result<(), &'static str> {
        let length = name.len();
        if !(1..=512).contains(&length) {
            Err("Worker name must be between 1 and 512 characters")
        } else if name.chars().any(|c| c.is_whitespace()) {
            Err("Worker name must not contain whitespaces")
        } else if name.contains('/') {
            Err("Worker name must not contain '/'")
        } else if name.starts_with('-') {
            Err("Worker name must not start with '-'")
        } else {
            Ok(())
        }
    }
}

impl FromStr for WorkerId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() == 2 {
            let component_id_uuid = Uuid::from_str(parts[0])
                .map_err(|_| format!("invalid component id: {s} - expected uuid"))?;
            let component_id = ComponentId(component_id_uuid);
            let worker_name = parts[1].to_string();
            Ok(Self {
                component_id,
                worker_name,
            })
        } else {
            Err(format!(
                "invalid worker id: {s} - expected format: <component_id>:<worker_name>"
            ))
        }
    }
}

impl Display for WorkerId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&format!("{}/{}", self.component_id, self.worker_name))
    }
}

impl AsRef<WorkerId> for &WorkerId {
    fn as_ref(&self) -> &WorkerId {
        self
    }
}

#[cfg(feature = "model")]
impl golem_wasm_rpc::IntoValue for WorkerId {
    fn into_value(self) -> golem_wasm_rpc::Value {
        golem_wasm_rpc::Value::Record(vec![
            self.component_id.into_value(),
            self.worker_name.into_value(),
        ])
    }

    fn get_type() -> golem_wasm_ast::analysis::AnalysedType {
        use golem_wasm_ast::analysis::analysed_type::{field, record};
        record(vec![
            field("component_id", ComponentId::get_type()),
            field("worker_name", String::get_type()),
        ])
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Encode, Decode)]
#[cfg_attr(feature = "model", derive(serde::Serialize, serde::Deserialize))]
pub struct TargetWorkerId {
    pub component_id: ComponentId,
    pub worker_name: Option<String>,
}

impl TargetWorkerId {
    /// Converts a `TargetWorkerId` to a `WorkerId` if the worker name is specified
    pub fn try_into_worker_id(self) -> Option<WorkerId> {
        self.worker_name.map(|worker_name| WorkerId {
            component_id: self.component_id,
            worker_name,
        })
    }

    /// Converts a `TargetWorkerId` to a `WorkerId`. If the worker name was not specified,
    /// it generates a new unique one, and if the `force_in_shard` set is not empty, it guarantees
    /// that the generated worker ID will belong to one of the provided shards.
    ///
    /// If the worker name was specified, `force_in_shard` is ignored.
    pub fn into_worker_id(
        self,
        force_in_shard: &HashSet<ShardId>,
        number_of_shards: usize,
    ) -> WorkerId {
        let TargetWorkerId {
            component_id,
            worker_name,
        } = self;
        match worker_name {
            Some(worker_name) => WorkerId {
                component_id,
                worker_name,
            },
            None => {
                if force_in_shard.is_empty() || number_of_shards == 0 {
                    let worker_name = Uuid::new_v4().to_string();
                    WorkerId {
                        component_id,
                        worker_name,
                    }
                } else {
                    let mut current = Uuid::new_v4().to_u128_le();
                    loop {
                        let uuid = Uuid::from_u128_le(current);
                        let worker_name = uuid.to_string();
                        let worker_id = WorkerId {
                            component_id: component_id.clone(),
                            worker_name,
                        };
                        let shard_id = ShardId::from_worker_id(&worker_id, number_of_shards);
                        if force_in_shard.contains(&shard_id) {
                            return worker_id;
                        }
                        current += 1;
                    }
                }
            }
        }
    }

    // NOTE: Deprecated, to be removed once the wasm-rpc constructor is changed to accept worker-id
    pub fn parse_worker_urn(urn: &str) -> Option<TargetWorkerId> {
        if !urn.starts_with("urn:worker:") {
            None
        } else {
            let remaining = &urn[11..];
            let parts: Vec<&str> = remaining.split('/').collect();
            match parts.len() {
                2 => {
                    let component_id = ComponentId::from_str(parts[0]).ok()?;
                    let worker_name = parts[1];
                    Some(TargetWorkerId {
                        component_id,
                        worker_name: Some(worker_name.to_string()),
                    })
                }
                1 => {
                    let component_id = ComponentId::from_str(parts[0]).ok()?;
                    Some(TargetWorkerId {
                        component_id,
                        worker_name: None,
                    })
                }
                _ => None,
            }
        }
    }
}

impl Display for TargetWorkerId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match &self.worker_name {
            Some(worker_name) => write!(f, "{}/{}", self.component_id, worker_name),
            None => write!(f, "{}/*", self.component_id),
        }
    }
}

impl From<WorkerId> for TargetWorkerId {
    fn from(value: WorkerId) -> Self {
        value.into_target_worker_id()
    }
}

impl From<&WorkerId> for TargetWorkerId {
    fn from(value: &WorkerId) -> Self {
        value.clone().into_target_worker_id()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Encode, Decode)]
#[cfg_attr(
    feature = "model",
    derive(serde::Serialize, serde::Deserialize, golem_wasm_rpc_derive::IntoValue)
)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[cfg_attr(feature = "model", serde(rename_all = "camelCase"))]
pub struct PromiseId {
    pub worker_id: WorkerId,
    pub oplog_idx: OplogIndex,
}

impl PromiseId {
    pub fn to_redis_key(&self) -> String {
        format!("{}:{}", self.worker_id.to_redis_key(), self.oplog_idx)
    }
}

impl Display for PromiseId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.worker_id, self.oplog_idx)
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Encode, Decode, Default)]
#[cfg_attr(feature = "poem", derive(poem_openapi::NewType))]
#[cfg_attr(
    feature = "model",
    derive(serde::Serialize, serde::Deserialize, golem_wasm_rpc_derive::IntoValue)
)]
pub struct OplogIndex(pub(crate) u64);

impl OplogIndex {
    pub const NONE: OplogIndex = OplogIndex(0);
    pub const INITIAL: OplogIndex = OplogIndex(1);

    pub const fn from_u64(value: u64) -> OplogIndex {
        OplogIndex(value)
    }

    /// Gets the previous oplog index
    pub fn previous(&self) -> OplogIndex {
        OplogIndex(self.0 - 1)
    }

    /// Gets the next oplog index
    pub fn next(&self) -> OplogIndex {
        OplogIndex(self.0 + 1)
    }

    /// Gets the last oplog index belonging to an inclusive range starting at this oplog index,
    /// having `count` elements.
    pub fn range_end(&self, count: u64) -> OplogIndex {
        OplogIndex(self.0 + count - 1)
    }

    /// Check whether the oplog index is not None.
    pub fn is_defined(&self) -> bool {
        self.0 > 0
    }
}

impl Display for OplogIndex {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<OplogIndex> for u64 {
    fn from(value: OplogIndex) -> Self {
        value.0
    }
}

#[cfg(test)]
mod tests {
    use super::{ComponentId, TargetWorkerId};
    use test_r::test;

    #[test]
    fn test_parse_worker_urn() {
        let component_id = ComponentId::new_v4();

        fn check(urn: String, expected: Option<TargetWorkerId>) {
            assert_eq!(TargetWorkerId::parse_worker_urn(&urn), expected, "{urn}");
        }

        check(
            format!("urn:worker:{component_id}"),
            Some(TargetWorkerId {
                component_id: component_id.clone(),
                worker_name: None,
            }),
        );
        check(
            format!("urn:worker:{component_id}/worker1"),
            Some(TargetWorkerId {
                component_id: component_id.clone(),
                worker_name: Some("worker1".to_string()),
            }),
        );
        check(format!("urn:worker:{component_id}/worker1/worker2"), None);
        check(format!("urn:component:{component_id}"), None);
    }
}

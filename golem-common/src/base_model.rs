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

use crate::model::agent::{AgentId, AgentTypeResolver};
use crate::model::component_metadata::ComponentMetadata;
use crate::model::component::ComponentId;
use bincode::{Decode, Encode};
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use uuid::Uuid;

#[derive(
    Clone,
    Copy,
    Debug,
    Eq,
    PartialEq,
    PartialOrd,
    Ord,
    Hash,
    Encode,
    Decode,
    serde::Serialize,
    serde::Deserialize,
    poem_openapi::Object,
)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
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

impl golem_wasm::IntoValue for ShardId {
    fn into_value(self) -> golem_wasm::Value {
        golem_wasm::Value::S64(self.value)
    }

    fn get_type() -> golem_wasm::analysis::AnalysedType {
        golem_wasm::analysis::analysed_type::s64()
    }
}

static WORKER_ID_MAX_LENGTH: usize = 512;

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

    pub fn from_agent_id(
        component_id: ComponentId,
        agent_id: &AgentId,
    ) -> Result<WorkerId, String> {
        let agent_id = agent_id.to_string();
        if agent_id.len() > WORKER_ID_MAX_LENGTH {
            return Err(format!(
                "Agent id is too long: {}, max length: {}, agent id: {}",
                agent_id.len(),
                WORKER_ID_MAX_LENGTH,
                agent_id,
            ));
        }
        Ok(Self {
            component_id,
            worker_name: agent_id,
        })
    }

    pub fn from_agent_id_literal<S: AsRef<str>>(
        component_id: ComponentId,
        agent_id: S,
        resolver: impl AgentTypeResolver,
    ) -> Result<WorkerId, String> {
        Self::from_agent_id(component_id, &AgentId::parse(agent_id, resolver)?)
    }

    pub fn from_component_metadata_and_worker_id<S: AsRef<str>>(
        component_id: ComponentId,
        component_metadata: &ComponentMetadata,
        id: S,
    ) -> Result<WorkerId, String> {
        if component_metadata.is_agent() {
            Self::from_agent_id_literal(component_id, id, component_metadata)
        } else {
            let id = id.as_ref();
            if id.len() > WORKER_ID_MAX_LENGTH {
                return Err(format!(
                    "Legacy worker id is too long: {}, max length: {}, worker id: {}",
                    id.len(),
                    WORKER_ID_MAX_LENGTH,
                    id,
                ));
            }
            if id.contains('/') {
                return Err(format!(
                    "Legacy worker id cannot contain '/', worker id: {}",
                    id,
                ));
            }

            Ok(WorkerId {
                component_id,
                worker_name: id.to_string(),
            })
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

impl golem_wasm::IntoValue for WorkerId {
    fn into_value(self) -> golem_wasm::Value {
        golem_wasm::Value::Record(vec![
            self.component_id.into_value(),
            self.worker_name.into_value(),
        ])
    }

    fn get_type() -> golem_wasm::analysis::AnalysedType {
        use golem_wasm::analysis::analysed_type::{field, record};
        record(vec![
            field("component_id", ComponentId::get_type()),
            field("worker_name", String::get_type()),
        ])
    }
}

#[derive(
    Clone,
    Debug,
    Eq,
    PartialEq,
    Hash,
    Encode,
    Decode,
    serde::Serialize,
    serde::Deserialize,
    golem_wasm_derive::IntoValue,
    poem_openapi::Object,
)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
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

#[derive(
    Debug,
    Copy,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Encode,
    Decode,
    Default,
    poem_openapi::NewType,
    serde::Serialize,
    serde::Deserialize,
    golem_wasm_derive::IntoValue,
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

    /// Subtract the given number of entries from the oplog index
    pub fn subtract(&self, n: u64) -> OplogIndex {
        OplogIndex(self.0 - n)
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

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Encode,
    Decode,
    Default,
    poem_openapi::NewType,
    serde::Serialize,
    serde::Deserialize,
    golem_wasm_derive::IntoValue,
)]
pub struct TransactionId(pub(crate) String);

impl TransactionId {
    pub fn new<Id: Display>(id: Id) -> Self {
        Self(id.to_string())
    }

    pub fn generate() -> Self {
        Self::new(uuid::Uuid::new_v4())
    }
}

impl Display for TransactionId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<TransactionId> for String {
    fn from(value: TransactionId) -> Self {
        value.0
    }
}

impl From<String> for TransactionId {
    fn from(value: String) -> Self {
        TransactionId(value)
    }
}

mod sql {
    use crate::model::TransactionId;
    use sqlx::encode::IsNull;
    use sqlx::error::BoxDynError;
    use sqlx::postgres::PgTypeInfo;
    use sqlx::{Database, Postgres, Type};
    use std::io::Write;

    impl sqlx::Decode<'_, Postgres> for TransactionId {
        fn decode(value: <Postgres as Database>::ValueRef<'_>) -> Result<Self, BoxDynError> {
            let bytes = value.as_bytes()?;
            Ok(TransactionId(
                u64::from_be_bytes(bytes.try_into()?).to_string(),
            ))
        }
    }

    impl sqlx::Encode<'_, Postgres> for TransactionId {
        fn encode_by_ref(
            &self,
            buf: &mut <Postgres as Database>::ArgumentBuffer<'_>,
        ) -> Result<IsNull, BoxDynError> {
            let u64 = self.0.parse::<u64>()?;
            let bytes = u64.to_be_bytes();
            buf.write_all(&bytes)?;
            Ok(IsNull::No)
        }
    }

    impl Type<Postgres> for TransactionId {
        fn type_info() -> PgTypeInfo {
            PgTypeInfo::with_name("xid8")
        }
    }
}

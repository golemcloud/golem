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

pub mod account;
pub mod agent;
pub mod application;
pub mod auth;
pub mod base64;
pub mod component;
#[allow(unused_assignments)]
// NOTE: from rust 1.92, a `value assigned to `cache` is never read` warning is emitted, most likely from the derived BinaryCodec. To be fixed in desert
pub mod component_metadata;
pub mod deployment;
pub mod diff;
pub mod domain_registration;
pub mod environment;
pub mod environment_plugin_grant;
pub mod environment_share;
pub mod error;
pub mod http_api_deployment;
pub mod invocation_context;
pub mod login;
pub mod oplog;
pub mod plan;
pub mod plugin_registration;
pub mod regions;
pub mod reports;
pub mod security_scheme;
pub mod worker;
pub mod worker_filter;

pub use worker_filter::*;

use crate::base_model::component::ComponentId;
use crate::declare_structs;
use golem_wasm::analysis::analysed_type::{field, record, u32, u64};
use golem_wasm::analysis::AnalysedType;
use golem_wasm::{FromValue, IntoValue, Value};
use golem_wasm_derive::{FromValue, IntoValue};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::cmp::Ordering;
use std::fmt::{Display, Formatter};
use std::ops::Add;
use std::str::FromStr;
use std::time::Duration;
use uuid::{uuid, Uuid};

declare_structs! {
    pub struct VersionInfo {
        pub version: String,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UntypedJsonBody(pub serde_json::Value);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(transparent)]
pub struct Timestamp(pub(crate) iso8601_timestamp::Timestamp);

impl Display for Timestamp {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for Timestamp {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match iso8601_timestamp::Timestamp::parse(s) {
            Some(ts) => Ok(Self(ts)),
            None => Err("Invalid timestamp".to_string()),
        }
    }
}

impl serde::Serialize for Timestamp {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for Timestamp {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        if deserializer.is_human_readable() {
            iso8601_timestamp::Timestamp::deserialize(deserializer).map(Self)
        } else {
            // For non-human-readable formats we assume it was an i64 representing milliseconds from epoch
            let timestamp = <i64 as Deserialize>::deserialize(deserializer)?;
            Ok(Timestamp(
                iso8601_timestamp::Timestamp::UNIX_EPOCH
                    .add(Duration::from_millis(timestamp as u64)),
            ))
        }
    }
}

impl From<u64> for Timestamp {
    fn from(value: u64) -> Self {
        Timestamp(iso8601_timestamp::Timestamp::UNIX_EPOCH.add(Duration::from_millis(value)))
    }
}

impl IntoValue for Timestamp {
    fn into_value(self) -> Value {
        let d = self
            .0
            .duration_since(iso8601_timestamp::Timestamp::UNIX_EPOCH);
        Value::Record(vec![
            Value::U64(d.whole_seconds() as u64),
            Value::U32(d.subsec_nanoseconds() as u32),
        ])
    }

    fn get_type() -> AnalysedType {
        record(vec![field("seconds", u64()), field("nanoseconds", u32())])
            .named("datetime")
            .owned("wasi:clocks@0.2.3/wall-clock")
    }
}

impl FromValue for Timestamp {
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::Record(fields) if fields.len() == 2 => {
                let mut iter = fields.into_iter();
                let seconds = u64::from_value(iter.next().unwrap())?;
                let nanos = u32::from_value(iter.next().unwrap())?;
                Ok(Self(
                    iso8601_timestamp::Timestamp::UNIX_EPOCH
                        .add(Duration::from_secs(seconds))
                        .add(Duration::from_nanos(nanos as u64)),
                ))
            }
            other => Err(format!(
                "Expected a record with two fields for Timestamp, got {other:?}"
            )),
        }
    }
}

#[cfg(feature = "full")]
impl From<Timestamp> for golem_wasm::wasi::clocks::wall_clock::Datetime {
    fn from(value: Timestamp) -> Self {
        let ms = value.to_millis();
        Self {
            seconds: ms / 1000,
            nanoseconds: ((ms % 1000) * 1_000_000) as u32,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct Empty {}

#[derive(
    Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
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
    fn into_value(self) -> Value {
        Value::S64(self.value)
    }

    fn get_type() -> AnalysedType {
        golem_wasm::analysis::analysed_type::s64()
    }
}

#[derive(
    Clone,
    Debug,
    Eq,
    PartialEq,
    Ord,
    PartialOrd,
    Hash,
    serde::Serialize,
    serde::Deserialize,
)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct WorkerId {
    pub component_id: ComponentId,
    pub worker_name: String,
}

impl IntoValue for WorkerId {
    fn into_value(self) -> Value {
        Value::Record(vec![self.component_id.into_value(), Value::String(self.worker_name)])
    }

    fn get_type() -> AnalysedType {
        use golem_wasm::analysis::analysed_type::*;
        record(vec![
            field("component-id", ComponentId::get_type()),
            field("agent-id", str()),
        ])
        .named("agent-id")
        .owned("golem:core@1.5.0/types")
    }
}

impl FromValue for WorkerId {
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::Record(fields) if fields.len() == 2 => {
                let mut iter = fields.into_iter();
                let component_id = ComponentId::from_value(iter.next().unwrap())?;
                let worker_name = String::from_value(iter.next().unwrap())?;
                Ok(WorkerId {
                    component_id,
                    worker_name,
                })
            }
            other => Err(format!(
                "Expected Record with 2 fields for WorkerId, got {other:?}"
            )),
        }
    }
}

impl WorkerId {
    pub fn to_redis_key(&self) -> String {
        format!("{}:{}", self.component_id.0, self.worker_name)
    }

    pub fn to_worker_urn(&self) -> String {
        format!("urn:worker:{}/{}", self.component_id, self.worker_name)
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

#[derive(Clone, Debug, Eq, PartialEq, Hash, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object, IntoValue, FromValue,)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
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
    Default,
    serde::Serialize,
    serde::Deserialize,
    golem_wasm_derive::IntoValue,
    golem_wasm_derive::FromValue,
)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::NewType)
)]
#[cfg_attr(feature = "full", desert(transparent))]
pub struct OplogIndex(pub(crate) u64);

impl OplogIndex {
    pub const NONE: OplogIndex = OplogIndex(0);
    pub const INITIAL: OplogIndex = OplogIndex(1);

    pub const fn from_u64(value: u64) -> OplogIndex {
        OplogIndex(value)
    }

    pub const fn as_u64(&self) -> u64 {
        self.0
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

    /// Get the signed distance between this oplog index and the other oplog index
    pub fn distance_from(&self, other: OplogIndex) -> i64 {
        (self.0 as i64) - (other.0 as i64)
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
    Default,
    serde::Serialize,
    serde::Deserialize,
    IntoValue,
    FromValue,
)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::NewType)
)]
#[cfg_attr(feature = "full", desert(transparent))]
pub struct TransactionId(pub(crate) String);

impl TransactionId {
    pub fn new<Id: Display>(id: Id) -> Self {
        Self(id.to_string())
    }

    pub fn generate() -> Self {
        Self::new(Uuid::new_v4())
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

pub fn validate_lower_kebab_case_identifier(
    field_name: &str,
    identifier: &str,
) -> Result<(), String> {
    if identifier.is_empty() {
        return Err(format!("{} cannot be empty", field_name));
    }

    let first = identifier.chars().next().unwrap();
    if !first.is_ascii_lowercase() {
        return Err(format!(
            "{} must start with a lowercase ASCII letter (a-z), but got '{}'",
            field_name, first
        ));
    }

    if !identifier
        .chars()
        .all(|c| matches!(c, 'a'..='z' | '0'..='9' | '-'))
    {
        return Err(format!(
            "{} may contain only lowercase ASCII letters (a-z), digits (0-9), and hyphens (-)",
            field_name
        ));
    }

    if identifier.starts_with('-') || identifier.ends_with('-') {
        return Err(format!(
            "{} must not start or end with a hyphen",
            field_name
        ));
    }

    if identifier.contains("--") {
        return Err(format!(
            "{} must not contain consecutive hyphens",
            field_name
        ));
    }

    Ok(())
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, IntoValue, FromValue)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(transparent))]
#[wit_transparent]
pub struct IdempotencyKey {
    pub value: String,
}

impl IdempotencyKey {
    const ROOT_NS: Uuid = uuid!("9C19B15A-C83D-46F7-9BC3-EAD7923733F4");

    pub fn new(value: String) -> Self {
        Self { value }
    }

    pub fn from_uuid(value: Uuid) -> Self {
        Self {
            value: value.to_string(),
        }
    }

    pub fn fresh() -> Self {
        Self::from_uuid(Uuid::new_v4())
    }

    /// Generates a deterministic new idempotency key using a base idempotency key and an oplog index.
    ///
    /// The base idempotency key determines the "namespace" of the generated key UUIDv5. If
    /// the base idempotency key is already a UUID, it is directly used as the namespace of the v5 algorithm,
    /// while the name part is derived from the given oplog index.
    ///
    /// If the base idempotency key is not a UUID (as it can be an arbitrary user-provided string), then first
    /// we generate a UUIDv5 in the ROOT_NS namespace and use that a unique namespace for generating
    /// the new idempotency key.
    pub fn derived(base: &IdempotencyKey, oplog_index: OplogIndex) -> Self {
        let namespace = if let Ok(base_uuid) = Uuid::parse_str(&base.value) {
            base_uuid
        } else {
            Uuid::new_v5(&Self::ROOT_NS, base.value.as_bytes())
        };
        let name = format!("oplog-index-{oplog_index}");
        Self::from_uuid(Uuid::new_v5(&namespace, name.as_bytes()))
    }
}

impl Serialize for IdempotencyKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        Serialize::serialize(&self.value, serializer)
    }
}

impl<'de> Deserialize<'de> for IdempotencyKey {
    fn deserialize<D>(deserializer: D) -> Result<IdempotencyKey, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = <String as Deserialize>::deserialize(deserializer)?;
        Ok(IdempotencyKey { value })
    }
}

impl Display for IdempotencyKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.value)
    }
}

impl From<&str> for IdempotencyKey {
    fn from(s: &str) -> Self {
        IdempotencyKey {
            value: s.to_string(),
        }
    }
}

impl From<String> for IdempotencyKey {
    fn from(s: String) -> Self {
        IdempotencyKey { value: s }
    }
}

impl FromStr for IdempotencyKey {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(IdempotencyKey {
            value: s.to_string(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
pub struct WorkerResourceDescription {
    pub created_at: Timestamp,
    pub resource_owner: String,
    pub resource_name: String,
}

/// Represents last known status of a worker
///
/// This is always recorded together with the current oplog index, and it can only be used
/// as a source of truth if there are no newer oplog entries since the record.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, IntoValue, FromValue)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec, poem_openapi::Enum))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub enum WorkerStatus {
    /// The worker is running an invoked function
    Running,
    /// The worker is ready to run an invoked function
    Idle,
    /// An invocation is active but waiting for something (sleeping, waiting for a promise)
    Suspended,
    /// The last invocation was interrupted but will be resumed
    Interrupted,
    /// The last invocation failed and a retry was scheduled
    Retrying,
    /// The last invocation failed and the worker can no longer be used
    Failed,
    /// The worker exited after a successful invocation and can no longer be invoked
    Exited,
}

impl PartialOrd for WorkerStatus {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for WorkerStatus {
    fn cmp(&self, other: &Self) -> Ordering {
        let v1: i32 = self.clone().into();
        let v2: i32 = other.clone().into();
        v1.cmp(&v2)
    }
}

impl FromStr for WorkerStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "running" => Ok(WorkerStatus::Running),
            "idle" => Ok(WorkerStatus::Idle),
            "suspended" => Ok(WorkerStatus::Suspended),
            "interrupted" => Ok(WorkerStatus::Interrupted),
            "retrying" => Ok(WorkerStatus::Retrying),
            "failed" => Ok(WorkerStatus::Failed),
            "exited" => Ok(WorkerStatus::Exited),
            _ => Err(format!("Unknown worker status: {s}")),
        }
    }
}

impl Display for WorkerStatus {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkerStatus::Running => write!(f, "Running"),
            WorkerStatus::Idle => write!(f, "Idle"),
            WorkerStatus::Suspended => write!(f, "Suspended"),
            WorkerStatus::Interrupted => write!(f, "Interrupted"),
            WorkerStatus::Retrying => write!(f, "Retrying"),
            WorkerStatus::Failed => write!(f, "Failed"),
            WorkerStatus::Exited => write!(f, "Exited"),
        }
    }
}

impl TryFrom<i32> for WorkerStatus {
    type Error = String;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(WorkerStatus::Running),
            1 => Ok(WorkerStatus::Idle),
            2 => Ok(WorkerStatus::Suspended),
            3 => Ok(WorkerStatus::Interrupted),
            4 => Ok(WorkerStatus::Retrying),
            5 => Ok(WorkerStatus::Failed),
            6 => Ok(WorkerStatus::Exited),
            _ => Err(format!("Unknown worker status: {value}")),
        }
    }
}

impl From<WorkerStatus> for i32 {
    fn from(value: WorkerStatus) -> Self {
        match value {
            WorkerStatus::Running => 0,
            WorkerStatus::Idle => 1,
            WorkerStatus::Suspended => 2,
            WorkerStatus::Interrupted => 3,
            WorkerStatus::Retrying => 4,
            WorkerStatus::Failed => 5,
            WorkerStatus::Exited => 6,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
pub struct ScanCursor {
    pub cursor: u64,
    pub layer: usize,
}

impl ScanCursor {
    pub fn is_active_layer_finished(&self) -> bool {
        self.cursor == 0
    }

    pub fn is_finished(&self) -> bool {
        self.cursor == 0 && self.layer == 0
    }

    pub fn into_option(self) -> Option<Self> {
        if self.is_finished() {
            None
        } else {
            Some(self)
        }
    }
}

impl Display for ScanCursor {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.layer, self.cursor)
    }
}

impl FromStr for ScanCursor {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts = s.split('/').collect::<Vec<&str>>();
        if parts.len() == 2 {
            Ok(ScanCursor {
                layer: parts[0]
                    .parse()
                    .map_err(|e| format!("Invalid layer part: {e}"))?,
                cursor: parts[1]
                    .parse()
                    .map_err(|e| format!("Invalid cursor part: {e}"))?,
            })
        } else {
            Err("Invalid cursor, must have 'layer/cursor' format".to_string())
        }
    }
}

#[cfg(feature = "full")]
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

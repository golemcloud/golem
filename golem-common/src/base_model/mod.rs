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

pub mod account;
pub mod account_path;
pub mod account_usage;
pub mod address_class;
pub mod agent;
pub mod agent_config;
pub(crate) mod agent_id;
pub mod agent_secret;
pub mod api;
pub mod application;
pub mod auth;
pub mod base64;
pub mod caller;
pub mod card;
pub mod component;
#[allow(unused_assignments)]
// NOTE: from rust 1.92, a `value assigned to `cache` is never read` warning is emitted, most likely from the derived BinaryCodec. To be fixed in desert
pub mod component_metadata;
pub mod deploy_validation_warning;
pub mod deployment;
pub mod diff;
pub mod domain_registration;
pub mod durable_stream;
pub mod environment;
pub mod environment_plugin_grant;
pub mod environment_tool_grant;
pub mod environment_tool_middleware_grant;
pub mod error;
pub mod external_agent_secret;
pub mod http_api_deployment;
pub mod invocation_context;
pub mod invocation_id;
pub mod json;
pub mod login;
pub mod mcp_deployment;
pub mod mcp_import;
pub mod oplog;
pub mod optional_field_update;
pub mod path;
pub mod permission_share;
pub mod plan;
pub mod plan_dimension;
pub mod plugin_registration;
pub mod quota;
pub mod regions;
pub mod reports;
pub mod retry_policy;
pub mod security_scheme;
pub mod shard_lease;
pub mod tool;
pub mod tool_middleware;
pub mod tool_middleware_release;
pub mod tool_release;
pub mod type_id;
pub mod worker;
pub mod worker_filter;

pub use durable_stream::*;
pub use invocation_id::{IdempotencyScope, InvocationRequestKey, OwnedInvocationId};
pub use worker_filter::*;

use crate::{declare_structs, newtype_uuid};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::cmp::Ordering;
use std::fmt::{Display, Formatter};
use std::ops::Add;
use std::str::FromStr;
use std::time::Duration;
use uuid::{Uuid, uuid};

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

// A stable, per-instance fingerprint for a worker, generated as a random UUID
// at creation time. Globally unique across recreations of the same agent ID.
newtype_uuid!(
    AgentFingerprint,
    wit_name: "agent-fingerprint",
    wit_owner: "golem:core@2.0.0/types"
);

// The unique part of an accepted invocation identity, without its owning agent.
newtype_uuid!(InvocationId);

#[cfg(feature = "full")]
impl From<Timestamp> for wasmtime_wasi::p3::bindings::clocks::system_clock::Instant {
    fn from(value: Timestamp) -> Self {
        let ms = value.to_millis();
        Self {
            seconds: (ms / 1000) as i64,
            nanoseconds: ((ms % 1000) * 1_000_000) as u32,
        }
    }
}

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

    pub fn value(&self) -> i64 {
        self.value
    }

    pub fn from_agent_id(agent_id: &AgentId, number_of_shards: usize) -> Self {
        let hash = Self::hash_agent_id(agent_id);
        Self::from_routing_hash(hash, number_of_shards)
    }

    pub fn from_routing_hash(hash: i64, number_of_shards: usize) -> Self {
        let value = hash.unsigned_abs() % number_of_shards as u64;
        Self {
            value: value as i64,
        }
    }

    pub fn hash_agent_id(agent_id: &AgentId) -> i64 {
        agent_id.routing_hash()
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

pub use agent_id::{AgentId, RESERVED_TYPE_TAG};

// The schema of `PromiseId` is implemented in `schema::common_impls`: it follows the WIT
// `promise-id` record, whose `fingerprint` is a plain `uuid`.
#[derive(Clone, Debug, Eq, PartialEq, Hash, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct PromiseId {
    pub agent_id: AgentId,
    /// The incarnation of the agent that created the promise. A re-created agent keeps its
    /// agent ID but gets a new fingerprint, so this tells apart the promises of the two.
    pub fingerprint: AgentFingerprint,
    pub oplog_idx: OplogIndex,
}

impl PromiseId {
    /// The storage key of the promise of an agent in the environment:
    /// `{env-uuid}/{agent-uuid}:{fingerprint}:{oplog-index}`.
    pub fn storage_key(&self, environment_id: environment::EnvironmentId) -> String {
        format!(
            "{environment_id}/{}:{}:{}",
            self.agent_id, self.fingerprint, self.oplog_idx
        )
    }
}

impl Display for PromiseId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}/{}/{}",
            self.agent_id, self.fingerprint, self.oplog_idx
        )
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
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::NewType)
)]
#[cfg_attr(feature = "full", desert(transparent))]
// Newtype over `u64`: represent it transparently in the schema so it matches the
// WIT/guest representation (plain `u64`), consistent with `desert(transparent)`
// and the OpenAPI `NewType`. Without this the schema derive would encode it as a
// single-element tuple, which fails to decode guest-produced `u64` values.
#[schema(transparent)]
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
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
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

#[derive(
    Clone,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(transparent))]
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
        let name = format!("oplog-index-{oplog_index}");
        Self::derived_from_bytes(base, name.as_bytes())
    }

    /// Generates a deterministic idempotency key in the namespace of `base` from arbitrary bytes.
    pub fn derived_from_bytes(base: &IdempotencyKey, name: &[u8]) -> Self {
        Self::from_uuid(Uuid::new_v5(&base.to_uuid(), name))
    }

    /// The UUID of this key: the key itself when it is a UUID, otherwise a UUIDv5 of the key in
    /// the ROOT_NS namespace.
    pub fn to_uuid(&self) -> Uuid {
        Uuid::parse_str(&self.value)
            .unwrap_or_else(|_| Uuid::new_v5(&Self::ROOT_NS, self.value.as_bytes()))
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
pub struct AgentResourceDescription {
    pub created_at: Timestamp,
    pub resource_owner: String,
    pub resource_name: String,
}

/// Represents last known status of a worker
///
/// This is always recorded together with the current oplog index, and it can only be used
/// as a source of truth if there are no newer oplog entries since the record.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec, poem_openapi::Enum))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub enum AgentStatus {
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
    /// The worker is a never-live sandbox: it replays its history and never runs live.
    /// The folded status underneath is kept as the underlying status.
    Sandboxed,
}

impl PartialOrd for AgentStatus {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for AgentStatus {
    fn cmp(&self, other: &Self) -> Ordering {
        let v1: i32 = (*self).into();
        let v2: i32 = (*other).into();
        v1.cmp(&v2)
    }
}

impl FromStr for AgentStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "running" => Ok(AgentStatus::Running),
            "idle" => Ok(AgentStatus::Idle),
            "suspended" => Ok(AgentStatus::Suspended),
            "interrupted" => Ok(AgentStatus::Interrupted),
            "retrying" => Ok(AgentStatus::Retrying),
            "failed" => Ok(AgentStatus::Failed),
            "exited" => Ok(AgentStatus::Exited),
            "sandboxed" => Ok(AgentStatus::Sandboxed),
            _ => Err(format!("Unknown worker status: {s}")),
        }
    }
}

impl Display for AgentStatus {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentStatus::Running => write!(f, "Running"),
            AgentStatus::Idle => write!(f, "Idle"),
            AgentStatus::Suspended => write!(f, "Suspended"),
            AgentStatus::Interrupted => write!(f, "Interrupted"),
            AgentStatus::Retrying => write!(f, "Retrying"),
            AgentStatus::Failed => write!(f, "Failed"),
            AgentStatus::Exited => write!(f, "Exited"),
            AgentStatus::Sandboxed => write!(f, "Sandboxed"),
        }
    }
}

impl TryFrom<i32> for AgentStatus {
    type Error = String;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(AgentStatus::Running),
            1 => Ok(AgentStatus::Idle),
            2 => Ok(AgentStatus::Suspended),
            3 => Ok(AgentStatus::Interrupted),
            4 => Ok(AgentStatus::Retrying),
            5 => Ok(AgentStatus::Failed),
            6 => Ok(AgentStatus::Exited),
            7 => Ok(AgentStatus::Sandboxed),
            _ => Err(format!("Unknown worker status: {value}")),
        }
    }
}

impl From<AgentStatus> for i32 {
    fn from(value: AgentStatus) -> Self {
        match value {
            AgentStatus::Running => 0,
            AgentStatus::Idle => 1,
            AgentStatus::Suspended => 2,
            AgentStatus::Interrupted => 3,
            AgentStatus::Retrying => 4,
            AgentStatus::Failed => 5,
            AgentStatus::Exited => 6,
            AgentStatus::Sandboxed => 7,
        }
    }
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    Default,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(transparent))]
#[serde(transparent)]
pub struct ScanCursor(String);

impl ScanCursor {
    pub fn new(value: String) -> Self {
        Self(value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }

    pub fn is_finished(&self) -> bool {
        self.0.is_empty()
    }

    pub fn into_option(self) -> Option<Self> {
        if self.is_finished() { None } else { Some(self) }
    }
}

/// The key share and the time that a call of an agent listing page asks for. The executor scans
/// at most the smaller of these and of its own page share and scan time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct PageScanRequest {
    pub keys: u64,
    pub millis: u64,
}

impl PageScanRequest {
    /// The request of the first call of a page, which takes the executor's own share and time.
    pub const FIRST: Self = Self {
        keys: u64::MAX,
        millis: u64::MAX,
    };
}

/// What the executor granted a call of an agent listing page: the most scanning calls of one
/// page, the most keys that this call scans apart from its last storage batch, and the time that
/// it may scan. It holds only configured values and the request clamped to them, never a
/// measurement of the scan, so it says nothing about the agents that the scan examined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct PageScanGrant {
    pub calls: u64,
    pub keys: u64,
    pub millis: u64,
}

impl Display for ScanCursor {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for ScanCursor {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::new(s.to_string()))
    }
}

impl From<String> for ScanCursor {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<ScanCursor> for String {
    fn from(value: ScanCursor) -> Self {
        value.into_inner()
    }
}

#[cfg(all(test, feature = "full"))]
mod scan_cursor_tests {
    use super::ScanCursor;
    use test_r::test;

    #[test]
    fn scan_cursor_preserves_opaque_value_through_json_and_binary_round_trips() {
        for value in ["", "gsc1_opaque-token_雪"] {
            let cursor = ScanCursor::new(value.to_string());

            let json = serde_json::to_string(&cursor).unwrap();
            let json_roundtrip: ScanCursor = serde_json::from_str(&json).unwrap();
            assert_eq!(json_roundtrip, cursor);

            let bytes = desert_rust::serialize_to_byte_vec(&cursor).unwrap();
            let binary_roundtrip: ScanCursor = desert_rust::deserialize(&bytes).unwrap();
            assert_eq!(binary_roundtrip, cursor);
        }
    }
}

#[cfg(feature = "full")]
impl poem_openapi::types::Type for ScanCursor {
    const IS_REQUIRED: bool = true;
    type RawValueType = Self;
    type RawElementValueType = Self;

    fn name() -> std::borrow::Cow<'static, str> {
        "ScanCursor".into()
    }

    fn schema_ref() -> poem_openapi::registry::MetaSchemaRef {
        poem_openapi::registry::MetaSchemaRef::Reference(Self::name().into_owned())
    }

    fn register(registry: &mut poem_openapi::registry::Registry) {
        registry.create_schema::<Self, _>(Self::name().into_owned(), |_| {
            poem_openapi::registry::MetaSchema::new("string")
        });
    }

    fn as_raw_value(&self) -> Option<&Self::RawValueType> {
        Some(self)
    }

    fn raw_element_iter<'a>(
        &'a self,
    ) -> Box<dyn Iterator<Item = &'a Self::RawElementValueType> + 'a> {
        Box::new(self.as_raw_value().into_iter())
    }
}

#[cfg(feature = "full")]
impl poem_openapi::types::ToJSON for ScanCursor {
    fn to_json(&self) -> Option<serde_json::Value> {
        Some(serde_json::Value::String(self.0.clone()))
    }
}

#[cfg(feature = "full")]
impl poem_openapi::types::ParseFromJSON for ScanCursor {
    fn parse_from_json(value: Option<serde_json::Value>) -> poem_openapi::types::ParseResult<Self> {
        match value {
            Some(serde_json::Value::String(value)) => Ok(Self::new(value)),
            _ => Err(poem_openapi::types::ParseError::custom(
                "ScanCursor must be a string",
            )),
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

pub fn render_config_path(path: &[String]) -> String {
    path.join(".")
}

#[cfg(feature = "full")]
fn canonicalize_agent_path(path: &[String]) -> Vec<String> {
    use heck::ToLowerCamelCase;
    path.iter()
        .map(|segment| segment.to_lower_camel_case())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::AgentId;
    use std::str::FromStr;
    use test_r::test;

    #[test]
    fn an_agent_id_round_trips_through_every_encoding() {
        let id = AgentId::from_parts(7, 0x2AA_AAAA_AAAA_AAAA_AAAA_AAAA);
        let text = "00000007-aaaa-8aaa-aaaa-aaaaaaaaaaaa";
        assert_eq!(id.to_string(), text);
        assert_eq!(AgentId::from_str(text), Ok(id));
        assert_eq!(
            AgentId::from_str("00000007-AAAA-8AAA-AAAA-AAAAAAAAAAAA"),
            Ok(id)
        );
        assert!(AgentId::from_str(r#"counter("a")"#).is_err());
        assert_eq!(serde_json::to_string(&id).unwrap(), format!("\"{text}\""));
        assert_eq!(
            serde_json::from_str::<AgentId>(&format!("\"{text}\"")).unwrap(),
            id
        );
        let bytes = desert_rust::serialize_to_byte_vec(&id).unwrap();
        assert_eq!(bytes.len(), 16);
        assert_eq!(desert_rust::deserialize::<AgentId>(&bytes).unwrap(), id);
        let proto: golem_api_grpc::proto::golem::worker::AgentId = id.into();
        assert_eq!(AgentId::try_from(proto), Ok(id));
    }
}

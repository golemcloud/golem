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

//! The report model of the obfuscated oplog export.
//!
//! A customer value becomes a node that shows only its token, unless a keep flag adds the
//! fields that the flag names. Golem metadata becomes a `text`, `number`, `timestamp` or `flag`
//! node.

use golem_common::model::Timestamp;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};

/// A keyed token, written `<domain>:<32 lower-case hex chars>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ReportToken(pub String);

impl ReportToken {
    /// The hex part of the token, after the last `:`.
    pub fn hex(&self) -> &str {
        self.0.rsplit(':').next().unwrap_or_default()
    }
}

impl Display for ReportToken {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One node of a rendered value or of the rendered entry structure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ReportNode {
    Record(RecordNode),
    Tuple(TupleNode),
    Case(CaseNode),
    List(ListNode),
    Map(MapNode),
    Bytes(BytesNode),
    Scalar(ScalarNode),
    Quantity(QuantityNode),
    Str(StrNode),
    Agent(ReportAgentRef),
    Redacted(RedactedNode),
    TypeDescription(TypeDescriptionNode),
    Text(TextNode),
    Number(NumberNode),
    Timestamp(TimestampNode),
    Flag(FlagNode),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordNode {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<KnownRole>,
    pub fields: Vec<ReportField>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportField {
    /// The field name; absent for the fields of a schema-less record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub value: ReportNode,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TupleNode {
    pub elements: Vec<ReportNode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CaseKind {
    Variant,
    Enum,
    Flags,
    Union,
    Option,
    Result,
}

/// A sum-type node. Without `--keep-shape`, and unless the case is kept Golem metadata, it holds
/// only `token`; otherwise `case`, `caseIndex` and `payload` show the case. A union without a
/// type to check its tag against has `tagToken`, a `str` token of the tag, instead of `case`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaseNode {
    pub case_kind: CaseKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<KnownRole>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub case: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub case_index: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<Box<ReportNode>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag_token: Option<ReportToken>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<ReportToken>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListNode {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<u64>,
    pub token: ReportToken,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elements: Option<Vec<ReportNode>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MapNode {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<u64>,
    pub token: ReportToken,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entries: Option<Vec<MapEntryNode>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MapEntryNode {
    pub key: ReportNode,
    pub value: ReportNode,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BytesNode {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<u64>,
    pub token: ReportToken,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ScalarKind {
    Int,
    Float,
    Bool,
    Datetime,
    Duration,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScalarNode {
    pub scalar_kind: ScalarKind,
    /// The value, only with `--keep-scalars`: a JSON number for integers and floats (a string
    /// for a non-finite float), a boolean, an RFC 3339 text for a datetime and nanoseconds for a
    /// duration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,
    pub token: ReportToken,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuantityNode {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mantissa: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<i32>,
    /// A declared unit of a typed quantity, only with `--keep-scalars`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// The token of the free-text unit of a schema-less quantity, only with `--keep-scalars`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit_token: Option<ReportToken>,
    pub token: ReportToken,
}

/// A string. `value` shows only for error text with `--keep-error-text`; otherwise the node
/// holds the token.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StrNode {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<ReportToken>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

/// A reference to an agent: one token over the text of the agent ID. It holds no agent ID, no
/// component ID and no agent type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportAgentRef {
    pub token: ReportToken,
}

impl ReportAgentRef {
    /// The display form: `agent:` and the first 12 hex chars of the token, then `…`.
    pub fn pseudonym(&self) -> String {
        let hex = self.token.hex();
        let prefix: String = hex.chars().take(12).collect();
        format!("agent:{prefix}…")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RedactionReason {
    Secret,
    QuotaToken,
    PermissionCard,
    Stream,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedactedNode {
    pub reason: RedactionReason,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TypeDescriptionNode {
    pub description: TypeDescription,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KnownRole {
    Multimodal,
    UnstructuredText,
    UnstructuredBinary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TypeKind {
    Ref,
    Bool,
    S8,
    S16,
    S32,
    S64,
    U8,
    U16,
    U32,
    U64,
    F32,
    F64,
    Char,
    String,
    Record,
    Variant,
    Enum,
    Flags,
    Tuple,
    List,
    FixedList,
    Map,
    Option,
    Result,
    Text,
    Binary,
    Path,
    Url,
    Uuid,
    Datetime,
    Duration,
    Quantity,
    Union,
    Secret,
    QuotaToken,
    PermissionCard,
    Future,
    Stream,
}

/// A schema held as data, without its metadata texts, restrictions and discriminator
/// literals. A recursive definition is described once in `definitions` and referred to by its
/// name from `ref` nodes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TypeDescription {
    pub kind: TypeKind,
    /// The type name of a named definition, or the referred name of a `ref` node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<KnownRole>,
    /// Record fields, variant cases and union branches (by tag).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<NamedTypeDescription>,
    /// Tuple elements, the list, fixed-list, option, future or stream element, the map key and
    /// value, the result `ok` and `err` types and the secret inner type, in this order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub elements: Vec<TypeDescription>,
    /// Enum case names and flag names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub names: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_unit: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_suffixes: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub definitions: Vec<NamedTypeDescription>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NamedTypeDescription {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<TypeDescription>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextNode {
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NumberNode {
    pub value: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimestampNode {
    pub value: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlagNode {
    pub value: bool,
}

/// One oplog entry of the report: either a rendered `body` or an `omitted` marker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportEntry {
    pub index: u64,
    pub timestamp: Timestamp,
    /// The entry type: the `type` field of the public oplog entry.
    pub kind: String,
    /// The rendered entity attribution; absent for the attribution to the agent itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribution: Option<ReportNode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<ReportNode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub omitted: Option<ReportOmission>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportOmission {
    pub reason: OmittedReason,
    pub size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OmittedReason {
    /// The entry is larger than the hydration limit, so the CLI did not read it in full.
    TooLarge,
    /// The read of the full entry failed.
    HydrationFailed,
}

/// One position of a rendered entry that holds an RPC value without its schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaLessRpcValueRef {
    pub index: u64,
    pub path: String,
}

/// The first document of an export: the keep flags, the root reference and the provenance of
/// the report. It never holds the query text, an agent ID or a component.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OplogExportHeader {
    pub obfuscate: bool,
    pub keep_scalars: bool,
    pub keep_shape: bool,
    pub keep_error_text: bool,
    /// `true` when the export read only the entries that matched a query.
    pub filtered: bool,
    pub root: ReportAgentRef,
    pub cli_version: String,
    pub generated_at: String,
}

/// The number of items each coverage list shows; `discovered` counts every item.
pub const COVERAGE_LIST_CAP: usize = 100;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoverageList<T> {
    pub shown: Vec<T>,
    pub discovered: u64,
}

impl<T> Default for CoverageList<T> {
    fn default() -> Self {
        Self {
            shown: Vec::new(),
            discovered: 0,
        }
    }
}

impl<T> CoverageList<T> {
    pub fn push(&mut self, item: T) {
        self.discovered += 1;
        if self.shown.len() < COVERAGE_LIST_CAP {
            self.shown.push(item);
        }
    }

    pub fn merge(&mut self, other: CoverageList<T>) {
        self.discovered += other.discovered;
        let room = COVERAGE_LIST_CAP.saturating_sub(self.shown.len());
        self.shown.extend(other.shown.into_iter().take(room));
    }
}

/// An omitted entry that the report shows without its body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OmittedEntryRef {
    pub index: u64,
    pub kind: String,
    pub size: u64,
    pub reason: OmittedReason,
}

/// An `End` or `Cancelled` whose `Start` function is unknown, so its value stayed a token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartUnknownRef {
    pub index: u64,
    pub start_index: u64,
}

/// The last document of an export: what the report does not show in full.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OplogExportCoverage {
    pub incomplete: bool,
    pub hydration_failures: CoverageList<OmittedEntryRef>,
    pub start_unknown: CoverageList<StartUnknownRef>,
    pub schema_less_rpc_values: CoverageList<SchemaLessRpcValueRef>,
    pub notes: Vec<String>,
}

/// The note of `--keep-scalars` without `--keep-shape`.
pub const KEEP_SCALARS_WITHOUT_SHAPE_NOTE: &str = "Numbers inside lists, maps, variants, options, results and unions stay inside the token of their container; add --keep-shape to show them.";

impl OplogExportCoverage {
    pub fn new(notes: Vec<String>) -> Self {
        Self {
            incomplete: false,
            hydration_failures: CoverageList::default(),
            start_unknown: CoverageList::default(),
            schema_less_rpc_values: CoverageList::default(),
            notes,
        }
    }

    /// Adds the lists of `other`, keeping the cap, and the notes that `self` does not hold yet.
    pub fn merge(&mut self, other: OplogExportCoverage) {
        self.hydration_failures.merge(other.hydration_failures);
        self.start_unknown.merge(other.start_unknown);
        self.schema_less_rpc_values
            .merge(other.schema_less_rpc_values);
        for note in other.notes {
            if !self.notes.contains(&note) {
                self.notes.push(note);
            }
        }
        self.update_incomplete();
    }

    pub fn update_incomplete(&mut self) {
        self.incomplete =
            self.hydration_failures.discovered > 0 || self.start_unknown.discovered > 0;
    }
}

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

//! The value walker: the default rendering of typed and schema-less values (each value kind
//! gives one token, and the keep flags add its parts), and the kept nodes of trusted host
//! payloads.

use super::canonical::{encode, encode_str};
use super::describe::{describe_agent_type, describe_schema, known_role};
use super::{ObfuscationContext, agent_reference};
use crate::oplog_export::model::{
    BytesNode, CaseKind, CaseNode, KnownRole, ListNode, MapEntryNode, MapNode, NumberNode,
    QuantityNode, RecordNode, RedactedNode, RedactionReason, ReportField, ReportNode, ReportToken,
    ScalarKind, ScalarNode, StrNode, TextNode, TupleNode, TypeDescriptionNode,
};
use golem_common::model::AgentId;
use golem_common::model::oplog::types::{
    FileSystemError, PermissionCardDeriveError, PermissionCardRevokeError, SecretRevealError,
    SerializableHostFailureKind, SerializableHttpErrorCode, SerializableHttpMethod,
    SerializableInvokeResult, SerializableP3FileSystemError, SerializableP3HttpClientSendResult,
    SerializableP3IpNameLookupError, SerializableP3SocketErrorCode, SerializablePermissionError,
    SerializableRdbmsError, SerializableResponseHeaders, SerializableRpcError,
    SerializableSocketError, SerializableStreamError, SerializableToolRpcError,
    SerializableWebsocketError,
};
use golem_common::schema::{
    FromSchema, IntoSchema, RegisteredAgentTypeSchema, ResultSpec, ResultValuePayload, SchemaGraph,
    SchemaType, SchemaValue, TypeId, TypedSchemaValue,
};
use std::collections::HashSet;
use std::sync::LazyLock;

/// Where a value node is. Customer data gets only the default rendering. A trusted host payload
/// gets the default rendering plus its kept nodes. An error subtree is a trusted region whose
/// strings are error text and whose enum and variant cases are kept, because every type in it
/// is a Golem error type; a customer value inside it is rendered in the customer region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Region {
    Customer,
    Trusted,
    Error,
}

/// The schema type IDs that the rules look for.
pub(super) struct KnownIds {
    pub agent_id: TypeId,
    pub schema_value: TypeId,
    pub typed_schema_value: TypeId,
    pub schema_graph: TypeId,
    pub schema_type: TypeId,
    pub registered_agent_type: TypeId,
    pub invoke_result: TypeId,
    pub http_method: TypeId,
    pub response_headers: TypeId,
    pub error_types: HashSet<TypeId>,
}

pub(super) static KNOWN_IDS: LazyLock<KnownIds> = LazyLock::new(|| KnownIds {
    agent_id: <AgentId as IntoSchema>::type_id(),
    schema_value: <SchemaValue as IntoSchema>::type_id(),
    typed_schema_value: <TypedSchemaValue as IntoSchema>::type_id(),
    schema_graph: <SchemaGraph as IntoSchema>::type_id(),
    schema_type: <SchemaType as IntoSchema>::type_id(),
    registered_agent_type: <RegisteredAgentTypeSchema as IntoSchema>::type_id(),
    invoke_result: <SerializableInvokeResult as IntoSchema>::type_id(),
    http_method: <SerializableHttpMethod as IntoSchema>::type_id(),
    response_headers: <SerializableResponseHeaders as IntoSchema>::type_id(),
    error_types: HashSet::from([
        <SerializableRpcError as IntoSchema>::type_id(),
        <SerializableHttpErrorCode as IntoSchema>::type_id(),
        <FileSystemError as IntoSchema>::type_id(),
        <SerializableP3FileSystemError as IntoSchema>::type_id(),
        <SerializableSocketError as IntoSchema>::type_id(),
        <SerializableP3SocketErrorCode as IntoSchema>::type_id(),
        <SerializableP3IpNameLookupError as IntoSchema>::type_id(),
        <SerializableStreamError as IntoSchema>::type_id(),
        <SerializableWebsocketError as IntoSchema>::type_id(),
        <SerializableRdbmsError as IntoSchema>::type_id(),
        <SecretRevealError as IntoSchema>::type_id(),
        <PermissionCardRevokeError as IntoSchema>::type_id(),
        <PermissionCardDeriveError as IntoSchema>::type_id(),
        <SerializablePermissionError as IntoSchema>::type_id(),
        <SerializableToolRpcError as IntoSchema>::type_id(),
        <SerializableP3HttpClientSendResult as IntoSchema>::type_id(),
        <SerializableHostFailureKind as IntoSchema>::type_id(),
        <golem_common::model::agent::AgentError as IntoSchema>::type_id(),
    ]),
});

/// Renders the values of one entry and collects the schema-less RPC value positions.
pub(super) struct Walker<'r, 'a> {
    pub ctx: &'r ObfuscationContext<'a>,
    pub rpc_values: Vec<&'static str>,
}

const STACK_RED_ZONE: usize = 64 * 1024;
const STACK_GROWTH: usize = 1024 * 1024;
const MAX_REF_HOPS: usize = 64;

impl<'r, 'a> Walker<'r, 'a> {
    pub fn new(ctx: &'r ObfuscationContext<'a>) -> Self {
        Self {
            ctx,
            rpc_values: Vec::new(),
        }
    }

    fn token(&self, domain: &str, bytes: &[u8]) -> ReportToken {
        self.ctx.token(domain, bytes)
    }

    // ---------------------------------------------------------------------------------------
    // Typed values
    // ---------------------------------------------------------------------------------------

    /// A typed value with its own graph, in the customer region.
    pub fn customer(&mut self, typed: &TypedSchemaValue) -> ReportNode {
        self.typed(
            typed.graph(),
            typed.root_type(),
            typed.value(),
            Region::Customer,
        )
    }

    pub fn typed(
        &mut self,
        graph: &SchemaGraph,
        ty: &SchemaType,
        value: &SchemaValue,
        region: Region,
    ) -> ReportNode {
        stacker::maybe_grow(STACK_RED_ZONE, STACK_GROWTH, || {
            self.typed_inner(graph, ty, value, region)
        })
    }

    fn typed_inner(
        &mut self,
        graph: &SchemaGraph,
        ty: &SchemaType,
        value: &SchemaValue,
        region: Region,
    ) -> ReportNode {
        let ids = &*KNOWN_IDS;
        let Some(resolved) = resolve(graph, ty) else {
            return self.schemaless(value, Region::Customer);
        };
        let has_id = |id: &TypeId| resolved.ids.contains(&id);

        if has_id(&ids.agent_id)
            && let Ok(agent_id) = AgentId::from_value(value)
        {
            return ReportNode::Agent(agent_reference(self.ctx, &agent_id));
        }
        if has_id(&ids.schema_value) {
            return match SchemaValue::from_value(value) {
                Ok(inner) => self.schemaless(&inner, Region::Customer),
                Err(_) => self.schemaless(value, Region::Customer),
            };
        }

        let mut region = region;
        if region != Region::Customer {
            if has_id(&ids.typed_schema_value) {
                return match TypedSchemaValue::from_value(value) {
                    Ok(inner) => self.customer(&inner),
                    Err(_) => self.schemaless(value, Region::Customer),
                };
            }
            if has_id(&ids.schema_graph) {
                return match SchemaGraph::from_value(value) {
                    Ok(schema) => type_description(&schema),
                    Err(_) => self.schemaless(value, Region::Customer),
                };
            }
            if has_id(&ids.schema_type) {
                return match SchemaType::from_value(value) {
                    Ok(schema) => type_description(&SchemaGraph::anonymous(schema)),
                    Err(_) => self.schemaless(value, Region::Customer),
                };
            }
            if has_id(&ids.registered_agent_type) {
                return match RegisteredAgentTypeSchema::from_value(value) {
                    Ok(agent_type) => describe_agent_type(&agent_type),
                    Err(_) => self.schemaless(value, Region::Customer),
                };
            }
            if has_id(&ids.invoke_result) {
                return self.invoke_result(graph, resolved.body, value, resolved.type_name);
            }
            if has_id(&ids.http_method) {
                return self.http_method(graph, resolved.body, value, resolved.type_name);
            }
            if has_id(&ids.response_headers) {
                return self.response_headers(graph, resolved.body, value, resolved.type_name);
            }
            if resolved.ids.iter().any(|id| ids.error_types.contains(*id)) {
                region = Region::Error;
            }
        }

        self.structural(graph, resolved.body, value, region, resolved.type_name)
    }

    /// A resolved (non-`Ref`) type and its value.
    fn structural(
        &mut self,
        graph: &SchemaGraph,
        ty: &SchemaType,
        value: &SchemaValue,
        region: Region,
        type_name: Option<String>,
    ) -> ReportNode {
        let role = ty.metadata().role.as_ref().and_then(known_role);
        if leaf_matches(ty, value) {
            return self.leaf(value, region, unit_declared(ty, value));
        }
        let shape = self.ctx.keep().shape;
        match (ty, value) {
            (SchemaType::Record { fields, .. }, SchemaValue::Record { fields: values })
                if fields.len() == values.len() =>
            {
                let fields = fields
                    .iter()
                    .zip(values)
                    .map(|(field, value)| ReportField {
                        name: Some(field.name.clone()),
                        value: self.typed(graph, &field.body, value, region),
                    })
                    .collect();
                ReportNode::Record(RecordNode {
                    type_name,
                    role,
                    fields,
                })
            }
            (SchemaType::Tuple { elements, .. }, SchemaValue::Tuple { elements: values })
                if elements.len() == values.len() =>
            {
                let elements = elements
                    .iter()
                    .zip(values)
                    .map(|(ty, value)| self.typed(graph, ty, value, region))
                    .collect();
                ReportNode::Tuple(TupleNode { elements })
            }
            (SchemaType::FixedList { element, .. }, SchemaValue::FixedList { elements }) => {
                let byte_list = is_u8_type(graph, element) || all_bytes(elements);
                self.list(value, elements, byte_list, |walker, value| {
                    walker.typed(graph, element, value, region)
                })
            }
            (SchemaType::List { element, .. }, SchemaValue::List { elements }) => {
                let byte_list = is_u8_type(graph, element) || all_bytes(elements);
                self.list(value, elements, byte_list, |walker, value| {
                    walker.typed(graph, element, value, region)
                })
            }
            (
                SchemaType::Map {
                    key: key_ty,
                    value: value_ty,
                    ..
                },
                SchemaValue::Map { entries },
            ) => self.map(value, entries, |walker, key, value| {
                (
                    walker.typed(graph, key_ty, key, region),
                    walker.typed(graph, value_ty, value, region),
                )
            }),
            (SchemaType::Variant { cases, .. }, SchemaValue::Variant(payload))
                if (payload.case as usize) < cases.len()
                    && cases[payload.case as usize].payload.is_some()
                        == payload.payload.is_some() =>
            {
                let case = &cases[payload.case as usize];
                self.case(
                    value,
                    CaseKind::Variant,
                    type_name,
                    role,
                    shape || region == Region::Error,
                    |walker| {
                        let payload = match (&case.payload, &payload.payload) {
                            (Some(ty), Some(value)) => Some(walker.typed(graph, ty, value, region)),
                            _ => None,
                        };
                        (Some(case.name.clone()), Some(payload_index(value)), payload)
                    },
                )
            }
            (SchemaType::Enum { cases, .. }, SchemaValue::Enum { case })
                if (*case as usize) < cases.len() =>
            {
                self.case(
                    value,
                    CaseKind::Enum,
                    type_name,
                    role,
                    shape || region == Region::Error,
                    |_| (Some(cases[*case as usize].clone()), Some(*case), None),
                )
            }
            (SchemaType::Flags { flags, .. }, SchemaValue::Flags { bits })
                if flags.len() == bits.len() =>
            {
                self.case(value, CaseKind::Flags, type_name, role, shape, |walker| {
                    let fields = flags
                        .iter()
                        .zip(bits)
                        .map(|(name, bit)| ReportField {
                            name: Some(name.clone()),
                            value: walker.flag(*bit),
                        })
                        .collect();
                    (None, None, Some(record(None, fields)))
                })
            }
            (SchemaType::Option { inner, .. }, SchemaValue::Option { inner: present }) => self
                .case(
                    value,
                    CaseKind::Option,
                    type_name,
                    role,
                    shape,
                    |walker| match present {
                        Some(present) => (
                            Some("some".to_string()),
                            None,
                            Some(walker.typed(graph, inner, present, region)),
                        ),
                        None => (Some("none".to_string()), None, None),
                    },
                ),
            (SchemaType::Result { spec, .. }, SchemaValue::Result(result))
                if result_matches(spec, result) =>
            {
                self.case(value, CaseKind::Result, type_name, role, shape, |walker| {
                    walker.result_parts(graph, spec, result, region, region)
                })
            }
            (SchemaType::Union { spec, .. }, SchemaValue::Union(union)) => {
                match spec.branches.iter().find(|branch| branch.tag == union.tag) {
                    Some(branch) => {
                        self.case(value, CaseKind::Union, type_name, role, shape, |walker| {
                            (
                                Some(branch.tag.clone()),
                                None,
                                Some(walker.typed(graph, &branch.body, &union.body, region)),
                            )
                        })
                    }
                    None => self.schemaless(value, Region::Customer),
                }
            }
            _ => self.schemaless(value, Region::Customer),
        }
    }

    /// The case, the case index and the payload of a result value, with the `ok` payload in
    /// `ok_region` and the `err` payload in `err_region`.
    fn result_parts(
        &mut self,
        graph: &SchemaGraph,
        spec: &ResultSpec,
        result: &ResultValuePayload,
        ok_region: Region,
        err_region: Region,
    ) -> (Option<String>, Option<u32>, Option<ReportNode>) {
        match result {
            ResultValuePayload::Ok { value } => (
                Some("ok".to_string()),
                None,
                match (spec.ok.as_deref(), value.as_deref()) {
                    (Some(ty), Some(value)) => Some(self.typed(graph, ty, value, ok_region)),
                    _ => None,
                },
            ),
            ResultValuePayload::Err { value } => (
                Some("err".to_string()),
                None,
                match (spec.err.as_deref(), value.as_deref()) {
                    (Some(ty), Some(value)) => Some(self.typed(graph, ty, value, err_region)),
                    _ => None,
                },
            ),
        }
    }

    /// A result whose branch is kept. The `ok` payload goes through `ok`; the `err`
    /// payload is an error subtree.
    pub fn kept_result(
        &mut self,
        graph: &SchemaGraph,
        ty: &SchemaType,
        value: &SchemaValue,
        ok: impl FnOnce(&mut Self, &SchemaGraph, &SchemaType, &SchemaValue) -> ReportNode,
    ) -> ReportNode {
        let Some(resolved) = resolve(graph, ty) else {
            return self.schemaless(value, Region::Customer);
        };
        match (resolved.body, value) {
            (SchemaType::Result { spec, .. }, SchemaValue::Result(result))
                if result_matches(spec, result) =>
            {
                let (case, payload) = match result {
                    ResultValuePayload::Ok { value } => (
                        "ok",
                        match (spec.ok.as_deref(), value.as_deref()) {
                            (Some(ty), Some(value)) => Some(ok(self, graph, ty, value)),
                            _ => None,
                        },
                    ),
                    ResultValuePayload::Err { value } => (
                        "err",
                        match (spec.err.as_deref(), value.as_deref()) {
                            (Some(ty), Some(value)) => {
                                Some(self.typed(graph, ty, value, Region::Error))
                            }
                            _ => None,
                        },
                    ),
                };
                kept_case(CaseKind::Result, resolved.type_name, case, None, payload)
            }
            _ => self.schemaless(value, Region::Customer),
        }
    }

    /// An RPC value position: the generic encoding of a `SchemaValue`, rendered schema-less and
    /// reported at `path`.
    pub fn rpc_value(&mut self, path: &'static str, value: &SchemaValue) -> ReportNode {
        match SchemaValue::from_value(value) {
            Ok(inner) => {
                self.rpc_values.push(path);
                self.schemaless(&inner, Region::Customer)
            }
            Err(_) => self.schemaless(value, Region::Customer),
        }
    }

    /// `SerializableInvokeResult`: the case is kept; `failed` and `failedClassified.message`
    /// are error text; `completed` keeps its branch, its `ok` value is an RPC value position.
    fn invoke_result(
        &mut self,
        graph: &SchemaGraph,
        ty: &SchemaType,
        value: &SchemaValue,
        type_name: Option<String>,
    ) -> ReportNode {
        let (SchemaType::Variant { cases, .. }, SchemaValue::Variant(payload)) = (ty, value) else {
            return self.schemaless(value, Region::Customer);
        };
        let Some(case) = cases.get(payload.case as usize) else {
            return self.schemaless(value, Region::Customer);
        };
        let rendered = match (&case.payload, payload.payload.as_deref()) {
            (Some(case_ty), Some(case_value)) => Some(match normalized(&case.name).as_str() {
                "completed" => self.kept_result(graph, case_ty, case_value, |walker, _, _, ok| {
                    walker.rpc_value("response.result.completed.ok", ok)
                }),
                "failed" | "failedclassified" => {
                    self.typed(graph, case_ty, case_value, Region::Error)
                }
                _ => self.typed(graph, case_ty, case_value, Region::Trusted),
            }),
            _ => None,
        };
        kept_case(
            CaseKind::Variant,
            type_name,
            &case.name,
            Some(payload.case),
            rendered,
        )
    }

    /// `SerializableHttpMethod`: the case is kept; the text of `Other` is a `str` token.
    fn http_method(
        &mut self,
        graph: &SchemaGraph,
        ty: &SchemaType,
        value: &SchemaValue,
        type_name: Option<String>,
    ) -> ReportNode {
        let (SchemaType::Variant { cases, .. }, SchemaValue::Variant(payload)) = (ty, value) else {
            return self.schemaless(value, Region::Customer);
        };
        let Some(case) = cases.get(payload.case as usize) else {
            return self.schemaless(value, Region::Customer);
        };
        let rendered = match (&case.payload, payload.payload.as_deref()) {
            (Some(case_ty), Some(case_value)) => {
                Some(self.typed(graph, case_ty, case_value, Region::Trusted))
            }
            _ => None,
        };
        kept_case(
            CaseKind::Variant,
            type_name,
            &case.name,
            Some(payload.case),
            rendered,
        )
    }

    /// `SerializableResponseHeaders`: `status` is kept, the headers get the default rendering.
    fn response_headers(
        &mut self,
        graph: &SchemaGraph,
        ty: &SchemaType,
        value: &SchemaValue,
        type_name: Option<String>,
    ) -> ReportNode {
        let (SchemaType::Record { fields, .. }, SchemaValue::Record { fields: values }) =
            (ty, value)
        else {
            return self.schemaless(value, Region::Customer);
        };
        if fields.len() != values.len() {
            return self.schemaless(value, Region::Customer);
        }
        let fields = fields
            .iter()
            .zip(values)
            .map(|(field, value)| ReportField {
                name: Some(field.name.clone()),
                value: match (field.name.as_str(), unsigned(value)) {
                    ("status", Some(status)) => number(status),
                    _ => self.typed(graph, &field.body, value, Region::Trusted),
                },
            })
            .collect();
        record(type_name, fields)
    }

    // ---------------------------------------------------------------------------------------
    // Schema-less values and leaves
    // ---------------------------------------------------------------------------------------

    pub fn schemaless(&mut self, value: &SchemaValue, region: Region) -> ReportNode {
        stacker::maybe_grow(STACK_RED_ZONE, STACK_GROWTH, || {
            self.schemaless_inner(value, region)
        })
    }

    fn schemaless_inner(&mut self, value: &SchemaValue, region: Region) -> ReportNode {
        let shape = self.ctx.keep().shape;
        match value {
            SchemaValue::Record { fields } => {
                let fields = fields
                    .iter()
                    .map(|value| ReportField {
                        name: None,
                        value: self.schemaless(value, region),
                    })
                    .collect();
                record(None, fields)
            }
            SchemaValue::Tuple { elements } => {
                let elements = elements
                    .iter()
                    .map(|value| self.schemaless(value, region))
                    .collect();
                ReportNode::Tuple(TupleNode { elements })
            }
            SchemaValue::List { elements } | SchemaValue::FixedList { elements } => {
                let byte_list = all_bytes(elements);
                self.list(value, elements, byte_list, |walker, value| {
                    walker.schemaless(value, region)
                })
            }
            SchemaValue::Map { entries } => self.map(value, entries, |walker, key, value| {
                (
                    walker.schemaless(key, region),
                    walker.schemaless(value, region),
                )
            }),
            SchemaValue::Variant(payload) => {
                self.case(value, CaseKind::Variant, None, None, shape, |walker| {
                    (
                        None,
                        Some(payload.case),
                        payload
                            .payload
                            .as_deref()
                            .map(|value| walker.schemaless(value, region)),
                    )
                })
            }
            SchemaValue::Enum { case } => {
                self.case(value, CaseKind::Enum, None, None, shape, |_| {
                    (None, Some(*case), None)
                })
            }
            SchemaValue::Flags { bits } => {
                self.case(value, CaseKind::Flags, None, None, shape, |walker| {
                    let fields = bits
                        .iter()
                        .map(|bit| ReportField {
                            name: None,
                            value: walker.flag(*bit),
                        })
                        .collect();
                    (None, None, Some(record(None, fields)))
                })
            }
            SchemaValue::Option { inner } => self.case(
                value,
                CaseKind::Option,
                None,
                None,
                shape,
                |walker| match inner {
                    Some(inner) => (
                        Some("some".to_string()),
                        None,
                        Some(walker.schemaless(inner, region)),
                    ),
                    None => (Some("none".to_string()), None, None),
                },
            ),
            SchemaValue::Result(result) => {
                self.case(value, CaseKind::Result, None, None, shape, |walker| {
                    let (case, inner) = match result {
                        ResultValuePayload::Ok { value } => ("ok", value),
                        ResultValuePayload::Err { value } => ("err", value),
                    };
                    (
                        Some(case.to_string()),
                        None,
                        inner
                            .as_deref()
                            .map(|value| walker.schemaless(value, region)),
                    )
                })
            }
            SchemaValue::Union(union) => {
                // No type declares the tag, so it is free text: a token, as a schema-less
                // quantity unit is.
                let mut node = self.case(value, CaseKind::Union, None, None, shape, |walker| {
                    (None, None, Some(walker.schemaless(&union.body, region)))
                });
                if let ReportNode::Case(case) = &mut node
                    && shape
                {
                    case.tag_token = Some(self.token("str", &encode_str(&union.tag)));
                }
                node
            }
            _ => self.leaf(value, region, false),
        }
    }

    /// A leaf value. `declared_unit` is true when a quantity's unit is declared by its type.
    fn leaf(&mut self, value: &SchemaValue, region: Region, declared_unit: bool) -> ReportNode {
        let scalars = self.ctx.keep().scalars;
        // Golem writes the numbers of trusted and error regions, so a reader can often predict
        // them. Their own domain keeps such a known value from naming an equal customer token.
        let prefix = if region == Region::Customer {
            "scalar"
        } else {
            "golem"
        };
        let scalar = |walker: &Self, kind: ScalarKind, name: &str, shown: serde_json::Value| {
            ReportNode::Scalar(ScalarNode {
                scalar_kind: kind,
                value: scalars.then_some(shown),
                token: walker.token(&format!("{prefix}:{name}"), &encode(value)),
            })
        };
        match value {
            SchemaValue::Bool(v) => scalar(self, ScalarKind::Bool, "bool", (*v).into()),
            SchemaValue::S8(v) => scalar(self, ScalarKind::Int, "int", (*v).into()),
            SchemaValue::S16(v) => scalar(self, ScalarKind::Int, "int", (*v).into()),
            SchemaValue::S32(v) => scalar(self, ScalarKind::Int, "int", (*v).into()),
            SchemaValue::S64(v) => scalar(self, ScalarKind::Int, "int", (*v).into()),
            SchemaValue::U8(v) => scalar(self, ScalarKind::Int, "int", (*v).into()),
            SchemaValue::U16(v) => scalar(self, ScalarKind::Int, "int", (*v).into()),
            SchemaValue::U32(v) => scalar(self, ScalarKind::Int, "int", (*v).into()),
            SchemaValue::U64(v) => scalar(self, ScalarKind::Int, "int", (*v).into()),
            SchemaValue::F32(v) => scalar(self, ScalarKind::Float, "float", float_json(*v as f64)),
            SchemaValue::F64(v) => scalar(self, ScalarKind::Float, "float", float_json(*v)),
            SchemaValue::Datetime { value: datetime } => scalar(
                self,
                ScalarKind::Datetime,
                "datetime",
                datetime.to_rfc3339().into(),
            ),
            SchemaValue::Duration(duration) => scalar(
                self,
                ScalarKind::Duration,
                "duration",
                duration.nanoseconds.into(),
            ),
            SchemaValue::Char(c) => self.str_leaf(c.encode_utf8(&mut [0u8; 4]), region),
            SchemaValue::String(s) => self.str_leaf(s, region),
            SchemaValue::Text(text) => self.str_leaf(&text.text, region),
            SchemaValue::Path { path } => self.str_leaf(path, region),
            SchemaValue::Url { url } => self.str_leaf(url, region),
            SchemaValue::Uuid(uuid) => self.str_leaf(&uuid.hyphenated().to_string(), region),
            SchemaValue::Binary(binary) => ReportNode::Bytes(BytesNode {
                length: self.ctx.keep().shape.then_some(binary.bytes.len() as u64),
                token: self.token("bytes", &encode(value)),
            }),
            SchemaValue::Quantity(quantity) => ReportNode::Quantity(QuantityNode {
                mantissa: scalars.then_some(quantity.mantissa),
                scale: scalars.then_some(quantity.scale),
                unit: (scalars && declared_unit).then(|| quantity.unit.clone()),
                unit_token: (scalars && !declared_unit)
                    .then(|| self.token("str", &encode_str(&quantity.unit))),
                token: self.token(&format!("{prefix}:quantity"), &encode(value)),
            }),
            SchemaValue::Secret(_) => redacted(RedactionReason::Secret),
            SchemaValue::QuotaToken(_) => redacted(RedactionReason::QuotaToken),
            SchemaValue::PermissionCard(_) => redacted(RedactionReason::PermissionCard),
            SchemaValue::Stream(_) => redacted(RedactionReason::Stream),
            SchemaValue::Record { .. }
            | SchemaValue::Tuple { .. }
            | SchemaValue::FixedList { .. }
            | SchemaValue::List { .. }
            | SchemaValue::Map { .. }
            | SchemaValue::Variant(_)
            | SchemaValue::Enum { .. }
            | SchemaValue::Flags { .. }
            | SchemaValue::Option { .. }
            | SchemaValue::Result(_)
            | SchemaValue::Union(_) => self.schemaless(value, region),
        }
    }

    /// A string leaf: a `str` token, or the text itself for error text with
    /// `--keep-error-text`.
    pub fn str_leaf(&self, text: &str, region: Region) -> ReportNode {
        if region == Region::Error && self.ctx.keep().error_text {
            ReportNode::Str(StrNode {
                token: None,
                value: Some(text.to_string()),
            })
        } else {
            ReportNode::Str(StrNode {
                token: Some(self.token("str", &encode_str(text))),
                value: None,
            })
        }
    }

    pub fn flag(&self, value: bool) -> ReportNode {
        flag(value)
    }

    // ---------------------------------------------------------------------------------------
    // Collections and cases
    // ---------------------------------------------------------------------------------------

    /// An option or a list whose presence, length and elements are kept, with its contents in
    /// the trusted region.
    pub fn kept_container(
        &mut self,
        graph: &SchemaGraph,
        ty: &SchemaType,
        value: &SchemaValue,
    ) -> ReportNode {
        let Some(resolved) = resolve(graph, ty) else {
            return self.schemaless(value, Region::Customer);
        };
        match (resolved.body, value) {
            (SchemaType::Option { inner, .. }, SchemaValue::Option { inner: present }) => self
                .case(
                    value,
                    CaseKind::Option,
                    resolved.type_name,
                    None,
                    true,
                    |walker| match present {
                        Some(present) => (
                            Some("some".to_string()),
                            None,
                            Some(walker.typed(graph, inner, present, Region::Trusted)),
                        ),
                        None => (Some("none".to_string()), None, None),
                    },
                ),
            (SchemaType::List { element, .. }, SchemaValue::List { elements }) => {
                let token = self.token("list", &encode(value));
                let shown = (!elements.is_empty()).then(|| {
                    elements
                        .iter()
                        .map(|value| self.typed(graph, element, value, Region::Trusted))
                        .collect()
                });
                ReportNode::List(ListNode {
                    length: Some(elements.len() as u64),
                    token,
                    elements: shown,
                })
            }
            _ => self.typed(graph, ty, value, Region::Trusted),
        }
    }

    fn list(
        &mut self,
        value: &SchemaValue,
        elements: &[SchemaValue],
        byte_list: bool,
        mut render: impl FnMut(&mut Self, &SchemaValue) -> ReportNode,
    ) -> ReportNode {
        let shape = self.ctx.keep().shape;
        let token = self.token("list", &encode(value));
        let shown = (shape && !byte_list && !elements.is_empty())
            .then(|| elements.iter().map(|value| render(self, value)).collect());
        ReportNode::List(ListNode {
            length: shape.then_some(elements.len() as u64),
            token,
            elements: shown,
        })
    }

    fn map(
        &mut self,
        value: &SchemaValue,
        entries: &[(SchemaValue, SchemaValue)],
        mut render: impl FnMut(&mut Self, &SchemaValue, &SchemaValue) -> (ReportNode, ReportNode),
    ) -> ReportNode {
        let shape = self.ctx.keep().shape;
        let token = self.token("map", &encode(value));
        let shown = shape.then(|| {
            let mut sorted: Vec<_> = entries
                .iter()
                .map(|(key, value)| ((encode(key), encode(value)), key, value))
                .collect();
            sorted.sort_by(|a, b| a.0.cmp(&b.0));
            sorted
                .into_iter()
                .map(|(_, key, value)| {
                    let (key, value) = render(self, key, value);
                    MapEntryNode { key, value }
                })
                .collect()
        });
        ReportNode::Map(MapNode {
            length: shape.then_some(entries.len() as u64),
            token,
            entries: shown,
        })
    }

    /// A sum-type node: the case parts when `keep` is set, otherwise one `subtree` token.
    #[allow(clippy::too_many_arguments)]
    fn case(
        &mut self,
        value: &SchemaValue,
        case_kind: CaseKind,
        type_name: Option<String>,
        role: Option<KnownRole>,
        keep: bool,
        parts: impl FnOnce(&mut Self) -> (Option<String>, Option<u32>, Option<ReportNode>),
    ) -> ReportNode {
        if keep {
            let (case, case_index, payload) = parts(self);
            ReportNode::Case(CaseNode {
                case_kind,
                type_name,
                role,
                case,
                case_index,
                payload: payload.map(Box::new),
                tag_token: None,
                token: None,
            })
        } else {
            ReportNode::Case(CaseNode {
                case_kind,
                type_name,
                role,
                case: None,
                case_index: None,
                payload: None,
                tag_token: None,
                token: Some(self.token("subtree", &encode(value))),
            })
        }
    }
}

// -------------------------------------------------------------------------------------------
// Helpers
// -------------------------------------------------------------------------------------------

pub(super) struct Resolved<'g> {
    pub ids: Vec<&'g TypeId>,
    pub type_name: Option<String>,
    pub body: &'g SchemaType,
}

/// Follows `Ref` nodes to a body. `None` for a dangling or cyclic chain.
pub(super) fn resolve<'g>(graph: &'g SchemaGraph, ty: &'g SchemaType) -> Option<Resolved<'g>> {
    let mut ids = Vec::new();
    let mut type_name = None;
    let mut current = ty;
    while let SchemaType::Ref { id, .. } = current {
        if ids.len() >= MAX_REF_HOPS || ids.contains(&id) {
            return None;
        }
        let def = graph.lookup(id)?;
        if type_name.is_none() {
            type_name = Some(def.name.clone().unwrap_or_else(|| id.to_string()));
        }
        ids.push(id);
        current = &def.body;
    }
    Some(Resolved {
        ids,
        type_name,
        body: current,
    })
}

/// A non-empty list whose elements are all `U8`: a byte list whatever its declared type.
fn all_bytes(elements: &[SchemaValue]) -> bool {
    !elements.is_empty()
        && elements
            .iter()
            .all(|element| matches!(element, SchemaValue::U8(_)))
}

/// Whether a quantity's unit is one its type declares, as value validation requires. A unit
/// that the type does not declare is free text.
fn unit_declared(ty: &SchemaType, value: &SchemaValue) -> bool {
    match (ty, value) {
        (SchemaType::Quantity { spec, .. }, SchemaValue::Quantity(quantity)) => {
            if spec.allowed_suffixes.is_empty() {
                quantity.unit == spec.base_unit
            } else {
                spec.allowed_suffixes.contains(&quantity.unit)
            }
        }
        _ => false,
    }
}

fn is_u8_type(graph: &SchemaGraph, ty: &SchemaType) -> bool {
    matches!(
        resolve(graph, ty).map(|resolved| resolved.body),
        Some(SchemaType::U8 { .. })
    )
}

fn leaf_matches(ty: &SchemaType, value: &SchemaValue) -> bool {
    matches!(
        (ty, value),
        (SchemaType::Bool { .. }, SchemaValue::Bool(_))
            | (SchemaType::S8 { .. }, SchemaValue::S8(_))
            | (SchemaType::S16 { .. }, SchemaValue::S16(_))
            | (SchemaType::S32 { .. }, SchemaValue::S32(_))
            | (SchemaType::S64 { .. }, SchemaValue::S64(_))
            | (SchemaType::U8 { .. }, SchemaValue::U8(_))
            | (SchemaType::U16 { .. }, SchemaValue::U16(_))
            | (SchemaType::U32 { .. }, SchemaValue::U32(_))
            | (SchemaType::U64 { .. }, SchemaValue::U64(_))
            | (SchemaType::F32 { .. }, SchemaValue::F32(_))
            | (SchemaType::F64 { .. }, SchemaValue::F64(_))
            | (SchemaType::Char { .. }, SchemaValue::Char(_))
            | (SchemaType::String { .. }, SchemaValue::String(_))
            | (SchemaType::Text { .. }, SchemaValue::Text(_))
            | (SchemaType::Binary { .. }, SchemaValue::Binary(_))
            | (SchemaType::Path { .. }, SchemaValue::Path { .. })
            | (SchemaType::Url { .. }, SchemaValue::Url { .. })
            | (SchemaType::Uuid { .. }, SchemaValue::Uuid(_))
            | (SchemaType::Datetime { .. }, SchemaValue::Datetime { .. })
            | (SchemaType::Duration { .. }, SchemaValue::Duration(_))
            | (SchemaType::Quantity { .. }, SchemaValue::Quantity(_))
            | (SchemaType::Secret { .. }, SchemaValue::Secret(_))
            | (SchemaType::QuotaToken { .. }, SchemaValue::QuotaToken(_))
            | (
                SchemaType::PermissionCard { .. },
                SchemaValue::PermissionCard(_)
            )
            | (SchemaType::Stream { .. }, SchemaValue::Stream(_))
    )
}

fn result_matches(spec: &ResultSpec, result: &ResultValuePayload) -> bool {
    match result {
        ResultValuePayload::Ok { value } => spec.ok.is_some() == value.is_some(),
        ResultValuePayload::Err { value } => spec.err.is_some() == value.is_some(),
    }
}

fn payload_index(value: &SchemaValue) -> u32 {
    match value {
        SchemaValue::Variant(payload) => payload.case,
        _ => 0,
    }
}

fn float_json(value: f64) -> serde_json::Value {
    serde_json::Number::from_f64(value)
        .map(serde_json::Value::Number)
        .unwrap_or_else(|| serde_json::Value::String(value.to_string()))
}

/// Folds an ASCII case name for comparison: lower case, without `-` and `_`.
pub(super) fn normalized(name: &str) -> String {
    name.chars()
        .filter(|c| *c != '-' && *c != '_')
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

pub(super) fn unsigned(value: &SchemaValue) -> Option<u64> {
    match value {
        SchemaValue::U8(v) => Some(*v as u64),
        SchemaValue::U16(v) => Some(*v as u64),
        SchemaValue::U32(v) => Some(*v as u64),
        SchemaValue::U64(v) => Some(*v),
        _ => None,
    }
}

fn redacted(reason: RedactionReason) -> ReportNode {
    ReportNode::Redacted(RedactedNode { reason })
}

fn type_description(schema: &SchemaGraph) -> ReportNode {
    ReportNode::TypeDescription(TypeDescriptionNode {
        description: describe_schema(schema),
    })
}

pub(super) fn record(type_name: Option<String>, fields: Vec<ReportField>) -> ReportNode {
    ReportNode::Record(RecordNode {
        type_name,
        role: None,
        fields,
    })
}

pub(super) fn kept_case(
    case_kind: CaseKind,
    type_name: Option<String>,
    case: &str,
    case_index: Option<u32>,
    payload: Option<ReportNode>,
) -> ReportNode {
    ReportNode::Case(CaseNode {
        case_kind,
        type_name,
        role: None,
        case: Some(case.to_string()),
        case_index,
        payload: payload.map(Box::new),
        tag_token: None,
        token: None,
    })
}

pub(super) fn number(value: u64) -> ReportNode {
    ReportNode::Number(NumberNode { value })
}

pub(super) fn text(value: impl Into<String>) -> ReportNode {
    ReportNode::Text(TextNode {
        value: value.into(),
    })
}

pub(super) fn flag(value: bool) -> ReportNode {
    ReportNode::Flag(crate::oplog_export::model::FlagNode { value })
}

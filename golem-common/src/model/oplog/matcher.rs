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

pub use super::PublicOplogEntry;
use crate::model::lucene::{LeafQuery, Query};
use crate::model::oplog::{
    PublicAgentInvocation, PublicAttribute, PublicAttributeValue, StringAttributeValue,
};
use crate::schema::graph::SchemaGraph;
use crate::schema::schema_type::SchemaType;
use crate::schema::schema_value::{ResultValuePayload, SchemaValue};

impl PublicOplogEntry {
    pub fn matches(&self, query: &Query) -> bool {
        fn matches_impl(entry: &PublicOplogEntry, query: &Query, field_stack: &[String]) -> bool {
            match query {
                Query::Or { queries } => queries
                    .iter()
                    .any(|query| matches_impl(entry, query, field_stack)),
                Query::And { queries } => queries
                    .iter()
                    .all(|query| matches_impl(entry, query, field_stack)),
                Query::Not { query } => !matches_impl(entry, query, field_stack),
                Query::Regex { .. } => {
                    entry.matches_leaf_query(field_stack, &query.clone().try_into().unwrap())
                }
                Query::Term { .. } => {
                    entry.matches_leaf_query(field_stack, &query.clone().try_into().unwrap())
                }
                Query::Phrase { .. } => {
                    entry.matches_leaf_query(field_stack, &query.clone().try_into().unwrap())
                }
                Query::Field { field, query } => {
                    let mut new_stack: Vec<String> = field_stack.to_vec();
                    let parts: Vec<String> = field.split(".").map(|s| s.to_string()).collect();
                    new_stack.extend(parts);
                    matches_impl(entry, query, &new_stack)
                }
            }
        }

        matches_impl(self, query, &[])
    }

    fn string_match(s: &str, path: &[String], query_path: &[String], query: &LeafQuery) -> bool {
        let lowercase_path = path
            .iter()
            .map(|s| s.to_lowercase())
            .collect::<Vec<String>>();
        let lowercase_query_path = query_path
            .iter()
            .map(|s| s.to_lowercase())
            .collect::<Vec<String>>();
        if lowercase_path == lowercase_query_path || query_path.is_empty() {
            query.matches(s)
        } else {
            false
        }
    }

    fn span_attribute_match(
        attributes: &Vec<PublicAttribute>,
        path_stack: &[String],
        query_path: &[String],
        query: &LeafQuery,
    ) -> bool {
        for attr in attributes {
            let key = &attr.key;
            let value = &attr.value;
            let mut new_path: Vec<String> = path_stack.to_vec();
            new_path.push(key.clone());

            match value {
                PublicAttributeValue::String(StringAttributeValue { value }) => {
                    if Self::string_match(value, &new_path, query_path, query) {
                        return true;
                    }
                }
            }
        }
        false
    }

    fn matches_leaf_query(&self, query_path: &[String], query: &LeafQuery) -> bool {
        match self {
            PublicOplogEntry::Create(_params) => {
                Self::string_match("create", &[], query_path, query)
            }
            PublicOplogEntry::Start(params) => {
                Self::string_match("Start", &[], query_path, query)
                    || Self::string_match("start", &[], query_path, query)
                    || Self::string_match("imported-function", &[], query_path, query)
                    || Self::string_match(&params.function_name, &[], query_path, query)
                    || params
                        .request
                        .as_ref()
                        .map(|req| {
                            Self::match_typed_schema_value(
                                req,
                                &["request".to_string()],
                                query_path,
                                query,
                            )
                        })
                        .unwrap_or(false)
            }
            PublicOplogEntry::End(params) => {
                Self::string_match("End", &[], query_path, query)
                    || Self::string_match("end", &[], query_path, query)
                    || params
                        .response
                        .as_ref()
                        .map(|resp| {
                            Self::match_typed_schema_value(
                                resp,
                                &["response".to_string()],
                                query_path,
                                query,
                            )
                        })
                        .unwrap_or(false)
            }
            PublicOplogEntry::Cancelled(params) => {
                Self::string_match("Cancelled", &[], query_path, query)
                    || Self::string_match("cancelled", &[], query_path, query)
                    || params
                        .partial
                        .as_ref()
                        .map(|partial| {
                            Self::match_typed_schema_value(
                                partial,
                                &["partial".to_string()],
                                query_path,
                                query,
                            )
                        })
                        .unwrap_or(false)
            }
            PublicOplogEntry::AgentInvocationStarted(params) => {
                Self::string_match("agentinvocationstarted", &[], query_path, query)
                    || Self::string_match("invoke", &[], query_path, query)
                    || Self::string_match("agent-invocation-started", &[], query_path, query)
                    || match &params.invocation {
                        PublicAgentInvocation::AgentInitialization(inv_params) => {
                            Self::string_match("agent-initialization", &[], query_path, query)
                                || Self::string_match(
                                    &inv_params.idempotency_key.value,
                                    &[],
                                    query_path,
                                    query,
                                )
                        }
                        PublicAgentInvocation::AgentMethodInvocation(inv_params) => {
                            Self::string_match("agent-method-invocation", &[], query_path, query)
                                || Self::string_match(
                                    &inv_params.method_name,
                                    &[],
                                    query_path,
                                    query,
                                )
                                || Self::string_match(
                                    &inv_params.idempotency_key.value,
                                    &[],
                                    query_path,
                                    query,
                                )
                        }
                        PublicAgentInvocation::SaveSnapshot(_) => {
                            Self::string_match("save-snapshot", &[], query_path, query)
                        }
                        PublicAgentInvocation::LoadSnapshot(_) => {
                            Self::string_match("load-snapshot", &[], query_path, query)
                        }
                        PublicAgentInvocation::ProcessOplogEntries(inv_params) => {
                            Self::string_match("process-oplog-entries", &[], query_path, query)
                                || Self::string_match(
                                    &inv_params.idempotency_key.value,
                                    &[],
                                    query_path,
                                    query,
                                )
                        }
                        PublicAgentInvocation::ManualUpdate(inv_params) => Self::string_match(
                            &inv_params.target_revision.to_string(),
                            &[],
                            query_path,
                            query,
                        ),
                    }
            }
            PublicOplogEntry::AgentInvocationFinished(params) => {
                Self::string_match("agentinvocationfinished", &[], query_path, query)
                    || Self::string_match("invoke", &[], query_path, query)
                    || Self::string_match("agent-invocation-finished", &[], query_path, query)
                    || Self::string_match(&params.consumed_fuel.to_string(), &[], query_path, query)
            }
            PublicOplogEntry::Suspend(_params) => {
                Self::string_match("suspend", &[], query_path, query)
            }
            PublicOplogEntry::Error(params) => {
                Self::string_match("error", &[], query_path, query)
                    || Self::string_match(&params.error, &[], query_path, query)
            }
            PublicOplogEntry::NoOp(_params) => Self::string_match("noop", &[], query_path, query),
            PublicOplogEntry::Jump(_params) => Self::string_match("jump", &[], query_path, query),
            PublicOplogEntry::Interrupted(_params) => {
                Self::string_match("interrupted", &[], query_path, query)
            }
            PublicOplogEntry::Exited(_params) => {
                Self::string_match("exited", &[], query_path, query)
            }
            PublicOplogEntry::BeginAtomicRegion(_params) => {
                Self::string_match("beginatomicregion", &[], query_path, query)
                    || Self::string_match("begin-atomic-region", &[], query_path, query)
            }
            PublicOplogEntry::EndAtomicRegion(_params) => {
                Self::string_match("endatomicregion", &[], query_path, query)
                    || Self::string_match("end-atomic-region", &[], query_path, query)
            }

            PublicOplogEntry::PendingAgentInvocation(params) => {
                Self::string_match("pendingagentinvocation", &[], query_path, query)
                    || Self::string_match("invoke", &[], query_path, query)
                    || Self::string_match("pending-agent-invocation", &[], query_path, query)
                    || match &params.invocation {
                        PublicAgentInvocation::AgentInitialization(params) => {
                            Self::string_match("agent-initialization", &[], query_path, query)
                                || Self::string_match(
                                    &params.idempotency_key.value,
                                    &[],
                                    query_path,
                                    query,
                                )
                        }
                        PublicAgentInvocation::AgentMethodInvocation(params) => {
                            Self::string_match("agent-method-invocation", &[], query_path, query)
                                || Self::string_match(&params.method_name, &[], query_path, query)
                                || Self::string_match(
                                    &params.idempotency_key.value,
                                    &[],
                                    query_path,
                                    query,
                                )
                        }
                        PublicAgentInvocation::SaveSnapshot(_) => {
                            Self::string_match("save-snapshot", &[], query_path, query)
                        }
                        PublicAgentInvocation::LoadSnapshot(_) => {
                            Self::string_match("load-snapshot", &[], query_path, query)
                        }
                        PublicAgentInvocation::ProcessOplogEntries(params) => {
                            Self::string_match("process-oplog-entries", &[], query_path, query)
                                || Self::string_match(
                                    &params.idempotency_key.value,
                                    &[],
                                    query_path,
                                    query,
                                )
                        }
                        PublicAgentInvocation::ManualUpdate(params) => Self::string_match(
                            &params.target_revision.to_string(),
                            &[],
                            query_path,
                            query,
                        ),
                    }
            }
            PublicOplogEntry::PendingUpdate(params) => {
                Self::string_match("pendingupdate", &[], query_path, query)
                    || Self::string_match("pending-update", &[], query_path, query)
                    || Self::string_match("update", &[], query_path, query)
                    || Self::string_match(
                        &params.target_revision.to_string(),
                        &[],
                        query_path,
                        query,
                    )
            }
            PublicOplogEntry::SuccessfulUpdate(params) => {
                Self::string_match("successfulupdate", &[], query_path, query)
                    || Self::string_match("successful-update", &[], query_path, query)
                    || Self::string_match("update", &[], query_path, query)
                    || Self::string_match(
                        &params.target_revision.to_string(),
                        &[],
                        query_path,
                        query,
                    )
            }
            PublicOplogEntry::FailedUpdate(params) => {
                Self::string_match("failedupdate", &[], query_path, query)
                    || Self::string_match("failed-update", &[], query_path, query)
                    || Self::string_match("update", &[], query_path, query)
                    || Self::string_match(
                        &params.target_revision.to_string(),
                        &[],
                        query_path,
                        query,
                    )
                    || params
                        .details
                        .as_ref()
                        .map(|details| Self::string_match(details, &[], query_path, query))
                        .unwrap_or(false)
            }
            PublicOplogEntry::GrowMemory(_params) => {
                Self::string_match("growmemory", &[], query_path, query)
                    || Self::string_match("grow-memory", &[], query_path, query)
            }
            PublicOplogEntry::FilesystemStorageUsageUpdate(_params) => {
                Self::string_match("filesystemstorageusageupdate", &[], query_path, query)
                    || Self::string_match("filesystem-storage-usage-update", &[], query_path, query)
            }
            PublicOplogEntry::CreateResource(_params) => {
                Self::string_match("createresource", &[], query_path, query)
                    || Self::string_match("create-resource", &[], query_path, query)
            }
            PublicOplogEntry::DropResource(_params) => {
                Self::string_match("dropresource", &[], query_path, query)
                    || Self::string_match("drop-resource", &[], query_path, query)
            }
            PublicOplogEntry::Log(params) => {
                Self::string_match("log", &[], query_path, query)
                    || Self::string_match(&params.context, &[], query_path, query)
                    || Self::string_match(&params.message, &[], query_path, query)
            }
            PublicOplogEntry::Restart(_params) => {
                Self::string_match("restart", &[], query_path, query)
            }
            PublicOplogEntry::ActivatePlugin(_params) => {
                Self::string_match("activateplugin", &[], query_path, query)
                    || Self::string_match("activate-plugin", &[], query_path, query)
            }
            PublicOplogEntry::DeactivatePlugin(_params) => {
                Self::string_match("deactivateplugin", &[], query_path, query)
                    || Self::string_match("deactivate-plugin", &[], query_path, query)
            }
            PublicOplogEntry::Revert(_params) => {
                Self::string_match("revert", &[], query_path, query)
            }
            PublicOplogEntry::CancelPendingInvocation(params) => {
                Self::string_match("cancel", &[], query_path, query)
                    || Self::string_match("cancel-invocation", &[], query_path, query)
                    || Self::string_match(&params.idempotency_key.value, &[], query_path, query)
            }
            PublicOplogEntry::StartSpan(params) => {
                Self::string_match("startspan", &[], query_path, query)
                    || Self::string_match("start-span", &[], query_path, query)
                    || Self::string_match(&params.span_id.to_string(), &[], query_path, query)
                    || Self::string_match(
                        &params
                            .parent_id
                            .as_ref()
                            .map(|id| id.to_string())
                            .unwrap_or_default(),
                        &[],
                        query_path,
                        query,
                    )
                    || Self::string_match(
                        &params
                            .linked_context
                            .as_ref()
                            .map(|id| id.to_string())
                            .unwrap_or_default(),
                        &[],
                        query_path,
                        query,
                    )
                    || Self::span_attribute_match(&params.attributes, &[], query_path, query)
            }
            PublicOplogEntry::FinishSpan(params) => {
                Self::string_match("finishspan", &[], query_path, query)
                    || Self::string_match("finish-span", &[], query_path, query)
                    || Self::string_match(&params.span_id.to_string(), &[], query_path, query)
            }
            PublicOplogEntry::SetSpanAttribute(params) => {
                let attributes = vec![PublicAttribute {
                    key: params.key.clone(),
                    value: params.value.clone(),
                }];
                Self::string_match("setspanattribute", &[], query_path, query)
                    || Self::string_match("set-span-attribute", &[], query_path, query)
                    || Self::string_match(&params.key, &[], query_path, query)
                    || Self::span_attribute_match(&attributes, &[], query_path, query)
            }
            PublicOplogEntry::ChangePersistenceLevel(_params) => {
                Self::string_match("changepersistencelevel", &[], query_path, query)
                    || Self::string_match("change-persistence-level", &[], query_path, query)
                    || Self::string_match("persistence-level", &[], query_path, query)
            }
            PublicOplogEntry::BeginRemoteTransaction(_params) => {
                Self::string_match("beginremotetransaction", &[], query_path, query)
                    || Self::string_match("begin-remote-transaction", &[], query_path, query)
            }
            PublicOplogEntry::PreCommitRemoteTransaction(_params) => {
                Self::string_match("precommitremotetransaction", &[], query_path, query)
                    || Self::string_match("pre-commit-remote-transaction", &[], query_path, query)
            }
            PublicOplogEntry::PreRollbackRemoteTransaction(_params) => {
                Self::string_match("prerollbackremotetransaction", &[], query_path, query)
                    || Self::string_match("pre-rollback-remote-transaction", &[], query_path, query)
            }
            PublicOplogEntry::CommittedRemoteTransaction(_params) => {
                Self::string_match("committedremotetransaction", &[], query_path, query)
                    || Self::string_match("committed-remote-transaction", &[], query_path, query)
            }
            PublicOplogEntry::RolledBackRemoteTransaction(_params) => {
                Self::string_match("rolledbackremotetransaction", &[], query_path, query)
                    || Self::string_match("rolled-back-remote-transaction", &[], query_path, query)
            }
            PublicOplogEntry::Snapshot(_params) => {
                Self::string_match("snapshot", &[], query_path, query)
            }
            PublicOplogEntry::OplogProcessorCheckpoint(_params) => {
                Self::string_match("oplogprocessorcheckpoint", &[], query_path, query)
                    || Self::string_match("oplog-processor-checkpoint", &[], query_path, query)
            }
            PublicOplogEntry::SetRetryPolicy(_params) => {
                Self::string_match("setretrypolicy", &[], query_path, query)
                    || Self::string_match("set-retry-policy", &[], query_path, query)
            }
            PublicOplogEntry::RemoveRetryPolicy(_params) => {
                Self::string_match("removeretrypolicy", &[], query_path, query)
                    || Self::string_match("remove-retry-policy", &[], query_path, query)
            }
        }
    }

    /// Walks a schema-native [`TypedSchemaValue`] for the public-oplog text
    /// matcher, recursing over the paired `(SchemaGraph, SchemaType,
    /// SchemaValue)` directly. Names (record fields, variant/enum/flags cases,
    /// union tags) come from the schema; the value tree carries only payloads.
    fn match_typed_schema_value(
        value: &crate::schema::TypedSchemaValue,
        path_stack: &[String],
        query_path: &[String],
        query: &LeafQuery,
    ) -> bool {
        Self::match_schema_value(
            value.graph(),
            value.root_type(),
            value.value(),
            path_stack,
            query_path,
            query,
        )
    }

    /// Follows [`SchemaType::Ref`] chains to the underlying definition body so
    /// composites behind named refs match. Bounded to avoid looping on a
    /// malformed graph.
    fn resolve_type<'a>(graph: &'a SchemaGraph, mut ty: &'a SchemaType) -> &'a SchemaType {
        let mut guard = 0;
        while let SchemaType::Ref { id, .. } = ty {
            match graph.lookup(id) {
                Some(def) => {
                    ty = &def.body;
                    guard += 1;
                    if guard > 1024 {
                        break;
                    }
                }
                None => break,
            }
        }
        ty
    }

    fn match_schema_value(
        graph: &SchemaGraph,
        ty: &SchemaType,
        value: &SchemaValue,
        path_stack: &[String],
        query_path: &[String],
        query: &LeafQuery,
    ) -> bool {
        let ty = Self::resolve_type(graph, ty);
        match value {
            SchemaValue::Bool(v) => {
                Self::string_match(&v.to_string(), path_stack, query_path, query)
            }
            SchemaValue::S8(v) => Self::string_match(&v.to_string(), path_stack, query_path, query),
            SchemaValue::S16(v) => {
                Self::string_match(&v.to_string(), path_stack, query_path, query)
            }
            SchemaValue::S32(v) => {
                Self::string_match(&v.to_string(), path_stack, query_path, query)
            }
            SchemaValue::S64(v) => {
                Self::string_match(&v.to_string(), path_stack, query_path, query)
            }
            SchemaValue::U8(v) => Self::string_match(&v.to_string(), path_stack, query_path, query),
            SchemaValue::U16(v) => {
                Self::string_match(&v.to_string(), path_stack, query_path, query)
            }
            SchemaValue::U32(v) => {
                Self::string_match(&v.to_string(), path_stack, query_path, query)
            }
            SchemaValue::U64(v) => {
                Self::string_match(&v.to_string(), path_stack, query_path, query)
            }
            SchemaValue::F32(v) => {
                Self::string_match(&v.to_string(), path_stack, query_path, query)
            }
            SchemaValue::F64(v) => {
                Self::string_match(&v.to_string(), path_stack, query_path, query)
            }
            SchemaValue::Char(v) => {
                Self::string_match(&v.to_string(), path_stack, query_path, query)
            }
            SchemaValue::String(v) => Self::string_match(v, path_stack, query_path, query),
            SchemaValue::Record { fields } => {
                if let SchemaType::Record {
                    fields: field_types,
                    ..
                } = ty
                {
                    fields.len() == field_types.len()
                        && fields.iter().zip(field_types.iter()).any(|(v, f)| {
                            let mut new_path: Vec<String> = path_stack.to_vec();
                            new_path.push(f.name.clone());
                            Self::match_schema_value(
                                graph, &f.body, v, &new_path, query_path, query,
                            )
                        })
                } else {
                    false
                }
            }
            SchemaValue::Tuple { elements } => {
                if let SchemaType::Tuple {
                    elements: elem_types,
                    ..
                } = ty
                {
                    elements.len() == elem_types.len()
                        && elements.iter().zip(elem_types.iter()).enumerate().any(
                            |(idx, (v, t))| {
                                let mut new_path: Vec<String> = path_stack.to_vec();
                                new_path.push(idx.to_string());
                                Self::match_schema_value(graph, t, v, &new_path, query_path, query)
                            },
                        )
                } else {
                    false
                }
            }
            SchemaValue::List { elements } => {
                if let SchemaType::List { element, .. } = ty {
                    elements.iter().any(|v| {
                        Self::match_schema_value(graph, element, v, path_stack, query_path, query)
                    })
                } else {
                    false
                }
            }
            SchemaValue::FixedList { elements } => {
                if let SchemaType::FixedList { element, .. } = ty {
                    elements.iter().any(|v| {
                        Self::match_schema_value(graph, element, v, path_stack, query_path, query)
                    })
                } else {
                    false
                }
            }
            SchemaValue::Map { entries } => {
                if let SchemaType::Map {
                    key, value: val_ty, ..
                } = ty
                {
                    entries.iter().any(|(k, v)| {
                        Self::match_schema_value(graph, key, k, path_stack, query_path, query)
                            || Self::match_schema_value(
                                graph, val_ty, v, path_stack, query_path, query,
                            )
                    })
                } else {
                    false
                }
            }
            SchemaValue::Option { inner } => match (inner, ty) {
                (
                    Some(v),
                    SchemaType::Option {
                        inner: inner_ty, ..
                    },
                ) => Self::match_schema_value(graph, inner_ty, v, path_stack, query_path, query),
                _ => false,
            },
            SchemaValue::Result(payload) => {
                if let SchemaType::Result { spec, .. } = ty {
                    match payload {
                        ResultValuePayload::Ok { value: Some(v) } => spec
                            .ok
                            .as_ref()
                            .map(|ok_ty| {
                                let mut new_path: Vec<String> = path_stack.to_vec();
                                new_path.push("ok".to_string());
                                Self::match_schema_value(
                                    graph, ok_ty, v, &new_path, query_path, query,
                                )
                            })
                            .unwrap_or(false),
                        ResultValuePayload::Err { value: Some(v) } => spec
                            .err
                            .as_ref()
                            .map(|err_ty| {
                                let mut new_path: Vec<String> = path_stack.to_vec();
                                new_path.push("err".to_string());
                                Self::match_schema_value(
                                    graph, err_ty, v, &new_path, query_path, query,
                                )
                            })
                            .unwrap_or(false),
                        _ => false,
                    }
                } else {
                    false
                }
            }
            SchemaValue::Variant(payload) => {
                if let SchemaType::Variant { cases, .. } = ty {
                    match cases.get(payload.case as usize) {
                        Some(case) => {
                            let case_name_matches =
                                Self::string_match(&case.name, path_stack, query_path, query);
                            let payload_matches = match (&payload.payload, &case.payload) {
                                (Some(v), Some(case_ty)) => {
                                    let mut new_path: Vec<String> = path_stack.to_vec();
                                    new_path.push(case.name.clone());
                                    Self::match_schema_value(
                                        graph, case_ty, v, &new_path, query_path, query,
                                    )
                                }
                                _ => false,
                            };
                            case_name_matches || payload_matches
                        }
                        None => false,
                    }
                } else {
                    false
                }
            }
            SchemaValue::Enum { case } => {
                if let SchemaType::Enum { cases, .. } = ty {
                    match cases.get(*case as usize) {
                        Some(name) => Self::string_match(name, path_stack, query_path, query),
                        None => false,
                    }
                } else {
                    false
                }
            }
            SchemaValue::Flags { bits } => {
                if let SchemaType::Flags { flags, .. } = ty {
                    bits.iter()
                        .enumerate()
                        .filter_map(|(idx, set)| if *set { flags.get(idx) } else { None })
                        .any(|name| Self::string_match(name, path_stack, query_path, query))
                } else {
                    false
                }
            }
            SchemaValue::Union(payload) => {
                let body_matches = if let SchemaType::Union { spec, .. } = ty {
                    spec.branches
                        .iter()
                        .find(|b| b.tag == payload.tag)
                        .map(|b| {
                            let mut new_path: Vec<String> = path_stack.to_vec();
                            new_path.push(payload.tag.clone());
                            Self::match_schema_value(
                                graph,
                                &b.body,
                                &payload.body,
                                &new_path,
                                query_path,
                                query,
                            )
                        })
                        .unwrap_or(false)
                } else {
                    false
                };
                Self::string_match(&payload.tag, path_stack, query_path, query) || body_matches
            }
            SchemaValue::Text(payload) => {
                Self::string_match(&payload.text, path_stack, query_path, query)
                    || payload
                        .language
                        .as_ref()
                        .map(|l| Self::string_match(l, path_stack, query_path, query))
                        .unwrap_or(false)
            }
            SchemaValue::Binary(payload) => payload
                .mime_type
                .as_ref()
                .map(|m| Self::string_match(m, path_stack, query_path, query))
                .unwrap_or(false),
            SchemaValue::Path { path } => Self::string_match(path, path_stack, query_path, query),
            SchemaValue::Url { url } => Self::string_match(url, path_stack, query_path, query),
            SchemaValue::Datetime { value } => {
                Self::string_match(&value.to_rfc3339(), path_stack, query_path, query)
            }
            SchemaValue::Duration(payload) => Self::string_match(
                &payload.nanoseconds.to_string(),
                path_stack,
                query_path,
                query,
            ),
            SchemaValue::Quantity(quantity) => {
                Self::string_match(&quantity.unit, path_stack, query_path, query)
                    || Self::string_match(
                        &quantity.mantissa.to_string(),
                        path_stack,
                        query_path,
                        query,
                    )
            }
            SchemaValue::Secret(payload) => {
                Self::string_match(&payload.secret_ref, path_stack, query_path, query)
            }
            SchemaValue::QuotaToken(payload) => {
                Self::string_match(&payload.resource_name, path_stack, query_path, query)
            }
        }
    }
}

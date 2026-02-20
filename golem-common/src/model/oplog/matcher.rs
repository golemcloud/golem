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

pub use super::PublicOplogEntry;
use crate::model::lucene::{LeafQuery, Query};
use crate::model::oplog::{
    PublicAttribute, PublicAttributeValue, PublicWorkerInvocation, StringAttributeValue,
};
use golem_wasm::analysis::{AnalysedType, NameOptionTypePair};
use golem_wasm::{IntoValueAndType, Value, ValueAndType};

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

            let vnt = match value {
                PublicAttributeValue::String(StringAttributeValue { value }) => {
                    value.clone().into_value_and_type()
                }
            };

            if Self::match_value(&vnt, &new_path, query_path, query) {
                return true;
            }
        }
        false
    }

    fn matches_leaf_query(&self, query_path: &[String], query: &LeafQuery) -> bool {
        match self {
            PublicOplogEntry::Create(_params) => {
                Self::string_match("create", &[], query_path, query)
            }
            PublicOplogEntry::HostCall(params) => {
                Self::string_match("HostCall", &[], query_path, query)
                    || Self::string_match("host-call", &[], query_path, query)
                    || Self::string_match("imported-function", &[], query_path, query)
                    || Self::string_match(&params.function_name, &[], query_path, query)
                    || Self::match_value(&params.request, &[], query_path, query)
                    || Self::match_value(&params.response, &[], query_path, query)
            }
            PublicOplogEntry::ExportedFunctionInvoked(params) => {
                Self::string_match("exportedfunctioninvoked", &[], query_path, query)
                    || Self::string_match("exported-function-invoked", &[], query_path, query)
                    || Self::string_match("exported-function", &[], query_path, query)
                    || Self::string_match(&params.function_name, &[], query_path, query)
                    || params
                        .request
                        .iter()
                        .any(|v| Self::match_value(v, &[], query_path, query))
                    || Self::string_match(&params.idempotency_key.value, &[], query_path, query)
            }
            PublicOplogEntry::ExportedFunctionCompleted(params) => {
                Self::string_match("exportedfunctioncompleted", &[], query_path, query)
                    || Self::string_match("exported-function-completed", &[], query_path, query)
                    || Self::string_match("exported-function", &[], query_path, query)
                    || match &params.response {
                        Some(response) => Self::match_value(response, &[], query_path, query),
                        None => false,
                    }
                // TODO: should we store function name and idempotency key in ExportedFunctionCompleted?
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
            PublicOplogEntry::ChangeRetryPolicy(_params) => {
                Self::string_match("changeretrypolicy", &[], query_path, query)
                    || Self::string_match("change-retry-policy", &[], query_path, query)
            }
            PublicOplogEntry::BeginAtomicRegion(_params) => {
                Self::string_match("beginatomicregion", &[], query_path, query)
                    || Self::string_match("begin-atomic-region", &[], query_path, query)
            }
            PublicOplogEntry::EndAtomicRegion(_params) => {
                Self::string_match("endatomicregion", &[], query_path, query)
                    || Self::string_match("end-atomic-region", &[], query_path, query)
            }
            PublicOplogEntry::BeginRemoteWrite(_params) => {
                Self::string_match("beginremotewrite", &[], query_path, query)
                    || Self::string_match("begin-remote-write", &[], query_path, query)
            }
            PublicOplogEntry::EndRemoteWrite(_params) => {
                Self::string_match("endremotewrite", &[], query_path, query)
                    || Self::string_match("end-remote-write", &[], query_path, query)
            }
            PublicOplogEntry::PendingWorkerInvocation(params) => {
                Self::string_match("pendingworkerinvocation", &[], query_path, query)
                    || Self::string_match("pending-worker-invocation", &[], query_path, query)
                    || match &params.invocation {
                        PublicWorkerInvocation::ExportedFunction(params) => {
                            Self::string_match(&params.full_function_name, &[], query_path, query)
                                || Self::string_match(
                                    &params.idempotency_key.value,
                                    &[],
                                    query_path,
                                    query,
                                )
                                || params
                                    .function_input
                                    .as_ref()
                                    .map(|params| {
                                        params
                                            .iter()
                                            .any(|v| Self::match_value(v, &[], query_path, query))
                                    })
                                    .unwrap_or(false)
                        }
                        PublicWorkerInvocation::ManualUpdate(params) => Self::string_match(
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
        }
    }

    fn match_value(
        value: &ValueAndType,
        path_stack: &[String],
        query_path: &[String],
        query: &LeafQuery,
    ) -> bool {
        match (&value.value, &value.typ) {
            (Value::Bool(value), _) => {
                Self::string_match(&value.to_string(), path_stack, query_path, query)
            }
            (Value::U8(value), _) => {
                Self::string_match(&value.to_string(), path_stack, query_path, query)
            }
            (Value::U16(value), _) => {
                Self::string_match(&value.to_string(), path_stack, query_path, query)
            }
            (Value::U32(value), _) => {
                Self::string_match(&value.to_string(), path_stack, query_path, query)
            }
            (Value::U64(value), _) => {
                Self::string_match(&value.to_string(), path_stack, query_path, query)
            }
            (Value::S8(value), _) => {
                Self::string_match(&value.to_string(), path_stack, query_path, query)
            }
            (Value::S16(value), _) => {
                Self::string_match(&value.to_string(), path_stack, query_path, query)
            }
            (Value::S32(value), _) => {
                Self::string_match(&value.to_string(), path_stack, query_path, query)
            }
            (Value::S64(value), _) => {
                Self::string_match(&value.to_string(), path_stack, query_path, query)
            }
            (Value::F32(value), _) => {
                Self::string_match(&value.to_string(), path_stack, query_path, query)
            }
            (Value::F64(value), _) => {
                Self::string_match(&value.to_string(), path_stack, query_path, query)
            }
            (Value::Char(value), _) => {
                Self::string_match(&value.to_string(), path_stack, query_path, query)
            }
            (Value::String(value), _) => {
                Self::string_match(&value.to_string(), path_stack, query_path, query)
            }
            (Value::List(elems), AnalysedType::List(list)) => elems.iter().any(|v| {
                Self::match_value(
                    &ValueAndType::new(v.clone(), (*list.inner).clone()),
                    path_stack,
                    query_path,
                    query,
                )
            }),
            (Value::Tuple(elems), AnalysedType::Tuple(tuple)) => {
                if elems.len() != tuple.items.len() {
                    false
                } else {
                    elems
                        .iter()
                        .zip(tuple.items.iter())
                        .enumerate()
                        .any(|(idx, (v, t))| {
                            let mut new_path: Vec<String> = path_stack.to_vec();
                            new_path.push(idx.to_string());
                            Self::match_value(
                                &ValueAndType::new(v.clone(), t.clone()),
                                &new_path,
                                query_path,
                                query,
                            )
                        })
                }
            }
            (Value::Record(fields), AnalysedType::Record(record)) => {
                if fields.len() != record.fields.len() {
                    false
                } else {
                    fields.iter().zip(record.fields.iter()).any(|(v, t)| {
                        let mut new_path: Vec<String> = path_stack.to_vec();
                        new_path.push(t.name.clone());
                        Self::match_value(
                            &ValueAndType::new(v.clone(), t.typ.clone()),
                            &new_path,
                            path_stack,
                            query,
                        )
                    })
                }
            }
            (
                Value::Variant {
                    case_value,
                    case_idx,
                },
                AnalysedType::Variant(variant),
            ) => {
                let case = variant.cases.get(*case_idx as usize);
                match (case_value, case) {
                    (
                        Some(value),
                        Some(NameOptionTypePair {
                            typ: Some(typ),
                            name,
                        }),
                    ) => {
                        let mut new_path: Vec<String> = path_stack.to_vec();
                        new_path.push(name.clone());
                        Self::match_value(
                            &ValueAndType::new((**value).clone(), typ.clone()),
                            &new_path,
                            query_path,
                            query,
                        )
                    }
                    _ => false,
                }
            }
            (Value::Enum(value), AnalysedType::Enum(typ)) => {
                if let Some(case) = typ.cases.get(*value as usize) {
                    Self::string_match(case, path_stack, query_path, query)
                } else {
                    false
                }
            }
            (Value::Flags(bitmap), AnalysedType::Flags(flags)) => {
                let names = bitmap
                    .iter()
                    .enumerate()
                    .filter_map(|(idx, set)| if *set { flags.names.get(idx) } else { None })
                    .collect::<Vec<_>>();
                names
                    .iter()
                    .any(|name| Self::string_match(name, path_stack, query_path, query))
            }
            (Value::Option(Some(value)), AnalysedType::Option(typ)) => Self::match_value(
                &ValueAndType::new((**value).clone(), (*typ.inner).clone()),
                path_stack,
                query_path,
                query,
            ),
            (Value::Result(value), AnalysedType::Result(typ)) => match value {
                Ok(Some(value)) if typ.ok.is_some() => {
                    let mut new_path = path_stack.to_vec();
                    new_path.push("ok".to_string());
                    Self::match_value(
                        &ValueAndType::new(
                            (**value).clone(),
                            (**(typ.ok.as_ref().unwrap())).clone(),
                        ),
                        &new_path,
                        query_path,
                        query,
                    )
                }
                Err(Some(value)) if typ.err.is_some() => {
                    let mut new_path = path_stack.to_vec();
                    new_path.push("err".to_string());
                    Self::match_value(
                        &ValueAndType::new(
                            (**value).clone(),
                            (**(typ.err.as_ref().unwrap())).clone(),
                        ),
                        &new_path,
                        query_path,
                        query,
                    )
                }
                _ => false,
            },
            (Value::Handle { .. }, _) => false,
            _ => false,
        }
    }
}

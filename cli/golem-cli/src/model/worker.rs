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

use crate::command::shared_args::StreamArgs;
use crate::fuzzy::{Error, FuzzySearch, Match};
use crate::model::component::{show_exported_functions, ComponentNameMatchKind};
use crate::model::environment::{
    EnvironmentReference, ResolvedEnvironmentIdentity, ResolvedEnvironmentIdentitySource,
};
use clap_verbosity_flag::Verbosity;
use colored::control::SHOULD_COLORIZE;
use golem_common::model::account::AccountId;
use golem_common::model::component::{ComponentName, ComponentRevision};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::trim_date::TrimDateTime;
use golem_common::model::worker::UpdateRecord;
use golem_common::model::{Timestamp, WorkerId, WorkerResourceDescription, WorkerStatus};
use golem_wasm::analysis::AnalysedExport;
use rib::{ParsedFunctionName, ParsedFunctionReference};
use serde_derive::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fmt::{Display, Formatter};
use std::str::FromStr;

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct WorkerName(pub String);

impl From<&str> for WorkerName {
    fn from(name: &str) -> Self {
        WorkerName(name.to_string())
    }
}

impl From<String> for WorkerName {
    fn from(name: String) -> Self {
        WorkerName(name)
    }
}

impl Display for WorkerName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AgentUpdateMode {
    Automatic,
    Manual,
}

impl Display for AgentUpdateMode {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentUpdateMode::Automatic => {
                write!(f, "auto")
            }
            AgentUpdateMode::Manual => {
                write!(f, "manual")
            }
        }
    }
}

impl FromStr for AgentUpdateMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "auto" => Ok(AgentUpdateMode::Automatic),
            "manual" => Ok(AgentUpdateMode::Manual),
            _ => Err(format!(
                "Unknown agent update mode: {s}. Expected one of \"auto\", \"manual\""
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerMetadataView {
    pub component_name: ComponentName,
    pub worker_name: WorkerName,
    pub created_by: AccountId,
    pub environment_id: EnvironmentId,
    pub env: HashMap<String, String>,
    pub status: WorkerStatus,
    pub component_revision: ComponentRevision,
    pub retry_count: u32,

    pub pending_invocation_count: u64,
    pub updates: Vec<UpdateRecord>,
    pub created_at: Timestamp,
    pub last_error: Option<String>,
    pub component_size: u64,
    pub total_linear_memory_size: u64,
    pub exported_resource_instances: HashMap<String, WorkerResourceDescription>,
}

impl TrimDateTime for WorkerMetadataView {
    fn trim_date_time_ms(self) -> Self {
        self
    }
}

impl From<WorkerMetadata> for WorkerMetadataView {
    fn from(value: WorkerMetadata) -> Self {
        WorkerMetadataView {
            component_name: value.component_name,
            worker_name: value.worker_id.worker_name.into(),
            created_by: value.created_by,
            environment_id: value.environment_id,
            env: value.env,
            status: value.status,
            component_revision: value.component_version,
            retry_count: value.retry_count,
            pending_invocation_count: value.pending_invocation_count,
            updates: value.updates,
            created_at: value.created_at,
            last_error: value.last_error,
            component_size: value.component_size,
            total_linear_memory_size: value.total_linear_memory_size,
            exported_resource_instances: value.exported_resource_instances,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkerMetadata {
    pub worker_id: WorkerId,
    pub component_name: ComponentName,
    pub environment_id: EnvironmentId,
    pub created_by: AccountId,
    pub env: HashMap<String, String>,
    pub status: WorkerStatus,
    pub component_version: ComponentRevision,
    pub retry_count: u32,
    pub pending_invocation_count: u64,
    pub updates: Vec<UpdateRecord>,
    pub created_at: Timestamp,
    pub last_error: Option<String>,
    pub component_size: u64,
    pub total_linear_memory_size: u64,
    pub exported_resource_instances: HashMap<String, WorkerResourceDescription>,
}

impl WorkerMetadata {
    pub fn from(
        component_name: ComponentName,
        value: golem_client::model::WorkerMetadataDto,
    ) -> Self {
        WorkerMetadata {
            worker_id: value.worker_id,
            component_name,
            created_by: value.created_by,
            environment_id: value.environment_id,
            env: value.env,
            status: value.status,
            component_version: value.component_version,
            retry_count: value.retry_count,
            pending_invocation_count: value.pending_invocation_count,
            updates: value.updates,
            created_at: value.created_at,
            last_error: value.last_error,
            component_size: value.component_size,
            total_linear_memory_size: value.total_linear_memory_size,
            exported_resource_instances: HashMap::from_iter(
                value
                    .exported_resource_instances
                    .into_iter()
                    .map(|desc| (desc.key.to_string(), desc.description)),
            ),
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkersMetadataResponseView {
    pub workers: Vec<WorkerMetadataView>,
    pub cursors: BTreeMap<String, String>,
}

impl TrimDateTime for WorkersMetadataResponseView {
    fn trim_date_time_ms(self) -> Self {
        Self {
            workers: self.workers.trim_date_time_ms(),
            ..self
        }
    }
}

pub trait HasVerbosity {
    fn verbosity(&self) -> Verbosity;
}

#[derive(Debug, Clone)]
pub struct AgentLogStreamOptions {
    pub colors: bool,
    pub show_timestamp: bool,
    pub show_level: bool,
    /// Only show entries coming from the agent, no output about invocation markers and stream status
    pub logs_only: bool,
}

impl From<StreamArgs> for AgentLogStreamOptions {
    fn from(args: StreamArgs) -> Self {
        AgentLogStreamOptions {
            colors: SHOULD_COLORIZE.should_colorize(),
            show_timestamp: !args.stream_no_timestamp,
            show_level: !args.stream_no_log_level,
            logs_only: args.logs_only,
        }
    }
}

pub struct WorkerNameMatch {
    pub environment: ResolvedEnvironmentIdentity,
    pub component_name_match_kind: ComponentNameMatchKind,
    pub component_name: ComponentName,
    pub worker_name: WorkerName,
}

impl WorkerNameMatch {
    pub fn environment_reference(&self) -> Option<&EnvironmentReference> {
        match &self.environment.source {
            ResolvedEnvironmentIdentitySource::Reference(reference) => Some(reference),
            ResolvedEnvironmentIdentitySource::DefaultFromManifest => None,
        }
    }
}

pub fn fuzzy_match_function_name(
    provided_function_name: &str,
    exports: &[AnalysedExport],
    interface_name_filter: Option<&str>,
) -> crate::fuzzy::Result {
    let component_function_names = duplicate_names_with_syntax_sugar(show_exported_functions(
        exports,
        false,
        interface_name_filter,
    ));

    // First see if the function name is a valid function name
    let (normalized_function_name, parsed_function_name) =
        match ParsedFunctionName::parse(provided_function_name) {
            Ok(parsed_function_name) => {
                // If it is we render the normalized function name for fuzzy-searching that
                let normalized_function_name = normalize_function_name(&parsed_function_name);
                (
                    normalized_function_name.to_string(),
                    Some(parsed_function_name),
                )
            }
            Err(_) => {
                // If it is not, we try to fuzzy-match the original one
                (provided_function_name.to_string(), None)
            }
        };

    let fuzzy_search = FuzzySearch::new(component_function_names.iter().map(|s| s.as_str()));
    match fuzzy_search.find(&normalized_function_name) {
        Ok(matched) => {
            match parsed_function_name {
                // if we have a match, but it is non-exact _or_ we have a parsed function name, we customize the parsed function name and render it
                // because it may have resource parameters
                Some(mut parsed_function_name) if !matched.exact_match => {
                    let parsed_matched_function_name = ParsedFunctionName::parse(&matched.option)
                        .expect("The rendered component export names should always be parseable");

                    adjust_parsed_function_name(
                        &mut parsed_function_name,
                        parsed_matched_function_name,
                        normalized_function_name,
                    )?;

                    let rendered_altered_function_name = parsed_function_name.to_string();
                    Ok(Match {
                        option: rendered_altered_function_name,
                        pattern: matched.pattern,
                        exact_match: false,
                    })
                }
                _ => Ok(normalized(matched)),
            }
        }
        Err(Error::Ambiguous {
            pattern,
            highlighted_options,
            raw_options,
        }) => try_disambiguate(pattern, highlighted_options, raw_options).map(normalized),
        Err(err) => {
            // No match, we just return the fuzzy-match result
            Err(err)
        }
    }
}

fn normalized(matched: Match) -> Match {
    let normalized_function_name = normalize_function_name(&static_as_method(
        ParsedFunctionName::parse(matched.option)
            .expect("The rendered component export names should always be parseable"),
    ));

    // This is not strictly necessary (as the server side would accept the syntax-sugared versions too)
    // but it makes the function's output more consistent
    Match {
        option: normalized_function_name.to_string(),
        ..matched
    }
}

fn static_as_method(name: ParsedFunctionName) -> ParsedFunctionName {
    ParsedFunctionName {
        site: name.site,
        function: match name.function {
            ParsedFunctionReference::RawResourceStaticMethod { resource, method } => {
                ParsedFunctionReference::RawResourceMethod { resource, method }
            }
            other => other,
        },
    }
}

fn try_disambiguate(
    pattern: String,
    highlighted_options: Vec<String>,
    raw_options: Vec<String>,
) -> crate::fuzzy::Result {
    let mut parsed_options = raw_options
        .iter()
        .map(|option| {
            static_as_method(
                ParsedFunctionName::parse(option)
                    .expect("The rendered component export names should always be parseable"),
            )
        })
        .collect::<Vec<_>>();
    parsed_options.dedup();
    if parsed_options.len() == 1 {
        let option = parsed_options[0].to_string();
        let exact_match = pattern == option;
        Ok(Match {
            option,
            pattern,
            exact_match,
        })
    } else {
        Err(Error::Ambiguous {
            pattern,
            highlighted_options,
            raw_options,
        })
    }
}

fn normalize_function_name(parsed_function_name: &ParsedFunctionName) -> ParsedFunctionName {
    let interface_name = parsed_function_name.site.interface_name();
    let raw_function_name = parsed_function_name.function.function_name();
    match interface_name {
        Some(interface_name) => ParsedFunctionName::on_interface(interface_name, raw_function_name),
        None => ParsedFunctionName::global(raw_function_name),
    }
}

fn adjust_parsed_function_name(
    original: &mut ParsedFunctionName,
    matched: ParsedFunctionName,
    matched_pattern: String,
) -> Result<(), Error> {
    original.site = matched.site;
    match &mut original.function {
        ParsedFunctionReference::Function { function } => {
            *function = matched.function.function_name();
        }
        ParsedFunctionReference::RawResourceConstructor { resource } => {
            match matched.function {
                ParsedFunctionReference::RawResourceConstructor {
                    resource: parsed_resource,
                } => {
                    *resource = parsed_resource;
                }
                _ => {
                    // We were looking for a resource constructor but found something else
                    return Err(crate::fuzzy::Error::NotFound {
                        pattern: matched_pattern,
                    });
                }
            }
        }
        ParsedFunctionReference::RawResourceDrop { resource } => {
            match matched.function {
                ParsedFunctionReference::RawResourceDrop {
                    resource: parsed_resource,
                } => {
                    *resource = parsed_resource;
                }
                _ => {
                    // We were looking for a resource drop but found something else
                    return Err(crate::fuzzy::Error::NotFound {
                        pattern: matched_pattern,
                    });
                }
            }
        }
        ParsedFunctionReference::RawResourceMethod { resource, method }
        | ParsedFunctionReference::RawResourceStaticMethod { resource, method } => {
            match matched.function {
                ParsedFunctionReference::RawResourceMethod {
                    resource: parsed_resource,
                    method: parsed_method,
                }
                | ParsedFunctionReference::RawResourceStaticMethod {
                    resource: parsed_resource,
                    method: parsed_method,
                } => {
                    *resource = parsed_resource;
                    *method = parsed_method;
                }
                _ => {
                    // We were looking for a resource method but found something else
                    return Err(crate::fuzzy::Error::NotFound {
                        pattern: matched_pattern,
                    });
                }
            }
        }
    }

    Ok(())
}

fn duplicate_names_with_syntax_sugar(names: Vec<String>) -> Vec<String> {
    let mut result = Vec::new();

    for item in names {
        let parsed = ParsedFunctionName::parse(&item)
            .expect("The rendered component export names should always be parseable");
        match parsed.function {
            ParsedFunctionReference::RawResourceConstructor { .. }
            | ParsedFunctionReference::RawResourceDrop { .. }
            | ParsedFunctionReference::RawResourceMethod { .. }
            | ParsedFunctionReference::RawResourceStaticMethod { .. } => {
                // the Display instance is using the syntax sugared version, while the original list
                // contains the raw function names
                result.push(item);
                result.push(parsed.to_string());

                if let Some(static_method) = parsed.function.method_as_static() {
                    result.push(
                        ParsedFunctionName {
                            function: static_method,
                            site: parsed.site.clone(),
                        }
                        .to_string(),
                    );
                }
            }
            _ => {
                // If the function is not related to resources, there is no syntax sugar
                result.push(item);
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use crate::model::worker::fuzzy_match_function_name;
    use golem_wasm::analysis::analysed_type::{
        case, f32, field, handle, list, record, str, u32, variant,
    };
    use golem_wasm::analysis::{
        AnalysedExport, AnalysedFunction, AnalysedFunctionParameter, AnalysedFunctionResult,
        AnalysedInstance, AnalysedResourceId, AnalysedResourceMode,
    };
    use test_r::test;

    #[test]
    fn test_fuzzy_match_simple_function_names() {
        assert_eq!(
            fuzzy_match_function_name(
                "golem:it/api.{initialize-cart}",
                &example_exported_global(),
                None
            )
            .unwrap()
            .option,
            "golem:it/api.{initialize-cart}"
        );
        assert_eq!(
            fuzzy_match_function_name("initialize-cart", &example_exported_global(), None)
                .unwrap()
                .option,
            "golem:it/api.{initialize-cart}"
        );
        assert_eq!(
            fuzzy_match_function_name("initialize", &example_exported_global(), None)
                .unwrap()
                .option,
            "golem:it/api.{initialize-cart}"
        );
    }

    #[test]
    fn test_fuzzy_match_constructor() {
        assert_eq!(
            fuzzy_match_function_name(
                "golem:it/api.{[constructor]cart}",
                &example_exported_resource(),
                None
            )
            .unwrap()
            .option,
            "golem:it/api.{[constructor]cart}"
        );
        assert_eq!(
            fuzzy_match_function_name(
                "golem:it/api.{cart.new}",
                &example_exported_resource(),
                None
            )
            .unwrap()
            .option,
            "golem:it/api.{[constructor]cart}"
        );
        assert_eq!(
            fuzzy_match_function_name("[constructor]cart", &example_exported_resource(), None)
                .unwrap()
                .option,
            "golem:it/api.{[constructor]cart}"
        );
        assert_eq!(
            fuzzy_match_function_name("cart.new", &example_exported_resource(), None)
                .unwrap()
                .option,
            "golem:it/api.{[constructor]cart}"
        );
    }

    #[test]
    fn test_fuzzy_match_drop() {
        assert_eq!(
            fuzzy_match_function_name(
                "golem:it/api.{[drop]cart}",
                &example_exported_resource(),
                None
            )
            .unwrap()
            .option,
            "golem:it/api.{[drop]cart}"
        );
        assert_eq!(
            fuzzy_match_function_name(
                "golem:it/api.{cart.drop}",
                &example_exported_resource(),
                None
            )
            .unwrap()
            .option,
            "golem:it/api.{[drop]cart}"
        );
        assert_eq!(
            fuzzy_match_function_name("api.[drop]cart", &example_exported_resource(), None)
                .unwrap()
                .option,
            "golem:it/api.{[drop]cart}"
        );
        assert_eq!(
            fuzzy_match_function_name("api.cart.drop", &example_exported_resource(), None)
                .unwrap()
                .option,
            "golem:it/api.{[drop]cart}"
        );
        assert_eq!(
            fuzzy_match_function_name("[drop]cart", &example_exported_resource(), None)
                .unwrap()
                .option,
            "golem:it/api.{[drop]cart}"
        );
        assert_eq!(
            fuzzy_match_function_name("cart.drop", &example_exported_resource(), None)
                .unwrap()
                .option,
            "golem:it/api.{[drop]cart}"
        );
    }

    #[test]
    fn test_fuzzy_match_method() {
        assert_eq!(
            fuzzy_match_function_name(
                "golem:it/api.{[method]cart.add-item}",
                &example_exported_resource(),
                None
            )
            .unwrap()
            .option,
            "golem:it/api.{[method]cart.add-item}"
        );
        assert_eq!(
            fuzzy_match_function_name(
                "golem:it/api.{cart.add-item}",
                &example_exported_resource(),
                None
            )
            .unwrap()
            .option,
            "golem:it/api.{[method]cart.add-item}"
        );
        assert_eq!(
            fuzzy_match_function_name(
                "api.[method]cart.add-item",
                &example_exported_resource(),
                None
            )
            .unwrap()
            .option,
            "golem:it/api.{[method]cart.add-item}"
        );
        assert_eq!(
            fuzzy_match_function_name("api.cart.add-item", &example_exported_resource(), None)
                .unwrap()
                .option,
            "golem:it/api.{[method]cart.add-item}"
        );
        assert_eq!(
            fuzzy_match_function_name("[method]cart.add-item", &example_exported_resource(), None)
                .unwrap()
                .option,
            "golem:it/api.{[method]cart.add-item}"
        );
        assert_eq!(
            fuzzy_match_function_name("cart.add-item", &example_exported_resource(), None)
                .unwrap()
                .option,
            "golem:it/api.{[method]cart.add-item}"
        );
        assert_eq!(
            fuzzy_match_function_name("cart.add", &example_exported_resource(), None)
                .unwrap()
                .option,
            "golem:it/api.{[method]cart.add-item}"
        );
    }

    #[test]
    fn test_fuzzy_match_static_method() {
        assert_eq!(
            fuzzy_match_function_name(
                "golem:it/api.{[method]cart.merge-with}",
                &example_exported_resource(),
                None
            )
            .unwrap()
            .option,
            "golem:it/api.{[method]cart.merge-with}"
        );
        assert_eq!(
            fuzzy_match_function_name(
                "golem:it/api.{[static]cart.merge-with}",
                &example_exported_resource(),
                None
            )
            .unwrap()
            .option,
            "golem:it/api.{[method]cart.merge-with}"
        );
        assert_eq!(
            fuzzy_match_function_name(
                "golem:it/api.{cart.merge-with}",
                &example_exported_resource(),
                None
            )
            .unwrap()
            .option,
            "golem:it/api.{[method]cart.merge-with}"
        );
        assert_eq!(
            fuzzy_match_function_name(
                "api.[method]cart.merge-with",
                &example_exported_resource(),
                None
            )
            .unwrap()
            .option,
            "golem:it/api.{[method]cart.merge-with}"
        );
        assert_eq!(
            fuzzy_match_function_name("api.cart.merge-with", &example_exported_resource(), None)
                .unwrap()
                .option,
            "golem:it/api.{[method]cart.merge-with}"
        );
        assert_eq!(
            fuzzy_match_function_name(
                "[method]cart.merge-with",
                &example_exported_resource(),
                None
            )
            .unwrap()
            .option,
            "golem:it/api.{[method]cart.merge-with}"
        );
        assert_eq!(
            fuzzy_match_function_name(
                "[static]cart.merge-with",
                &example_exported_resource(),
                None
            )
            .unwrap()
            .option,
            "golem:it/api.{[method]cart.merge-with}"
        );
        assert_eq!(
            fuzzy_match_function_name("cart.merge-with", &example_exported_resource(), None)
                .unwrap()
                .option,
            "golem:it/api.{[method]cart.merge-with}"
        );
        assert_eq!(
            fuzzy_match_function_name("cart.merge", &example_exported_resource(), None)
                .unwrap()
                .option,
            "golem:it/api.{[method]cart.merge-with}"
        );
        assert_eq!(
            fuzzy_match_function_name("cart.merge", &example_exported_resource(), None)
                .unwrap()
                .option,
            "golem:it/api.{[method]cart.merge-with}"
        );
    }

    #[test]
    fn test_fuzzy_interface_filter() {
        assert_eq!(
            fuzzy_match_function_name(
                "initialize-cart",
                &example_exported_global(),
                Some("golem:it/api")
            )
            .unwrap()
            .option,
            "golem:it/api.{initialize-cart}"
        );
        assert_eq!(
            fuzzy_match_function_name(
                "initialize-cart",
                &example_exported_global(),
                Some("golem:it/api-2")
            )
            .ok(),
            None
        );
    }

    fn example_exported_global() -> Vec<AnalysedExport> {
        vec![AnalysedExport::Instance(AnalysedInstance {
            name: "golem:it/api".to_string(),
            functions: vec![
                AnalysedFunction {
                    name: "initialize-cart".to_string(),
                    parameters: vec![AnalysedFunctionParameter {
                        name: "user-id".to_string(),
                        typ: str(),
                    }],
                    result: None,
                },
                AnalysedFunction {
                    name: "add-item".to_string(),
                    parameters: vec![AnalysedFunctionParameter {
                        name: "item".to_string(),
                        typ: record(vec![
                            field("product-id", str()),
                            field("name", str()),
                            field("price", f32()),
                            field("quantity", u32()),
                        ]),
                    }],
                    result: None,
                },
                AnalysedFunction {
                    name: "remove-item".to_string(),
                    parameters: vec![AnalysedFunctionParameter {
                        name: "product-id".to_string(),
                        typ: str(),
                    }],
                    result: None,
                },
                AnalysedFunction {
                    name: "update-item-quantity".to_string(),
                    parameters: vec![
                        AnalysedFunctionParameter {
                            name: "product-id".to_string(),
                            typ: str(),
                        },
                        AnalysedFunctionParameter {
                            name: "quantity".to_string(),
                            typ: u32(),
                        },
                    ],
                    result: None,
                },
                AnalysedFunction {
                    name: "checkout".to_string(),
                    parameters: vec![],
                    result: Some(AnalysedFunctionResult {
                        typ: variant(vec![
                            case("error", str()),
                            case("success", record(vec![field("order-id", str())])),
                        ]),
                    }),
                },
                AnalysedFunction {
                    name: "get-cart-contents".to_string(),
                    parameters: vec![],
                    result: Some(AnalysedFunctionResult {
                        typ: list(record(vec![
                            field("product-id", str()),
                            field("name", str()),
                            field("price", f32()),
                            field("quantity", u32()),
                        ])),
                    }),
                },
            ],
        })]
    }

    fn example_exported_resource() -> Vec<AnalysedExport> {
        vec![AnalysedExport::Instance(AnalysedInstance {
            name: "golem:it/api".to_string(),
            functions: vec![
                AnalysedFunction {
                    name: "[constructor]cart".to_string(),
                    parameters: vec![AnalysedFunctionParameter {
                        name: "user-id".to_string(),
                        typ: str(),
                    }],
                    result: Some(AnalysedFunctionResult {
                        typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Owned),
                    }),
                },
                AnalysedFunction {
                    name: "[method]cart.add-item".to_string(),
                    parameters: vec![
                        AnalysedFunctionParameter {
                            name: "self".to_string(),
                            typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
                        },
                        AnalysedFunctionParameter {
                            name: "item".to_string(),
                            typ: record(vec![
                                field("product-id", str()),
                                field("name", str()),
                                field("price", f32()),
                                field("quantity", u32()),
                            ]),
                        },
                    ],
                    result: None,
                },
                AnalysedFunction {
                    name: "[method]cart.remove-item".to_string(),
                    parameters: vec![
                        AnalysedFunctionParameter {
                            name: "self".to_string(),
                            typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
                        },
                        AnalysedFunctionParameter {
                            name: "product-id".to_string(),
                            typ: str(),
                        },
                    ],
                    result: None,
                },
                AnalysedFunction {
                    name: "[method]cart.update-item-quantity".to_string(),
                    parameters: vec![
                        AnalysedFunctionParameter {
                            name: "self".to_string(),
                            typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
                        },
                        AnalysedFunctionParameter {
                            name: "product-id".to_string(),
                            typ: str(),
                        },
                        AnalysedFunctionParameter {
                            name: "quantity".to_string(),
                            typ: u32(),
                        },
                    ],
                    result: None,
                },
                AnalysedFunction {
                    name: "[method]cart.checkout".to_string(),
                    parameters: vec![AnalysedFunctionParameter {
                        name: "self".to_string(),
                        typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
                    }],
                    result: Some(AnalysedFunctionResult {
                        typ: variant(vec![
                            case("error", str()),
                            case("success", record(vec![field("order-id", str())])),
                        ]),
                    }),
                },
                AnalysedFunction {
                    name: "[method]cart.get-cart-contents".to_string(),
                    parameters: vec![AnalysedFunctionParameter {
                        name: "self".to_string(),
                        typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
                    }],
                    result: Some(AnalysedFunctionResult {
                        typ: list(record(vec![
                            field("product-id", str()),
                            field("name", str()),
                            field("price", f32()),
                            field("quantity", u32()),
                        ])),
                    }),
                },
                AnalysedFunction {
                    name: "[method]cart.merge-with".to_string(),
                    parameters: vec![
                        AnalysedFunctionParameter {
                            name: "self".to_string(),
                            typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
                        },
                        AnalysedFunctionParameter {
                            name: "other-cart".to_string(),
                            typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
                        },
                    ],
                    result: None,
                },
                AnalysedFunction {
                    name: "[drop]cart".to_string(),
                    parameters: vec![AnalysedFunctionParameter {
                        name: "self".to_string(),
                        typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Owned),
                    }],
                    result: None,
                },
            ],
        })]
    }
}

// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::clients::worker::WorkerClient;
use crate::command::worker::WorkerConnectOptions;
use crate::model::component::{
    format_function_name, function_params_types, show_exported_function, Component,
};
use crate::model::deploy::TryUpdateAllWorkersResult;
use crate::model::invoke_result_view::InvokeResultView;
use crate::model::text::worker::{WorkerAddView, WorkerGetView};
use crate::model::{
    Format, GolemError, GolemResult, IdempotencyKey, WorkerMetadata, WorkerName, WorkerUpdateMode,
    WorkersMetadataResponseView,
};
use crate::service::component::ComponentService;
use async_trait::async_trait;
use golem_client::model::{AnalysedType, InvokeParameters, InvokeResult, ScanCursor};
use golem_common::model::TargetWorkerId;
use golem_common::uri::oss::uri::{ComponentUri, WorkerUri};
use golem_common::uri::oss::url::{ComponentUrl, WorkerUrl};
use golem_common::uri::oss::urn::{ComponentUrn, WorkerUrn};
use golem_wasm_ast::analysis::{AnalysedExport, AnalysedFunction, AnalysedInstance};
use golem_wasm_rpc::json::TypeAnnotatedValueJsonExtensions;
use golem_wasm_rpc::parse_type_annotated_value;
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use itertools::Itertools;
use serde_json::Value;
use std::sync::Arc;
use tokio::task::JoinHandle;
use tracing::{error, info, Instrument};
use uuid::Uuid;

#[async_trait]
pub trait WorkerService {
    type ProjectContext: Send + Sync;

    async fn add(
        &self,
        component_uri: ComponentUri,
        worker_name: WorkerName,
        env: Vec<(String, String)>,
        args: Vec<String>,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError>;

    async fn add_by_urn(
        &self,
        component_urn: ComponentUrn,
        worker_name: WorkerName,
        env: Vec<(String, String)>,
        args: Vec<String>,
    ) -> Result<GolemResult, GolemError>;

    async fn idempotency_key(&self) -> Result<GolemResult, GolemError> {
        let key = IdempotencyKey(Uuid::new_v4().to_string());
        Ok(GolemResult::Ok(Box::new(key)))
    }

    async fn resolve_uri(
        &self,
        worker_uri: WorkerUri,
        project: Option<Self::ProjectContext>,
    ) -> Result<WorkerUrn, GolemError>;

    async fn invoke_and_await(
        &self,
        format: Format,
        worker_uri: WorkerUri,
        idempotency_key: Option<IdempotencyKey>,
        function: String,
        parameters: Option<Value>,
        wave: Vec<String>,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError>;

    async fn invoke(
        &self,
        worker_uri: WorkerUri,
        idempotency_key: Option<IdempotencyKey>,
        function: String,
        parameters: Option<Value>,
        wave: Vec<String>,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError>;

    async fn connect(
        &self,
        worker_uri: WorkerUri,
        project: Option<Self::ProjectContext>,
        connect_options: WorkerConnectOptions,
        format: Format,
    ) -> Result<GolemResult, GolemError>;

    async fn interrupt(
        &self,
        worker_uri: WorkerUri,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError>;

    async fn resume(
        &self,
        worker_uri: WorkerUri,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError>;

    async fn simulated_crash(
        &self,
        worker_uri: WorkerUri,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError>;

    async fn delete(
        &self,
        worker_uri: WorkerUri,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError>;

    async fn delete_by_urn(&self, worker_urn: WorkerUrn) -> Result<GolemResult, GolemError>;

    async fn get(
        &self,
        worker_uri: WorkerUri,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError>;

    async fn get_function(
        &self,
        worker_uri: WorkerUri,
        function: &str,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError>;

    async fn list(
        &self,
        component_uri: ComponentUri,
        filter: Option<Vec<String>>,
        count: Option<u64>,
        cursor: Option<ScanCursor>,
        precise: Option<bool>,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError>;

    async fn update(
        &self,
        worker_uri: WorkerUri,
        target_version: u64,
        mode: WorkerUpdateMode,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError>;

    async fn update_by_urn(
        &self,
        worker_urn: WorkerUrn,
        target_version: u64,
        mode: WorkerUpdateMode,
    ) -> Result<GolemResult, GolemError>;

    async fn update_many(
        &self,
        component_uri: ComponentUri,
        filter: Option<Vec<String>>,
        target_version: u64,
        mode: WorkerUpdateMode,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError>;

    async fn update_many_by_urn(
        &self,
        component_urn: ComponentUrn,
        filter: Option<Vec<String>>,
        target_version: u64,
        mode: WorkerUpdateMode,
    ) -> Result<GolemResult, GolemError>;

    async fn list_worker_metadata(
        &self,
        component_urn: &ComponentUrn,
        filter: Option<Vec<String>>,
        precise: Option<bool>,
    ) -> Result<Vec<WorkerMetadata>, GolemError>;

    async fn get_oplog(
        &self,
        worker_uri: WorkerUri,
        from: u64,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError>;

    async fn search_oplog(
        &self,
        worker_uri: WorkerUri,
        query: String,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError>;
}

pub struct WorkerServiceLive<ProjectContext: Send + Sync> {
    pub client: Arc<dyn WorkerClient + Send + Sync>,
    pub components: Arc<dyn ComponentService<ProjectContext = ProjectContext> + Send + Sync>,
}

async fn resolve_worker_component_version<ProjectContext: Send + Sync>(
    client: &(dyn WorkerClient + Send + Sync),
    components: &(dyn ComponentService<ProjectContext = ProjectContext> + Send + Sync),
    worker_urn: WorkerUrn,
) -> Result<Option<Component>, GolemError> {
    if worker_urn.id.worker_name.is_some() {
        let component_urn = ComponentUrn {
            id: worker_urn.id.component_id.clone(),
        };

        let worker_metadata = client.get_metadata_opt(worker_urn).await?;
        if let Some(worker_metadata) = worker_metadata {
            let component_metadata = components
                .get_metadata(&component_urn, worker_metadata.component_version)
                .await?;
            Ok(Some(component_metadata))
        } else {
            Ok(None)
        }
    } else {
        Ok(None)
    }
}

fn parse_parameter(wave: &str, typ: &AnalysedType) -> Result<TypeAnnotatedValue, GolemError> {
    // Avoid converting from typ to AnalysedType
    match parse_type_annotated_value(typ, wave) {
        Ok(value) => Ok(value),
        Err(err) => Err(GolemError(format!(
            "Failed to parse wave parameter {wave}: {err:?}"
        ))),
    }
}

async fn get_component_metadata_for_worker<ProjectContext: Send + Sync>(
    client: &(dyn WorkerClient + Send + Sync),
    components: &(dyn ComponentService<ProjectContext = ProjectContext> + Send + Sync),
    worker_urn: &WorkerUrn,
) -> Result<Component, GolemError> {
    if let Some(component) =
        resolve_worker_component_version(client, components, worker_urn.clone()).await?
    {
        Ok(component)
    } else {
        if let Some(worker_name) = &worker_urn.id.worker_name {
            info!("No worker found with name {worker_name}. Assuming it should be create with the latest component version");
        }
        let component_urn = ComponentUrn {
            id: worker_urn.id.component_id.clone(),
        };

        let component = components.get_latest_metadata(&component_urn).await?;
        Ok(component)
    }
}

async fn resolve_parameters<ProjectContext: Send + Sync>(
    client: &(dyn WorkerClient + Send + Sync),
    components: &(dyn ComponentService<ProjectContext = ProjectContext> + Send + Sync),
    worker_urn: &WorkerUrn,
    parameters: Option<Value>,
    wave: Vec<String>,
    function: &str,
) -> Result<
    (
        Vec<golem_client::model::TypeAnnotatedValue>,
        Option<Component>,
    ),
    GolemError,
> {
    if let Some(parameters) = parameters {
        // A JSON parameter was provided. It is either an array of serialized TypeAnnotatedValues
        // or an array of the JSON representation of the parameters with no type information.
        let parameters = parameters
            .as_array()
            .ok_or_else(|| GolemError("Parameters must be an array".to_string()))?;

        let attempt1 = parameters
            .iter()
            .map(|v| serde_json::from_value::<TypeAnnotatedValue>(v.clone()))
            .collect::<Result<Vec<_>, _>>();

        if let Ok(type_annotated_values) = attempt1 {
            // All elements were valid TypeAnnotatedValues, we don't need component metadata to do the invocation
            Ok((type_annotated_values, None))
        } else {
            // Some elements were not valid TypeAnnotatedValues, we need component metadata to interpret them
            let component =
                get_component_metadata_for_worker(client, components, worker_urn).await?;
            let types = function_params_types(&component, function)?;

            if types.len() != parameters.len() {
                return Err(GolemError(format!(
                    "Unexpected number of parameters: got {}, expected {}",
                    parameters.len(),
                    types.len()
                )));
            }

            let mut type_annotated_values = Vec::new();
            for (json_param, typ) in parameters.iter().zip(types) {
                match TypeAnnotatedValue::parse_with_type(json_param, typ) {
                    Ok(tav) => type_annotated_values.push(tav),
                    Err(err) => {
                        return Err(GolemError(format!(
                            "Failed to parse parameter: {}",
                            err.join(", ")
                        )))
                    }
                }
            }

            Ok((type_annotated_values, Some(component)))
        }
    } else {
        // No JSON parameters, we use the WAVE ones
        let component = get_component_metadata_for_worker(client, components, worker_urn).await?;
        let types = function_params_types(&component, function)?;

        if types.len() != wave.len() {
            return Err(GolemError(format!(
                "Unexpected number of parameters: got {}, expected {}",
                wave.len(),
                types.len()
            )));
        }

        let type_annotated_values = wave
            .iter()
            .zip(types)
            .map(|(wave, typ)| parse_parameter(wave, typ))
            .collect::<Result<Vec<_>, _>>()?;

        Ok((type_annotated_values, Some(component)))
    }
}

async fn to_invoke_result_view<ProjectContext: Send + Sync>(
    res: InvokeResult,
    async_component_request: AsyncComponentRequest,
    client: &(dyn WorkerClient + Send + Sync),
    components: &(dyn ComponentService<ProjectContext = ProjectContext> + Send + Sync),
    worker_urn: &WorkerUrn,
    function: &str,
) -> Result<InvokeResultView, GolemError> {
    let component = match async_component_request {
        AsyncComponentRequest::Empty => None,
        AsyncComponentRequest::Resolved(component) => Some(component),
        AsyncComponentRequest::Async(join_meta) => match join_meta.await.unwrap() {
            Ok(Some(component)) => Some(component),
            _ => None,
        },
    };

    let component = match component {
        None => {
            match resolve_worker_component_version(client, components, worker_urn.clone()).await {
                Ok(Some(component)) => component,
                _ => {
                    error!("Failed to get worker metadata after successful call.");

                    let json = serde_json::to_value(&res.result)
                        .map_err(|err| GolemError(err.to_string()))?;
                    return Ok(InvokeResultView::Json(json));
                }
            }
        }
        Some(component) => component,
    };

    InvokeResultView::try_parse_or_json(res, &component, function)
}

enum AsyncComponentRequest {
    Empty,
    Resolved(Component),
    Async(JoinHandle<Result<Option<Component>, GolemError>>),
}

#[async_trait]
impl<ProjectContext: Send + Sync + 'static> WorkerService for WorkerServiceLive<ProjectContext> {
    type ProjectContext = ProjectContext;

    async fn add(
        &self,
        component_uri: ComponentUri,
        worker_name: WorkerName,
        env: Vec<(String, String)>,
        args: Vec<String>,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError> {
        let component_urn = self.components.resolve_uri(component_uri, &project).await?;
        self.add_by_urn(component_urn, worker_name, env, args).await
    }

    async fn add_by_urn(
        &self,
        component_urn: ComponentUrn,
        worker_name: WorkerName,
        env: Vec<(String, String)>,
        args: Vec<String>,
    ) -> Result<GolemResult, GolemError> {
        let worker_id = self
            .client
            .new_worker(worker_name, component_urn, args, env)
            .await?;

        Ok(GolemResult::Ok(Box::new(WorkerAddView(WorkerUrn {
            id: worker_id.into_target_worker_id(),
        }))))
    }

    async fn resolve_uri(
        &self,
        worker_uri: WorkerUri,
        project: Option<Self::ProjectContext>,
    ) -> Result<WorkerUrn, GolemError> {
        match worker_uri {
            WorkerUri::URN(urn) => Ok(urn),
            WorkerUri::URL(WorkerUrl {
                component_name,
                worker_name,
            }) => {
                let component_uri = ComponentUri::URL(ComponentUrl {
                    name: component_name,
                });
                let component_urn = self.components.resolve_uri(component_uri, &project).await?;

                Ok(WorkerUrn {
                    id: TargetWorkerId {
                        component_id: component_urn.id,
                        worker_name,
                    },
                })
            }
        }
    }

    async fn invoke_and_await(
        &self,
        format: Format,
        worker_uri: WorkerUri,
        idempotency_key: Option<IdempotencyKey>,
        function: String,
        parameters: Option<Value>,
        wave: Vec<String>,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError> {
        let human_readable = format == Format::Text;
        let worker_urn = self.resolve_uri(worker_uri, project).await?;

        let (parameters, component_meta) = resolve_parameters(
            self.client.as_ref(),
            self.components.as_ref(),
            &worker_urn,
            parameters,
            wave,
            &function,
        )
        .await?;

        let async_component_request = if let Some(component) = component_meta {
            AsyncComponentRequest::Resolved(component)
        } else if human_readable {
            let worker_client = self.client.clone();
            let component_service = self.components.clone();
            let worker_urn = worker_urn.clone();
            AsyncComponentRequest::Async(tokio::spawn(
                async move {
                    resolve_worker_component_version(
                        worker_client.as_ref(),
                        component_service.as_ref(),
                        worker_urn,
                    )
                    .await
                }
                .in_current_span(),
            ))
        } else {
            AsyncComponentRequest::Empty
        };

        let res = self
            .client
            .invoke_and_await(
                worker_urn.clone(),
                function.clone(),
                InvokeParameters { params: parameters },
                idempotency_key,
            )
            .await?;

        if human_readable {
            let view = to_invoke_result_view(
                res,
                async_component_request,
                self.client.as_ref(),
                self.components.as_ref(),
                &worker_urn,
                &function,
            )
            .await?;

            Ok(GolemResult::Ok(Box::new(view)))
        } else {
            let json =
                serde_json::to_value(&res.result).map_err(|err| GolemError(err.to_string()))?;
            Ok(GolemResult::Json(json))
        }
    }

    async fn invoke(
        &self,
        worker_uri: WorkerUri,
        idempotency_key: Option<IdempotencyKey>,
        function: String,
        parameters: Option<Value>,
        wave: Vec<String>,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError> {
        let worker_urn = self.resolve_uri(worker_uri, project).await?;

        let (parameters, _) = resolve_parameters(
            self.client.as_ref(),
            self.components.as_ref(),
            &worker_urn,
            parameters,
            wave,
            &function,
        )
        .await?;

        self.client
            .invoke(
                worker_urn,
                function,
                InvokeParameters { params: parameters },
                idempotency_key,
            )
            .await?;

        Ok(GolemResult::Str("Invoked".to_string()))
    }

    async fn connect(
        &self,
        worker_uri: WorkerUri,
        project: Option<Self::ProjectContext>,
        connect_options: WorkerConnectOptions,
        format: Format,
    ) -> Result<GolemResult, GolemError> {
        let worker_urn = self.resolve_uri(worker_uri, project).await?;

        self.client
            .connect_forever(worker_urn, connect_options, format)
            .await?;

        Err(GolemError("Unexpected connection closure".to_string()))
    }

    async fn interrupt(
        &self,
        worker_uri: WorkerUri,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError> {
        let worker_urn = self.resolve_uri(worker_uri, project).await?;

        self.client.interrupt(worker_urn).await?;

        Ok(GolemResult::Str("Interrupted".to_string()))
    }

    async fn resume(
        &self,
        worker_uri: WorkerUri,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError> {
        let worker_urn = self.resolve_uri(worker_uri, project).await?;

        self.client.resume(worker_urn).await?;

        Ok(GolemResult::Str("Resumed".to_string()))
    }

    async fn simulated_crash(
        &self,
        worker_uri: WorkerUri,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError> {
        let worker_urn = self.resolve_uri(worker_uri, project).await?;

        self.client.simulated_crash(worker_urn).await?;

        Ok(GolemResult::Str("Done".to_string()))
    }

    async fn delete(
        &self,
        worker_uri: WorkerUri,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError> {
        let worker_urn = self.resolve_uri(worker_uri, project).await?;
        self.delete_by_urn(worker_urn).await
    }

    async fn delete_by_urn(&self, worker_urn: WorkerUrn) -> Result<GolemResult, GolemError> {
        self.client.delete(worker_urn).await?;

        Ok(GolemResult::Str("Deleted".to_string()))
    }

    async fn get(
        &self,
        worker_uri: WorkerUri,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError> {
        let worker_urn = self.resolve_uri(worker_uri, project).await?;

        let response: WorkerGetView = self.client.get_metadata(worker_urn).await?.into();

        Ok(GolemResult::Ok(Box::new(response)))
    }

    async fn get_function(
        &self,
        worker_uri: WorkerUri,
        function_name: &str,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError> {
        let worker_urn = self.resolve_uri(worker_uri, project).await?;
        let worker = self.client.get_metadata(worker_urn.clone()).await?;

        let component_urn = ComponentUrn {
            id: worker_urn.id.component_id,
        };
        let component = self
            .components
            .get_metadata(&component_urn, worker.component_version)
            .await?;

        fn match_function(
            target_function_name: &str,
            prefix: Option<&str>,
            function: &AnalysedFunction,
        ) -> Option<String> {
            let function_name = format_function_name(prefix, &function.name);
            if function_name == target_function_name {
                Some(show_exported_function(prefix, function))
            } else {
                None
            }
        }

        let function = component.metadata.exports.iter().find_map(|exp| match exp {
            AnalysedExport::Instance(AnalysedInstance {
                name: prefix,
                functions,
            }) => functions
                .iter()
                .find_map(|f| match_function(function_name, Some(prefix), f)),
            AnalysedExport::Function(f) => match_function(function_name, None, f),
        });

        match function {
            None => Err(GolemError(format!(
                "Can't find function '{function_name}' in component {component_urn}."
            ))),
            Some(function) => Ok(GolemResult::Str(function)),
        }
    }

    async fn list(
        &self,
        component_uri: ComponentUri,
        filter: Option<Vec<String>>,
        count: Option<u64>,
        cursor: Option<ScanCursor>,
        precise: Option<bool>,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError> {
        let component_urn = self.components.resolve_uri(component_uri, &project).await?;

        if count.is_some() {
            let response: WorkersMetadataResponseView = self
                .client
                .list_metadata(component_urn, filter, cursor, count, precise)
                .await?
                .into();

            Ok(GolemResult::Ok(Box::new(response)))
        } else {
            let workers = self
                .list_worker_metadata(&component_urn, filter, precise)
                .await?;
            Ok(GolemResult::Ok(Box::new(WorkersMetadataResponseView {
                workers: workers.into_iter().map_into().collect(),
                cursor: None,
            })))
        }
    }

    async fn update(
        &self,
        worker_uri: WorkerUri,
        target_version: u64,
        mode: WorkerUpdateMode,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError> {
        let worker_urn = self.resolve_uri(worker_uri, project).await?;
        self.update_by_urn(worker_urn, target_version, mode).await
    }

    async fn update_by_urn(
        &self,
        worker_urn: WorkerUrn,
        target_version: u64,
        mode: WorkerUpdateMode,
    ) -> Result<GolemResult, GolemError> {
        let _ = self.client.update(worker_urn, mode, target_version).await?;

        Ok(GolemResult::Str("Updated".to_string()))
    }

    async fn update_many(
        &self,
        component_uri: ComponentUri,
        filter: Option<Vec<String>>,
        target_version: u64,
        mode: WorkerUpdateMode,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError> {
        let component_urn = self.components.resolve_uri(component_uri, &project).await?;
        self.update_many_by_urn(component_urn, filter, target_version, mode)
            .await
    }

    async fn update_many_by_urn(
        &self,
        component_urn: ComponentUrn,
        filter: Option<Vec<String>>,
        target_version: u64,
        mode: WorkerUpdateMode,
    ) -> Result<GolemResult, GolemError> {
        let known_workers = self
            .list_worker_metadata(&component_urn, filter, Some(true))
            .await?;

        let to_update = known_workers
            .into_iter()
            .filter(|worker| worker.component_version < target_version)
            .collect::<Vec<_>>();

        let mut triggered = Vec::new();
        let mut failed = Vec::new();
        for worker in to_update {
            let worker_urn = WorkerUrn {
                id: worker.worker_id.clone().into_target_worker_id(),
            };
            let result = self
                .update_by_urn(worker_urn.clone(), target_version, mode.clone())
                .await;

            if result.is_ok() {
                triggered.push(worker_urn);
            } else {
                failed.push(worker_urn);
            }
        }

        Ok(GolemResult::Ok(Box::new(TryUpdateAllWorkersResult {
            triggered,
            failed,
        })))
    }

    async fn list_worker_metadata(
        &self,
        component_urn: &ComponentUrn,
        filter: Option<Vec<String>>,
        precise: Option<bool>,
    ) -> Result<Vec<WorkerMetadata>, GolemError> {
        let mut workers: Vec<WorkerMetadata> = vec![];
        let mut new_cursor = None;

        loop {
            let response = self
                .client
                .list_metadata(
                    component_urn.clone(),
                    filter.clone(),
                    new_cursor,
                    Some(50),
                    precise,
                )
                .await?;

            workers.extend(response.workers);

            new_cursor = response.cursor;

            if new_cursor.is_none() {
                break;
            }
        }

        Ok(workers)
    }

    async fn get_oplog(
        &self,
        worker_uri: WorkerUri,
        from: u64,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError> {
        let worker_urn = self.resolve_uri(worker_uri, project).await?;

        let entries = self.client.get_oplog(worker_urn, from).await?;
        Ok(GolemResult::Ok(Box::new(entries)))
    }

    async fn search_oplog(
        &self,
        worker_uri: WorkerUri,
        query: String,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError> {
        let worker_urn = self.resolve_uri(worker_uri, project).await?;

        let entries = self.client.search_oplog(worker_urn, query).await?;
        Ok(GolemResult::Ok(Box::new(entries)))
    }
}

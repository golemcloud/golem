// Copyright 2024 Golem Cloud
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
use crate::model::component::{function_params_types, show_exported_function, Component};
use crate::model::invoke_result_view::InvokeResultView;
use crate::model::text::WorkerAddView;
use crate::model::wave::type_to_analysed;
use crate::model::{
    Format, GolemError, GolemResult, IdempotencyKey, WorkerMetadata, WorkerMetadataView,
    WorkerName, WorkerUpdateMode, WorkersMetadataResponseView,
};
use crate::service::component::ComponentService;
use async_trait::async_trait;
use golem_client::model::{
    Export, ExportInstance, InvokeParameters, InvokeResult, ScanCursor, StringFilterComparator,
    Type, WorkerFilter, WorkerNameFilter,
};
use golem_common::model::precise_json::PreciseJson;
use golem_common::model::{ComponentId, WorkerId};
use golem_common::uri::oss::uri::{ComponentUri, WorkerUri};
use golem_common::uri::oss::url::{ComponentUrl, WorkerUrl};
use golem_common::uri::oss::urn::{ComponentUrn, WorkerUrn};
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::type_annotated_value_from_str;
use itertools::Itertools;
use serde_json::Value;
use tokio::task::JoinHandle;
use tracing::{error, info};
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
    ) -> Result<GolemResult, GolemError>;
    async fn interrupt(
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
}

pub trait WorkerClientBuilder {
    fn build(&self) -> Result<Box<dyn WorkerClient + Send + Sync>, GolemError>;
}

pub trait ComponentServiceBuilder<ProjectContext: Send + Sync> {
    fn build(
        &self,
    ) -> Result<Box<dyn ComponentService<ProjectContext = ProjectContext> + Send + Sync>, GolemError>;
}

pub struct WorkerServiceLive<ProjectContext: Send + Sync> {
    pub client: Box<dyn WorkerClient + Send + Sync>,
    pub components: Box<dyn ComponentService<ProjectContext = ProjectContext> + Send + Sync>,
    pub client_builder: Box<dyn WorkerClientBuilder + Send + Sync>,
    pub component_service_builder: Box<dyn ComponentServiceBuilder<ProjectContext> + Send + Sync>,
}

// same as resolve_worker_component_version, but with no borrowing, so we can spawn it.
async fn resolve_worker_component_version_no_ref<ProjectContext: Send + Sync>(
    worker_client: Box<dyn WorkerClient + Send + Sync>,
    component_service: Box<dyn ComponentService<ProjectContext = ProjectContext> + Send + Sync>,
    worker_urn: WorkerUrn,
) -> Result<Option<Component>, GolemError> {
    resolve_worker_component_version(
        worker_client.as_ref(),
        component_service.as_ref(),
        worker_urn,
    )
    .await
}

async fn resolve_worker_component_version<ProjectContext: Send + Sync>(
    client: &(dyn WorkerClient + Send + Sync),
    components: &(dyn ComponentService<ProjectContext = ProjectContext> + Send + Sync),
    worker_urn: WorkerUrn,
) -> Result<Option<Component>, GolemError> {
    let WorkerId {
        component_id,
        worker_name,
    } = worker_urn.id;
    let component_urn = ComponentUrn { id: component_id };

    let worker_meta = client
        .find_metadata(
            component_urn.clone(),
            Some(WorkerFilter::Name(WorkerNameFilter {
                comparator: StringFilterComparator::Equal,
                value: worker_name,
            })),
            None,
            Some(2),
            Some(true),
        )
        .await?;

    if worker_meta.workers.len() > 1 {
        Err(GolemError(
            "Multiple workers with the same name".to_string(),
        ))
    } else if let Some(worker) = worker_meta.workers.first() {
        Ok(Some(
            components
                .get_metadata(&component_urn, worker.component_version)
                .await?,
        ))
    } else {
        Ok(None)
    }
}

fn wave_parameters_to_json(
    wave: &[String],
    component: &Component,
    function: &str,
) -> Result<Vec<Value>, GolemError> {
    let types = function_params_types(component, function)?;

    if wave.len() != types.len() {
        return Err(GolemError(format!(
            "Invalid number of wave parameters for function {function}. Expected {}, but got {}.",
            types.len(),
            wave.len()
        )));
    }

    let params = wave
        .iter()
        .zip(types)
        .map(|(param, typ)| parse_parameter(param, typ))
        .collect::<Result<Vec<_>, _>>()?;

    params
        .into_iter()
        .map(|v| {
            serde_json::to_value(PreciseJson::from(v)).map_err(|err| GolemError(err.to_string()))
        })
        .collect::<Result<Vec<_>, GolemError>>()
}

fn parse_parameter(wave: &str, typ: &Type) -> Result<TypeAnnotatedValue, GolemError> {
    // Avoid converting from typ to AnalysedType
    match type_annotated_value_from_str(&type_to_analysed(typ), wave) {
        Ok(value) => Ok(value),
        Err(err) => Err(GolemError(format!(
            "Failed to parse wave parameter {wave}: {err:?}"
        ))),
    }
}

async fn resolve_parameters<ProjectContext: Send + Sync>(
    client: &(dyn WorkerClient + Send + Sync),
    components: &(dyn ComponentService<ProjectContext = ProjectContext> + Send + Sync),
    worker_urn: &WorkerUrn,
    parameters: Option<Value>,
    wave: Vec<String>,
    function: &str,
) -> Result<(Vec<Value>, Option<Component>), GolemError> {
    if let Some(parameters) = parameters {
        let parameters = parameters
            .as_array()
            .ok_or_else(|| GolemError("Parameters must be an array".to_string()))?;

        Ok((parameters.clone(), None))
    } else if let Some(component) =
        resolve_worker_component_version(client, components, worker_urn.clone()).await?
    {
        let precise_json_array = wave_parameters_to_json(&wave, &component, function)?;

        Ok((precise_json_array, Some(component)))
    } else {
        info!("No worker found with name {}. Assuming it should be create with the latest component version", worker_urn.id.worker_name);
        let component_urn = ComponentUrn {
            id: worker_urn.id.component_id.clone(),
        };

        let component = components.get_latest_metadata(&component_urn).await?;

        let json_array = wave_parameters_to_json(&wave, &component, function)?;

        // We are not going to use this component for result parsing.
        Ok((json_array, None))
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

                    return Ok(InvokeResultView::Json(res.result));
                }
            }
        }
        Some(component) => component,
    };

    Ok(InvokeResultView::try_parse_or_json(
        res, &component, function,
    ))
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
        let component_urn = self.components.resolve_uri(component_uri, project).await?;

        let inst = self
            .client
            .new_worker(worker_name, component_urn, args, env)
            .await?;

        Ok(GolemResult::Ok(Box::new(WorkerAddView(WorkerUrn {
            id: WorkerId {
                component_id: ComponentId(inst.component_id),
                worker_name: inst.worker_name,
            },
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
                let component_urn = self.components.resolve_uri(component_uri, project).await?;

                Ok(WorkerUrn {
                    id: WorkerId {
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
            let worker_client = self.client_builder.build()?;
            let component_service = self.component_service_builder.build()?;
            AsyncComponentRequest::Async(tokio::spawn(resolve_worker_component_version_no_ref(
                worker_client,
                component_service,
                worker_urn.clone(),
            )))
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
            Ok(GolemResult::Json(res.result))
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
    ) -> Result<GolemResult, GolemError> {
        let worker_urn = self.resolve_uri(worker_uri, project).await?;

        self.client.connect(worker_urn).await?;

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

        self.client.delete(worker_urn).await?;

        Ok(GolemResult::Str("Deleted".to_string()))
    }

    async fn get(
        &self,
        worker_uri: WorkerUri,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError> {
        let worker_urn = self.resolve_uri(worker_uri, project).await?;

        let response: WorkerMetadataView = self.client.get_metadata(worker_urn).await?.into();

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

        let function = component
            .metadata
            .exports
            .iter()
            .flat_map(|exp| match exp {
                Export::Instance(ExportInstance {
                    name: prefix,
                    functions,
                }) => functions
                    .iter()
                    .map(|f| {
                        (
                            format!("{prefix}.{{{}}}", f.name),
                            show_exported_function(&format!("{prefix}."), f),
                        )
                    })
                    .collect::<Vec<_>>(),
                Export::Function(f) => {
                    vec![(f.name.clone(), show_exported_function("", f))]
                }
            })
            .find(|(name, _)| name == function_name);

        match function {
            None => Err(GolemError(format!(
                "Can't find function '{function_name}' in component {component_urn}."
            ))),
            Some((_, function)) => Ok(GolemResult::Str(function)),
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
        let component_urn = self.components.resolve_uri(component_uri, project).await?;

        if count.is_some() {
            let response: WorkersMetadataResponseView = self
                .client
                .list_metadata(component_urn, filter, cursor, count, precise)
                .await?
                .into();

            Ok(GolemResult::Ok(Box::new(response)))
        } else {
            let mut workers: Vec<WorkerMetadata> = vec![];
            let mut new_cursor = cursor;

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
        let _ = self.client.update(worker_urn, mode, target_version).await?;

        Ok(GolemResult::Str("Updated".to_string()))
    }
}

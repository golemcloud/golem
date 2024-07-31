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
use crate::model::component::{function_params_types, Component};
use crate::model::invoke_result_view::InvokeResultView;
use crate::model::text::WorkerAddView;
use crate::model::wave::type_to_analysed;
use crate::model::{
    ComponentId, ComponentIdOrName, Format, GolemError, GolemResult, IdempotencyKey,
    WorkerMetadata, WorkerName, WorkerUpdateMode, WorkersMetadataResponse,
};
use crate::service::component::ComponentService;
use async_trait::async_trait;
use golem_client::model::{
    InvokeParameters, InvokeResult, ScanCursor, StringFilterComparator, Type, WorkerFilter,
    WorkerNameFilter,
};
use golem_common::precise_json::PreciseJson;
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::type_annotated_value_from_str;
use serde_json::Value;
use tokio::task::JoinHandle;
use tracing::{error, info};
use uuid::Uuid;

#[async_trait]
pub trait WorkerService {
    type ProjectContext: Send + Sync;

    async fn add(
        &self,
        component_id_or_name: ComponentIdOrName,
        worker_name: WorkerName,
        env: Vec<(String, String)>,
        args: Vec<String>,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError>;
    async fn idempotency_key(&self) -> Result<GolemResult, GolemError> {
        let key = IdempotencyKey(Uuid::new_v4().to_string());
        Ok(GolemResult::Ok(Box::new(key)))
    }
    async fn invoke_and_await(
        &self,
        format: Format,
        component_id_or_name: ComponentIdOrName,
        worker_name: WorkerName,
        idempotency_key: Option<IdempotencyKey>,
        function: String,
        parameters: Option<Value>,
        wave: Vec<String>,
        use_stdio: bool,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError>;
    async fn invoke(
        &self,
        component_id_or_name: ComponentIdOrName,
        worker_name: WorkerName,
        idempotency_key: Option<IdempotencyKey>,
        function: String,
        parameters: Option<Value>,
        wave: Vec<String>,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError>;
    async fn connect(
        &self,
        component_id_or_name: ComponentIdOrName,
        worker_name: WorkerName,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError>;
    async fn interrupt(
        &self,
        component_id_or_name: ComponentIdOrName,
        worker_name: WorkerName,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError>;
    async fn simulated_crash(
        &self,
        component_id_or_name: ComponentIdOrName,
        worker_name: WorkerName,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError>;
    async fn delete(
        &self,
        component_id_or_name: ComponentIdOrName,
        worker_name: WorkerName,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError>;
    async fn get(
        &self,
        component_id_or_name: ComponentIdOrName,
        worker_name: WorkerName,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError>;
    async fn list(
        &self,
        component_id_or_name: ComponentIdOrName,
        filter: Option<Vec<String>>,
        count: Option<u64>,
        cursor: Option<ScanCursor>,
        precise: Option<bool>,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError>;
    async fn update(
        &self,
        component_id_or_name: ComponentIdOrName,
        worker_name: WorkerName,
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
    component_id: ComponentId,
    worker_name: WorkerName,
) -> Result<Option<Component>, GolemError> {
    resolve_worker_component_version(
        worker_client.as_ref(),
        component_service.as_ref(),
        &component_id,
        worker_name,
    )
    .await
}

async fn resolve_worker_component_version<ProjectContext: Send + Sync>(
    client: &(dyn WorkerClient + Send + Sync),
    components: &(dyn ComponentService<ProjectContext = ProjectContext> + Send + Sync),
    component_id: &ComponentId,
    worker_name: WorkerName,
) -> Result<Option<Component>, GolemError> {
    let worker_meta = client
        .find_metadata(
            component_id.clone(),
            Some(WorkerFilter::Name(WorkerNameFilter {
                comparator: StringFilterComparator::Equal,
                value: worker_name.0,
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
                .get_metadata(component_id, worker.component_version)
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
) -> Result<Vec<PreciseJson>, GolemError> {
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

    let json_params = params
        .into_iter()
        .map(PreciseJson::from)
        .collect::<Vec<_>>();

    Ok(json_params)
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
    component_id: &ComponentId,
    worker_name: &WorkerName,
    parameters: Option<Value>,
    wave: Vec<String>,
    function: &str,
) -> Result<(Value, Option<Component>), GolemError> {
    if let Some(parameters) = parameters {
        Ok((parameters, None))
    } else if let Some(component) =
        resolve_worker_component_version(client, components, component_id, worker_name.clone())
            .await?
    {
        let json = wave_parameters_to_json(&wave, &component, function)?;

        let json_array: Vec<Value> = json.into_iter().map(|v| v.into()).collect();
        Ok((Value::Array(json_array), Some(component)))
    } else {
        info!("No worker found with name {worker_name}. Assuming it should be create with the latest component version");
        let component = components.get_latest_metadata(component_id).await?;

        let json = wave_parameters_to_json(&wave, &component, function)?;
        let json_array: Vec<Value> = json.into_iter().map(|v| v.into()).collect();

        // We are not going to use this component for result parsing.
        Ok((Value::Array(json_array), None))
    }
}

async fn to_invoke_result_view<ProjectContext: Send + Sync>(
    res: InvokeResult,
    async_component_request: AsyncComponentRequest,
    client: &(dyn WorkerClient + Send + Sync),
    components: &(dyn ComponentService<ProjectContext = ProjectContext> + Send + Sync),
    component_id: &ComponentId,
    worker_name: &WorkerName,
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
            match resolve_worker_component_version(
                client,
                components,
                component_id,
                worker_name.clone(),
            )
            .await
            {
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
        component_id_or_name: ComponentIdOrName,
        worker_name: WorkerName,
        env: Vec<(String, String)>,
        args: Vec<String>,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError> {
        let component_id = self
            .components
            .resolve_id(component_id_or_name, project)
            .await?;

        let inst = self
            .client
            .new_worker(worker_name, component_id, args, env)
            .await?;

        Ok(GolemResult::Ok(Box::new(WorkerAddView(inst))))
    }

    async fn invoke_and_await(
        &self,
        format: Format,
        component_id_or_name: ComponentIdOrName,
        worker_name: WorkerName,
        idempotency_key: Option<IdempotencyKey>,
        function: String,
        parameters: Option<Value>,
        wave: Vec<String>,
        use_stdio: bool,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError> {
        let human_readable = format == Format::Text;
        let component_id = self
            .components
            .resolve_id(component_id_or_name, project)
            .await?;

        let (parameters, component_meta) = resolve_parameters(
            self.client.as_ref(),
            self.components.as_ref(),
            &component_id,
            &worker_name,
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
                component_id.clone(),
                worker_name.clone(),
            )))
        } else {
            AsyncComponentRequest::Empty
        };

        let res = self
            .client
            .invoke_and_await(
                worker_name.clone(),
                component_id.clone(),
                function.clone(),
                InvokeParameters { params: parameters },
                idempotency_key,
                use_stdio,
            )
            .await?;

        if human_readable {
            let view = to_invoke_result_view(
                res,
                async_component_request,
                self.client.as_ref(),
                self.components.as_ref(),
                &component_id,
                &worker_name,
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
        component_id_or_name: ComponentIdOrName,
        worker_name: WorkerName,
        idempotency_key: Option<IdempotencyKey>,
        function: String,
        parameters: Option<Value>,
        wave: Vec<String>,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError> {
        let component_id = self
            .components
            .resolve_id(component_id_or_name, project)
            .await?;

        let (parameters, _) = resolve_parameters(
            self.client.as_ref(),
            self.components.as_ref(),
            &component_id,
            &worker_name,
            parameters,
            wave,
            &function,
        )
        .await?;

        self.client
            .invoke(
                worker_name,
                component_id,
                function,
                InvokeParameters { params: parameters },
                idempotency_key,
            )
            .await?;

        Ok(GolemResult::Str("Invoked".to_string()))
    }

    async fn connect(
        &self,
        component_id_or_name: ComponentIdOrName,
        worker_name: WorkerName,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError> {
        let component_id = self
            .components
            .resolve_id(component_id_or_name, project)
            .await?;

        self.client.connect(worker_name, component_id).await?;

        Err(GolemError("Unexpected connection closure".to_string()))
    }

    async fn interrupt(
        &self,
        component_id_or_name: ComponentIdOrName,
        worker_name: WorkerName,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError> {
        let component_id = self
            .components
            .resolve_id(component_id_or_name, project)
            .await?;

        self.client.interrupt(worker_name, component_id).await?;

        Ok(GolemResult::Str("Interrupted".to_string()))
    }

    async fn simulated_crash(
        &self,
        component_id_or_name: ComponentIdOrName,
        worker_name: WorkerName,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError> {
        let component_id = self
            .components
            .resolve_id(component_id_or_name, project)
            .await?;

        self.client
            .simulated_crash(worker_name, component_id)
            .await?;

        Ok(GolemResult::Str("Done".to_string()))
    }

    async fn delete(
        &self,
        component_id_or_name: ComponentIdOrName,
        worker_name: WorkerName,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError> {
        let component_id = self
            .components
            .resolve_id(component_id_or_name, project)
            .await?;

        self.client.delete(worker_name, component_id).await?;

        Ok(GolemResult::Str("Deleted".to_string()))
    }

    async fn get(
        &self,
        component_id_or_name: ComponentIdOrName,
        worker_name: WorkerName,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError> {
        let component_id = self
            .components
            .resolve_id(component_id_or_name, project)
            .await?;

        let response = self.client.get_metadata(worker_name, component_id).await?;

        Ok(GolemResult::Ok(Box::new(response)))
    }

    async fn list(
        &self,
        component_id_or_name: ComponentIdOrName,
        filter: Option<Vec<String>>,
        count: Option<u64>,
        cursor: Option<ScanCursor>,
        precise: Option<bool>,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError> {
        let component_id = self
            .components
            .resolve_id(component_id_or_name, project)
            .await?;

        if count.is_some() {
            let response = self
                .client
                .list_metadata(component_id, filter, cursor, count, precise)
                .await?;

            Ok(GolemResult::Ok(Box::new(response)))
        } else {
            let mut workers: Vec<WorkerMetadata> = vec![];
            let mut new_cursor = cursor;

            loop {
                let response = self
                    .client
                    .list_metadata(
                        component_id.clone(),
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

            Ok(GolemResult::Ok(Box::new(WorkersMetadataResponse {
                workers,
                cursor: None,
            })))
        }
    }

    async fn update(
        &self,
        component_id_or_name: ComponentIdOrName,
        worker_name: WorkerName,
        target_version: u64,
        mode: WorkerUpdateMode,
        project: Option<Self::ProjectContext>,
    ) -> Result<GolemResult, GolemError> {
        let component_id = self
            .components
            .resolve_id(component_id_or_name, project)
            .await?;
        let _ = self
            .client
            .update(worker_name, component_id, mode, target_version)
            .await?;

        Ok(GolemResult::Str("Updated".to_string()))
    }
}

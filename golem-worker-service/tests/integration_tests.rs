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

use async_trait::async_trait;
use golem_common::model::{ComponentId, ComponentVersion};
use golem_service_base::auth::{DefaultNamespace, EmptyAuthCtx};
use golem_service_base::model::VersionedComponentId;
use golem_worker_service_base::gateway_api_definition::http::HttpApiDefinitionRequest as CoreHttpApiDefinitionRequest;
use golem_worker_service_base::gateway_api_definition::http::{
    CompiledHttpApiDefinition, CompiledRoute,
};
use golem_worker_service_base::gateway_api_definition::{ApiDefinitionId, ApiVersion};
use golem_worker_service_base::gateway_binding::gateway_binding_compiled::GatewayBindingCompiled;
use golem_worker_service_base::gateway_binding::worker_binding_compiled::{
    ResponseMappingCompiled, WorkerBindingCompiled,
};
use golem_worker_service_base::service::gateway::api_definition::{
    ApiDefinitionError, ApiDefinitionService, ApiResult,
};
use rib::InferredType;
use std::sync::Arc;

// Mocks
struct MockApiDefinitionService;

#[async_trait]
impl ApiDefinitionService<EmptyAuthCtx, DefaultNamespace> for MockApiDefinitionService {
    async fn create(
        &self,
        _: &CoreHttpApiDefinitionRequest,
        _: &DefaultNamespace,
        _: &EmptyAuthCtx,
    ) -> Result<CompiledHttpApiDefinition<DefaultNamespace>, ApiDefinitionError> {
        Err(ApiDefinitionError::InvalidInput(
            "Not implemented".to_string(),
        ))
    }

    async fn update(
        &self,
        _: &CoreHttpApiDefinitionRequest,
        _: &DefaultNamespace,
        _: &EmptyAuthCtx,
    ) -> Result<CompiledHttpApiDefinition<DefaultNamespace>, ApiDefinitionError> {
        Err(ApiDefinitionError::InvalidInput(
            "Not implemented".to_string(),
        ))
    }

    async fn get(
        &self,
        _id: &ApiDefinitionId,
        _version: &ApiVersion,
        _namespace: &DefaultNamespace,
        _auth_ctx: &EmptyAuthCtx,
    ) -> Result<Option<CompiledHttpApiDefinition<DefaultNamespace>>, ApiDefinitionError> {
        Ok(Some(CompiledHttpApiDefinition {
            id: ApiDefinitionId("test-id".to_string()),
            version: ApiVersion("1.0".to_string()),
            routes: vec![CompiledRoute {
                path: "/test-route".to_string().parse().unwrap(),
                method: "GET".to_string().parse().unwrap(),
                binding: GatewayBindingCompiled::Worker(WorkerBindingCompiled {
                    component_id: VersionedComponentId {
                        component_id: ComponentId(Default::default()),
                        version: ComponentVersion::default(),
                    },
                    worker_name_compiled: None,
                    idempotency_key_compiled: None,
                    response_compiled: ResponseMappingCompiled {
                        response_mapping_expr: rib::Expr::Literal(
                            "test-response".to_string(),
                            InferredType::Bool,
                        ),
                        response_mapping_compiled: rib::RibByteCode {
                            instructions: vec![],
                        },
                        rib_input: rib::RibInputTypeInfo {
                            types: Default::default(),
                        },
                        worker_calls: None,
                        rib_output: None,
                    },
                }),
                middlewares: None,
            }],
            draft: false,
            created_at: Default::default(),
            namespace: DefaultNamespace(),
        }))
    }

    async fn delete(
        &self,
        _: &ApiDefinitionId,
        _: &ApiVersion,
        _: &DefaultNamespace,
        _: &EmptyAuthCtx,
    ) -> Result<(), ApiDefinitionError> {
        Err(ApiDefinitionError::InvalidInput(
            "Not implemented".to_string(),
        ))
    }

    async fn get_all(
        &self,
        _: &DefaultNamespace,
        _: &EmptyAuthCtx,
    ) -> ApiResult<Vec<CompiledHttpApiDefinition<DefaultNamespace>>> {
        Ok(vec![]) // Return empty for simplicity
    }

    async fn get_all_versions(
        &self,
        _: &ApiDefinitionId,
        _: &DefaultNamespace,
        _: &EmptyAuthCtx,
    ) -> ApiResult<Vec<CompiledHttpApiDefinition<DefaultNamespace>>> {
        Ok(vec![]) // Return empty for simplicity
    }
}

// Integration tests
#[cfg(test)]
mod integration_tests {
    use super::*;
    use golem_service_base::auth::{DefaultNamespace, EmptyAuthCtx};
    use golem_worker_service::api::api_definition::RegisterApiDefinitionApi;
    use golem_worker_service_base::api::ApiEndpointError;
    use golem_worker_service_base::gateway_api_definition::{ApiDefinitionId, ApiVersion};
    use poem_openapi::param::Path;

    #[tokio::test]
    async fn test_export_with_valid_definition() {
        let definition_service = Arc::new(MockApiDefinitionService);
        let api = RegisterApiDefinitionApi::new(definition_service);

        let id = Path(ApiDefinitionId("test-id".to_string()));
        let version = Path(ApiVersion("1.0".to_string()));

        let response = api.export(id, version).await;


        if let Err(e) = &response {
            println!("Error: {:?}", e);
        }

        assert!(response.is_ok());

        let openapi_spec = response.unwrap().0;
        assert_eq!(openapi_spec.openapi, "3.0.0");
        assert_eq!(openapi_spec.info.title, "Generated API");
        assert!(openapi_spec.paths.contains_key("/test-route"));
    }

    #[tokio::test]
    async fn test_export_with_nonexistent_definition() {
        struct EmptyDefinitionService;

        #[async_trait]
        impl ApiDefinitionService<EmptyAuthCtx, DefaultNamespace> for EmptyDefinitionService {
            async fn create(
                &self,
                _: &CoreHttpApiDefinitionRequest,
                _: &DefaultNamespace,
                _: &EmptyAuthCtx,
            ) -> Result<CompiledHttpApiDefinition<DefaultNamespace>, ApiDefinitionError>
            {
                unimplemented!()
            }

            async fn update(
                &self,
                _: &CoreHttpApiDefinitionRequest,
                _: &DefaultNamespace,
                _: &EmptyAuthCtx,
            ) -> Result<CompiledHttpApiDefinition<DefaultNamespace>, ApiDefinitionError>
            {
                unimplemented!()
            }

            async fn get(
                &self,
                _: &ApiDefinitionId,
                _: &ApiVersion,
                _: &DefaultNamespace,
                _: &EmptyAuthCtx,
            ) -> Result<Option<CompiledHttpApiDefinition<DefaultNamespace>>, ApiDefinitionError>
            {
                Ok(None) // Simulate missing definition
            }

            async fn delete(
                &self,
                _: &ApiDefinitionId,
                _: &ApiVersion,
                _: &DefaultNamespace,
                _: &EmptyAuthCtx,
            ) -> Result<(), ApiDefinitionError> {
                unimplemented!()
            }

            async fn get_all(
                &self,
                _: &DefaultNamespace,
                _: &EmptyAuthCtx,
            ) -> ApiResult<Vec<CompiledHttpApiDefinition<DefaultNamespace>>> {
                Ok(vec![]) // Return empty for simplicity
            }

            async fn get_all_versions(
                &self,
                _: &ApiDefinitionId,
                _: &DefaultNamespace,
                _: &EmptyAuthCtx,
            ) -> ApiResult<Vec<CompiledHttpApiDefinition<DefaultNamespace>>> {
                Ok(vec![]) // Return empty for simplicity
            }
        }

        let definition_service = Arc::new(EmptyDefinitionService);
        let api = RegisterApiDefinitionApi::new(definition_service);

        let id = Path(ApiDefinitionId("invalid-id".to_string()));
        let version = Path(ApiVersion("1.0".to_string()));

        let response = api.export(id, version).await;
        assert!(response.is_err());

        if let Err(ApiEndpointError::NotFound(message)) = response {
            assert!(message.0.error.contains("No API definition found"));
        } else {
            panic!("Expected ApiEndpointError::NotFound");
        }
    }
}

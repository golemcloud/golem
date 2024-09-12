use std::fmt::Display;

use std::io::Read;

use async_trait::async_trait;

use golem_cloud_client::model::{
    AnalysedResourceMode, AnalysedType, GolemWorkerBindingWithTypeInfo, HttpApiDefinitionRequest,
    HttpApiDefinitionWithTypeInfo, MethodPattern, RibInputTypeInfo, RouteWithTypeInfo,
};

use golem_cli::clients::api_definition::ApiDefinitionClient;
use golem_cli::cloud::ProjectId;
use tokio::fs::read_to_string;
use tracing::info;

use crate::cloud::clients::errors::CloudGolemError;
use golem_cli::model::{ApiDefinitionId, ApiDefinitionVersion, GolemError, PathBufOrStdin};

#[derive(Clone)]
pub struct ApiDefinitionClientLive<C: golem_cloud_client::api::ApiDefinitionClient + Sync + Send> {
    pub client: C,
}

#[derive(Debug, Copy, Clone)]
enum Action {
    Create,
    Update,
    Import,
}

impl Display for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let str = match self {
            Action::Create => "Creating",
            Action::Update => "Updating",
            Action::Import => "Importing",
        };
        write!(f, "{}", str)
    }
}

async fn create_or_update_api_definition<
    C: golem_cloud_client::api::ApiDefinitionClient + Sync + Send,
>(
    action: Action,
    client: &C,
    path: PathBufOrStdin,
    project_id: &ProjectId,
) -> Result<golem_client::model::HttpApiDefinitionWithTypeInfo, GolemError> {
    info!("{action} api definition from {path:?}");

    let definition_str: String = match path {
        PathBufOrStdin::Path(path) => read_to_string(path)
            .await
            .map_err(|e| GolemError(format!("Failed to read from file: {e:?}")))?,
        PathBufOrStdin::Stdin => {
            let mut content = String::new();

            let _ = std::io::stdin()
                .read_to_string(&mut content)
                .map_err(|e| GolemError(format!("Failed to read stdin: {e:?}")))?;

            content
        }
    };

    let res = match action {
        Action::Import => {
            let value: serde_json::value::Value = serde_json::from_str(definition_str.as_str())
                .map_err(|e| GolemError(format!("Failed to parse json: {e:?}")))?;

            client
                .import_open_api(&project_id.0, &value)
                .await
                .map_err(CloudGolemError::from)?
        }
        Action::Create => {
            let value: HttpApiDefinitionRequest = serde_json::from_str(definition_str.as_str())
                .map_err(|e| GolemError(format!("Failed to parse HttpApiDefinition: {e:?}")))?;

            client
                .create_definition(&project_id.0, &value)
                .await
                .map_err(CloudGolemError::from)?
        }
        Action::Update => {
            let value: HttpApiDefinitionRequest = serde_json::from_str(definition_str.as_str())
                .map_err(|e| GolemError(format!("Failed to parse HttpApiDefinition: {e:?}")))?;

            client
                .update_definition(&project_id.0, &value.id, &value.version, &value)
                .await
                .map_err(CloudGolemError::from)?
        }
    };

    Ok(to_oss_http_api_definition(res))
}

fn to_oss_method_pattern(p: MethodPattern) -> golem_client::model::MethodPattern {
    match p {
        MethodPattern::Get => golem_client::model::MethodPattern::Get,
        MethodPattern::Connect => golem_client::model::MethodPattern::Connect,
        MethodPattern::Post => golem_client::model::MethodPattern::Post,
        MethodPattern::Delete => golem_client::model::MethodPattern::Delete,
        MethodPattern::Put => golem_client::model::MethodPattern::Put,
        MethodPattern::Patch => golem_client::model::MethodPattern::Patch,
        MethodPattern::Options => golem_client::model::MethodPattern::Options,
        MethodPattern::Trace => golem_client::model::MethodPattern::Trace,
        MethodPattern::Head => golem_client::model::MethodPattern::Head,
    }
}

fn to_oss_rib_type_info(v: RibInputTypeInfo) -> golem_client::model::RibInputTypeInfo {
    golem_client::model::RibInputTypeInfo {
        types: v
            .types
            .into_iter()
            .map(|(k, v)| (k, to_oss_analysed_type(v)))
            .collect(),
    }
}

fn to_oss_analysed_type(v: AnalysedType) -> golem_client::model::AnalysedType {
    match v {
        AnalysedType::Variant(t) => {
            let cases = t
                .cases
                .into_iter()
                .map(|t| golem_client::model::NameOptionTypePair {
                    name: t.name,
                    typ: t.typ.map(to_oss_analysed_type),
                })
                .collect();
            golem_client::model::AnalysedType::Variant(golem_client::model::TypeVariant { cases })
        }
        AnalysedType::Result(t) => {
            let ok: Option<AnalysedType> = t.ok;
            let err: Option<AnalysedType> = t.err;
            golem_client::model::AnalysedType::Result(Box::new(golem_client::model::TypeResult {
                ok: ok.map(to_oss_analysed_type),
                err: err.map(to_oss_analysed_type),
            }))
        }
        AnalysedType::Option(t) => {
            let v: AnalysedType = t.inner;
            golem_client::model::AnalysedType::Option(Box::new(golem_client::model::TypeOption {
                inner: to_oss_analysed_type(v),
            }))
        }
        AnalysedType::Enum(t) => {
            golem_client::model::AnalysedType::Enum(golem_client::model::TypeEnum {
                cases: t.cases,
            })
        }
        AnalysedType::Flags(t) => {
            golem_client::model::AnalysedType::Flags(golem_client::model::TypeFlags {
                names: t.names,
            })
        }
        AnalysedType::Record(t) => {
            let fields = t
                .fields
                .into_iter()
                .map(|t| golem_client::model::NameTypePair {
                    name: t.name,
                    typ: to_oss_analysed_type(t.typ),
                })
                .collect();
            golem_client::model::AnalysedType::Record(golem_client::model::TypeRecord { fields })
        }
        AnalysedType::Tuple(t) => {
            let items = t.items.into_iter().map(to_oss_analysed_type).collect();
            golem_client::model::AnalysedType::Tuple(golem_client::model::TypeTuple { items })
        }
        AnalysedType::List(list) => {
            let v: AnalysedType = list.inner;
            golem_client::model::AnalysedType::List(Box::new(golem_client::model::TypeList {
                inner: to_oss_analysed_type(v),
            }))
        }
        AnalysedType::Str(_) => {
            golem_client::model::AnalysedType::Str(golem_client::model::TypeStr {})
        }
        AnalysedType::Chr(_) => {
            golem_client::model::AnalysedType::Chr(golem_client::model::TypeChr {})
        }
        AnalysedType::F64(_) => {
            golem_client::model::AnalysedType::F64(golem_client::model::TypeF64 {})
        }
        AnalysedType::F32(_) => {
            golem_client::model::AnalysedType::F32(golem_client::model::TypeF32 {})
        }
        AnalysedType::U64(_) => {
            golem_client::model::AnalysedType::U64(golem_client::model::TypeU64 {})
        }
        AnalysedType::S64(_) => {
            golem_client::model::AnalysedType::S64(golem_client::model::TypeS64 {})
        }
        AnalysedType::U32(_) => {
            golem_client::model::AnalysedType::U32(golem_client::model::TypeU32 {})
        }
        AnalysedType::S32(_) => {
            golem_client::model::AnalysedType::S32(golem_client::model::TypeS32 {})
        }
        AnalysedType::U16(_) => {
            golem_client::model::AnalysedType::U16(golem_client::model::TypeU16 {})
        }
        AnalysedType::S16(_) => {
            golem_client::model::AnalysedType::S16(golem_client::model::TypeS16 {})
        }
        AnalysedType::U8(_) => {
            golem_client::model::AnalysedType::U8(golem_client::model::TypeU8 {})
        }
        AnalysedType::S8(_) => {
            golem_client::model::AnalysedType::S8(golem_client::model::TypeS8 {})
        }
        AnalysedType::Bool(_) => {
            golem_client::model::AnalysedType::Bool(golem_client::model::TypeBool {})
        }
        AnalysedType::Handle(h) => {
            let mode = match h.mode {
                AnalysedResourceMode::Owned => golem_client::model::AnalysedResourceMode::Owned,
                AnalysedResourceMode::Borrowed => {
                    golem_client::model::AnalysedResourceMode::Borrowed
                }
            };
            golem_client::model::AnalysedType::Handle(golem_client::model::TypeHandle {
                resource_id: h.resource_id,
                mode,
            })
        }
    }
}

fn to_oss_golem_worker_binding(
    b: GolemWorkerBindingWithTypeInfo,
) -> golem_client::model::GolemWorkerBindingWithTypeInfo {
    let GolemWorkerBindingWithTypeInfo {
        component_id,
        worker_name,
        idempotency_key,
        response,
        response_mapping_input,
        worker_name_input,
        idempotency_key_input,
    } = b;

    golem_client::model::GolemWorkerBindingWithTypeInfo {
        component_id: golem_client::model::VersionedComponentId {
            component_id: component_id.component_id,
            version: component_id.version,
        },
        worker_name,
        idempotency_key,
        response,
        response_mapping_input: response_mapping_input.map(to_oss_rib_type_info),
        worker_name_input: worker_name_input.map(to_oss_rib_type_info),
        idempotency_key_input: idempotency_key_input.map(to_oss_rib_type_info),
    }
}

fn to_oss_route(r: RouteWithTypeInfo) -> golem_client::model::RouteWithTypeInfo {
    let RouteWithTypeInfo {
        method,
        path,
        binding,
    } = r;

    golem_client::model::RouteWithTypeInfo {
        method: to_oss_method_pattern(method),
        path,
        binding: to_oss_golem_worker_binding(binding),
    }
}

fn to_oss_http_api_definition(
    d: HttpApiDefinitionWithTypeInfo,
) -> golem_client::model::HttpApiDefinitionWithTypeInfo {
    let HttpApiDefinitionWithTypeInfo {
        id,
        version,
        routes,
        draft,
        created_at,
    } = d;

    golem_client::model::HttpApiDefinitionWithTypeInfo {
        id,
        version,
        routes: routes.into_iter().map(to_oss_route).collect(),
        draft,
        created_at,
    }
}

#[async_trait]
impl<C: golem_cloud_client::api::ApiDefinitionClient + Sync + Send> ApiDefinitionClient
    for ApiDefinitionClientLive<C>
{
    type ProjectContext = ProjectId;

    async fn list(
        &self,
        id: Option<&ApiDefinitionId>,
        project: &Self::ProjectContext,
    ) -> Result<Vec<golem_client::model::HttpApiDefinitionWithTypeInfo>, GolemError> {
        info!("Getting api definitions");

        let definitions = self
            .client
            .list_definitions(&project.0, id.map(|id| id.0.as_str()))
            .await
            .map_err(CloudGolemError::from)?;

        Ok(definitions
            .into_iter()
            .map(to_oss_http_api_definition)
            .collect())
    }

    async fn get(
        &self,
        id: ApiDefinitionId,
        version: ApiDefinitionVersion,
        project: &Self::ProjectContext,
    ) -> Result<golem_client::model::HttpApiDefinitionWithTypeInfo, GolemError> {
        info!("Getting api definition for {}/{}", id.0, version.0);

        let definition = self
            .client
            .get_definition(&project.0, id.0.as_str(), version.0.as_str())
            .await
            .map_err(CloudGolemError::from)?;

        Ok(to_oss_http_api_definition(definition))
    }

    async fn create(
        &self,
        path: PathBufOrStdin,
        project: &Self::ProjectContext,
    ) -> Result<golem_client::model::HttpApiDefinitionWithTypeInfo, GolemError> {
        create_or_update_api_definition(Action::Create, &self.client, path, project).await
    }

    async fn update(
        &self,
        path: PathBufOrStdin,
        project: &Self::ProjectContext,
    ) -> Result<golem_client::model::HttpApiDefinitionWithTypeInfo, GolemError> {
        create_or_update_api_definition(Action::Update, &self.client, path, project).await
    }

    async fn import(
        &self,
        path: PathBufOrStdin,
        project: &Self::ProjectContext,
    ) -> Result<golem_client::model::HttpApiDefinitionWithTypeInfo, GolemError> {
        create_or_update_api_definition(Action::Import, &self.client, path, project).await
    }

    async fn delete(
        &self,
        id: ApiDefinitionId,
        version: ApiDefinitionVersion,
        project: &Self::ProjectContext,
    ) -> Result<String, GolemError> {
        info!("Deleting api definition for {}/{}", id.0, version.0);
        Ok(self
            .client
            .delete_definition(&project.0, id.0.as_str(), version.0.as_str())
            .await
            .map_err(CloudGolemError::from)?)
    }
}

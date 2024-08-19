use crate::worker_binding::{GolemWorkerBinding, ResponseMapping};
use bincode::{Decode, Encode};
use golem_service_base::model::VersionedComponentId;
use golem_wasm_ast::analysis::{AnalysedExport, AnalysedType, TypeStr, TypeU64};
use rib::{Expr, RibByteCode, RibInputTypeInfo};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub struct CompiledGolemWorkerBinding {
    pub component_id: VersionedComponentId,
    pub worker_name_compiled: WorkerNameCompiled,
    pub idempotency_key_compiled: Option<IdempotencyKeyCompiled>,
    pub response_compiled: ResponseMappingCompiled,
}

impl CompiledGolemWorkerBinding {
    pub fn from_golem_worker_binding(
        golem_worker_binding: &GolemWorkerBinding,
        export_metadata: &Vec<AnalysedExport>,
    ) -> Result<Self, String> {
        let worker_name_compiled = WorkerNameCompiled::from_worker_name(
            &golem_worker_binding.worker_name,
            export_metadata,
        )?;
        let idempotency_key_compiled = match &golem_worker_binding.idempotency_key {
            Some(idempotency_key) => Some(IdempotencyKeyCompiled::from_idempotency_key(
                idempotency_key,
                export_metadata,
            )?),
            None => None,
        };
        let response_compiled = ResponseMappingCompiled::from_response_mapping(
            &golem_worker_binding.response,
            export_metadata,
        )?;

        Ok(CompiledGolemWorkerBinding {
            component_id: golem_worker_binding.component_id.clone(),
            worker_name_compiled,
            idempotency_key_compiled,
            response_compiled,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub struct WorkerNameCompiled {
    pub worker_name: Expr,
    pub compiled_worker_name: RibByteCode,
    pub rib_input: RibInputTypeInfo,
}

impl WorkerNameCompiled {
    pub fn from_worker_name(
        worker_name: &Expr,
        exports: &Vec<AnalysedExport>,
    ) -> Result<Self, String> {
        let worker_name_compiled =
            rib::compile_pure(worker_name, exports, Some(AnalysedType::U64(TypeU64)))?;

        Ok(WorkerNameCompiled {
            worker_name: worker_name.clone(),
            compiled_worker_name: worker_name_compiled.byte_code,
            rib_input: worker_name_compiled.rib_input,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub struct IdempotencyKeyCompiled {
    pub idempotency_key: Expr,
    pub compiled_idempotency_key: RibByteCode,
    pub rib_input: RibInputTypeInfo,
}

impl IdempotencyKeyCompiled {
    pub fn from_idempotency_key(
        idempotency_key: &Expr,
        exports: &Vec<AnalysedExport>,
    ) -> Result<Self, String> {
        let idempotency_key_compiled =
            rib::compile_pure(idempotency_key, exports, Some(AnalysedType::U64(TypeU64)))?;

        Ok(IdempotencyKeyCompiled {
            idempotency_key: idempotency_key.clone(),
            compiled_idempotency_key: idempotency_key_compiled.byte_code,
            rib_input: idempotency_key_compiled.rib_input,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub struct ResponseMappingCompiled {
    pub response_rib_expr: Expr,
    pub compiled_response: RibByteCode,
    pub rib_input: RibInputTypeInfo,
}

impl ResponseMappingCompiled {
    pub fn from_response_mapping(
        response_mapping: &ResponseMapping,
        exports: &Vec<AnalysedExport>,
    ) -> Result<Self, String> {
        let response_compiled = rib::compile(&response_mapping.0, exports)?;

        Ok(ResponseMappingCompiled {
            response_rib_expr: response_mapping.0.clone(),
            compiled_response: response_compiled.byte_code,
            rib_input: response_compiled.rib_input,
        })
    }
}

impl TryFrom<golem_api_grpc::proto::golem::apidefinition::CompiledWorkerBinding>
    for CompiledGolemWorkerBinding
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::apidefinition::CompiledWorkerBinding,
    ) -> Result<Self, Self::Error> {
        let component_id = value
            .component
            .ok_or("Missing component".to_string())
            .and_then(|x| VersionedComponentId::try_from(x))?;
        let worker_name_compiled = value
            .compiled_worker_name_expr
            .ok_or("Missing compiled worker name expr".to_string())
            .and_then(|x| RibByteCode::try_from(x))?;
        let worker_name_input = value
            .worker_name_rib_input
            .ok_or("Missing worker name rib input".to_string())
            .and_then(|x| RibInputTypeInfo::try_from(x))?;
        let idempotency_key_compiled = match value.compiled_idempotency_key_expr {
            Some(x) => Some(RibByteCode::try_from(x)?),
            None => None,
        };
        let idempotency_key_input = match value.idempotency_key_rib_input {
            Some(x) => Some(RibInputTypeInfo::try_from(x)?),
            None => None,
        };

        let response_compiled = value
            .compiled_response_expr
            .ok_or("Missing compiled response".to_string())
            .and_then(|x| RibByteCode::try_from(x))?;
        let response_input = value
            .response_rib_input
            .ok_or("Missing response rib input".to_string())
            .and_then(|x| RibInputTypeInfo::try_from(x))?;

        let worker_name_compiled = WorkerNameCompiled {
            worker_name: value
                .worker_name
                .ok_or("Missing worker name".to_string())
                .and_then(|x| Expr::try_from(x))?,
            compiled_worker_name: worker_name_compiled,
            rib_input: worker_name_input,
        };

        let idempotency_key_compiled = match (idempotency_key_compiled, idempotency_key_input) {
            (Some(compiled), Some(input)) => Some(IdempotencyKeyCompiled {
                idempotency_key: value
                    .idempotency_key
                    .ok_or("Missing idempotency key".to_string())
                    .and_then(|x| Expr::try_from(x))?,
                compiled_idempotency_key: compiled,
                rib_input: input,
            }),
            (None, None) => None,
            _ => return Err("Missing idempotency key".to_string()),
        };

        let response_compiled = ResponseMappingCompiled {
            response_rib_expr: value
                .response
                .ok_or("Missing response".to_string())
                .and_then(|x| Expr::try_from(x))?,
            compiled_response: response_compiled,
            rib_input: response_input,
        };

        Ok(CompiledGolemWorkerBinding {
            component_id,
            worker_name_compiled,
            idempotency_key_compiled,
            response_compiled,
        })
    }
}

impl TryFrom<CompiledGolemWorkerBinding>
    for golem_api_grpc::proto::golem::apidefinition::CompiledWorkerBinding
{
    type Error = String;

    fn try_from(value: CompiledGolemWorkerBinding) -> Result<Self, Self::Error> {
        let component = Some(value.component_id.into());
        let worker_name = Some(value.worker_name_compiled.worker_name.into());
        let compiled_worker_name_expr =
            Some(value.worker_name_compiled.compiled_worker_name.into());
        let worker_name_rib_input = Some(value.worker_name_compiled.rib_input.into());
        let (idempotency_key, compiled_idempotency_key_expr, idempotency_key_rib_input) =
            match value.idempotency_key_compiled {
                Some(x) => (
                    Some(x.idempotency_key.into()),
                    Some(x.compiled_idempotency_key.into()),
                    Some(x.rib_input.into()),
                ),
                None => (None, None, None),
            };

        let response = Some(value.response_compiled.response_rib_expr.into());
        let compiled_response_expr = Some(value.response_compiled.compiled_response.into());
        let response_rib_input = Some(value.response_compiled.rib_input.into());

        Ok(
            golem_api_grpc::proto::golem::apidefinition::CompiledWorkerBinding {
                component,
                worker_name,
                compiled_worker_name_expr,
                worker_name_rib_input,
                idempotency_key,
                compiled_idempotency_key_expr,
                idempotency_key_rib_input,
                response,
                compiled_response_expr,
                response_rib_input,
            },
        )
    }
}

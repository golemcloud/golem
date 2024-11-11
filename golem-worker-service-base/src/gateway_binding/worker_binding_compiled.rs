use crate::gateway_binding::{ResponseMapping, WorkerBinding};
use crate::gateway_middleware::Middlewares;
use crate::gateway_rib_compiler::{DefaultRibCompiler, WorkerServiceRibCompiler};
use bincode::{Decode, Encode};
use golem_common::model::GatewayBindingType;
use golem_service_base::model::VersionedComponentId;
use golem_wasm_ast::analysis::AnalysedExport;
use rib::{Expr, RibByteCode, RibInputTypeInfo, WorkerFunctionsInRib};

#[derive(Debug, Clone, PartialEq)]
pub struct WorkerBindingCompiled {
    pub component_id: VersionedComponentId,
    pub worker_name_compiled: Option<WorkerNameCompiled>,
    pub idempotency_key_compiled: Option<IdempotencyKeyCompiled>,
    pub response_compiled: ResponseMappingCompiled,
    pub middlewares: Option<Middlewares>,
}

impl WorkerBindingCompiled {
    pub fn from_raw_worker_binding(
        gateway_worker_binding: &WorkerBinding,
        export_metadata: &[AnalysedExport],
    ) -> Result<Self, String> {
        let worker_name_compiled: Option<WorkerNameCompiled> = gateway_worker_binding
            .worker_name
            .clone()
            .map(|worker_name_expr| {
                WorkerNameCompiled::from_worker_name(&worker_name_expr, export_metadata)
            })
            .transpose()?;

        let idempotency_key_compiled = match &gateway_worker_binding.idempotency_key {
            Some(idempotency_key) => Some(IdempotencyKeyCompiled::from_idempotency_key(
                idempotency_key,
                export_metadata,
            )?),
            None => None,
        };
        let response_compiled = ResponseMappingCompiled::from_response_mapping(
            &gateway_worker_binding.response_mapping,
            export_metadata,
        )?;

        let middleware = gateway_worker_binding.middleware.clone();

        Ok(WorkerBindingCompiled {
            component_id: gateway_worker_binding.component_id.clone(),
            worker_name_compiled,
            idempotency_key_compiled,
            response_compiled,
            middlewares: middleware,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct WorkerNameCompiled {
    pub worker_name: Expr,
    pub compiled_worker_name: RibByteCode,
    pub rib_input_type_info: RibInputTypeInfo,
}

impl WorkerNameCompiled {
    pub fn from_worker_name(
        worker_name: &Expr,
        exports: &[AnalysedExport],
    ) -> Result<Self, String> {
        let compiled_worker_name = DefaultRibCompiler::compile(worker_name, exports)?;

        Ok(WorkerNameCompiled {
            worker_name: worker_name.clone(),
            compiled_worker_name: compiled_worker_name.byte_code,
            rib_input_type_info: compiled_worker_name.global_input_type_info,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct IdempotencyKeyCompiled {
    pub idempotency_key: Expr,
    pub compiled_idempotency_key: RibByteCode,
    pub rib_input: RibInputTypeInfo,
}

impl IdempotencyKeyCompiled {
    pub fn from_idempotency_key(
        idempotency_key: &Expr,
        exports: &[AnalysedExport],
    ) -> Result<Self, String> {
        let idempotency_key_compiled = DefaultRibCompiler::compile(idempotency_key, exports)?;

        Ok(IdempotencyKeyCompiled {
            idempotency_key: idempotency_key.clone(),
            compiled_idempotency_key: idempotency_key_compiled.byte_code,
            rib_input: idempotency_key_compiled.global_input_type_info,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResponseMappingCompiled {
    pub response_mapping_expr: Expr,
    pub response_mapping_compiled: RibByteCode,
    pub rib_input: RibInputTypeInfo,
    pub worker_calls: Option<WorkerFunctionsInRib>,
}

impl ResponseMappingCompiled {
    pub fn from_response_mapping(
        response_mapping: &ResponseMapping,
        exports: &[AnalysedExport],
    ) -> Result<Self, String> {
        let response_compiled = DefaultRibCompiler::compile(&response_mapping.0, exports)?;

        Ok(ResponseMappingCompiled {
            response_mapping_expr: response_mapping.0.clone(),
            response_mapping_compiled: response_compiled.byte_code,
            rib_input: response_compiled.global_input_type_info,
            worker_calls: response_compiled.worker_invoke_calls,
        })
    }
}

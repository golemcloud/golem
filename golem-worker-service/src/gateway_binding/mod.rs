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

mod gateway_binding_compiled;
mod http_handler_binding;
mod static_binding;
mod worker_binding;

pub(crate) use self::http_handler_binding::*;
pub(crate) use self::worker_binding::*;
pub(crate) use crate::gateway_execution::gateway_binding_resolver::*;
use crate::gateway_rib_compiler::DefaultWorkerServiceRibCompiler;
use crate::gateway_rib_compiler::WorkerServiceRibCompiler;
pub(crate) use gateway_binding_compiled::*;
use golem_common::model::component::VersionedComponentId;
use rib::{ComponentDependency, Expr, RibByteCode, RibCompilationError, RibInputTypeInfo};
pub use static_binding::*;

// A gateway binding is integration to the backend. This is similar to AWS's x-amazon-gateway-integration
// where it holds the details of where to re-route.

// The default integration is `worker`
// Certain integrations can exist as a static binding, which is restricted
// from anything dynamic in nature. This implies, there will not be Rib in either pre-compiled or raw form.
#[derive(Debug, Clone, PartialEq)]
pub enum GatewayBinding {
    Default(Box<WorkerBinding>),
    FileServer(Box<FileServerBinding>),
    Static(StaticBinding),
    HttpHandler(Box<HttpHandlerBinding>),
}

impl GatewayBinding {
    pub fn is_http_cors_binding(&self) -> bool {
        match self {
            Self::Default(_) => false,
            Self::FileServer(_) => false,
            Self::HttpHandler(_) => false,
            Self::Static(s) => match s {
                StaticBinding::HttpCorsPreflight(_) => true,
                StaticBinding::HttpAuthCallBack(_) => false,
            },
        }
    }

    pub fn is_security_binding(&self) -> bool {
        match self {
            Self::Default(_) => false,
            Self::FileServer(_) => false,
            Self::HttpHandler(_) => false,
            Self::Static(s) => match s {
                StaticBinding::HttpCorsPreflight(_) => false,
                StaticBinding::HttpAuthCallBack(_) => true,
            },
        }
    }

    pub fn static_binding(value: StaticBinding) -> GatewayBinding {
        GatewayBinding::Static(value)
    }

    pub fn get_component_id(&self) -> Option<VersionedComponentId> {
        match self {
            Self::Default(worker_binding) => Some(worker_binding.component_id.clone()),
            Self::FileServer(worker_binding) => Some(worker_binding.component_id.clone()),
            Self::HttpHandler(http_handler_binding) => {
                Some(http_handler_binding.component_id.clone())
            }
            Self::Static(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkerNameCompiled {
    pub worker_name: Expr,
    pub compiled_worker_name: RibByteCode,
    pub rib_input_type_info: RibInputTypeInfo,
}

impl WorkerNameCompiled {
    pub fn from_worker_name(worker_name: &Expr) -> Result<Self, RibCompilationError> {
        let compiled_worker_name = DefaultWorkerServiceRibCompiler::compile(worker_name, &[])?;

        Ok(WorkerNameCompiled {
            worker_name: worker_name.clone(),
            compiled_worker_name: compiled_worker_name.byte_code,
            rib_input_type_info: compiled_worker_name.rib_input_type_info,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct IdempotencyKeyCompiled {
    pub idempotency_key: Expr,
    pub compiled_idempotency_key: RibByteCode,
    pub rib_input: RibInputTypeInfo,
}

impl IdempotencyKeyCompiled {
    pub fn from_idempotency_key(idempotency_key: &Expr) -> Result<Self, RibCompilationError> {
        let idempotency_key_compiled =
            DefaultWorkerServiceRibCompiler::compile(idempotency_key, &[])?;

        Ok(IdempotencyKeyCompiled {
            idempotency_key: idempotency_key.clone(),
            compiled_idempotency_key: idempotency_key_compiled.byte_code,
            rib_input: idempotency_key_compiled.rib_input_type_info,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct InvocationContextCompiled {
    pub invocation_context: Expr,
    pub compiled_invocation_context: RibByteCode,
    pub rib_input: RibInputTypeInfo,
}

impl InvocationContextCompiled {
    pub fn from_invocation_context(
        invocation_context: &Expr,
        exports: &[ComponentDependency],
    ) -> Result<Self, RibCompilationError> {
        let invocation_context_compiled =
            DefaultWorkerServiceRibCompiler::compile(invocation_context, exports)?;

        Ok(InvocationContextCompiled {
            invocation_context: invocation_context.clone(),
            compiled_invocation_context: invocation_context_compiled.byte_code,
            rib_input: invocation_context_compiled.rib_input_type_info,
        })
    }
}

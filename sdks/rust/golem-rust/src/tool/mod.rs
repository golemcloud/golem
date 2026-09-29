// Copyright 2024-2026 Golem Cloud
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

#[cfg(not(feature = "export_golem_agentic"))]
pub use crate::bindings::golem::agent::common::Principal;
#[cfg(not(feature = "export_golem_agentic"))]
pub use crate::bindings::golem::tool::streams::ToolStdoutWriter;
#[cfg(feature = "export_golem_agentic")]
pub use crate::golem_agentic::golem::agent::common::Principal;
#[cfg(feature = "export_golem_agentic")]
pub use crate::golem_agentic::golem::tool::streams::ToolStdoutWriter;
pub use crate::schema::tool::Tool;
pub use crate::schema::tool::{
    MonomorphicToolMiddlewareScope, ToolMiddleware, ToolMiddlewareScope,
};
pub use tool_middleware::{
    InputStream, InvocationResult, RawCustomToolError, ToolInvokeError, TypedUnderlyingInvocation,
    UnderlyingInvocation, UnderlyingTool, decode_result_empty, decode_result_stdout_only,
    decode_result_value, decode_result_with_stdout,
};

#[doc(hidden)]
#[derive(crate::IntoSchema, crate::FromSchema)]
pub struct EmptyMiddlewareParameters {}

#[doc(hidden)]
pub fn decode_middleware_parameters<T>(
    parameters: crate::TypedSchemaValue,
) -> Result<T, ToolInvokeError<RawCustomToolError>>
where
    T: crate::IntoSchema + crate::FromSchema,
{
    let expected = crate::schema::try_into_schema_graph::<T>()
        .map_err(|error| ToolInvokeError::InvalidInput(error.to_string()))?;
    if parameters.graph() != &expected {
        return Err(ToolInvokeError::InvalidInput(
            "tool middleware installation parameters do not match the declared schema".to_string(),
        ));
    }
    T::from_value(parameters.value())
        .map_err(|error| ToolInvokeError::InvalidInput(error.to_string()))
}
pub use tool_middleware::OutputStream;
#[doc(hidden)]
pub use tool_middleware::{ToolMiddlewareInvokeFuture, ToolMiddlewareInvokeFutureFor};
#[doc(hidden)]
pub use tool_middleware_registry::{
    ToolMiddlewareInvoker, get_all_tool_middlewares, get_tool_middleware_by_name,
    get_tool_middleware_invoker_by_name, register_tool_middleware,
};

#[cfg(any(test, feature = "export_golem_agentic"))]
pub(crate) use crate::schema::tool::wit::wire;

#[cfg(any(test, feature = "export_golem_agentic"))]
pub(crate) mod invocation_result;
mod tool_middleware;
#[cfg(feature = "export_golem_agentic")]
mod tool_middleware_impl;
#[cfg(feature = "export_golem_agentic")]
#[doc(hidden)]
pub use tool_middleware_impl::install_middleware_exports;
mod tool_middleware_registry;

#[doc(hidden)]
pub trait ToolUnderlying: Sized {
    fn __golem_from_underlying(underlying: UnderlyingTool) -> Self;

    fn __golem_tool_descriptor() -> Tool;
}

#[cfg(test)]
mod parameter_tests {
    use super::*;
    use crate::{FromSchema, IntoSchema, IntoTypedSchemaValue};
    use test_r::test;

    #[derive(Debug, PartialEq, IntoSchema, FromSchema)]
    struct NestedParameters {
        prefix: String,
        rules: Vec<Rule>,
    }

    #[derive(Debug, PartialEq, IntoSchema, FromSchema)]
    struct Rule {
        label: String,
        enabled: bool,
    }

    #[test]
    fn decodes_distinct_invocation_local_parameter_values() {
        for prefix in ["first", "second"] {
            let value = NestedParameters {
                prefix: prefix.to_string(),
                rules: vec![Rule {
                    label: "nested".to_string(),
                    enabled: true,
                }],
            };
            let decoded = decode_middleware_parameters::<NestedParameters>(
                value.into_typed_schema_value().unwrap(),
            )
            .unwrap();
            assert_eq!(decoded, value);
        }
    }

    #[test]
    fn rejects_parameter_value_with_wrong_declared_type() {
        let error = decode_middleware_parameters::<NestedParameters>(
            "not parameters"
                .to_string()
                .into_typed_schema_value()
                .unwrap(),
        )
        .unwrap_err();
        assert!(matches!(error, ToolInvokeError::InvalidInput(_)));
    }

    #[test]
    fn omitted_parameters_use_an_empty_record_schema() {
        let schema = crate::schema::try_into_schema_graph::<EmptyMiddlewareParameters>().unwrap();
        assert!(matches!(
            schema.resolve_ref(&schema.root).unwrap(),
            crate::schema::SchemaType::Record { fields, .. } if fields.is_empty()
        ));
    }
}

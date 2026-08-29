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

use super::tool_middleware_registry::{
    get_all_tool_middlewares, get_tool_middleware_by_name, get_tool_middleware_invoker_by_name,
};
use super::wire;
use super::{
    InputStream, InvocationResult, Principal, ToolInvokeError, ToolMiddleware, ToolMiddlewareScope,
    UnderlyingTool,
};
use crate::schema::tool as native;
use crate::schema::tool::wit::{decode_tool, encode_tool};
use crate::{decode_typed_schema_value_owned, encode_typed_schema_value_owned};

pub(crate) fn discover_tool_middlewares() -> Result<Vec<wire::ToolMiddleware>, wire::ToolError> {
    get_all_tool_middlewares()
        .iter()
        .map(encode_middleware)
        .collect()
}

pub(crate) fn get_tool_middleware(name: String) -> Result<wire::ToolMiddleware, wire::ToolError> {
    let middleware = get_tool_middleware_by_name(&name)
        .ok_or_else(|| wire::ToolError::InvalidToolName(name.clone()))?;
    encode_middleware(&middleware)
}

pub(crate) async fn invoke_tool_middleware(
    middleware_name: String,
    tool_name: String,
    tool_metadata: wire::Tool,
    command_path: Vec<String>,
    input: crate::schema::wit::wire::TypedSchemaValue,
    stdin: Option<InputStream>,
    principal: Principal,
    wrapped: wire::UnderlyingTool,
) -> Result<wire::InvocationResult, wire::ToolError> {
    let invoker = get_tool_middleware_invoker_by_name(&middleware_name)
        .ok_or_else(|| wire::ToolError::InvalidToolName(middleware_name.clone()))?;
    let tool_metadata = decode_tool(tool_metadata)
        .map_err(|error| wire::ToolError::InvalidInput(error.to_string()))?;
    let input = decode_typed_schema_value_owned(input)
        .map_err(|error| wire::ToolError::InvalidInput(error.to_string()))?;

    match invoker(
        tool_name,
        tool_metadata,
        command_path,
        input,
        stdin,
        principal,
        UnderlyingTool::from_raw(wrapped),
    )
    .await
    {
        Ok(result) => encode_invocation_result(result),
        Err(error) => Err(encode_invocation_error(error)),
    }
}

fn encode_middleware(middleware: &ToolMiddleware) -> Result<wire::ToolMiddleware, wire::ToolError> {
    Ok(wire::ToolMiddleware {
        name: middleware.name.clone(),
        aliases: middleware.aliases.clone(),
        doc: encode_doc(&middleware.doc),
        scope: match &middleware.scope {
            ToolMiddlewareScope::Monomorphic(scope) => {
                wire::ToolMiddlewareScope::Monomorphic(wire::MonomorphicScope {
                    presented: encode_tool(&scope.presented)
                        .map_err(|error| wire::ToolError::InvalidResult(error.to_string()))?,
                    expected: scope
                        .expected
                        .as_ref()
                        .map(encode_tool)
                        .transpose()
                        .map_err(|error| wire::ToolError::InvalidResult(error.to_string()))?,
                })
            }
            ToolMiddlewareScope::Universal => wire::ToolMiddlewareScope::Universal,
        },
    })
}

fn encode_doc(doc: &native::Doc) -> wire::Doc {
    wire::Doc {
        summary: doc.summary.clone(),
        description: doc.description.clone(),
        examples: doc
            .examples
            .iter()
            .map(|example| wire::Example {
                title: example.title.clone(),
                body: example.body.clone(),
            })
            .collect(),
    }
}

fn encode_invocation_result(
    result: InvocationResult,
) -> Result<wire::InvocationResult, wire::ToolError> {
    Ok(wire::InvocationResult {
        result: result
            .result
            .map(encode_typed_schema_value_owned)
            .transpose()
            .map_err(|error| wire::ToolError::InvalidResult(error.to_string()))?,
        stdout: result.stdout,
    })
}

fn encode_invocation_error(error: ToolInvokeError<crate::TypedSchemaValue>) -> wire::ToolError {
    match error {
        ToolInvokeError::InvalidToolName(name) => wire::ToolError::InvalidToolName(name),
        ToolInvokeError::InvalidCommandPath(path) => wire::ToolError::InvalidCommandPath(path),
        ToolInvokeError::InvalidInput(message) => wire::ToolError::InvalidInput(message),
        ToolInvokeError::ConstraintViolation(message) => {
            wire::ToolError::ConstraintViolation(message)
        }
        ToolInvokeError::InvalidResult(message) => wire::ToolError::InvalidResult(message),
        ToolInvokeError::Tool(error) => match encode_typed_schema_value_owned(error) {
            Ok(error) => wire::ToolError::CustomError(error),
            Err(error) => wire::ToolError::InvalidResult(error.to_string()),
        },
    }
}

#[cfg(all(
    feature = "export_golem_tool_middleware",
    not(feature = "export_golem_agentic_tool_middleware")
))]
struct PureMiddlewareComponent;

#[cfg(all(
    feature = "export_golem_tool_middleware",
    not(feature = "export_golem_agentic_tool_middleware")
))]
impl crate::golem_tool_middleware::exports::golem::tool::tool_middleware_guest::Guest
    for PureMiddlewareComponent
{
    fn discover_tool_middlewares() -> Result<Vec<wire::ToolMiddleware>, wire::ToolError> {
        discover_tool_middlewares()
    }

    fn get_tool_middleware(name: String) -> Result<wire::ToolMiddleware, wire::ToolError> {
        get_tool_middleware(name)
    }

    async fn invoke_tool_middleware(
        middleware_name: String,
        tool_name: String,
        tool_metadata: wire::Tool,
        command_path: Vec<String>,
        input: crate::schema::wit::wire::TypedSchemaValue,
        stdin: Option<InputStream>,
        principal: Principal,
        wrapped: wire::UnderlyingTool,
    ) -> Result<wire::InvocationResult, wire::ToolError> {
        invoke_tool_middleware(
            middleware_name,
            tool_name,
            tool_metadata,
            command_path,
            input,
            stdin,
            principal,
            wrapped,
        )
        .await
    }
}

#[cfg(all(
    feature = "export_golem_tool_middleware",
    not(feature = "export_golem_agentic_tool_middleware")
))]
crate::golem_tool_middleware::export_golem_tool_middleware!(
    PureMiddlewareComponent with_types_in crate::golem_tool_middleware
);

#[cfg(feature = "export_golem_agentic_tool_middleware")]
impl crate::golem_agentic_tool_middleware::exports::golem::tool::tool_middleware_guest::Guest
    for crate::agentic::Component
{
    fn discover_tool_middlewares() -> Result<Vec<wire::ToolMiddleware>, wire::ToolError> {
        discover_tool_middlewares()
    }

    fn get_tool_middleware(name: String) -> Result<wire::ToolMiddleware, wire::ToolError> {
        get_tool_middleware(name)
    }

    async fn invoke_tool_middleware(
        middleware_name: String,
        tool_name: String,
        tool_metadata: wire::Tool,
        command_path: Vec<String>,
        input: crate::schema::wit::wire::TypedSchemaValue,
        stdin: Option<InputStream>,
        principal: Principal,
        wrapped: wire::UnderlyingTool,
    ) -> Result<wire::InvocationResult, wire::ToolError> {
        invoke_tool_middleware(
            middleware_name,
            tool_name,
            tool_metadata,
            command_path,
            input,
            stdin,
            principal,
            wrapped,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::IntoTypedSchemaValue;
    use test_r::test;

    #[test]
    fn guest_error_encoding_preserves_every_protocol_variant_and_custom_payload() {
        let errors = [
            ToolInvokeError::InvalidToolName("missing".to_string()),
            ToolInvokeError::InvalidCommandPath(vec!["bad".to_string(), "path".to_string()]),
            ToolInvokeError::InvalidInput("input".to_string()),
            ToolInvokeError::ConstraintViolation("constraint".to_string()),
            ToolInvokeError::InvalidResult("result".to_string()),
        ];

        assert!(matches!(
            encode_invocation_error(errors[0].clone()),
            wire::ToolError::InvalidToolName(name) if name == "missing"
        ));
        assert!(matches!(
            encode_invocation_error(errors[1].clone()),
            wire::ToolError::InvalidCommandPath(path) if path == ["bad", "path"]
        ));
        assert!(matches!(
            encode_invocation_error(errors[2].clone()),
            wire::ToolError::InvalidInput(message) if message == "input"
        ));
        assert!(matches!(
            encode_invocation_error(errors[3].clone()),
            wire::ToolError::ConstraintViolation(message) if message == "constraint"
        ));
        assert!(matches!(
            encode_invocation_error(errors[4].clone()),
            wire::ToolError::InvalidResult(message) if message == "result"
        ));

        let payload = "custom".to_string().into_typed_schema_value().unwrap();
        let encoded = encode_invocation_error(ToolInvokeError::Tool(payload));
        let wire::ToolError::CustomError(encoded) = encoded else {
            panic!("custom middleware error was not preserved")
        };
        let decoded = decode_typed_schema_value_owned(encoded).unwrap();
        assert_eq!(
            decoded.value(),
            &crate::SchemaValue::String("custom".to_string())
        );
    }
}

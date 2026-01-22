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

use super::request_handler::RequestHandlerError;
use super::RouteExecutionResult;
use anyhow::anyhow;
use golem_common::model::agent::{
    AgentError, BinaryReference, DataSchema, DataValue, ElementValue, ElementValues,
    UntypedDataValue,
};
use golem_wasm::analysis::AnalysedType;
use golem_wasm::{FromValue, ValueAndType};
use http::StatusCode;

pub fn interpret_agent_response(
    invoke_result: Option<ValueAndType>,
    expected_type: &DataSchema,
) -> Result<RouteExecutionResult, RequestHandlerError> {
    match invoke_result.map(|ir| ir.value) {
        Some(golem_wasm::Value::Result(Ok(Some(data_value_value)))) => {
            let untyped_data_value = UntypedDataValue::from_value(*data_value_value)
                .map_err(|err| anyhow!("DataValue conversion error: {err}"))?;
            let mapped_response = map_successful_agent_response(untyped_data_value, expected_type)?;
            Ok(mapped_response)
        }
        Some(golem_wasm::Value::Result(Err(Some(agent_error_value)))) => {
            let agent_error = AgentError::from_value(*agent_error_value)
                .map_err(|err| anyhow!("AgentError conversion error: {err}"))?;
            let mapped_error = map_agent_error(agent_error)?;
            Ok(mapped_error)
        }
        _ => Err(anyhow!("Unexpected invoke result type").into()),
    }
}

fn map_agent_error(agent_error: AgentError) -> Result<RouteExecutionResult, RequestHandlerError> {
    match agent_error {
        AgentError::InvalidAgentId(_)
        | AgentError::InvalidMethod(_)
        | AgentError::InvalidInput(_)
        | AgentError::InvalidType(_) => Err(RequestHandlerError::invariant_violated(
            "unexpected agent error type",
        )),
        AgentError::CustomError(inner) => {
            Ok(RouteExecutionResult::CustomAgentError { body: inner })
        }
    }
}

fn map_successful_agent_response(
    agent_response: UntypedDataValue,
    expected_type: &DataSchema,
) -> Result<RouteExecutionResult, RequestHandlerError> {
    let typed_value = DataValue::try_from_untyped(agent_response, expected_type.clone())
        .map_err(|error| RequestHandlerError::AgentResponseTypeMismatch { error })?;

    match typed_value {
        DataValue::Tuple(ElementValues { elements }) => match elements.len() {
            0 => Ok(RouteExecutionResult::NoBody {
                status: StatusCode::NO_CONTENT,
            }),
            1 => map_single_element_agent_response(elements.into_iter().next().unwrap()),
            _ => Err(RequestHandlerError::invariant_violated(
                "Unexpected number of response tuple elements",
            )),
        },
        DataValue::Multimodal(_) => Err(RequestHandlerError::invariant_violated(
            "Unexpected multimodal response",
        )),
    }
}

fn map_single_element_agent_response(
    element: ElementValue,
) -> Result<RouteExecutionResult, RequestHandlerError> {
    match element {
        ElementValue::ComponentModel(value_and_type) => {
            map_component_model_agent_response(value_and_type)
        }

        ElementValue::UnstructuredBinary(BinaryReference::Inline(binary)) => {
            Ok(RouteExecutionResult::UnstructuredBinaryBody { body: binary })
        }

        _ => Err(RequestHandlerError::invariant_violated(
            "Unexpected response type",
        )),
    }
}

fn map_component_model_agent_response(
    value_and_type: ValueAndType,
) -> Result<RouteExecutionResult, RequestHandlerError> {
    use golem_wasm::Value;

    match value_and_type.value {
        Value::Option(None) => Ok(RouteExecutionResult::NoBody {
            status: StatusCode::NO_CONTENT,
        }),

        Value::Option(Some(inner)) => {
            let inner_type = unwrap_option_type(value_and_type.typ)?;
            Ok(ok_body(*inner, inner_type))
        }

        Value::Result(Ok(None)) => Ok(RouteExecutionResult::NoBody {
            status: StatusCode::NO_CONTENT,
        }),

        Value::Result(Ok(Some(inner))) => {
            let inner_type = unwrap_result_ok_option_type(value_and_type.typ)?;
            Ok(ok_body(*inner, inner_type))
        }

        Value::Result(Err(None)) => Ok(RouteExecutionResult::NoBody {
            status: StatusCode::INTERNAL_SERVER_ERROR,
        }),

        Value::Result(Err(Some(inner))) => {
            let inner_type = unwrap_result_err_option_type(value_and_type.typ)?;
            Ok(err_body(*inner, inner_type))
        }

        other => Ok(RouteExecutionResult::ComponentModelJsonBody {
            body: ValueAndType::new(other, value_and_type.typ),
            status: StatusCode::OK,
        }),
    }
}

fn unwrap_option_type(typ: AnalysedType) -> Result<AnalysedType, RequestHandlerError> {
    use golem_wasm::analysis;

    if let AnalysedType::Option(analysis::TypeOption { inner, .. }) = typ {
        Ok(*inner)
    } else {
        Err(RequestHandlerError::invariant_violated(
            "analysed type did not match value",
        ))
    }
}

fn unwrap_result_ok_option_type(typ: AnalysedType) -> Result<AnalysedType, RequestHandlerError> {
    use golem_wasm::analysis;

    let ok = if let AnalysedType::Result(analysis::TypeResult { ok: Some(ok), .. }) = typ {
        *ok
    } else {
        return Err(RequestHandlerError::invariant_violated(
            "analysed type did not match value",
        ));
    };

    unwrap_option_type(ok)
}

fn unwrap_result_err_option_type(typ: AnalysedType) -> Result<AnalysedType, RequestHandlerError> {
    use golem_wasm::analysis;

    let err = if let AnalysedType::Result(analysis::TypeResult { err: Some(err), .. }) = typ {
        *err
    } else {
        return Err(RequestHandlerError::invariant_violated(
            "analysed type did not match value",
        ));
    };

    unwrap_option_type(err)
}

fn ok_body(value: golem_wasm::Value, typ: AnalysedType) -> RouteExecutionResult {
    RouteExecutionResult::ComponentModelJsonBody {
        body: ValueAndType::new(value, typ),
        status: StatusCode::OK,
    }
}

fn err_body(value: golem_wasm::Value, typ: AnalysedType) -> RouteExecutionResult {
    RouteExecutionResult::ComponentModelJsonBody {
        body: ValueAndType::new(value, typ),
        status: StatusCode::INTERNAL_SERVER_ERROR,
    }
}

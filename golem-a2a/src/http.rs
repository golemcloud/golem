// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

//! HTTP+JSON binding metadata and §11.6 error bodies (not HTTP handlers).
use a2a::A2AError;
use a2a::errordetails::{PROTOCOL_DOMAIN, TypedDetail};
use a2a::errors::{error_code, error_reason, protocol_error_code};
use a2a::jsonrpc::methods;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Route {
    pub verb: &'static str,
    pub path: &'static str,
    pub method: &'static str,
}

/// Exactly the v1.0.1 §5.3 table, without compatibility or tenant routes.
pub const ROUTES: &[Route] = &[
    Route {
        verb: "POST",
        path: "/message:send",
        method: methods::SEND_MESSAGE,
    },
    Route {
        verb: "POST",
        path: "/message:stream",
        method: methods::SEND_STREAMING_MESSAGE,
    },
    Route {
        verb: "GET",
        path: "/tasks/{id}",
        method: methods::GET_TASK,
    },
    Route {
        verb: "GET",
        path: "/tasks",
        method: methods::LIST_TASKS,
    },
    Route {
        verb: "POST",
        path: "/tasks/{id}:cancel",
        method: methods::CANCEL_TASK,
    },
    Route {
        verb: "POST",
        path: "/tasks/{id}:subscribe",
        method: methods::SUBSCRIBE_TO_TASK,
    },
    Route {
        verb: "POST",
        path: "/tasks/{id}/pushNotificationConfigs",
        method: methods::CREATE_PUSH_CONFIG,
    },
    Route {
        verb: "GET",
        path: "/tasks/{id}/pushNotificationConfigs/{configId}",
        method: methods::GET_PUSH_CONFIG,
    },
    Route {
        verb: "GET",
        path: "/tasks/{id}/pushNotificationConfigs",
        method: methods::LIST_PUSH_CONFIGS,
    },
    Route {
        verb: "DELETE",
        path: "/tasks/{id}/pushNotificationConfigs/{configId}",
        method: methods::DELETE_PUSH_CONFIG,
    },
    Route {
        verb: "GET",
        path: "/extendedAgentCard",
        method: methods::GET_EXTENDED_AGENT_CARD,
    },
];

#[derive(Debug, PartialEq, Eq)]
pub struct RouteMatch<'a> {
    pub route: &'static Route,
    /// The escaped path segment; URL decoding belongs to the HTTP boundary.
    pub task_id: Option<&'a str>,
    pub config_id: Option<&'a str>,
}

/// Match a path without its query string. Captures remain percent-encoded so
/// an encoded action delimiter cannot turn into a route action.
pub fn match_route<'a>(verb: &str, path: &'a str) -> Option<RouteMatch<'a>> {
    for route in ROUTES.iter().filter(|route| route.verb == verb) {
        let mut actual = path.split('/');
        let mut task_id = None;
        let mut config_id = None;
        let matched = route.path.split('/').all(|segment| {
            let Some(value) = actual.next() else {
                return false;
            };
            if let Some(suffix) = segment.strip_prefix("{id}") {
                if route.method == methods::GET_TASK
                    && (value.ends_with(":cancel") || value.ends_with(":subscribe"))
                {
                    return false;
                }
                if let Some(id) = value.strip_suffix(suffix).filter(|id| !id.is_empty()) {
                    task_id = Some(id);
                    return true;
                }
                false
            } else if segment == "{configId}" {
                config_id = Some(value);
                !value.is_empty()
            } else {
                segment == value
            }
        });
        if matched && actual.next().is_none() {
            return Some(RouteMatch {
                route,
                task_id,
                config_id,
            });
        }
    }
    None
}

#[derive(Serialize, Deserialize)]
struct ErrorBody {
    error: Status,
}

#[derive(Serialize, Deserialize)]
struct Status {
    code: u16,
    #[serde(default)]
    status: String,
    message: String,
    #[serde(default, deserialize_with = "crate::deserialize_error_details")]
    details: Vec<TypedDetail>,
}

fn grpc_status(code: i32) -> &'static str {
    match code {
        error_code::TASK_NOT_FOUND => "NOT_FOUND",
        error_code::TASK_NOT_CANCELABLE
        | error_code::PUSH_NOTIFICATION_NOT_SUPPORTED
        | error_code::UNSUPPORTED_OPERATION
        | error_code::EXTENDED_CARD_NOT_CONFIGURED
        | error_code::EXTENSION_SUPPORT_REQUIRED
        | error_code::VERSION_NOT_SUPPORTED => "FAILED_PRECONDITION",
        error_code::CONTENT_TYPE_NOT_SUPPORTED
        | error_code::PARSE_ERROR
        | error_code::INVALID_REQUEST
        | error_code::INVALID_PARAMS => "INVALID_ARGUMENT",
        error_code::METHOD_NOT_FOUND => "UNIMPLEMENTED",
        _ => "INTERNAL",
    }
}

pub fn encode_error(error: &A2AError) -> Result<Value, serde_json::Error> {
    let mut details = error.details.clone().unwrap_or_default();
    // Preserve caller details, but put the authoritative protocol reason last.
    if protocol_error_code(&details) != Some(error.code) {
        details.push(TypedDetail::error_info(
            error_reason(error.code),
            PROTOCOL_DOMAIN,
            None,
        ));
    }
    serde_json::to_value(ErrorBody {
        error: Status {
            code: error.http_status_code(),
            status: grpc_status(error.code).into(),
            message: error.message.clone(),
            details,
        },
    })
}

/// ErrorInfo disambiguates A2A errors sharing one HTTP status. When a peer
/// supplies no protocol reason, preserve the failure as a generic A2A error.
pub fn decode_error(value: Value) -> Result<A2AError, serde_json::Error> {
    let Status {
        code,
        status,
        message,
        details,
    } = serde_json::from_value::<ErrorBody>(value)?.error;
    let code = protocol_error_code(&details).unwrap_or(match status.as_str() {
        "NOT_FOUND" => error_code::TASK_NOT_FOUND,
        "INVALID_ARGUMENT" => error_code::INVALID_PARAMS,
        "UNIMPLEMENTED" => error_code::METHOD_NOT_FOUND,
        "INTERNAL" | "FAILED_PRECONDITION" => error_code::INTERNAL_ERROR,
        _ => match code {
            400 => error_code::INVALID_PARAMS,
            404 => error_code::TASK_NOT_FOUND,
            501 => error_code::METHOD_NOT_FOUND,
            _ => error_code::INTERNAL_ERROR,
        },
    });
    Ok(A2AError {
        code,
        message,
        details: (!details.is_empty()).then_some(details),
    })
}

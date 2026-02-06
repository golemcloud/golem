mod model;

use model::*;

use golem_rust::{agent_definition, agent_implementation, endpoint, AllowedMimeTypes};
use golem_rust::agentic::UnstructuredBinary;

#[agent_definition(mount = "/http-agents/{agent_name}")]
pub trait HttpAgent {
    fn new(agent_name: String) -> Self;

    #[endpoint(get = "/string-path-var/{path_var}")]
    fn string_path_var(path_var: String) -> StringPathVarResponse;

    #[endpoint(get = "/multi-path-vars/{first}/{second}")]
    fn multi_path_vars(
        first: String,
        second: String,
    ) -> MultiPathVarsResponse;

    #[endpoint(get = "/rest/{*tail}")]
    fn remaining_path(tail: String) -> RemainingPathResponse;

    #[endpoint(get = "/path-and-query/{item_id}?limit={limit}")]
    fn path_and_query(
        item_id: String,
        limit: u64,
    ) -> PathAndQueryResponse;

    #[endpoint(
        get = "/path-and-header/{resource_id}",
        headers = { "x-request-id" => "request_id" }
    )]
    fn path_and_header(
        resource_id: String,
        request_id: String,
    ) -> PathAndHeaderResponse;

    #[endpoint(post = "/json-body/{id}")]
    fn json_body(
        id: String,
        name: String,
        count: u64,
    ) -> JsonBodyResponse;

    #[endpoint(post = "/unrestricted-unstructured-binary/{bucket}")]
    fn unrestricted_unstructured_binary(
        bucket: String,
        payload: UnstructuredBinary<String>,
    ) -> i64;

    #[endpoint(post = "/restricted-unstructured-binary/{bucket}")]
    fn restricted_unstructured_binary(
        bucket: String,
        payload: UnstructuredBinary<MyMimeTypes>,
    ) -> i64;

    #[endpoint(get = "/resp/no-content")]
    fn no_content();

    #[endpoint(get = "/resp/json")]
    fn json_response() -> JsonResponse;

    #[endpoint(get = "/resp/optional/{found}")]
    fn optional_response(found: bool) -> Option<OptionalResponse>;

    #[endpoint(get = "/resp/result-json-json/{ok}")]
    fn result_ok_or_err(
        ok: bool,
    ) -> Result<ResultOkResponse, ResultErrResponse>;

    #[endpoint(post = "/resp/result-void-json")]
    fn result_void_err() -> Result<(), ResultErrResponse>;

    #[endpoint(get = "/resp/result-json-void")]
    fn result_json_void() -> Result<ResultOkResponse, ()>;

    #[endpoint(get = "/resp/binary")]
    fn binary_response() -> UnstructuredBinary<String>;
}

#[derive(AllowedMimeTypes, Clone, Debug)]
pub enum MyMimeTypes {
    ImageGif
}


struct HttpAgentImpl {
    agent_name: String,
}


impl HttpAgent for HttpAgentImpl {
    fn new(agent_name: String) -> HttpAgentImpl {
        HttpAgentImpl {
            agent_name,
        }
    }

    fn string_path_var(path_var: String) -> StringPathVarResponse {
        StringPathVarResponse {
            value: path_var,
        }
    }

    fn multi_path_vars(
        first: String,
        second: String,
    ) -> MultiPathVarsResponse {
        MultiPathVarsResponse {
            joined: format!("{}:{}", first, second),
        }
    }

    fn remaining_path(tail: String) -> RemainingPathResponse {
        RemainingPathResponse { tail }
    }

    fn path_and_query(
        item_id: String,
        limit: u64,
    ) -> PathAndQueryResponse {
        PathAndQueryResponse {
            id: item_id,
            limit,
        }
    }

    fn path_and_header(
        resource_id: String,
        request_id: String,
    ) -> PathAndHeaderResponse {
        PathAndHeaderResponse {
            resource_id,
            request_id,
        }
    }

    fn json_body(
        _id: String,
        _name: String,
        _count: u64,
    ) -> JsonBodyResponse {
        JsonBodyResponse { ok: true }
    }

    fn unrestricted_unstructured_binary(
        _bucket: String,
        payload: UnstructuredBinary<String>,
    ) -> i64 {
        match payload {
            UnstructuredBinary::Url(_) => -1,
            UnstructuredBinary::Inline { data, .. } => data.len() as i64,
        }
    }

    fn restricted_unstructured_binary(
        _bucket: String,
        payload: UnstructuredBinary<MyMimeTypes>,
    ) -> i64 {
        match payload {
            UnstructuredBinary::Url(_) => -1,
            UnstructuredBinary::Inline{ data, .. } => data.len() as i64,
        }
    }

    fn no_content() {
        // intentionally empty (204)
    }

    fn json_response() -> JsonResponse {
        JsonResponse {
            value: "ok".to_string(),
        }
    }

    fn optional_response(found: bool) -> Option<OptionalResponse> {
        if found {
            Some(OptionalResponse {
                value: "yes".to_string(),
            })
        } else {
            None
        }
    }

    fn result_ok_or_err(
        ok: bool,
    ) -> Result<ResultOkResponse, ResultErrResponse> {
        if ok {
            Ok(ResultOkResponse {
                value: "ok".to_string(),
            })
        } else {
            Err(ResultErrResponse {
                error: "boom".to_string(),
            })
        }
    }

    fn result_void_err() -> Result<(), ResultErrResponse> {
        Err(ResultErrResponse {
            error: "fail".to_string(),
        })
    }

    fn result_json_void() -> Result<ResultOkResponse, ()> {
        Ok(ResultOkResponse {
            value: "ok".to_string(),
        })
    }

    fn binary_response() -> UnstructuredBinary<String> {
        UnstructuredBinary::Inline {
            data: vec![1, 2, 3, 4],
            mime_type: "application/octet-stream".to_string(),
        }
    }
}


#[agent_definition(
    mount = "/cors-agents/{agent_name}",
    cors = ["https://mount.example.com"]
)]
pub trait CorsAgent {
    fn new(agent_name: String) -> Self;

    #[endpoint(
        get = "/wildcard",
        cors = ["*"]
    )]
    fn wildcard() -> OkResponse;

    #[endpoint(get = "/inherited")]
    fn inherited() -> OkResponse;

    #[endpoint(
        post = "/preflight-required",
        cors = ["https://app.example.com"]
    )]
    fn preflight(body: PreflightRequest) -> PreflightResponse;
}

pub struct CorsAgentImpl {
    pub agent_name: String,
}

#[agent_implementation]
impl CorsAgent for CorsAgentImpl {
    fn new(agent_name: String) -> CorsAgentImpl {
        CorsAgentImpl {
            agent_name,
        }
    }

    fn wildcard() -> OkResponse {
        OkResponse { ok: true }
    }

    fn inherited() -> OkResponse {
        OkResponse { ok: true }
    }

    fn preflight(body: PreflightRequest) -> PreflightResponse {
        PreflightResponse {
            received: body.name,
        }
    }
}
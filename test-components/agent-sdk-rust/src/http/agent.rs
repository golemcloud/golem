use super::model::*;
use golem_rust::{agent_definition, agent_implementation, endpoint, AllowedMimeTypes};
use golem_rust::agentic::{create_webhook, UnstructuredBinary, Principal};
use serde::Deserialize;
use serde::Serialize;
use wstd::http::{Body, Client, HeaderValue, Request};
use golem_rust::Schema;

#[agent_definition(mount = "/http-agents/{agent_name}")]
pub trait HttpAgent {
    fn new(agent_name: String) -> Self;

    #[endpoint(get = "/string-path-var/{path_var}")]
    fn string_path_var(&self, path_var: String) -> StringPathVarResponse;

    #[endpoint(get = "/multi-path-vars/{first}/{second}")]
    fn multi_path_vars(
        &self,
        first: String,
        second: String,
    ) -> MultiPathVarsResponse;

    #[endpoint(get = "/rest/{*tail}")]
    fn remaining_path(&self, tail: String) -> RemainingPathResponse;

    #[endpoint(get = "/path-and-query/{item_id}?limit={limit}")]
    fn path_and_query(
        &self,
        item_id: String,
        limit: u64,
    ) -> PathAndQueryResponse;

    #[endpoint(
        get = "/path-and-header/{resource_id}",
        headers("x-request-id" = "request_id")
    )]
    fn path_and_header(
        &self,
        resource_id: String,
        request_id: String,
    ) -> PathAndHeaderResponse;

    #[endpoint(post = "/json-body/{id}")]
    fn json_body(
        &self,
        id: String,
        name: String,
        count: u64,
    ) -> JsonBodyResponse;

    #[endpoint(post = "/unrestricted-unstructured-binary/{bucket}")]
    fn unrestricted_unstructured_binary(
        &self,
        bucket: String,
        payload: UnstructuredBinary<String>,
    ) -> i64;

    #[endpoint(post = "/restricted-unstructured-binary/{bucket}")]
    fn restricted_unstructured_binary(
        &self,
        bucket: String,
        payload: UnstructuredBinary<MyMimeTypes>,
    ) -> i64;

    #[endpoint(get = "/resp/no-content")]
    fn no_content(&self);

    // https://github.com/golemcloud/golem/issues/2725
    #[endpoint(get = "/resp/json")]
    fn json_response_func(&self) -> JsonResponse;

    #[endpoint(get = "/resp/optional/{found}")]
    fn optional_response_func(&self, found: bool) -> Option<OptionalResponse>;

    #[endpoint(get = "/resp/result-json-json/{ok}")]
    fn result_ok_or_err(
        &self,
        ok: bool,
    ) -> Result<ResultOkResponse, ResultErrResponse>;

    #[endpoint(post = "/resp/result-void-json")]
    fn result_void_err(&self) -> Result<(), ResultErrResponse>;

    #[endpoint(get = "/resp/result-json-void")]
    fn result_json_void(&self) -> Result<ResultOkResponse, ()>;

    #[endpoint(get = "/resp/binary")]
    fn binary_response(&self) -> UnstructuredBinary<String>;
}

#[derive(AllowedMimeTypes, Clone, Debug)]
pub enum MyMimeTypes {
    #[mime_type("image/gif")]
    ImageGif
}


struct HttpAgentImpl;

#[agent_implementation]
impl HttpAgent for HttpAgentImpl {
    fn new(_agent_name: String) -> Self {
        Self
    }

    fn string_path_var(&self, path_var: String) -> StringPathVarResponse {
        StringPathVarResponse {
            value: path_var,
        }
    }

    fn multi_path_vars(
        &self,
        first: String,
        second: String,
    ) -> MultiPathVarsResponse {
        MultiPathVarsResponse {
            joined: format!("{}:{}", first, second),
        }
    }

    fn remaining_path(&self, tail: String) -> RemainingPathResponse {
        RemainingPathResponse { tail }
    }

    fn path_and_query(
        &self,
        item_id: String,
        limit: u64,
    ) -> PathAndQueryResponse {
        PathAndQueryResponse {
            id: item_id,
            limit,
        }
    }

    fn path_and_header(
        &self,
        resource_id: String,
        request_id: String,
    ) -> PathAndHeaderResponse {
        PathAndHeaderResponse {
            resource_id,
            request_id,
        }
    }

    fn json_body(
        &self,
        _id: String,
        _name: String,
        _count: u64,
    ) -> JsonBodyResponse {
        JsonBodyResponse { ok: true }
    }

    fn unrestricted_unstructured_binary(
        &self,
        _bucket: String,
        payload: UnstructuredBinary<String>,
    ) -> i64 {
        match payload {
            UnstructuredBinary::Url(_) => -1,
            UnstructuredBinary::Inline { data, .. } => data.len() as i64,
        }
    }

    fn restricted_unstructured_binary(
        &self,
        _bucket: String,
        payload: UnstructuredBinary<MyMimeTypes>,
    ) -> i64 {
        match payload {
            UnstructuredBinary::Url(_) => -1,
            UnstructuredBinary::Inline{ data, .. } => data.len() as i64,
        }
    }

    fn no_content(&self) {
        // intentionally empty (204)
    }

    fn json_response_func(&self) -> JsonResponse {
        JsonResponse {
            value: "ok".to_string(),
        }
    }

    fn optional_response_func(&self, found: bool) -> Option<OptionalResponse> {
        if found {
            Some(OptionalResponse {
                value: "yes".to_string(),
            })
        } else {
            None
        }
    }

    fn result_ok_or_err(
        &self,
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

    fn result_void_err(&self) -> Result<(), ResultErrResponse> {
        Err(ResultErrResponse {
            error: "fail".to_string(),
        })
    }

    fn result_json_void(&self) -> Result<ResultOkResponse, ()> {
        Ok(ResultOkResponse {
            value: "ok".to_string(),
        })
    }

    fn binary_response(&self) -> UnstructuredBinary<String> {
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
    fn wildcard(&self) -> OkResponse;

    #[endpoint(get = "/inherited")]
    fn inherited(&self) -> OkResponse;

    #[endpoint(
        post = "/preflight-required",
        cors = ["https://app.example.com"]
    )]
    fn preflight(&self, body: PreflightRequest) -> PreflightResponse;
}

pub struct CorsAgentImpl;

#[agent_implementation]
impl CorsAgent for CorsAgentImpl {
    fn new(_agent_name: String) -> Self {
        Self
    }

    fn wildcard(&self) -> OkResponse {
        OkResponse { ok: true }
    }

    fn inherited(&self) -> OkResponse {
        OkResponse { ok: true }
    }

    fn preflight(&self, body: PreflightRequest) -> PreflightResponse {
        PreflightResponse {
            received: body.name,
        }
    }
}

#[agent_definition(
    mount = "/webhook-agents/{agent_name}",
    webhook_suffix = "/webhook-agent"
)]
pub trait WebhookAgent {
    fn new(agent_name: String) -> Self;

    #[endpoint(post = "/set-test-server-url")]
    fn set_test_server_url(&mut self, test_server_url: String);

    #[endpoint(post = "/test-webhook")]
    async fn test_webhook(&self) -> WebhookResponse;
}

struct WebhookAgentImpl {
    test_server_url: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct WebhookUrl {
    // Important since we have a common server implementation in integration tests to
    // accept callbacks through webhook
    #[serde(rename = "webhookUrl")]
    webhook_url: String
}

#[agent_implementation]
impl WebhookAgent for WebhookAgentImpl {
    fn new(_agent_name: String) -> Self {
        WebhookAgentImpl{
            test_server_url: None,
        }
    }

    fn set_test_server_url(&mut self, test_server_url: String) {
        self.test_server_url = Some(test_server_url);
    }

    async fn test_webhook(&self) -> WebhookResponse {
        let webhook = create_webhook();

        let url = WebhookUrl {
            webhook_url: webhook.url().to_string(),
        };

        let body = Body::from_json(&url).map_err(|err| err.to_string()).unwrap();
        let request = Request::post(self.test_server_url.clone().unwrap())
            .header("Accept", HeaderValue::from_str("application/json").unwrap())
            .header("Content-Type", "application/json")
            .body(body).map_err(|err| err.to_string()).unwrap();

        let _ =
            Client::new().send(request).await.map_err(|err| err.to_string()).unwrap();

        let request = webhook.await;

        let data: String = request.json().unwrap();

        WebhookResponse {
            payload_length: data.len() as u64,
        }
    }
}

#[agent_definition(
    mount = "/principal-agent/{agent_name}",
)]
pub trait PrincipalAgent {
    fn new(agent_name: String) -> Self;

    #[endpoint(get = "/echo-principal")]
    fn echo_principal(&self, #[principal] principal: Principal) -> EchoPrincipalResponse;

    #[endpoint(get = "/echo-principal-mid/{foo}/{bar}")]
    fn echo_principal2(&self, foo: String, #[principal] principal: Principal, bar: u32) -> EchoPrincipal2Response;

    #[endpoint(get = "/echo-principal-last/{foo}/{bar}")]
    fn echo_principal3(&self, foo: String, bar: u32, #[principal] principal: Principal) -> EchoPrincipal3Response;

    #[endpoint(get = "/authed-principal", auth = true)]
    fn authed_principal(&self, #[principal] principal: Principal) -> AuthedPrincipalResponse;
}

#[derive(Schema)]
pub struct EchoPrincipalResponse {
    pub value: Principal
}

#[derive(Schema)]
pub struct EchoPrincipal2Response {
    pub value: Principal,
    pub foo: String,
    pub bar: u32
}

#[derive(Schema)]
pub struct EchoPrincipal3Response {
    pub value: Principal,
    pub foo: String,
    pub bar: u32
}

#[derive(Schema)]
pub struct AuthedPrincipalResponse {
    pub value: Principal,
}

pub struct PrincipalAgentImpl;

#[agent_implementation]
impl PrincipalAgent for PrincipalAgentImpl {
    fn new(_agent_name: String) -> Self {
        Self
    }

    fn echo_principal(&self, #[principal] principal: Principal) -> EchoPrincipalResponse {
        EchoPrincipalResponse {
            value: principal
        }
    }

    fn echo_principal2(&self, foo: String, #[principal] principal: Principal, bar: u32) -> EchoPrincipal2Response {
        EchoPrincipal2Response {
            value: principal,
            foo,
            bar
        }
    }

    fn echo_principal3(&self, foo: String, bar: u32, #[principal] principal: Principal) -> EchoPrincipal3Response {
        EchoPrincipal3Response {
            value: principal,
            foo,
            bar
        }
    }

    fn authed_principal(&self, #[principal] principal: Principal) -> AuthedPrincipalResponse {
        AuthedPrincipalResponse {
            value: principal
        }
    }
}

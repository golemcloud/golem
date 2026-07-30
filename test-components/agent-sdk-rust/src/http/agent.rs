use super::model::*;
use golem_rust::agentic::{UnstructuredBinary, create_webhook};
use golem_rust::wasip3::{
    http::{client, types},
    wit_future, wit_stream,
};
use golem_rust::{AllowedMimeTypes, agent_definition, agent_implementation, description, endpoint};
use serde::Deserialize;
use serde::Serialize;

#[agent_definition(mount = "/http-agents/{agent_name}")]
pub trait HttpAgent {
    fn new(agent_name: String) -> Self;

    #[endpoint(get = "/string-path-var/{path_var}")]
    #[description("Returns the provided path variable as a response")]
    fn string_path_var(&self, path_var: String) -> StringPathVarResponse;

    #[endpoint(get = "/multi-path-vars/{first}/{second}")]
    #[description("Combines two path variables with a colon separator")]
    fn multi_path_vars(&self, first: String, second: String) -> MultiPathVarsResponse;

    #[endpoint(get = "/rest/{*tail}")]
    #[description("Returns the remaining path after the /rest prefix")]
    fn remaining_path(&self, tail: String) -> RemainingPathResponse;

    #[endpoint(get = "/path-and-query/{item_id}?limit={limit}")]
    #[description("Combines a path variable with a query parameter in the response")]
    fn path_and_query(&self, item_id: String, limit: u64) -> PathAndQueryResponse;

    #[endpoint(
        get = "/path-and-header/{resource_id}",
        headers("x-request-id" = "request_id")
    )]
    #[description("Combines a path variable with a header value in the response")]
    fn path_and_header(&self, resource_id: String, request_id: String) -> PathAndHeaderResponse;

    #[endpoint(post = "/json-body/{id}")]
    #[description("Accepts JSON body parameters and returns a success response")]
    fn json_body(&self, id: String, name: String, count: u64) -> JsonBodyResponse;

    #[endpoint(post = "/unrestricted-unstructured-binary/{bucket}")]
    #[description("Accepts unrestricted binary data and returns the payload size")]
    fn unrestricted_unstructured_binary(
        &self,
        bucket: String,
        payload: UnstructuredBinary<String>,
    ) -> i64;

    #[endpoint(post = "/restricted-unstructured-binary/{bucket}")]
    #[description("Accepts restricted binary data (image/gif only) and returns the payload size")]
    fn restricted_unstructured_binary(
        &self,
        bucket: String,
        payload: UnstructuredBinary<MyMimeTypes>,
    ) -> i64;

    #[endpoint(get = "/resp/no-content")]
    #[description("Returns a 204 No Content response")]
    fn no_content(&self);

    // https://github.com/golemcloud/golem/issues/2725
    #[endpoint(get = "/resp/json")]
    #[description("Returns a JSON response with a value field")]
    fn json_response_func(&self) -> JsonResponse;

    #[endpoint(get = "/resp/optional/{found}")]
    #[description("Returns an optional response based on the found parameter")]
    fn optional_response_func(&self, found: bool) -> Option<OptionalResponse>;

    #[endpoint(get = "/resp/result-json-json/{ok}")]
    #[description("Returns either a success or error result based on the ok parameter")]
    fn result_ok_or_err(&self, ok: bool) -> Result<ResultOkResponse, ResultErrResponse>;

    #[endpoint(post = "/resp/result-void-json")]
    #[description("Returns either unit success or a JSON error result")]
    fn result_void_err(&self) -> Result<(), ResultErrResponse>;

    #[endpoint(get = "/resp/result-json-void")]
    #[description("Returns either a JSON success result or unit error")]
    fn result_json_void(&self) -> Result<ResultOkResponse, ()>;

    #[endpoint(get = "/resp/binary")]
    #[description("Returns binary data as an unstructured binary response")]
    fn binary_response(&self) -> UnstructuredBinary<String>;

    // PATCH method endpoints
    #[endpoint(patch = "/resource/{id}")]
    #[description("Updates a resource using PATCH method with provided update data")]
    fn patch_resource(&self, id: String, update: ResourceUpdate) -> ResourceResponse;

    #[endpoint(patch = "/resource/{id}/partial")]
    #[description("Performs a partial update on a resource using PATCH method")]
    fn patch_partial(&self, id: String) -> ResourceResponse;
}

#[derive(AllowedMimeTypes, Clone, Debug)]
pub enum MyMimeTypes {
    #[mime_type("image/gif")]
    ImageGif,
}

struct HttpAgentImpl;

#[agent_implementation]
impl HttpAgent for HttpAgentImpl {
    fn new(_agent_name: String) -> Self {
        Self
    }

    fn string_path_var(&self, path_var: String) -> StringPathVarResponse {
        StringPathVarResponse { value: path_var }
    }

    fn multi_path_vars(&self, first: String, second: String) -> MultiPathVarsResponse {
        MultiPathVarsResponse {
            joined: format!("{}:{}", first, second),
        }
    }

    fn remaining_path(&self, tail: String) -> RemainingPathResponse {
        RemainingPathResponse { tail }
    }

    fn path_and_query(&self, item_id: String, limit: u64) -> PathAndQueryResponse {
        PathAndQueryResponse { id: item_id, limit }
    }

    fn path_and_header(&self, resource_id: String, request_id: String) -> PathAndHeaderResponse {
        PathAndHeaderResponse {
            resource_id,
            request_id,
        }
    }

    fn json_body(&self, _id: String, _name: String, _count: u64) -> JsonBodyResponse {
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
            UnstructuredBinary::Inline { data, .. } => data.len() as i64,
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

    fn result_ok_or_err(&self, ok: bool) -> Result<ResultOkResponse, ResultErrResponse> {
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

    // PATCH method implementations
    fn patch_resource(&self, id: String, update: ResourceUpdate) -> ResourceResponse {
        ResourceResponse {
            id: id.clone(),
            updated: true,
            method: "PATCH".to_string(),
        }
    }

    fn patch_partial(&self, id: String) -> ResourceResponse {
        ResourceResponse {
            id: id.clone(),
            updated: true,
            method: "PATCH".to_string(),
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
    webhook_url: String,
}

#[agent_implementation]
impl WebhookAgent for WebhookAgentImpl {
    fn new(_agent_name: String) -> Self {
        WebhookAgentImpl {
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

        send_json_post(
            &self.test_server_url.clone().unwrap(),
            serde_json::to_vec(&url).unwrap(),
        )
        .await
        .map_err(|err| format!("{err:?}"))
        .unwrap();

        let request = webhook.await;

        let data: String = request.json().unwrap();

        WebhookResponse {
            payload_length: data.len() as u64,
        }
    }
}

async fn send_json_post(url: &str, body: Vec<u8>) -> Result<(), types::ErrorCode> {
    let Some(rest) = url.strip_prefix("http://") else {
        panic!("test webhook URL must use http://");
    };
    let (authority, path_with_query) = match rest.split_once('/') {
        Some((authority, path)) => (authority, format!("/{path}")),
        None => match rest.split_once('?') {
            Some((authority, query)) => (authority, format!("/?{query}")),
            None => (rest, "/".to_string()),
        },
    };

    let headers = types::Fields::from_list(&[
        ("accept".to_string(), b"application/json".to_vec()),
        ("content-type".to_string(), b"application/json".to_vec()),
    ])
    .expect("valid HTTP headers");

    let (mut body_tx, body_rx) = wit_stream::new();
    let (trailers_tx, trailers_rx) = wit_future::new(|| Ok(None));
    let (request, transmit) = types::Request::new(headers, Some(body_rx), trailers_rx, None);
    request.set_method(&types::Method::Post).unwrap();
    request.set_scheme(Some(&types::Scheme::Http)).unwrap();
    request.set_authority(Some(authority)).unwrap();
    request.set_path_with_query(Some(&path_with_query)).unwrap();

    let (send_result, transmit_result, ()) = futures::join!(
        async { client::send(request).await },
        async { transmit.await },
        async {
            let remaining = body_tx.write_all(body).await;
            assert!(remaining.is_empty());
            let _ = trailers_tx.write(Ok(None)).await;
            drop(body_tx);
        }
    );

    let response = send_result?;
    drop(response);
    transmit_result
}

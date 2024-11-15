use http::StatusCode;
use crate::gateway_binding::HttpRequestDetails;
use crate::gateway_execution::gateway_session::{DataKey, DataValue, GatewaySessionStore, SessionId};
use crate::gateway_middleware::HttpAuthorizer;
use crate::gateway_request::http_request::InputHttpRequest;

trait MiddlewareIn<In, Out> {
    async fn process_input(&self, input: In, session_store: GatewaySessionStore) -> Result<MiddlewareResult<Out>, String>;
}

enum MiddlewareResult<R> {
    PassThrough,
    Redirect(R)
}

pub struct PassThroughMetadata {
    value: serde_json::Value
}

impl MiddlewareIn<InputHttpRequest, MiddlewareResult<poem::Response>> for HttpAuthorizer {
    async fn process_input(&self, input: HttpRequestDetails, session_store: GatewaySessionStore) -> Result<MiddlewareResult<poem::Response>, String> {

        // validate first if the input has headers consisting of auth bearer token
        // or cookie that acess_token, and then based on it fetch things from session store
        // and validate. Probably use a session store itself that simply keeps the access_token

        // if false {
        //     // do nothing for now
        // }

        let redirect_uri = input.get_uri();

        let identity_provider =
            &self.scheme_internal.identity_provider;

        let client =
            identity_provider.get_client(&self.scheme_internal.security_scheme).map_err(|err| err.to_string())?;

        let authorization =
            identity_provider.get_authorization_url(&client, self.get_scopes()).map_err(
                |err| err.to_string(),
            )?;

        let state = authorization.csrf_state.secret();

        // TODO Handle session-id
        let session_id = SessionId(state.clone());
        let nonce_data_key = DataKey::nonce();
        let nonce_data_value = DataValue(serde_json::Value::String(authorization.nonce.secret().clone()));

        let redirect_url_data_key = DataKey::redirect_uri();

        let redirect_url_data_value = DataValue(serde_json::Value::String(redirect_uri));

        session_store.0.insert(session_id, nonce_data_key, nonce_data_value).await.map_err(|err| err.to_string())?;
        session_store.0.insert(session_id, redirect_url_data_key, redirect_url_data_value).await.map_err(|err| err.to_string())?;

        let mut response = poem::Response::builder();
        let result = response
            .header("Location", authorization.url.to_string())
            .status(StatusCode::FOUND)
            .body(());

        Ok(MiddlewareResult::Redirect(result))

    }
}
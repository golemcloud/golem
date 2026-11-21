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

use crate::aws::{AwsCredentials, SigningFailure, sign_request};
use aws_smithy_runtime_api::box_error::BoxError;
use aws_smithy_runtime_api::client::auth::{
    AuthScheme, AuthSchemeEndpointConfig, AuthSchemeId, Sign,
};
use aws_smithy_runtime_api::client::identity::{
    Identity, IdentityFuture, ResolveIdentity, SharedIdentityResolver,
};
use aws_smithy_runtime_api::client::orchestrator::HttpRequest;
use aws_smithy_runtime_api::client::runtime_components::{GetIdentityResolver, RuntimeComponents};
use aws_smithy_types::config_bag::ConfigBag;
use sha2::{Digest, Sha256};
use std::sync::Arc;

#[derive(Debug)]
pub(super) struct Signing {
    pub region: Arc<str>,
    pub identity: SharedIdentityResolver,
}

fn payload_hash(bytes: Option<&[u8]>, tls: bool) -> Result<Box<str>, SigningFailure> {
    match bytes {
        Some(bytes) => Ok(hex::encode(Sha256::digest(bytes)).into_boxed_str()),
        None if tls => Ok("UNSIGNED-PAYLOAD".into()),
        None => Err(SigningFailure::InvalidRequest),
    }
}

impl ResolveIdentity for AwsCredentials {
    fn resolve_identity<'a>(
        &'a self,
        _: &'a RuntimeComponents,
        _: &'a ConfigBag,
    ) -> IdentityFuture<'a> {
        IdentityFuture::ready(Ok(Identity::new(self.clone(), None)))
    }
}

impl AuthScheme for Signing {
    fn scheme_id(&self) -> AuthSchemeId {
        AuthSchemeId::new("sigv4")
    }

    fn identity_resolver(&self, _: &dyn GetIdentityResolver) -> Option<SharedIdentityResolver> {
        Some(self.identity.clone())
    }

    fn signer(&self) -> &dyn Sign {
        self
    }
}

impl Sign for Signing {
    fn sign_http_request(
        &self,
        request: &mut HttpRequest,
        identity: &Identity,
        _: AuthSchemeEndpointConfig<'_>,
        components: &RuntimeComponents,
        _: &ConfigBag,
    ) -> Result<(), BoxError> {
        let credentials = identity
            .data::<AwsCredentials>()
            .ok_or(SigningFailure::InvalidRequest)?;
        let hash = payload_hash(
            request.body().bytes(),
            request.uri().starts_with("https://"),
        )?;
        let original = std::mem::replace(request, HttpRequest::empty());
        let original = original
            .try_into_http1x()
            .map_err(|_| SigningFailure::InvalidRequest)?;
        let (mut head, body) = original.into_parts();
        let headers = sign_request(
            &head,
            &hash,
            credentials,
            &self.region,
            "s3",
            components.time_source().unwrap_or_default().now(),
        )?;
        head.headers = headers;
        *request = http::Request::from_parts(head, body)
            .try_into()
            .map_err(|_| SigningFailure::InvalidRequest)?;
        Ok(())
    }
}

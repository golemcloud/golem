//! Single-attempt HTTP dispatch with host-owned admission and accounting.

use super::{HttpSend, TransportError};
use bytes::Bytes;
use http::{Request, Response, Uri};
use std::future::Future;

pub trait HttpPolicy: Send {
    type Error: From<TransportError> + Send;

    /// Recheck the calling context's authority, then charge one HTTP attempt.
    /// Denied requests must not consume quota. Do not admit replayed effects.
    fn admit(&mut self, target: &Uri) -> impl Future<Output = Result<(), Self::Error>> + Send;
}

/// The client cannot be supplied externally: its retry, redirect, proxy, and
/// decoding settings are part of the durable host's dispatch contract.
pub struct HttpSender<P> {
    client: HttpClient,
    policy: P,
}

/// A cloneable, contract-preserving HTTP client for sharing connection pools.
/// Its underlying reqwest client cannot be supplied by callers.
#[derive(Clone)]
pub struct HttpClient(reqwest::Client);

impl HttpClient {
    pub fn new() -> Result<Self, TransportError> {
        Self::build(reqwest::Client::builder())
    }

    pub fn with_root_certificate(
        certificate: reqwest::Certificate,
    ) -> Result<Self, TransportError> {
        Self::build(reqwest::Client::builder().add_root_certificate(certificate))
    }

    fn build(builder: reqwest::ClientBuilder) -> Result<Self, TransportError> {
        builder
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd()
            .no_proxy()
            .referer(false)
            .build()
            .map(Self)
            .map_err(|_| TransportError::Configuration("HTTP client initialization failed".into()))
    }

    pub fn sender<P: HttpPolicy>(&self, policy: P) -> HttpSender<P> {
        HttpSender {
            client: self.clone(),
            policy,
        }
    }
}

impl<P: HttpPolicy> HttpSender<P> {
    pub fn new(policy: P) -> Result<Self, TransportError> {
        Ok(HttpClient::new()?.sender(policy))
    }

    pub fn with_root_certificate(
        policy: P,
        certificate: reqwest::Certificate,
    ) -> Result<Self, TransportError> {
        Ok(HttpClient::with_root_certificate(certificate)?.sender(policy))
    }
}

impl<P: HttpPolicy> HttpSend for HttpSender<P> {
    type Body = reqwest::Body;
    type Error = P::Error;

    async fn send(&mut self, request: Request<Bytes>) -> Result<Response<Self::Body>, Self::Error> {
        let target = request.uri().clone();
        let request = reqwest::Request::try_from(request)
            .map_err(|_| TransportError::InvalidInput("invalid HTTP request".into()))?;
        self.policy.admit(&target).await?;
        let response = self
            .client
            .0
            .execute(request)
            .await
            .map_err(|_| TransportError::Network)?;
        Ok(response.into())
    }
}

#[cfg(test)]
mod tests;

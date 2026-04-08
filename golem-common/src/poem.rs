use crate::base_model::api;
use poem::endpoint::EitherEndpoint;
use poem::{Endpoint, IntoEndpoint, Middleware, Request, Result};
use tracing::info;

#[derive(Debug, Clone, Default)]
pub struct CliClientInfo {
    pub client_version: Option<String>,
    pub client_platform: Option<String>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CliClientInfoMiddleware;

impl CliClientInfoMiddleware {
    pub fn new() -> Self {
        Self
    }
}

impl<E: Endpoint> Middleware<E> for CliClientInfoMiddleware {
    type Output = CliClientInfoEndpoint<E>;

    fn transform(&self, next: E) -> Self::Output {
        CliClientInfoEndpoint { next }
    }
}

pub struct CliClientInfoEndpoint<E> {
    next: E,
}

impl<E: Endpoint> Endpoint for CliClientInfoEndpoint<E> {
    type Output = E::Output;

    async fn call(&self, mut req: Request) -> Result<Self::Output> {
        let client_info = CliClientInfo {
            client_version: req
                .header(api::header::GOLEM_CLI_VERSION)
                .map(ToString::to_string),
            client_platform: req
                .header(api::header::GOLEM_CLI_PLATFORM)
                .map(ToString::to_string),
        };

        if client_info.client_version.is_some() || client_info.client_platform.is_some() {
            info!(
                client_version = client_info.client_version.as_deref(),
                client_platform = client_info.client_platform.as_deref(),
                "OpenAPI client headers detected"
            );
        }

        req.set_data(client_info);
        self.next.call(req).await
    }
}

pub trait LazyEndpointExt: IntoEndpoint {
    fn with_if_lazy<T>(
        self,
        enable: bool,
        middleware: impl FnOnce() -> T,
    ) -> EitherEndpoint<Self, T::Output>
    where
        T: Middleware<Self::Endpoint>,
        Self: Sized,
    {
        if !enable {
            EitherEndpoint::A(self)
        } else {
            EitherEndpoint::B(middleware().transform(self.into_endpoint()))
        }
    }
}

impl<T: IntoEndpoint> LazyEndpointExt for T {}

use crate::base_model::api;
use crate::model::error::ErrorBody;
use opentelemetry::metrics::{Counter, Histogram};
use opentelemetry::propagation::Extractor;
use opentelemetry::trace::{FutureExt, Span, SpanKind, TraceContextExt, Tracer};
use opentelemetry::{Context, Key, KeyValue, global};
use poem::PathPattern;
use poem::endpoint::EitherEndpoint;
use poem::http::{Method, StatusCode};
use poem::web::RealIp;
use poem::{
    Endpoint, FromRequest, IntoEndpoint, IntoResponse, Middleware, Request, Response, Result,
};
use prometheus::{Encoder, Registry, TextEncoder};
use std::sync::Arc;
use std::time::Instant;
use tracing::Instrument;

pub struct PrometheusExporter {
    registry: Registry,
}

impl PrometheusExporter {
    pub fn new(registry: Registry) -> Self {
        Self { registry }
    }
}

impl IntoEndpoint for PrometheusExporter {
    type Endpoint = PrometheusExporterEndpoint;

    fn into_endpoint(self) -> Self::Endpoint {
        PrometheusExporterEndpoint {
            registry: self.registry,
        }
    }
}

pub struct PrometheusExporterEndpoint {
    registry: Registry,
}

impl Endpoint for PrometheusExporterEndpoint {
    type Output = Response;

    async fn call(&self, req: Request) -> Result<Self::Output> {
        if req.method() != Method::GET {
            return Ok(StatusCode::METHOD_NOT_ALLOWED.into());
        }

        let encoder = TextEncoder::new();
        let mut result = Vec::new();
        encoder
            .encode(&self.registry.gather(), &mut result)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        Ok(Response::builder()
            .content_type(encoder.format_type())
            .body(result))
    }
}

pub struct OpenTelemetryTracing<T> {
    tracer: Arc<T>,
}

impl<T> OpenTelemetryTracing<T> {
    pub fn new(tracer: T) -> Self {
        Self {
            tracer: Arc::new(tracer),
        }
    }
}

impl<T, E> Middleware<E> for OpenTelemetryTracing<T>
where
    T: Tracer + Send + Sync,
    T::Span: Send + Sync + 'static,
    E: Endpoint,
{
    type Output = OpenTelemetryTracingEndpoint<T, E>;

    fn transform(&self, inner: E) -> Self::Output {
        OpenTelemetryTracingEndpoint {
            tracer: self.tracer.clone(),
            inner,
        }
    }
}

pub struct OpenTelemetryTracingEndpoint<T, E> {
    tracer: Arc<T>,
    inner: E,
}

struct HeaderExtractor<'a>(&'a poem::http::HeaderMap);

impl Extractor for HeaderExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|value| value.to_str().ok())
    }

    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(|key| key.as_str()).collect()
    }
}

impl<T, E> Endpoint for OpenTelemetryTracingEndpoint<T, E>
where
    T: Tracer + Send + Sync,
    T::Span: Send + Sync + 'static,
    E: Endpoint,
{
    type Output = Response;

    async fn call(&self, req: Request) -> Result<Self::Output> {
        let remote_addr = RealIp::from_request_without_body(&req)
            .await
            .ok()
            .and_then(|real_ip| real_ip.0)
            .map(|addr| addr.to_string())
            .unwrap_or_else(|| req.remote_addr().to_string());

        let parent_context = global::get_text_map_propagator(|propagator| {
            propagator.extract(&HeaderExtractor(req.headers()))
        });
        let method = req.method().to_string();
        let attributes = vec![
            KeyValue::new("telemetry.sdk.name", "poem"),
            KeyValue::new("telemetry.sdk.version", "3.1.12"),
            KeyValue::new("telemetry.sdk.language", "rust"),
            KeyValue::new("http.request.method", method.clone()),
            KeyValue::new("url.full", req.original_uri().to_string()),
            KeyValue::new("client.address", remote_addr),
            KeyValue::new("network.protocol.version", format!("{:?}", req.version())),
        ];

        let mut span = self
            .tracer
            .span_builder(format!("{} {}", method, req.uri()))
            .with_kind(SpanKind::Server)
            .with_attributes(attributes)
            .start_with_context(&*self.tracer, &parent_context);
        span.add_event("request.started".to_string(), vec![]);

        async move {
            let result = self.inner.call(req).await;
            let context = Context::current();
            let span = context.span();

            match result {
                Ok(response) => {
                    let response = response.into_response();
                    set_path_pattern(&span, &method, response.data::<PathPattern>());
                    span.add_event("request.completed".to_string(), vec![]);
                    span.set_attribute(KeyValue::new(
                        "http.response.status_code",
                        response.status().as_u16() as i64,
                    ));
                    if let Some(content_length) = response
                        .headers()
                        .get(poem::http::header::CONTENT_LENGTH)
                        .and_then(|value| value.to_str().ok())
                        .and_then(|value| value.parse::<i64>().ok())
                    {
                        span.set_attribute(KeyValue::new(
                            "http.response.body.size",
                            content_length,
                        ));
                    }
                    Ok(response)
                }
                Err(error) => {
                    set_path_pattern(&span, &method, error.data::<PathPattern>());
                    span.set_attribute(KeyValue::new(
                        "http.response.status_code",
                        error.status().as_u16() as i64,
                    ));
                    span.add_event(
                        "request.error".to_string(),
                        vec![KeyValue::new("exception.message", error.to_string())],
                    );
                    Err(error)
                }
            }
        }
        .with_context(Context::current_with_span(span))
        .await
    }
}

fn set_path_pattern(
    span: &opentelemetry::trace::SpanRef<'_>,
    method: &str,
    path: Option<&PathPattern>,
) {
    if let Some(path) = path {
        const HTTP_PATH_PATTERN: Key = Key::from_static_str("http.path_pattern");
        span.update_name(format!("{} {}", method, path.0));
        span.set_attribute(KeyValue::new(HTTP_PATH_PATTERN, path.0.to_string()));
    }
}

pub struct OpenTelemetryMetrics {
    request_count: Counter<u64>,
    error_count: Counter<u64>,
    duration: Histogram<f64>,
}

impl Default for OpenTelemetryMetrics {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenTelemetryMetrics {
    pub fn new() -> Self {
        Self::from_meter(global::meter("poem"))
    }

    fn from_meter(meter: opentelemetry::metrics::Meter) -> Self {
        Self {
            request_count: meter
                .u64_counter("poem_requests_count")
                .with_description("total request count (since start of service)")
                .build(),
            error_count: meter
                .u64_counter("poem_errors_count")
                .with_description("failed request count (since start of service)")
                .build(),
            duration: meter
                .f64_histogram("poem_request_duration_ms")
                .with_unit("milliseconds")
                .with_description(
                    "request duration histogram (in milliseconds, since start of service)",
                )
                .build(),
        }
    }
}

impl<E: Endpoint> Middleware<E> for OpenTelemetryMetrics {
    type Output = OpenTelemetryMetricsEndpoint<E>;

    fn transform(&self, inner: E) -> Self::Output {
        OpenTelemetryMetricsEndpoint {
            request_count: self.request_count.clone(),
            error_count: self.error_count.clone(),
            duration: self.duration.clone(),
            inner,
        }
    }
}

pub struct OpenTelemetryMetricsEndpoint<E> {
    request_count: Counter<u64>,
    error_count: Counter<u64>,
    duration: Histogram<f64>,
    inner: E,
}

impl<E: Endpoint> Endpoint for OpenTelemetryMetricsEndpoint<E> {
    type Output = Response;

    async fn call(&self, req: Request) -> Result<Self::Output> {
        let mut labels = vec![
            KeyValue::new("http.request.method", req.method().to_string()),
            KeyValue::new("url.full", req.original_uri().to_string()),
        ];
        let start = Instant::now();
        let result = self.inner.call(req).await.map(IntoResponse::into_response);

        match &result {
            Ok(response) => {
                add_path_pattern(&mut labels, response.data::<PathPattern>());
                labels.push(KeyValue::new(
                    "http.response.status_code",
                    response.status().as_u16() as i64,
                ));
            }
            Err(error) => {
                add_path_pattern(&mut labels, error.data::<PathPattern>());
                labels.push(KeyValue::new(
                    "http.response.status_code",
                    error.status().as_u16() as i64,
                ));
                self.error_count.add(1, &labels);
                labels.push(KeyValue::new("exception.message", error.to_string()));
            }
        }

        self.request_count.add(1, &labels);
        self.duration
            .record(start.elapsed().as_secs_f64() * 1000.0, &labels);
        result
    }
}

fn add_path_pattern(labels: &mut Vec<KeyValue>, path: Option<&PathPattern>) {
    if let Some(path) = path {
        labels.push(KeyValue::new("http.path_pattern", path.0.to_string()));
    }
}

#[derive(Debug, Clone, Default)]
pub struct CliClientInfo {
    pub client_version: Option<String>,
    pub client_platform: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct CliClientInfoMiddleware {
    // Placeholder: future fields for version/platform policy configuration.
    // e.g. min_version: Option<semver::Version>, denied_platforms: Vec<String>
}

impl CliClientInfoMiddleware {
    pub fn new() -> Self {
        Self::default()
    }

    /// Decides whether to reject the client with a 410 Gone response.
    ///
    /// Returns `Some(message)` to reject, `None` to allow through.
    /// Currently always returns `None` — placeholder for future policy.
    fn should_reject_client(&self, _client_info: &CliClientInfo) -> Option<String> {
        None
    }
}

impl<E: Endpoint> Middleware<E> for CliClientInfoMiddleware {
    type Output = CliClientInfoEndpoint<E>;

    fn transform(&self, next: E) -> Self::Output {
        CliClientInfoEndpoint {
            middleware: self.clone(),
            next,
        }
    }
}

pub struct CliClientInfoEndpoint<E> {
    middleware: CliClientInfoMiddleware,
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

        let has_client_headers =
            client_info.client_version.is_some() || client_info.client_platform.is_some();

        if has_client_headers
            && let Some(message) = self.middleware.should_reject_client(&client_info)
        {
            return Err(poem::Error::from_response(
                poem::Response::builder()
                    .status(StatusCode::GONE)
                    .content_type("application/json")
                    .body(
                        serde_json::to_string(&ErrorBody {
                            error: message,
                            code: api::error_code::CLI_UPDATE_REQUIRED.to_string(),
                            cause: None,
                        })
                        .unwrap_or_default(),
                    ),
            ));
        }

        req.set_data(client_info.clone());

        if has_client_headers {
            let span = tracing::info_span!(
                "cli_client",
                client_version = client_info.client_version.as_deref().unwrap_or(""),
                client_platform = client_info.client_platform.as_deref().unwrap_or(""),
            );
            self.next.call(req).instrument(span).await
        } else {
            self.next.call(req).await
        }
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

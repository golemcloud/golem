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

use std::sync::{Arc, Mutex};

use golem_common::poem::{HttpMetrics, OpenTelemetryTracing, PrometheusExporter};
use opentelemetry::trace::{SpanId, TracerProvider};
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::{SdkTracerProvider, SimpleSpanProcessor, SpanData, SpanExporter};
use poem::EndpointExt;
use poem::http::StatusCode;
use poem::test::TestClient;
use prometheus::Registry;
use test_r::test;

test_r::enable!();

#[test]
// The exporter is tested with a private registry and metric, outside the catalog.
#[allow(clippy::disallowed_methods)]
async fn prometheus_exporter_serves_registered_metrics() {
    let registry = Registry::new();
    let counter = prometheus::IntCounter::new("requests", "request count").unwrap();
    registry.register(Box::new(counter.clone())).unwrap();
    counter.inc_by(3);

    let client = TestClient::new(PrometheusExporter::new(registry));
    let response = client.get("/").send().await;
    response.assert_status_is_ok();
    response.assert_content_type("text/plain; version=0.0.4");
    response
        .assert_text("# HELP requests request count\n# TYPE requests counter\nrequests 3\n")
        .await;

    client
        .post("/")
        .send()
        .await
        .assert_status(StatusCode::METHOD_NOT_ALLOWED);
}

#[test]
async fn metrics_use_the_matched_route_and_response_status() {
    let app = poem::Route::new()
        .at(
            "/items/:id",
            poem::endpoint::make_sync(|_| StatusCode::CREATED),
        )
        .with(HttpMetrics);

    let client = TestClient::new(app);
    client
        .get("/items/42")
        .send()
        .await
        .assert_status(StatusCode::CREATED);
    client
        .request(
            poem::http::Method::from_bytes(b"BREW").unwrap(),
            "/items/42",
        )
        .send()
        .await
        .assert_status(StatusCode::CREATED);
    client
        .get("/unknown/42")
        .send()
        .await
        .assert_status(StatusCode::NOT_FOUND);

    let output = prometheus::TextEncoder::new()
        .encode_to_string(&prometheus::gather())
        .unwrap();
    assert!(
        output.contains(
            r#"golem_http_requests_total{method="GET",route="/items/:id",status="201"} 1"#
        ),
        "{output}"
    );
    assert!(
        output.contains(
            r#"golem_http_request_duration_seconds_count{method="GET",route="/items/:id",status="201"} 1"#
        ),
        "{output}"
    );
    assert!(
        output.contains(
            r#"golem_http_requests_total{method="other",route="/items/:id",status="201"} 1"#
        ),
        "{output}"
    );
    assert!(
        output.contains(r#"golem_http_requests_total{method="GET",route="other",status="404"} 1"#),
        "{output}"
    );
    assert!(!output.contains("/items/42"), "{output}");

    // The gateway nests the custom HTTP APIs at `/`, which poem records as an empty pattern.
    let nested = poem::Route::new()
        .nest("/", poem::endpoint::make_sync(|_| StatusCode::OK))
        .with(HttpMetrics);
    TestClient::new(nested)
        .get("/custom/api")
        .send()
        .await
        .assert_status(StatusCode::OK);
    let output = prometheus::TextEncoder::new()
        .encode_to_string(&prometheus::gather())
        .unwrap();
    assert!(
        output.contains(r#"golem_http_requests_total{method="GET",route="other",status="200"} 1"#),
        "{output}"
    );
    assert!(!output.contains(r#"route="""#), "{output}");

    // The single executable mounts the registry API with `.at("*", ...)`, which prefixes its
    // patterns with `/*`.
    let inner = poem::Route::new().at(
        "/v1/components/:component_id",
        poem::endpoint::make_sync(|_| StatusCode::ACCEPTED),
    );
    let wildcard = poem::Route::new().at("*", inner).with(HttpMetrics);
    let client = TestClient::new(wildcard);
    client
        .get("/v1/components/c1")
        .send()
        .await
        .assert_status(StatusCode::ACCEPTED);
    client
        .put("/v1/nothing")
        .send()
        .await
        .assert_status(StatusCode::NOT_FOUND);
    let output = prometheus::TextEncoder::new()
        .encode_to_string(&prometheus::gather())
        .unwrap();
    assert!(
        output.contains(
            r#"golem_http_requests_total{method="GET",route="/v1/components/:component_id",status="202"} 1"#
        ),
        "{output}"
    );
    assert!(
        output.contains(r#"golem_http_requests_total{method="PUT",route="other",status="404"} 1"#),
        "{output}"
    );
    assert!(!output.contains(r#"route="/*"#), "{output}");
}

#[derive(Clone, Debug, Default)]
struct CollectingExporter {
    spans: Arc<Mutex<Vec<SpanData>>>,
}

impl SpanExporter for CollectingExporter {
    fn export(
        &self,
        batch: Vec<SpanData>,
    ) -> impl std::future::Future<Output = OTelSdkResult> + Send {
        self.spans.lock().unwrap().extend(batch);
        std::future::ready(Ok(()))
    }
}

#[test]
async fn tracing_extracts_remote_parent_and_names_the_matched_route() {
    opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());
    let exporter = CollectingExporter::default();
    let spans = exporter.spans.clone();
    let provider = SdkTracerProvider::builder()
        .with_span_processor(SimpleSpanProcessor::new(exporter))
        .build();
    let app = poem::Route::new()
        .at("/items/:id", poem::endpoint::make_sync(|_| "ok"))
        .with(OpenTelemetryTracing::new(provider.tracer("test")));

    TestClient::new(app)
        .get("/items/42")
        .header(
            "traceparent",
            "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01",
        )
        .send()
        .await
        .assert_status_is_ok();
    provider.force_flush().unwrap();

    let spans = spans.lock().unwrap();
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].name, "GET /items/:id");
    assert_eq!(
        spans[0].parent_span_id,
        SpanId::from_hex("b7ad6b7169203331").unwrap()
    );
    assert!(spans[0].parent_span_is_remote);
    assert_eq!(
        spans[0]
            .attributes
            .iter()
            .find(|attribute| attribute.key.as_str() == "telemetry.sdk.version")
            .map(|attribute| attribute.value.to_string()),
        Some("3.1.12".to_string())
    );
}

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

use golem_common::poem::{OpenTelemetryMetrics, OpenTelemetryTracing, PrometheusExporter};
use opentelemetry::trace::{SpanId, TracerProvider};
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::{SdkTracerProvider, SimpleSpanProcessor, SpanData, SpanExporter};
use poem::EndpointExt;
use poem::http::StatusCode;
use poem::test::TestClient;
use prometheus::Registry;
use test_r::test;

test_r::enable!();

#[test]
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
    let exporter = opentelemetry_prometheus_text_exporter::ExporterBuilder::default()
        .without_counter_suffixes()
        .without_units()
        .build();
    let provider = SdkMeterProvider::builder()
        .with_reader(exporter.clone())
        .build();
    opentelemetry::global::set_meter_provider(provider);
    let app = poem::Route::new()
        .at(
            "/items/:id",
            poem::endpoint::make_sync(|_| StatusCode::CREATED),
        )
        .with(OpenTelemetryMetrics::new());

    TestClient::new(app)
        .get("/items/42")
        .send()
        .await
        .assert_status(StatusCode::CREATED);

    let mut output = Vec::new();
    exporter.export(&mut output).unwrap();
    let output = String::from_utf8(output).unwrap();
    assert!(output.contains("poem_requests_count"), "{output}");
    assert!(output.contains("poem_request_duration_ms"), "{output}");
    assert!(
        output.contains("http_path_pattern=\"/items/:id\""),
        "{output}"
    );
    assert!(output.contains("http_request_method=\"GET\""), "{output}");
    assert!(
        output.contains("http_response_status_code=\"201\""),
        "{output}"
    );
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

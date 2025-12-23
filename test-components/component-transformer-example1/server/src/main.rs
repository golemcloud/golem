use axum::body::Bytes;
use axum::extract::Multipart;
use axum::{routing::post, Router};
use tracing::{debug, info};
use wac_graph::types::Package;
use wac_graph::{plug, CompositionGraph, EncodeOptions, Processor};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let app = Router::new().route("/transform", post(transform));

    // run our app with hyper, listening globally on port 3000
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn transform(mut multipart: Multipart) -> Vec<u8> {
    let mut component = None;

    while let Some(mut field) = multipart.next_field().await.unwrap() {
        let name = field.name().unwrap().to_string();
        let data = field.bytes().await.unwrap();
        debug!("Length of `{}` is {} bytes", name, data.len());

        match name.as_str() {
            "component" => {
                component = Some(data);
            }
            "metadata" => {
                let json = std::str::from_utf8(&data).expect("Failed to parse metadata as UTF-8");
                info!("Metadata: {}", json);
            }
            _ => {
                let value = std::str::from_utf8(&data).expect("Failed to parse field as UTF-8");
                info!("Configuration field: {} = {}", name, value);
            }
        }
    }

    transform_component(component.expect("did not receive a component part"))
        .expect("Failed to transform component") // TODO: error handling and returning a proper HTTP response with failed status code
}

fn transform_component(component: Bytes) -> anyhow::Result<Vec<u8>> {
    let mut graph = CompositionGraph::new();
    let component = Package::from_bytes("component", None, component, graph.types_mut())?;
    let component = graph.register_package(component)?;

    #[cfg(debug_assertions)]
    let adapter_bytes = include_bytes!("../../target/wasm32-wasip1/debug/adapter.wasm");
    #[cfg(not(debug_assertions))]
    let adapter_bytes = include_bytes!("../../target/wasm32-wasip1/release/adapter.wasm");

    let adapter = Package::from_bytes("adapter", None, adapter_bytes, graph.types_mut())?;
    let adapter = graph.register_package(adapter)?;

    plug(&mut graph, vec![adapter], component)?;

    let transformed_bytes = graph.encode(EncodeOptions {
        processor: Some(Processor {
            name: "component-transformer-example1",
            version: "0.1.0",
        }),
        ..Default::default()
    })?;

    Ok(transformed_bytes)
}

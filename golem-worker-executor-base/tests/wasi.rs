use crate::common;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::AtomicU8;
use std::sync::Arc;
use std::time::Duration;

use crate::common::val_string;
use assert2::{assert, check};

use http::{Response, StatusCode};
use tonic::transport::Body;
use warp::Filter;

#[tokio::test]
async fn write_stdout() {
    let mut executor = common::start().await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/write-stdout.wasm"));
    let worker_id = executor.start_worker(&template_id, "write-stdout-1").await;

    let mut rx = executor.capture_output(&worker_id).await;

    let _result = executor.invoke_and_await(&worker_id, "run", vec![]).await;

    tokio::time::sleep(Duration::from_secs(2)).await;
    let mut events = vec![];
    rx.recv_many(&mut events, 100).await;

    drop(executor);

    check!(
        events
            == vec![common::stdout_event(
                "Sample text written to the output NOT GOOD\n"
            )]
    );
}

#[tokio::test]
async fn read_stdin() {
    let mut executor = common::start().await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/read-stdin.wasm"));
    let worker_id = executor.start_worker(&template_id, "read-stdin-1").await;

    let result = executor.invoke_and_await(&worker_id, "run", vec![]).await;

    drop(executor);

    assert!(result.is_err()); // stdin is disabled in component calling convention
}

#[tokio::test]
async fn http_client() {
    let mut executor = common::start().await.unwrap();

    let http_server = tokio::spawn(async {
        let route = warp::path::end()
            .and(warp::post())
            .and(warp::header::<String>("X-Test"))
            .and(warp::body::bytes())
            .map(|header: String, body: bytes::Bytes| {
                Response::builder()
                    .status(StatusCode::OK)
                    .body(Body::from(format!(
                        "response is {} {}",
                        header,
                        String::from_utf8(body.to_vec()).unwrap()
                    )))
                    .unwrap()
            });

        warp::serve(route)
            .run("0.0.0.0:9999".parse::<SocketAddr>().unwrap())
            .await;
    });

    let template_id = executor.store_template(Path::new("../test-templates/http-client.wasm"));
    let worker_id = executor.start_worker(&template_id, "http-client-1").await;
    let rx = executor.capture_output(&worker_id).await;

    let result = executor
        .invoke_and_await(&worker_id, "golem:it/api/run", vec![])
        .await;

    drop(executor);
    drop(rx);
    http_server.abort();

    check!(
        result
            == Ok(vec![common::val_string(
                "200 response is test-header test-body"
            )])
    );
}

#[tokio::test]
async fn http_client_response_persisted_between_invocations() {
    let mut executor = common::start().await.unwrap();

    let http_server = tokio::spawn(async {
        let call_count = Arc::new(AtomicU8::new(0));
        let route = warp::path::end()
            .and(warp::post())
            .and(warp::header::<String>("X-Test"))
            .and(warp::body::bytes())
            .map(move |header: String, body: bytes::Bytes| {
                let old_count = call_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                match old_count {
                    0 => Response::builder()
                        .status(StatusCode::OK)
                        .body(Body::from(format!(
                            "response is {} {}",
                            header,
                            String::from_utf8(body.to_vec()).unwrap()
                        )))
                        .unwrap(),
                    _ => Response::builder()
                        .status(StatusCode::NOT_FOUND)
                        .body(Body::empty())
                        .unwrap(),
                }
            });

        warp::serve(route)
            .run("0.0.0.0:9999".parse::<SocketAddr>().unwrap())
            .await;
    });

    let template_id = executor.store_template(Path::new("../test-templates/http-client.wasm"));
    let worker_id = executor.start_worker(&template_id, "http-client-2").await;
    let rx = executor.capture_output(&worker_id).await;

    let _ = executor
        .invoke_and_await(&worker_id, "golem:it/api/send-request", vec![])
        .await
        .expect("first send-request failed");

    drop(executor);
    drop(rx);

    let mut executor = common::start().await.unwrap();
    let _rx = executor.capture_output(&worker_id).await;

    let result = executor
        .invoke_and_await(&worker_id, "golem:it/api/process-response", vec![])
        .await;

    http_server.abort();

    check!(result == Ok(vec![val_string("200 response is test-header test-body")]));
}

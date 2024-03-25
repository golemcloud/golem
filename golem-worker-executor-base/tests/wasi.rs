use crate::common;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::AtomicU8;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use assert2::{assert, check};
use golem_common::model::WorkerStatus;
use golem_wasm_rpc::protobuf::{val, Val};
use http_02::{Response, StatusCode};
use tokio::spawn;
use tokio::time::Instant;
use tonic::transport::Body;
use tracing::info;
use warp::Filter;

#[tokio::test]
#[tracing::instrument]
async fn write_stdout() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/write-stdout.wasm"));
    let worker_id = executor.start_worker(&template_id, "write-stdout-1").await;

    let mut rx = executor.capture_output(&worker_id).await;

    let _result = executor.invoke_and_await(&worker_id, "run", vec![]).await;

    tokio::time::sleep(Duration::from_secs(2)).await;
    let mut events = vec![];
    rx.recv_many(&mut events, 100).await;

    drop(executor);

    check!(events == vec![common::stdout_event("Sample text written to the output\n")]);
}

#[tokio::test]
#[tracing::instrument]
async fn write_stderr() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/write-stderr.wasm"));
    let worker_id = executor.start_worker(&template_id, "write-stderr-1").await;

    let mut rx = executor.capture_output(&worker_id).await;

    let _result = executor.invoke_and_await(&worker_id, "run", vec![]).await;

    tokio::time::sleep(Duration::from_secs(2)).await;
    let mut events = vec![];
    rx.recv_many(&mut events, 100).await;

    drop(executor);

    check!(
        events
            == vec![common::stderr_event(
                "Sample text written to the error output\n"
            )]
    );
}

#[tokio::test]
#[tracing::instrument]
async fn read_stdin() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/read-stdin.wasm"));
    let worker_id = executor.start_worker(&template_id, "read-stdin-1").await;

    let result = executor.invoke_and_await(&worker_id, "run", vec![]).await;

    drop(executor);

    assert!(result.is_err()); // stdin is disabled in component calling convention
}

#[tokio::test]
#[tracing::instrument]
async fn clocks() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/clocks.wasm"));
    let worker_id = executor.start_worker(&template_id, "clocks-1").await;

    let result = executor
        .invoke_and_await(&worker_id, "run", vec![])
        .await
        .unwrap();

    drop(executor);

    check!(result.len() == 1);
    let Val {
        val: Some(val::Val::Tuple(tuple)),
    } = &result[0]
    else {
        panic!("expected tuple")
    };
    check!(tuple.values.len() == 3);

    let Val {
        val: Some(val::Val::F64(elapsed1)),
    } = &tuple.values[0]
    else {
        panic!("expected f64")
    };
    let Val {
        val: Some(val::Val::F64(elapsed2)),
    } = &tuple.values[1]
    else {
        panic!("expected f64")
    };
    let Val {
        val: Some(val::Val::String(odt)),
    } = &tuple.values[2]
    else {
        panic!("expected string")
    };

    let epoch_seconds = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    let diff1 = (epoch_seconds - *elapsed1).abs();
    let parsed_odt = chrono::DateTime::parse_from_rfc3339(odt.as_str()).unwrap();
    let odt_diff = epoch_seconds - parsed_odt.timestamp() as f64;

    check!(diff1 < 5.0);
    check!(*elapsed2 >= 2.0);
    check!(*elapsed2 < 3.0);
    check!(odt_diff < 5.0);
}

#[tokio::test]
#[tracing::instrument]
async fn file_write_read_delete() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let template_id =
        executor.store_template(Path::new("../test-templates/file-write-read-delete.wasm"));
    let mut env = HashMap::new();
    env.insert("RUST_BACKTRACE".to_string(), "full".to_string());
    let worker_id = executor
        .try_start_worker_versioned(&template_id, 0, "file-write-read-delete-1", vec![], env)
        .await
        .unwrap();

    let result = executor
        .invoke_and_await(&worker_id, "run", vec![])
        .await
        .unwrap();

    drop(executor);

    check!(
        result
            == vec![common::val_triple(
                common::val_option(None),
                common::val_option(Some(common::val_string("hello world"))),
                common::val_option(None)
            )]
    );
}

#[tokio::test]
#[tracing::instrument]
async fn directories() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/directories.wasm"));
    let worker_id = executor.start_worker(&template_id, "directories-1").await;

    let result = executor
        .invoke_and_await(&worker_id, "run", vec![])
        .await
        .unwrap();

    drop(executor);

    let Val {
        val: Some(val::Val::Tuple(tuple)),
    } = &result[0]
    else {
        panic!("expected tuple")
    };
    check!(tuple.values.len() == 4); //  tuple<u32, list<tuple<string, bool>>, list<tuple<string, bool>>, u32>;

    check!(tuple.values[0] == common::val_u32(0)); // initial number of entries
    check!(
        tuple.values[1]
            == common::val_list(vec![common::val_pair(
                common::val_string("/test"),
                common::val_bool(true)
            )])
    ); // contents of /

    // contents of /test
    let Val {
        val: Some(val::Val::List(list)),
    } = &tuple.values[2]
    else {
        panic!("expected list")
    };
    check!(
        list.values
            == vec![
                common::val_pair(common::val_string("/test/dir1"), common::val_bool(true)),
                common::val_pair(common::val_string("/test/dir2"), common::val_bool(true)),
                common::val_pair(
                    common::val_string("/test/hello.txt"),
                    common::val_bool(false)
                ),
            ]
    );
    check!(tuple.values[3] == common::val_u32(1)); // final number of entries NOTE: this should be 0 if remove_directory worked
}

#[tokio::test]
#[tracing::instrument]
async fn directories_replay() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/directories.wasm"));
    let worker_id = executor.start_worker(&template_id, "directories-1").await;

    let result = executor
        .invoke_and_await(&worker_id, "run", vec![])
        .await
        .unwrap();

    drop(executor);
    let mut executor = common::start(&context).await.unwrap();

    // NOTE: if the directory listing would not be stable, replay would fail with divergence error

    tokio::time::sleep(Duration::from_secs(5)).await;
    let metadata = executor.get_worker_metadata(&worker_id).await.unwrap();

    check!(metadata.last_known_status.status == WorkerStatus::Idle);

    let Val {
        val: Some(val::Val::Tuple(tuple)),
    } = &result[0]
    else {
        panic!("expected tuple")
    };
    check!(tuple.values.len() == 4); //  tuple<u32, list<tuple<string, bool>>, list<tuple<string, bool>>, u32>;

    check!(tuple.values[0] == common::val_u32(0)); // initial number of entries
    check!(
        tuple.values[1]
            == common::val_list(vec![common::val_pair(
                common::val_string("/test"),
                common::val_bool(true)
            )])
    ); // contents of /

    // contents of /test
    let Val {
        val: Some(val::Val::List(list)),
    } = &tuple.values[2]
    else {
        panic!("expected list")
    };
    check!(
        list.values
            == vec![
                common::val_pair(common::val_string("/test/dir1"), common::val_bool(true)),
                common::val_pair(common::val_string("/test/dir2"), common::val_bool(true)),
                common::val_pair(
                    common::val_string("/test/hello.txt"),
                    common::val_bool(false)
                ),
            ]
    );
    check!(tuple.values[3] == common::val_u32(1)); // final number of entries NOTE: this should be 0 if remove_directory worked
}

#[tokio::test]
#[tracing::instrument]
async fn file_write_read() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/file-service.wasm"));
    let worker_id = executor.start_worker(&template_id, "file-service-1").await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/write-file",
            vec![
                common::val_string("/testfile.txt"),
                common::val_string("hello world"),
            ],
        )
        .await
        .unwrap();

    drop(executor);
    let mut executor = common::start(&context).await.unwrap();

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/read-file",
            vec![common::val_string("/testfile.txt")],
        )
        .await
        .unwrap();

    check!(result == vec![common::val_result(Ok(common::val_string("hello world")))]);
}

#[tokio::test]
#[tracing::instrument]
async fn http_client() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let host_http_port = context.host_http_port();
    let http_server = tokio::spawn(async move {
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
            .run(
                format!("0.0.0.0:{}", host_http_port)
                    .parse::<SocketAddr>()
                    .unwrap(),
            )
            .await;
    });

    let template_id = executor.store_template(Path::new("../test-templates/http-client.wasm"));
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());
    env.insert("RUST_BACKTRACE".to_string(), "full".to_string());

    let worker_id = executor
        .try_start_worker_versioned(&template_id, 0, "http-client-1", vec![], env)
        .await
        .unwrap();
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
#[tracing::instrument]
async fn http_client_using_reqwest() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();
    let captured_body: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let captured_body_clone = captured_body.clone();
    let host_http_port = context.host_http_port();
    let http_server = tokio::spawn(async move {
        let route = warp::path("post-example")
            .and(warp::post())
            .and(warp::header::optional::<String>("X-Test"))
            .and(warp::body::bytes())
            .map(move |header: Option<String>, body: bytes::Bytes| {
                let body_str = String::from_utf8(body.to_vec()).unwrap();
                {
                    let mut capture = captured_body_clone.lock().unwrap();
                    *capture = Some(body_str.clone());
                }
                Response::builder()
                    .status(StatusCode::OK)
                    .body(Body::from(format!(
                        "{{ \"percentage\" : 0.25, \"message\": \"response message {}\" }}",
                        header.unwrap_or("no X-Test header".to_string()),
                    )))
                    .unwrap()
            });

        warp::serve(route)
            .run(
                format!("0.0.0.0:{}", host_http_port)
                    .parse::<SocketAddr>()
                    .unwrap(),
            )
            .await;
    });

    let template_id = executor.store_template(Path::new("../test-templates/http-client-2.wasm"));
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = executor
        .try_start_worker_versioned(&template_id, 0, "http-client-reqwest-1", vec![], env)
        .await
        .unwrap();

    let result = executor
        .invoke_and_await(&worker_id, "golem:it/api/run", vec![])
        .await
        .unwrap();
    let captured_body = captured_body.lock().unwrap().clone().unwrap();

    drop(executor);
    http_server.abort();

    check!(result == vec![common::val_string("200 ExampleResponse { percentage: 0.25, message: Some(\"response message Golem\") }")]);
    check!(
        captured_body
            == "{\"name\":\"Something\",\"amount\":42,\"comments\":[\"Hello\",\"World\"]}"
                .to_string()
    );
}

#[tokio::test]
#[tracing::instrument]
async fn environment_service() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let template_id =
        executor.store_template(Path::new("../test-templates/environment-service.wasm"));
    let args = vec!["test-arg".to_string()];
    let mut env = HashMap::new();
    env.insert("TEST_ENV".to_string(), "test-value".to_string());
    let worker_id = executor
        .try_start_worker_versioned(&template_id, 0, "environment-service-1", args, env)
        .await
        .unwrap();

    let args_result = executor
        .invoke_and_await(&worker_id, "golem:it/api/get-arguments", vec![])
        .await
        .unwrap();

    let env_result = executor
        .invoke_and_await(&worker_id, "golem:it/api/get-environment", vec![])
        .await
        .unwrap();

    drop(executor);

    check!(
        args_result
            == vec![common::val_result(Ok(common::val_list(vec![
                common::val_string("test-arg")
            ])))]
    );
    check!(
        env_result
            == vec![common::val_result(Ok(common::val_list(vec![
                common::val_pair(
                    common::val_string("TEST_ENV"),
                    common::val_string("test-value")
                ),
                common::val_pair(
                    common::val_string("GOLEM_WORKER_NAME"),
                    common::val_string("environment-service-1")
                ),
                common::val_pair(
                    common::val_string("GOLEM_TEMPLATE_ID"),
                    common::val_string(&template_id.to_string())
                ),
                common::val_pair(
                    common::val_string("GOLEM_TEMPLATE_VERSION"),
                    common::val_string("0")
                ),
            ])))]
    );
}

#[tokio::test]
#[tracing::instrument]
async fn http_client_response_persisted_between_invocations() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();
    let host_http_port = context.host_http_port();

    let http_server = tokio::spawn(async move {
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
            .run(
                format!("0.0.0.0:{}", host_http_port)
                    .parse::<SocketAddr>()
                    .unwrap(),
            )
            .await;
    });

    let template_id = executor.store_template(Path::new("../test-templates/http-client.wasm"));
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let worker_id = executor
        .try_start_worker_versioned(&template_id, 0, "http-client-2", vec![], env)
        .await
        .unwrap();
    let rx = executor.capture_output(&worker_id).await;

    let _ = executor
        .invoke_and_await(&worker_id, "golem:it/api/send-request", vec![])
        .await
        .expect("first send-request failed");

    drop(executor);
    drop(rx);

    let mut executor = common::start(&context).await.unwrap();
    let _rx = executor.capture_output(&worker_id).await;

    let result = executor
        .invoke_and_await(&worker_id, "golem:it/api/process-response", vec![])
        .await;

    http_server.abort();

    check!(
        result
            == Ok(vec![common::val_string(
                "200 response is test-header test-body"
            )])
    );
}

#[tokio::test]
#[tracing::instrument]
async fn sleep() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/clock-service.wasm"));
    let worker_id = executor.start_worker(&template_id, "clock-service-1").await;

    let _ = executor
        .invoke_and_await(&worker_id, "golem:it/api/sleep", vec![common::val_u64(10)])
        .await
        .unwrap();

    drop(executor);
    let mut executor = common::start(&context).await.unwrap();

    let start = Instant::now();
    let _ = executor
        .invoke_and_await(&worker_id, "golem:it/api/sleep", vec![common::val_u64(0)])
        .await
        .unwrap();
    let duration = start.elapsed();

    check!(duration.as_secs() < 2);
}

#[tokio::test]
#[tracing::instrument]
async fn resuming_sleep() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/clock-service.wasm"));
    let worker_id = executor.start_worker(&template_id, "clock-service-2").await;

    let mut executor_clone = executor.async_clone().await;
    let worker_id_clone = worker_id.clone();
    let fiber = spawn(async move {
        executor_clone
            .invoke_and_await(
                &worker_id_clone,
                "golem:it/api/sleep",
                vec![common::val_u64(10)],
            )
            .await
            .unwrap();
    });

    tokio::time::sleep(Duration::from_secs(5)).await;

    drop(executor);
    let _ = fiber.await;

    info!("Restarting worker...");

    let mut executor = common::start(&context).await.unwrap();

    info!("Worker restarted");

    let start = Instant::now();
    let _ = executor
        .invoke_and_await(&worker_id, "golem:it/api/sleep", vec![common::val_u64(10)])
        .await
        .unwrap();
    let duration = start.elapsed();

    check!(duration.as_secs() < 20);
    check!(duration.as_secs() >= 10);
}

#[tokio::test]
#[tracing::instrument]
async fn failing_worker() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let template_id =
        executor.store_template(Path::new("../test-templates/failing-component.wasm"));
    let worker_id = executor
        .start_worker(&template_id, "failing-worker-1")
        .await;

    let result1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:component/api/add",
            vec![common::val_u64(5)],
        )
        .await;

    let result2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:component/api/add",
            vec![common::val_u64(50)],
        )
        .await;

    let result3 = executor
        .invoke_and_await(&worker_id, "golem:component/api/get", vec![])
        .await;

    drop(executor);

    check!(result1.is_ok());
    check!(result2.is_err());
    check!(result3.is_err());
    check!(result2
        .clone()
        .err()
        .unwrap()
        .to_string()
        .starts_with("Runtime error: error while executing at wasm backtrace:"));
    check!(result2
        .err()
        .unwrap()
        .to_string()
        .contains("<unknown>!golem:component/api#add"));
    check!(
        result3.err().unwrap().to_string() == "The previously invoked function failed".to_string()
    );
}

#[tokio::test]
#[tracing::instrument]
async fn file_service_write_direct() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/file-service.wasm"));
    let worker_id = executor.start_worker(&template_id, "file-service-2").await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/write-file-direct",
            vec![
                common::val_string("testfile.txt"),
                common::val_string("hello world"),
            ],
        )
        .await
        .unwrap();

    drop(executor);
    let mut executor = common::start(&context).await.unwrap();

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/read-file",
            vec![common::val_string("/testfile.txt")],
        )
        .await
        .unwrap();

    check!(result == vec![common::val_result(Ok(common::val_string("hello world")))]);
}

#[tokio::test]
#[tracing::instrument]
async fn filesystem_write_replay_restores_file_times() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/file-service.wasm"));
    let worker_id = executor.start_worker(&template_id, "file-service-3").await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/write-file-direct",
            vec![
                common::val_string("testfile.txt"),
                common::val_string("hello world"),
            ],
        )
        .await
        .unwrap();
    let times1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/get-file-info",
            vec![common::val_string("/testfile.txt")],
        )
        .await
        .unwrap();

    drop(executor);
    let mut executor = common::start(&context).await.unwrap();

    let times2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/get-file-info",
            vec![common::val_string("/testfile.txt")],
        )
        .await
        .unwrap();

    check!(times1 == times2);
}

#[tokio::test]
#[tracing::instrument]
async fn filesystem_create_dir_replay_restores_file_times() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/file-service.wasm"));
    let worker_id = executor.start_worker(&template_id, "file-service-4").await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/create-directory",
            vec![common::val_string("/test")],
        )
        .await
        .unwrap();
    let times1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/get-info",
            vec![common::val_string("/")],
        )
        .await
        .unwrap();

    drop(executor);
    let mut executor = common::start(&context).await.unwrap();

    let times2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/get-info",
            vec![common::val_string("/")],
        )
        .await
        .unwrap();

    check!(times1 == times2);
}

#[tokio::test]
#[tracing::instrument]
async fn file_hard_link() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/file-service.wasm"));
    let worker_id = executor.start_worker(&template_id, "file-service-5").await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/write-file",
            vec![
                common::val_string("/testfile.txt"),
                common::val_string("hello world"),
            ],
        )
        .await
        .unwrap();

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/create-link",
            vec![
                common::val_string("/testfile.txt"),
                common::val_string("/link.txt"),
            ],
        )
        .await
        .unwrap();

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/read-file",
            vec![common::val_string("/link.txt")],
        )
        .await
        .unwrap();

    check!(result == vec![common::val_result(Ok(common::val_string("hello world")))]);
}

#[tokio::test]
#[tracing::instrument]
async fn filesystem_link_replay_restores_file_times() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/file-service.wasm"));
    let worker_id = executor.start_worker(&template_id, "file-service-6").await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/create-directory",
            vec![common::val_string("/test")],
        )
        .await
        .unwrap();
    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/create-directory",
            vec![common::val_string("/test2")],
        )
        .await
        .unwrap();
    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/write-file",
            vec![
                common::val_string("/test/testfile.txt"),
                common::val_string("hello world"),
            ],
        )
        .await
        .unwrap();
    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/create-link",
            vec![
                common::val_string("/test/testfile.txt"),
                common::val_string("/test2/link.txt"),
            ],
        )
        .await
        .unwrap();

    let times_file_1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/get-info",
            vec![common::val_string("/test2/link.txt")],
        )
        .await
        .unwrap();
    let times_dir_1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/get-info",
            vec![common::val_string("/test2")],
        )
        .await
        .unwrap();

    drop(executor);
    let mut executor = common::start(&context).await.unwrap();

    let times_dir_2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/get-info",
            vec![common::val_string("/test2")],
        )
        .await
        .unwrap();
    let times_file_2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/get-info",
            vec![common::val_string("/test2/link.txt")],
        )
        .await
        .unwrap();

    check!(times_dir_1 == times_dir_2);
    check!(times_file_1 == times_file_2);
}

#[tokio::test]
#[tracing::instrument]
async fn filesystem_remove_dir_replay_restores_file_times() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/file-service.wasm"));
    let worker_id = executor.start_worker(&template_id, "file-service-7").await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/create-directory",
            vec![common::val_string("/test")],
        )
        .await
        .unwrap();
    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/create-directory",
            vec![common::val_string("/test/a")],
        )
        .await
        .unwrap();
    _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/remove-directory",
            vec![common::val_string("/test/a")],
        )
        .await
        .unwrap();
    let times1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/get-info",
            vec![common::val_string("/test")],
        )
        .await
        .unwrap();

    drop(executor);
    let mut executor = common::start(&context).await.unwrap();

    let times2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/get-info",
            vec![common::val_string("/test")],
        )
        .await
        .unwrap();

    check!(times1 == times2);
}

#[tokio::test]
#[tracing::instrument]
async fn filesystem_symlink_replay_restores_file_times() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/file-service.wasm"));
    let worker_id = executor.start_worker(&template_id, "file-service-8").await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/create-directory",
            vec![common::val_string("/test")],
        )
        .await
        .unwrap();
    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/create-directory",
            vec![common::val_string("/test2")],
        )
        .await
        .unwrap();
    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/write-file-direct",
            vec![
                common::val_string("test/testfile.txt"),
                common::val_string("hello world"),
            ],
        )
        .await
        .unwrap();
    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/create-sym-link",
            vec![
                common::val_string("../test/testfile.txt"),
                common::val_string("/test2/link.txt"),
            ],
        )
        .await
        .unwrap();

    let times_file_1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/get-info",
            vec![common::val_string("/test2/link.txt")],
        )
        .await
        .unwrap();
    let times_dir_1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/get-info",
            vec![common::val_string("/test2")],
        )
        .await
        .unwrap();

    drop(executor);

    let mut executor = common::start(&context).await.unwrap();

    let times_dir_2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/get-info",
            vec![common::val_string("/test2")],
        )
        .await
        .unwrap();
    let times_file_2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/get-info",
            vec![common::val_string("/test2/link.txt")],
        )
        .await
        .unwrap();

    check!(times_dir_1 == times_dir_2);
    check!(times_file_1 == times_file_2);
}

#[tokio::test]
#[tracing::instrument]
async fn filesystem_rename_replay_restores_file_times() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/file-service.wasm"));
    let worker_id = executor.start_worker(&template_id, "file-service-9").await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/create-directory",
            vec![common::val_string("/test")],
        )
        .await
        .unwrap();
    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/create-directory",
            vec![common::val_string("/test2")],
        )
        .await
        .unwrap();
    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/write-file",
            vec![
                common::val_string("/test/testfile.txt"),
                common::val_string("hello world"),
            ],
        )
        .await
        .unwrap();
    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/rename-file",
            vec![
                common::val_string("/test/testfile.txt"),
                common::val_string("/test2/link.txt"),
            ],
        )
        .await
        .unwrap();

    let times_srcdir_1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/get-info",
            vec![common::val_string("/test")],
        )
        .await
        .unwrap();
    let times_destdir_1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/get-info",
            vec![common::val_string("/test2")],
        )
        .await
        .unwrap();
    let times_file_1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/get-info",
            vec![common::val_string("/test2/link.txt")],
        )
        .await
        .unwrap();

    drop(executor);
    let mut executor = common::start(&context).await.unwrap();

    let times_srcdir_2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/get-info",
            vec![common::val_string("/test")],
        )
        .await
        .unwrap();
    let times_destdir_2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/get-info",
            vec![common::val_string("/test2")],
        )
        .await
        .unwrap();
    let times_file_2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/get-info",
            vec![common::val_string("/test2/link.txt")],
        )
        .await
        .unwrap();

    check!(times_srcdir_1 == times_srcdir_2);
    check!(times_destdir_1 == times_destdir_2);
    check!(times_file_1 == times_file_2);
}

#[tokio::test]
#[tracing::instrument]
async fn filesystem_remove_file_replay_restores_file_times() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/file-service.wasm"));
    let worker_id = executor.start_worker(&template_id, "file-service-10").await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/create-directory",
            vec![common::val_string("/test")],
        )
        .await
        .unwrap();
    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/write-file",
            vec![
                common::val_string("/test/testfile.txt"),
                common::val_string("hello world"),
            ],
        )
        .await
        .unwrap();
    _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/remove-file",
            vec![common::val_string("/test/testfile.txt")],
        )
        .await
        .unwrap();
    let times1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/get-info",
            vec![common::val_string("/test")],
        )
        .await
        .unwrap();

    drop(executor);
    let mut executor = common::start(&context).await.unwrap();

    let times2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/get-info",
            vec![common::val_string("/test")],
        )
        .await
        .unwrap();

    check!(times1 == times2);
}

#[tokio::test]
#[tracing::instrument]
async fn filesystem_write_via_stream_replay_restores_file_times() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/file-service.wasm"));
    let worker_id = executor.start_worker(&template_id, "file-service-3").await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/write-file",
            vec![
                common::val_string("/testfile.txt"),
                common::val_string("hello world"),
            ],
        )
        .await
        .unwrap();
    let times1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/get-file-info",
            vec![common::val_string("/testfile.txt")],
        )
        .await
        .unwrap();

    drop(executor);
    let mut executor = common::start(&context).await.unwrap();

    let times2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/get-file-info",
            vec![common::val_string("/testfile.txt")],
        )
        .await
        .unwrap();

    check!(times1 == times2);
}

#[tokio::test]
#[tracing::instrument]
async fn filesystem_metadata_hash() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/file-service.wasm"));
    let worker_id = executor.start_worker(&template_id, "file-service-3").await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/write-file-direct",
            vec![
                common::val_string("testfile.txt"),
                common::val_string("hello world"),
            ],
        )
        .await
        .unwrap();
    let hash1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/hash",
            vec![common::val_string("testfile.txt")],
        )
        .await
        .unwrap();

    drop(executor);
    let mut executor = common::start(&context).await.unwrap();

    let hash2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/hash",
            vec![common::val_string("testfile.txt")],
        )
        .await
        .unwrap();

    check!(hash1 == hash2);
}

#[tokio::test]
#[tracing::instrument]
async fn ip_address_resolve() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/networking.wasm"));
    let worker_id = executor
        .start_worker(&template_id, "ip-address-resolve-1")
        .await;

    let result1 = executor
        .invoke_and_await(&worker_id, "golem:it/api/get", vec![])
        .await
        .unwrap();

    drop(executor);
    let mut executor = common::start(&context).await.unwrap();

    // If the recovery succeeds, that means that the replayed IP address resolution produced the same result as expected

    let result2 = executor
        .invoke_and_await(&worker_id, "golem:it/api/get", vec![])
        .await
        .unwrap();

    // Result 2 is a fresh resolution which is not guaranteed to return the same addresses (or the same order) but we can expect
    // that it could resolve golem.cloud to at least one address.
    check!(result1.len() > 0);
    check!(result2.len() > 0);
}

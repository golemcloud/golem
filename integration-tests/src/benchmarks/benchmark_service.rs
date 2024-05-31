use std::num::ParseIntError;
use poem::{
    get, handler, listener::TcpListener, middleware::Tracing, post, web::Json, web::Path,
    EndpointExt, Route, Server,
};
use golem_test_framework::config::CliParams;
use golem_test_framework::config::cli::TestMode;
use golem_test_framework::dsl::benchmark::{BenchmarkConfig, BenchmarkResult};
use integration_tests::benchmarks::cold_start_large;

#[handler]
async fn run_cold_start_large() -> Json<BenchmarkResult> {
   let result = cold_start_large::run_with_params(CliParams {
        mode: TestMode::Spawned {
            workspace_root: ".".to_string(),
            build_target: "target/debug".to_string(),
            redis_port: 6379,
            redis_prefix: "".to_string(),
            shard_manager_http_port: 9021,
            shard_manager_grpc_port: 9020,
            component_service_http_port: 8081,
            component_service_grpc_port: 9091,
            component_compilation_service_http_port: 8083,
            component_compilation_service_grpc_port: 9094,
            component_compilation_disabled: false,
            worker_service_http_port: 8082,
            worker_service_grpc_port: 9092,
            worker_service_custom_request_port: 9093,
            worker_executor_base_http_port: 9000,
            worker_executor_base_grpc_port: 9100,
            mute_child: false,
        },
        component_directory: "test_components".to_string(),
        benchmark_config: BenchmarkConfig {
            iterations: 3,
            cluster_size: vec![3],
            size: vec![10],
            length: vec![100]
        },
        quiet: false,
        verbose: false,
    }).await;
    Json(result)
}

#[handler]
fn echo(Path(s): Path<String>) -> String {
    s
}

#[tokio::main]
async fn main() -> Result<(), std::io::Error> {
    let port: u16 = std::env::var("HTTP_PORT")
        .map_err(|e| e.to_string())
        .and_then(|v| v.parse().map_err(|e: ParseIntError| e.to_string()))
        .unwrap_or(3001);

    let addr = format!("0.0.0.0:{}", port);

    let app = Route::new()
        .at("/cold_start_large", get(run_cold_start_large));

    Server::new(TcpListener::bind(addr))
        .name("benchmark-servic")
        .run(app)
        .await
}

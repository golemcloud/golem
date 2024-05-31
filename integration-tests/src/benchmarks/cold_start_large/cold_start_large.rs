use integration_tests::benchmarks::cold_start_large::ColdStartEchoLarge;
use integration_tests::benchmarks::run_benchmark;

#[tokio::main]
async fn main() {
    run_benchmark::<ColdStartEchoLarge>().await;
}

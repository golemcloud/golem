// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

#[tokio::main]
async fn main() {
    match std::env::args_os()
        .next()
        .map(|s| s.to_string_lossy().into_owned())
        .as_deref()
    {
        Some("cold_start_large") => {
            integration_tests::benchmarks::cold_start_large::run().await;
        }
        Some("cold_start_medium") => {
            integration_tests::benchmarks::cold_start_medium::run().await;
        }
        Some("cold_start_small") => {
            integration_tests::benchmarks::cold_start_small::run().await;
        }
        Some("durability_overhead") => {
            integration_tests::benchmarks::durability_overhead::run().await;
        }
        Some("large_dynamic_memory") => {
            integration_tests::benchmarks::large_dynamic_memory::run().await;
        }
        Some("large_initial_memory") => {
            integration_tests::benchmarks::large_initial_memory::run().await;
        }
        Some("latency_large") => {
            integration_tests::benchmarks::latency_large::run().await;
        }
        Some("latency_medium") => {
            integration_tests::benchmarks::latency_medium::run().await;
        }
        Some("latency_small") => {
            integration_tests::benchmarks::latency_small::run().await;
        }
        Some("rpc") => {
            integration_tests::benchmarks::rpc::run().await;
        }
        Some("rpc_cpu_intensive") => {
            integration_tests::benchmarks::rpc_cpu_intensive::run().await;
        }
        Some("rpc_large_input") => {
            integration_tests::benchmarks::rpc_large_input::run().await;
        }
        Some("simple_worker_echo") => {
            integration_tests::benchmarks::simple_worker_echo::run().await;
        }
        Some("suspend_worker") => {
            integration_tests::benchmarks::suspend_worker::run().await;
        }
        Some("throughput") => {
            integration_tests::benchmarks::throughput::run().await;
        }
        Some("throughput_cpu_intensive") => {
            integration_tests::benchmarks::throughput_cpu_intensive::run().await;
        }
        Some("throughput_large_input") => {
            integration_tests::benchmarks::throughput_large_input::run().await;
        }

        _ => {
            eprintln!("No benchmark specified. Please provide a benchmark name as an argument.");
            std::process::exit(1);
        }
    }
}

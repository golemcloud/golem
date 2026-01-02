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

use golem_common::tracing::init_tracing_with_default_env_filter;
use golem_common::SafeDisplay;
use golem_debugging_service::config::make_debug_config_loader;
use golem_debugging_service::run;
use golem_worker_executor::metrics as base_metrics;
use std::sync::Arc;
use tracing::info;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    match make_debug_config_loader().load_or_dump_config() {
        Some(debug_config) => {
            rustls::crypto::ring::default_provider()
                .install_default()
                .expect("Failed to install crypto provider");

            init_tracing_with_default_env_filter(&debug_config.tracing);
            info!(
                "Using configuration:\n{}",
                debug_config.to_safe_string_indented()
            );

            let prometheus = base_metrics::register_all();

            let runtime = Arc::new(
                tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .unwrap(),
            );

            runtime.block_on(run(debug_config, prometheus, runtime.handle().clone()))
        }
        None => Ok(()),
    }
}

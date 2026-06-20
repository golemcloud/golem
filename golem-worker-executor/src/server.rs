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

use golem_common::SafeDisplay;
use golem_common::tracing::init_tracing_with_default_env_filter;
use golem_worker_executor::bootstrap;
use golem_worker_executor::metrics;
use golem_worker_executor::services::golem_config::{GolemConfig, make_config_loader};
use std::sync::Arc;
use tokio::task::JoinSet;
use tracing::info;

#[cfg(feature = "jemalloc-prof")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[cfg(not(feature = "jemalloc-prof"))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> Result<(), anyhow::Error> {
    match make_config_loader().load_or_dump_config() {
        Some(mut config) => {
            rustls::crypto::ring::default_provider()
                .install_default()
                .expect("Failed to install crypto provider");

            config.add_port_to_tracing_file_name_if_enabled();
            init_tracing_with_default_env_filter(&config.tracing);
            info!("Using configuration:\n{}", config.to_safe_string_indented());

            let prometheus = metrics::register_all();

            let runtime = Arc::new(
                tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()?,
            );

            runtime.block_on(async_main(config, prometheus, runtime.clone()))
        }
        None => Ok(()),
    }
}

async fn async_main(
    config: GolemConfig,
    prometheus: prometheus::Registry,
    runtime: Arc<tokio::runtime::Runtime>,
) -> Result<(), anyhow::Error> {
    spawn_heap_profile_dump_on_signal(runtime.handle().clone());

    let mut join_set = JoinSet::new();

    let _run_details =
        bootstrap::run(config, prometheus, runtime.handle().clone(), &mut join_set).await?;

    while let Some(res) = join_set.join_next().await {
        res??
    }
    Ok(())
}

/// On jemalloc profiling builds, install a SIGUSR2 handler that writes a heap
/// profile to `/heap`. Configure jemalloc with MALLOC_CONF, for example:
/// `prof:true,prof_active:true,lg_prof_sample:21,prof_prefix:/heap/worker-executor`.
#[cfg(all(feature = "jemalloc-prof", target_os = "linux"))]
fn spawn_heap_profile_dump_on_signal(handle: tokio::runtime::Handle) {
    use std::ffi::CString;
    use tracing::{error, info, warn};

    handle.spawn(async move {
        let mut sigusr2 =
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::user_defined2()) {
                Ok(stream) => stream,
                Err(err) => {
                    error!("failed to install SIGUSR2 heap-profile handler: {err}");
                    return;
                }
            };

        info!("SIGUSR2 jemalloc heap-profile handler installed");
        loop {
            sigusr2.recv().await;

            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let path = format!(
                "/heap/worker-executor-{}-{timestamp}.heap",
                std::process::id()
            );
            let c_path = match CString::new(path.clone()) {
                Ok(path) => path,
                Err(err) => {
                    warn!("failed to prepare heap profile path {path}: {err}");
                    continue;
                }
            };

            let dump_result = unsafe {
                tikv_jemalloc_ctl::raw::write(
                    b"prof.dump\0",
                    c_path.as_ptr() as *const std::os::raw::c_char,
                )
            };

            match dump_result {
                Ok(()) => info!("jemalloc heap profile written to {path}"),
                Err(err) => warn!("failed to write jemalloc heap profile to {path}: {err}"),
            }
        }
    });
}

#[cfg(not(all(feature = "jemalloc-prof", target_os = "linux")))]
fn spawn_heap_profile_dump_on_signal(_handle: tokio::runtime::Handle) {}

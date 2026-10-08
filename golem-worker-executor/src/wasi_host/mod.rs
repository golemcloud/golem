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

use std::time::Duration;

use crate::durable_host::DurableWorkerCtx;
use crate::workerctx::WorkerCtx;
use wasmtime::Engine;
use wasmtime::component::{HasSelf, Linker};
use wasmtime_wasi::cli::{StdinStream, StdoutStream};
use wasmtime_wasi::{IoCtx, ResourceTable, WasiCtx, WasiCtxBuilder};

pub mod helpers;
pub mod logging;

pub fn create_linker<Ctx: WorkerCtx + Send + Sync>(
    engine: &Engine,
    get: fn(&mut Ctx) -> &mut DurableWorkerCtx<Ctx>,
) -> wasmtime::Result<Linker<Ctx>> {
    let mut linker = Linker::new(engine);

    // Register Golem-owned wrappers for all p3 WASI and wasi-http interfaces.
    crate::durable_host::p3::add_to_linker(&mut linker, get)?;

    let mut network_link_options =
        wasmtime_wasi::p2::bindings::sockets::network::LinkOptions::default();
    network_link_options.network_error_code(true);

    wasmtime_wasi::p2::bindings::cli::environment::add_to_linker::<
        _,
        HasSelf<DurableWorkerCtx<Ctx>>,
    >(&mut linker, get)?;
    wasmtime_wasi::p2::bindings::cli::exit::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
        &mut linker,
        get,
    )?;
    wasmtime_wasi::p2::bindings::cli::stderr::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
        &mut linker,
        get,
    )?;
    wasmtime_wasi::p2::bindings::cli::stdin::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
        &mut linker,
        get,
    )?;
    wasmtime_wasi::p2::bindings::cli::stdout::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
        &mut linker,
        get,
    )?;
    wasmtime_wasi::p2::bindings::cli::terminal_input::add_to_linker::<
        _,
        HasSelf<DurableWorkerCtx<Ctx>>,
    >(&mut linker, get)?;
    wasmtime_wasi::p2::bindings::cli::terminal_output::add_to_linker::<
        _,
        HasSelf<DurableWorkerCtx<Ctx>>,
    >(&mut linker, get)?;
    wasmtime_wasi::p2::bindings::cli::terminal_stderr::add_to_linker::<
        _,
        HasSelf<DurableWorkerCtx<Ctx>>,
    >(&mut linker, get)?;
    wasmtime_wasi::p2::bindings::cli::terminal_stdin::add_to_linker::<
        _,
        HasSelf<DurableWorkerCtx<Ctx>>,
    >(&mut linker, get)?;
    wasmtime_wasi::p2::bindings::cli::terminal_stdout::add_to_linker::<
        _,
        HasSelf<DurableWorkerCtx<Ctx>>,
    >(&mut linker, get)?;
    crate::preview2::p2_monotonic_clock::wasi::clocks0_2_6::monotonic_clock::add_to_linker::<
        _,
        HasSelf<DurableWorkerCtx<Ctx>>,
    >(&mut linker, get)?;
    wasmtime_wasi::p2::bindings::clocks::wall_clock::add_to_linker::<
        _,
        HasSelf<DurableWorkerCtx<Ctx>>,
    >(&mut linker, get)?;
    crate::wasi_filesystem::p2::add_to_linker(&mut linker, get)?;
    wasmtime_wasi::p2::bindings::io::error::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
        &mut linker,
        get,
    )?;
    crate::durable_host::io::poll::add_to_linker(&mut linker, get)?;
    wasmtime_wasi::p2::bindings::io::streams::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
        &mut linker,
        get,
    )?;
    wasmtime_wasi::p2::bindings::random::random::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
        &mut linker,
        get,
    )?;
    wasmtime_wasi::p2::bindings::random::insecure::add_to_linker::<
        _,
        HasSelf<DurableWorkerCtx<Ctx>>,
    >(&mut linker, get)?;
    wasmtime_wasi::p2::bindings::random::insecure_seed::add_to_linker::<
        _,
        HasSelf<DurableWorkerCtx<Ctx>>,
    >(&mut linker, get)?;
    wasmtime_wasi::p2::bindings::sockets::instance_network::add_to_linker::<
        _,
        HasSelf<DurableWorkerCtx<Ctx>>,
    >(&mut linker, get)?;
    wasmtime_wasi::p2::bindings::sockets::ip_name_lookup::add_to_linker::<
        _,
        HasSelf<DurableWorkerCtx<Ctx>>,
    >(&mut linker, get)?;
    wasmtime_wasi::p2::bindings::sockets::network::add_to_linker::<
        _,
        HasSelf<DurableWorkerCtx<Ctx>>,
    >(&mut linker, &network_link_options, get)?;
    wasmtime_wasi::p2::bindings::sockets::tcp::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
        &mut linker,
        get,
    )?;
    wasmtime_wasi::p2::bindings::sockets::tcp_create_socket::add_to_linker::<
        _,
        HasSelf<DurableWorkerCtx<Ctx>>,
    >(&mut linker, get)?;
    wasmtime_wasi::p2::bindings::sockets::udp::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
        &mut linker,
        get,
    )?;
    wasmtime_wasi::p2::bindings::sockets::udp_create_socket::add_to_linker::<
        _,
        HasSelf<DurableWorkerCtx<Ctx>>,
    >(&mut linker, get)?;

    wasmtime_wasi_http::p2::bindings::http::outgoing_handler::add_to_linker::<
        _,
        HasSelf<DurableWorkerCtx<Ctx>>,
    >(&mut linker, get)?;
    let http_link_options = wasmtime_wasi_http::p2::bindings::LinkOptions::default();
    wasmtime_wasi_http::p2::bindings::http::types::add_to_linker::<
        _,
        HasSelf<DurableWorkerCtx<Ctx>>,
    >(&mut linker, &(&http_link_options).into(), get)?;

    crate::preview2::wasi::blobstore::blobstore::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
        &mut linker,
        get,
    )?;
    crate::preview2::wasi::blobstore::container::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
        &mut linker,
        get,
    )?;
    crate::preview2::wasi::blobstore::types::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
        &mut linker,
        get,
    )?;
    crate::preview2::wasi::keyvalue::cache::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
        &mut linker,
        get,
    )?;
    crate::preview2::wasi::keyvalue::eventual::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
        &mut linker,
        get,
    )?;
    crate::preview2::wasi::keyvalue::eventual_batch::add_to_linker::<
        _,
        HasSelf<DurableWorkerCtx<Ctx>>,
    >(&mut linker, get)?;
    crate::preview2::wasi::keyvalue::types::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
        &mut linker,
        get,
    )?;
    crate::preview2::wasi::keyvalue::wasi_keyvalue_error::add_to_linker::<
        _,
        HasSelf<DurableWorkerCtx<Ctx>>,
    >(&mut linker, get)?;
    crate::preview2::wasi::logging::logging::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
        &mut linker,
        get,
    )?;
    crate::preview2::wasi::config::store::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
        &mut linker,
        get,
    )?;

    crate::preview2::golem::rdbms::ignite2::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
        &mut linker,
        get,
    )?;
    crate::preview2::golem::rdbms::mysql::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
        &mut linker,
        get,
    )?;
    crate::preview2::golem::rdbms::postgres::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
        &mut linker,
        get,
    )?;

    crate::preview2::golem::websocket::client::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
        &mut linker,
        get,
    )?;

    crate::preview2::golem::quota::types::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
        &mut linker,
        get,
    )?;

    crate::preview2::golem::secrets::types::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
        &mut linker,
        get,
    )?;
    crate::preview2::golem::secrets::reveal::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
        &mut linker,
        get,
    )?;

    Ok(linker)
}

pub fn create_context(
    args: &[impl AsRef<str>],
    stdin: impl StdinStream + Sized + 'static,
    stdout: impl StdoutStream + Sized + 'static,
    stderr: impl StdoutStream + Sized + 'static,
    suspend_signal: impl Fn(Duration) -> wasmtime::Error + Send + Sync + 'static,
    suspend_threshold: Option<Duration>,
) -> Result<(WasiCtx, IoCtx, ResourceTable), anyhow::Error> {
    let table = ResourceTable::new();
    let mut builder = WasiCtxBuilder::new();
    if let Some(threshold) = suspend_threshold {
        builder.set_suspend(threshold, suspend_signal);
    }
    let (wasi, io_ctx) = builder
        .args(args)
        .stdin(stdin)
        .stdout(stdout)
        .stderr(stderr)
        .monotonic_clock(helpers::clocks::monotonic_clock())
        .allow_ip_name_lookup(true)
        .inherit_network()
        .build();

    Ok((wasi, io_ctx, table))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use test_r::test;
    use wasmtime_wasi::p2::bindings::io::poll::Host;

    struct Ready;

    #[async_trait::async_trait]
    impl wasmtime_wasi::Pollable for Ready {
        async fn ready(&mut self) {}
    }

    #[test]
    async fn durable_borrowed_poll_disables_native_deadline_callback_and_preserves_empty_error() {
        let callbacks = Arc::new(AtomicUsize::new(0));
        for threshold in [None, Some(Duration::ZERO)] {
            let count = callbacks.clone();
            let (_, mut io_ctx, mut table) = create_context(
                &[] as &[&str],
                std::io::empty(),
                std::io::empty(),
                std::io::empty(),
                move |_| {
                    count.fetch_add(1, Ordering::AcqRel);
                    wasmtime::Error::msg("ephemeral deadline")
                },
                threshold,
            )
            .unwrap();
            let parent = table.push(Ready).unwrap();
            let pollable = wasmtime_wasi::subscribe(
                &mut table,
                parent,
                Some(std::time::Instant::now() + Duration::from_secs(60)),
            )
            .unwrap();
            let mut io = wasmtime_wasi::IoData {
                table: &mut table,
                io_ctx: &mut io_ctx,
            };
            let result = Host::poll(&mut io, vec![pollable]).await;
            if threshold.is_none() {
                assert_eq!(result.unwrap(), vec![0]);
                assert_eq!(callbacks.load(Ordering::Acquire), 0);
            } else {
                assert!(
                    result
                        .unwrap_err()
                        .to_string()
                        .contains("ephemeral deadline")
                );
                assert_eq!(callbacks.load(Ordering::Acquire), 1);
            }
            assert!(
                Host::poll(&mut io, vec![])
                    .await
                    .unwrap_err()
                    .to_string()
                    .contains("empty poll list")
            );
        }
    }
}

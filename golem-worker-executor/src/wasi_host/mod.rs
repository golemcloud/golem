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

use std::path::PathBuf;
use std::time::Duration;

use crate::durable_host::DurableWorkerCtx;
use crate::workerctx::WorkerCtx;
use wasmtime::component::{HasSelf, Linker};
use wasmtime::Engine;
use wasmtime_wasi::cli::{StdinStream, StdoutStream};
use wasmtime_wasi::{DirPerms, FilePerms, IoCtx, ResourceTable, WasiCtx, WasiCtxBuilder};

pub mod helpers;
pub mod logging;

pub fn create_linker<Ctx: WorkerCtx + Send + Sync>(
    engine: &Engine,
    get: fn(&mut Ctx) -> &mut DurableWorkerCtx<Ctx>,
) -> wasmtime::Result<Linker<Ctx>> {
    let mut linker = Linker::new(engine);

    let mut exit_link_options = wasmtime_wasi::p2::bindings::cli::exit::LinkOptions::default();
    exit_link_options.cli_exit_with_code(true);

    let mut network_link_options =
        wasmtime_wasi::p2::bindings::sockets::network::LinkOptions::default();
    network_link_options.network_error_code(true);

    wasmtime_wasi::p2::bindings::cli::environment::add_to_linker::<
        _,
        HasSelf<DurableWorkerCtx<Ctx>>,
    >(&mut linker, get)?;
    wasmtime_wasi::p2::bindings::cli::exit::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
        &mut linker,
        &exit_link_options,
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
    wasmtime_wasi::p2::bindings::clocks::monotonic_clock::add_to_linker::<
        _,
        HasSelf<DurableWorkerCtx<Ctx>>,
    >(&mut linker, get)?;
    wasmtime_wasi::p2::bindings::clocks::wall_clock::add_to_linker::<
        _,
        HasSelf<DurableWorkerCtx<Ctx>>,
    >(&mut linker, get)?;
    wasmtime_wasi::p2::bindings::filesystem::preopens::add_to_linker::<
        _,
        HasSelf<DurableWorkerCtx<Ctx>>,
    >(&mut linker, get)?;
    wasmtime_wasi::p2::bindings::filesystem::types::add_to_linker::<
        _,
        HasSelf<DurableWorkerCtx<Ctx>>,
    >(&mut linker, get)?;
    wasmtime_wasi::p2::bindings::io::error::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
        &mut linker,
        get,
    )?;
    wasmtime_wasi::p2::bindings::io::poll::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
        &mut linker,
        get,
    )?;
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

    wasmtime_wasi_http::bindings::http::outgoing_handler::add_to_linker::<
        _,
        HasSelf<DurableWorkerCtx<Ctx>>,
    >(&mut linker, get)?;
    let http_link_options = wasmtime_wasi_http::bindings::LinkOptions::default();
    wasmtime_wasi_http::bindings::http::types::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
        &mut linker,
        &(&http_link_options).into(),
        get,
    )?;

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
    crate::preview2::wasi::keyvalue::atomic::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
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

    crate::preview2::golem::rdbms::mysql::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
        &mut linker,
        get,
    )?;
    crate::preview2::golem::rdbms::postgres::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
        &mut linker,
        get,
    )?;

    Ok(linker)
}

pub fn create_context(
    args: &[impl AsRef<str>],
    root_dir: PathBuf,
    stdin: impl StdinStream + Sized + 'static,
    stdout: impl StdoutStream + Sized + 'static,
    stderr: impl StdoutStream + Sized + 'static,
    suspend_signal: impl Fn(Duration) -> wasmtime::Error + Send + Sync + 'static,
    suspend_threshold: Duration,
) -> Result<(WasiCtx, IoCtx, ResourceTable), anyhow::Error> {
    let table = ResourceTable::new();
    let (wasi, io_ctx) = WasiCtxBuilder::new()
        .args(args)
        .stdin(stdin)
        .stdout(stdout)
        .stderr(stderr)
        .monotonic_clock(helpers::clocks::monotonic_clock())
        .preopened_dir(root_dir.clone(), "/", DirPerms::all(), FilePerms::all())?
        .preopened_dir(root_dir, ".", DirPerms::all(), FilePerms::all())?
        .set_suspend(suspend_threshold, suspend_signal)
        .allow_ip_name_lookup(true)
        .build();

    Ok((wasi, io_ctx, table))
}

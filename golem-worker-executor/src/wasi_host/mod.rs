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

use std::path::PathBuf;
use std::time::Duration;

use crate::durable_host::DurableWorkerCtx;
use crate::workerctx::WorkerCtx;
use wasmtime::Engine;
use wasmtime::component::{HasSelf, Linker};
use wasmtime_wasi::cli::{StdinStream, StdoutStream};
use wasmtime_wasi::{DirPerms, FilePerms, IoCtx, ResourceTable, WasiCtx, WasiCtxBuilder};

pub mod helpers;
pub mod logging;

pub fn create_linker<Ctx: WorkerCtx + Send + Sync>(
    engine: &Engine,
    _get: fn(&mut Ctx) -> &mut DurableWorkerCtx<Ctx>,
) -> wasmtime::Result<Linker<Ctx>> {
    let mut linker = Linker::new(engine);

    // Bulk register all p3 WASI interfaces. Requires Ctx: WasiView.
    wasmtime_wasi::p3::add_to_linker(&mut linker)?;
    // Bulk register all p3 wasi-http interfaces. Requires Ctx: WasiHttpView.
    wasmtime_wasi_http::p3::add_to_linker(&mut linker)?;

    crate::preview2::wasi::blobstore::blobstore::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
        &mut linker,
        _get,
    )?;
    crate::preview2::wasi::blobstore::container::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
        &mut linker,
        _get,
    )?;
    crate::preview2::wasi::blobstore::types::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
        &mut linker,
        _get,
    )?;
    crate::preview2::wasi::keyvalue::atomic::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
        &mut linker,
        _get,
    )?;
    crate::preview2::wasi::keyvalue::cache::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
        &mut linker,
        _get,
    )?;
    crate::preview2::wasi::keyvalue::eventual::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
        &mut linker,
        _get,
    )?;
    crate::preview2::wasi::keyvalue::eventual_batch::add_to_linker::<
        _,
        HasSelf<DurableWorkerCtx<Ctx>>,
    >(&mut linker, _get)?;
    crate::preview2::wasi::keyvalue::types::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
        &mut linker,
        _get,
    )?;
    crate::preview2::wasi::keyvalue::wasi_keyvalue_error::add_to_linker::<
        _,
        HasSelf<DurableWorkerCtx<Ctx>>,
    >(&mut linker, _get)?;
    crate::preview2::wasi::logging::logging::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
        &mut linker,
        _get,
    )?;
    crate::preview2::wasi::config::store::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
        &mut linker,
        _get,
    )?;

    crate::preview2::golem::rdbms::ignite2::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
        &mut linker,
        _get,
    )?;
    crate::preview2::golem::rdbms::mysql::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
        &mut linker,
        _get,
    )?;
    crate::preview2::golem::rdbms::postgres::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
        &mut linker,
        _get,
    )?;

    crate::preview2::golem::websocket::client::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
        &mut linker,
        _get,
    )?;

    crate::preview2::golem::quota::types::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
        &mut linker,
        _get,
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

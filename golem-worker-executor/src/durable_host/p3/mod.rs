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

//! Golem-owned wrappers for the WASI Preview 3 interfaces.
//!
//! These mirror the Preview 2 wrappers in the parent module: every host function has a
//! Golem-owned implementation so that durability can be layered on later. For now every
//! method directly delegates to the underlying `wasmtime_wasi` / `wasmtime_wasi_http`
//! implementation.
//!
//! The Preview 3 bindings split each interface into a synchronous `Host` trait (implemented
//! on a `HasData::Data` view type) and a store-based `HostWithStore` trait (implemented on
//! the `HasData` marker type itself, with method-generic, unbounded store type parameters).
//!
//! To intercept both halves we register every interface with a single Golem marker
//! [`DurableP3`] whose [`HasData::Data`] is [`DurableP3View`]. The synchronous `Host` traits
//! are implemented on [`DurableP3View`] and delegate to the matching `wasmtime_wasi` view.
//! The store-based `HostWithStore` traits are implemented on [`DurableP3`] and delegate to the
//! built-in `wasmtime_wasi` marker implementations (which own private stream/body machinery)
//! by re-projecting the [`Access`](wasmtime::component::Access) /
//! [`Accessor`](wasmtime::component::Accessor) onto the built-in view via the bridge getters
//! below.

use std::any::{Any, type_name};
use std::future::Future;
use std::marker::PhantomData;

use crate::durable_host::DurableWorkerCtx;
use crate::durable_host::concurrent::{CallHandle, CallReplayOutcome, Cancellable};
use crate::workerctx::WorkerCtx;
use golem_common::model::oplog::{DurableFunctionType, HostPayloadPair};
use wasmtime::component::{HasData, Linker};

use wasmtime_wasi::cli::{WasiCliCtxView, WasiCliView};
use wasmtime_wasi::clocks::{WasiClocksCtxView, WasiClocksView};
use wasmtime_wasi::filesystem::{WasiFilesystemCtxView, WasiFilesystemView};
use wasmtime_wasi::sockets::{WasiSocketsCtxView, WasiSocketsView};
use wasmtime_wasi_http::p3::{WasiHttpCtxView, WasiHttpView};

mod cli;
mod clocks;
mod filesystem;
mod http;
mod random;
mod sockets;

/// `HasData` marker selecting [`DurableP3View`] as the per-call view for all Preview 3
/// interfaces. The store-based `HostWithStore` traits are implemented on this type.
pub struct DurableP3<Ctx>(PhantomData<fn() -> Ctx>);

impl<Ctx: WorkerCtx> HasData for DurableP3<Ctx> {
    type Data<'a> = DurableP3View<'a, Ctx>;
}

/// Per-call view wrapping the worker context. The synchronous `Host` traits are implemented on
/// this type and delegate to the underlying `wasmtime_wasi` views.
pub struct DurableP3View<'a, Ctx: WorkerCtx>(&'a mut Ctx);

/// Getter projecting the store data into the Golem Preview 3 view.
fn durable_p3_view<Ctx: WorkerCtx>(ctx: &mut Ctx) -> DurableP3View<'_, Ctx> {
    DurableP3View(ctx)
}

/// Re-projects the method-generic, unbounded store type `U` back to the concrete `Ctx`.
///
/// The Preview 3 `HostWithStore` methods are generic over the store type with no bounds, but
/// these wrappers are only ever registered into a `Linker<Ctx>`, so `U` is always `Ctx` at
/// runtime. This downcast recovers `Ctx` so we can produce the built-in `wasmtime_wasi` view
/// required to delegate to the built-in `HostWithStore` implementations.
fn expect_ctx<Ctx: WorkerCtx, U: 'static>(u: &mut U) -> &mut Ctx {
    (u as &mut dyn Any)
        .downcast_mut::<Ctx>()
        .unwrap_or_else(|| {
            panic!(
                "durable p3 WASI wrapper registered with unexpected store data type: expected {}, got {}",
                type_name::<Ctx>(),
                type_name::<U>(),
            )
        })
}

fn durable_worker_ctx<Ctx: WorkerCtx, U: 'static>(u: &mut U) -> &mut DurableWorkerCtx<Ctx> {
    expect_ctx::<Ctx, U>(u).durable_ctx_mut()
}

async fn run_read_access<T, D, Ctx, Pair, F, Fut>(
    store: &wasmtime::component::Accessor<T, D>,
    request: Pair::Req,
    function_type: DurableFunctionType,
    live: F,
) -> wasmtime::Result<Pair::Resp>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
    Pair: HostPayloadPair,
    F: FnOnce() -> Fut,
    Fut: Future<Output = wasmtime::Result<Pair::Resp>>,
{
    let mut handle = CallHandle::<Pair, Cancellable>::start_access(
        store,
        durable_worker_ctx::<Ctx, T>,
        request,
        function_type,
    )
    .await
    .map_err(wasmtime::Error::from)?;

    if !handle.is_live() {
        match handle
            .replay_access(store, durable_worker_ctx::<Ctx, T>)
            .await
            .map_err(wasmtime::Error::from)?
        {
            CallReplayOutcome::Replayed(response) => return Ok(response),
            CallReplayOutcome::Incomplete(live_handle) => handle = live_handle,
        }
    }

    let response = match live().await {
        Ok(response) => response,
        Err(err) => {
            // Mark the escaping trap with this call's own scope so post-trap retry grouping is
            // immune to ambient state a sibling subtask could have clobbered. The context is pure
            // (call-owned), so no store access is needed here.
            return Err(wasmtime::Error::from_anyhow(handle.trap(err)));
        }
    };

    handle
        .complete_access(store, durable_worker_ctx::<Ctx, T>, response)
        .await
        .map_err(wasmtime::Error::from)
}

fn wasi_clocks_view<Ctx: WorkerCtx, U: 'static>(u: &mut U) -> WasiClocksCtxView<'_> {
    WasiClocksView::clocks(expect_ctx::<Ctx, U>(u))
}

#[allow(dead_code)]
fn wasi_cli_view<Ctx: WorkerCtx, U: 'static>(u: &mut U) -> WasiCliCtxView<'_> {
    WasiCliView::cli(expect_ctx::<Ctx, U>(u))
}

#[allow(dead_code)]
fn wasi_filesystem_view<Ctx: WorkerCtx, U: 'static>(u: &mut U) -> WasiFilesystemCtxView<'_> {
    WasiFilesystemView::filesystem(expect_ctx::<Ctx, U>(u))
}

fn wasi_sockets_view<Ctx: WorkerCtx, U: 'static>(u: &mut U) -> WasiSocketsCtxView<'_> {
    WasiSocketsView::sockets(expect_ctx::<Ctx, U>(u))
}

fn wasi_http_view<Ctx: WorkerCtx, U: 'static>(u: &mut U) -> WasiHttpCtxView<'_> {
    WasiHttpView::http(expect_ctx::<Ctx, U>(u))
}

/// Registers Golem-owned wrappers for all WASI Preview 3 interfaces (both `wasmtime_wasi::p3`
/// and `wasmtime_wasi_http::p3`) into the given linker.
pub fn add_to_linker<Ctx: WorkerCtx>(
    linker: &mut Linker<Ctx>,
    _get: fn(&mut Ctx) -> &mut DurableWorkerCtx<Ctx>,
) -> wasmtime::Result<()> {
    use wasmtime_wasi::p3::bindings::cli;
    use wasmtime_wasi::p3::bindings::clocks;
    use wasmtime_wasi::p3::bindings::filesystem;
    use wasmtime_wasi::p3::bindings::random;
    use wasmtime_wasi::p3::bindings::sockets;
    use wasmtime_wasi_http::p3::bindings::http;

    cli::environment::add_to_linker::<_, DurableP3<Ctx>>(linker, durable_p3_view::<Ctx>)?;
    let exit_options = cli::exit::LinkOptions::default();
    cli::exit::add_to_linker::<_, DurableP3<Ctx>>(linker, &exit_options, durable_p3_view::<Ctx>)?;
    cli::stdin::add_to_linker::<_, DurableP3<Ctx>>(linker, durable_p3_view::<Ctx>)?;
    cli::stdout::add_to_linker::<_, DurableP3<Ctx>>(linker, durable_p3_view::<Ctx>)?;
    cli::stderr::add_to_linker::<_, DurableP3<Ctx>>(linker, durable_p3_view::<Ctx>)?;
    cli::terminal_input::add_to_linker::<_, DurableP3<Ctx>>(linker, durable_p3_view::<Ctx>)?;
    cli::terminal_output::add_to_linker::<_, DurableP3<Ctx>>(linker, durable_p3_view::<Ctx>)?;
    cli::terminal_stdin::add_to_linker::<_, DurableP3<Ctx>>(linker, durable_p3_view::<Ctx>)?;
    cli::terminal_stdout::add_to_linker::<_, DurableP3<Ctx>>(linker, durable_p3_view::<Ctx>)?;
    cli::terminal_stderr::add_to_linker::<_, DurableP3<Ctx>>(linker, durable_p3_view::<Ctx>)?;

    clocks::types::add_to_linker::<_, DurableP3<Ctx>>(linker, durable_p3_view::<Ctx>)?;
    clocks::system_clock::add_to_linker::<_, DurableP3<Ctx>>(linker, durable_p3_view::<Ctx>)?;
    clocks::monotonic_clock::add_to_linker::<_, DurableP3<Ctx>>(linker, durable_p3_view::<Ctx>)?;

    filesystem::types::add_to_linker::<_, DurableP3<Ctx>>(linker, durable_p3_view::<Ctx>)?;
    filesystem::preopens::add_to_linker::<_, DurableP3<Ctx>>(linker, durable_p3_view::<Ctx>)?;

    random::random::add_to_linker::<_, DurableP3<Ctx>>(linker, durable_p3_view::<Ctx>)?;
    random::insecure::add_to_linker::<_, DurableP3<Ctx>>(linker, durable_p3_view::<Ctx>)?;
    random::insecure_seed::add_to_linker::<_, DurableP3<Ctx>>(linker, durable_p3_view::<Ctx>)?;

    sockets::types::add_to_linker::<_, DurableP3<Ctx>>(linker, durable_p3_view::<Ctx>)?;
    sockets::ip_name_lookup::add_to_linker::<_, DurableP3<Ctx>>(linker, durable_p3_view::<Ctx>)?;

    http::types::add_to_linker::<_, DurableP3<Ctx>>(linker, durable_p3_view::<Ctx>)?;
    http::client::add_to_linker::<_, DurableP3<Ctx>>(linker, durable_p3_view::<Ctx>)?;

    Ok(())
}

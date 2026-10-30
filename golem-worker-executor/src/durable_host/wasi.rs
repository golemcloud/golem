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

use super::*;
use wasmtime_wasi_http::{Error as HttpTransportError, WasiBody};

impl DurableHttpHooks {
    /// Classifies errors after transport ownership has moved outside the Store.
    pub(super) fn transport_error_to_p3(
        &mut self,
        error: &HttpTransportError,
    ) -> wasmtime_wasi_http::p3::bindings::http::types::ErrorCode {
        wasmtime_wasi_http::p3::error_to_p3(self, error)
    }
}

impl wasmtime_wasi_http::WasiHttpHooks for DurableHttpHooks {
    fn send_request(
        &mut self,
        request: ::http::Request<WasiBody>,
        options: Option<RequestOptions>,
        fut: Box<dyn Future<Output = Result<(), HttpTransportError>> + Send>,
    ) -> Box<
        dyn Future<
                Output = Result<
                    (
                        ::http::Response<WasiBody>,
                        Box<dyn Future<Output = Result<(), HttpTransportError>> + Send>,
                    ),
                    HttpTransportError,
                >,
            > + Send,
    > {
        _ = fut;
        let connection_pool = self.connection_pool.clone();
        Box::new(async move {
            match connection_pool {
                Some(pool) => {
                    let (response, io, _pooled_connection) =
                        pool.send_request(request, options).await?;
                    Ok((response, io))
                }
                None => {
                    let (res, io) =
                        wasmtime_wasi_http::default_send_request(request, options).await?;
                    let io: Box<dyn Future<Output = Result<(), HttpTransportError>> + Send> =
                        Box::new(io);
                    Ok((res.map(BodyExt::boxed_unsync), io))
                }
            }
        })
    }
}

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    pub fn as_wasi_view(&mut self) -> DurableWorkerCtxWasiView<'_, Ctx> {
        DurableWorkerCtxWasiView(self)
    }

    pub fn as_wasi_http_view_p3(&mut self) -> wasmtime_wasi_http::WasiHttpCtxView<'_> {
        let is_replay = self.state.is_replay();
        self.http_hooks
            .is_replay
            .store(is_replay, std::sync::atomic::Ordering::Release);
        let inner = &mut *self;
        let table = Arc::get_mut(&mut inner.table)
            .expect("ResourceTable is shared and cannot be borrowed mutably")
            .get_mut()
            .expect("ResourceTable mutex must never fail");
        wasmtime_wasi_http::WasiHttpCtxView {
            ctx: &mut inner.wasi_http,
            table,
            hooks: &mut inner.http_hooks,
        }
    }
}

impl<Ctx: WorkerCtx> IoView for DurableWorkerCtxWasiView<'_, Ctx> {
    fn table(&mut self) -> &mut ResourceTable {
        self.0.table()
    }

    fn io_ctx(&mut self) -> &mut IoCtx {
        self.0.io_ctx()
    }

    fn io_data(&mut self) -> IoData<'_> {
        let inner = &mut *self.0;
        let table = Arc::get_mut(&mut inner.table)
            .expect("ResourceTable is shared and cannot be borrowed mutably")
            .get_mut()
            .expect("ResourceTable mutex must never fail");
        let io_ctx = Arc::get_mut(&mut inner.io_ctx)
            .expect("IoCtx is shared and cannot be borrowed mutably")
            .get_mut()
            .expect("IoCtx mutex must never fail");
        IoData { table, io_ctx }
    }
}

impl<Ctx: WorkerCtx> WasiView for DurableWorkerCtx<Ctx> {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        let ctx = Arc::get_mut(&mut self.wasi)
            .expect("WasiCtx is shared and cannot be borrowed mutably")
            .get_mut()
            .expect("WasiCtx mutex must never fail");
        let table = Arc::get_mut(&mut self.table)
            .expect("ResourceTable is shared and cannot be borrowed mutably")
            .get_mut()
            .expect("ResourceTable mutex must never fail");
        let io_ctx = Arc::get_mut(&mut self.io_ctx)
            .expect("IoCtx is shared and cannot be borrowed mutably")
            .get_mut()
            .expect("IoCtx mutex must never fail");
        WasiCtxView { ctx, table, io_ctx }
    }
}

// This wrapper forces the compiler to choose the wasmtime_wasi implementations for T: WasiView
impl<Ctx: WorkerCtx> WasiView for DurableWorkerCtxWasiView<'_, Ctx> {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        let inner = &mut *self.0;
        let ctx = Arc::get_mut(&mut inner.wasi)
            .expect("WasiCtx is shared and cannot be borrowed mutably")
            .get_mut()
            .expect("WasiCtx mutex must never fail");
        let table = Arc::get_mut(&mut inner.table)
            .expect("ResourceTable is shared and cannot be borrowed mutably")
            .get_mut()
            .expect("ResourceTable mutex must never fail");
        let io_ctx = Arc::get_mut(&mut inner.io_ctx)
            .expect("IoCtx is shared and cannot be borrowed mutably")
            .get_mut()
            .expect("IoCtx mutex must never fail");
        WasiCtxView { ctx, table, io_ctx }
    }
}

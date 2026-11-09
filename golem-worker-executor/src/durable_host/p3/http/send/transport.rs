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

pub(in crate::durable_host::p3::http) fn poison_p3_pooled_connection(
    connection: &Option<PooledConnection>,
) {
    if let Some(connection) = connection {
        connection.poison();
    }
}

impl<Ctx: WorkerCtx> PhysicalSendRequest<Ctx> {
    pub(super) async fn attempt(
        &self,
    ) -> Result<
        (
            http::Response<UnsyncBoxBody<Bytes, ErrorCode>>,
            Box<dyn Future<Output = Result<(), ErrorCode>> + Send>,
            Option<PooledConnection>,
        ),
        ErrorCode,
    > {
        let body = self
            .body
            .replayer()
            .map_err(wasmtime_wasi_http::Error::from)
            .boxed_unsync();
        let mut request = http::Request::builder()
            .method(self.method.clone())
            .uri(self.uri.clone())
            .version(self.version)
            .body(body)
            .map_err(|err| ErrorCode::InternalError(Some(err.to_string())))?;
        *request.headers_mut() = self.headers.clone();

        let sent = match &self.pool {
            Some(pool) => pool
                .send_request(request, self.options)
                .await
                .map(|(response, io, connection)| (response, io, Some(connection))),
            None => match wasmtime_wasi_http::default_send_request(request, self.options).await {
                Ok((response, io)) => Ok((
                    response.map(http_body_util::BodyExt::boxed_unsync),
                    Box::new(io)
                        as Box<dyn Future<Output = Result<(), wasmtime_wasi_http::Error>> + Send>,
                    None,
                )),
                Err(error) => Err(error),
            },
        };
        let (response, io, connection) =
            sent.map_err(|error| self.hooks.clone().transport_error_to_p3(&error))?;
        let mut body_hooks = self.hooks.clone();
        let response = response.map(|body| {
            body.map_err(move |error| body_hooks.transport_error_to_p3(&error))
                .boxed_unsync()
        });
        let mut io_hooks = self.hooks.clone();
        let io = Box::new(async move {
            Box::into_pin(io)
                .await
                .map_err(|error| io_hooks.transport_error_to_p3(&error))
        });
        Ok((response, io, connection))
    }
}

pub(in crate::durable_host::p3::http) async fn convert_physical_send_request<Ctx, U>(
    store: &Accessor<U, DurableP3<Ctx>>,
    req: Resource<Request>,
    send_start_index: OplogIndex,
    recording_enabled: bool,
    restored_body: Option<DurableRequestBody>,
    restored_transmission: Option<oneshot::Sender<Result<(), ErrorCode>>>,
) -> Result<PhysicalSend<Ctx>, PhysicalSendConversionError>
where
    Ctx: WorkerCtx,
    U: Send + 'static,
{
    let (final_transmission_tx, final_transmission_rx) = oneshot::channel();
    let final_transmission_tx = restored_transmission.unwrap_or(final_transmission_tx);
    let (hooks, oplog) = store.with(|mut access| {
        let ctx = durable_worker_ctx::<Ctx, U>(access.data_mut());
        (ctx.http_hooks.clone(), ctx.state.oplog.clone())
    });
    let http_store = store.with_getter::<WasiHttp>(wasi_http_view::<Ctx, U>);
    let converted = http_store.with(|mut access| {
        let request = access
            .get()
            .table
            .delete(req)
            .context("failed to delete p3 HTTP request from table")
            .map_err(wasmtime::Error::from_anyhow)
            .map_err(HttpError::trap)?;
        let converted = request.into_http_with_getter(
            access.as_context_mut(),
            async move {
                final_transmission_rx
                    .await
                    .unwrap_or(Ok(()))
                    .map_err(Into::into)
            },
            wasi_http_view::<Ctx, U>,
        )?;
        HttpResult::Ok(converted)
    });

    let (http_request, options) = match converted {
        Ok(converted) => converted,
        Err(error) => {
            if let Some(error_code) = error.downcast_ref().cloned() {
                return Err(PhysicalSendConversionError::HttpError(
                    PhysicalSendHttpError {
                        error_code,
                        final_transmission_tx,
                    },
                ));
            }
            return Err(PhysicalSendConversionError::Trap(anyhow::anyhow!(
                "failed to convert p3 HTTP request: {error:?}"
            )));
        }
    };

    let options = options.as_deref().copied();
    let (parts, body) = http_request.into_parts();
    let mut body_hooks = hooks.clone();
    let body = body
        .map_err(move |error| body_hooks.transport_error_to_p3(&error))
        .boxed_unsync();
    let structural_append_tasks = store.with(|mut access| {
        durable_worker_ctx::<Ctx, U>(access.data_mut())
            .public_state
            .worker()
            .runtime_append_tasks()
    });
    Ok(PhysicalSend::Replayable(PhysicalSendRequest {
        pool: hooks.connection_pool.clone(),
        hooks,
        method: parts.method,
        uri: parts.uri,
        version: parts.version,
        headers: parts.headers,
        body: restored_body.unwrap_or_else(|| {
            DurableRequestBody::new(
                body,
                oplog,
                send_start_index,
                recording_enabled,
                structural_append_tasks,
            )
        }),
        options,
        final_transmission_tx,
        _phantom: PhantomData,
    }))
}

pub(in crate::durable_host::p3::http) fn response_resource_from_http<Ctx, U>(
    store: &Accessor<U, DurableP3<Ctx>>,
    response: http::Response<UnsyncBoxBody<Bytes, ErrorCode>>,
    io: Box<dyn Future<Output = Result<(), ErrorCode>> + Send>,
    final_transmission_tx: oneshot::Sender<Result<(), ErrorCode>>,
    recovery: RequestBodyRecovery,
) -> HttpResult<Resource<Response>>
where
    Ctx: WorkerCtx,
    U: Send + 'static,
{
    let (parts, body) = response.into_parts();
    let mut io = Box::into_pin(io);
    let body = match io.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(result) => {
            let _ = final_transmission_tx.send(result.clone());
            body
        }
        Poll::Pending => {
            let (io_result_tx, io_result_rx) = oneshot::channel();
            let io_task = tokio::task::spawn(async move {
                let result = io.await;
                debug!(?result, "p3 HTTP send I/O future finished");
                let _ = io_result_tx.send(result);
            });
            let io_guard = AbortOnDropIoTask(io_task, recovery);
            tokio::task::spawn(async move {
                let result = io_result_rx.await.unwrap_or(Ok(()));
                let _ = final_transmission_tx.send(result);
            });
            BodyWithState {
                body,
                _state: io_guard,
            }
            .boxed_unsync()
        }
    };
    let response = http::Response::from_parts(parts, body);
    let http_store = store.with_getter::<WasiHttp>(wasi_http_view::<Ctx, U>);
    let (response, response_io) = http_store.with(|mut access| {
        wasmtime_wasi_http::p3::Response::from_http(access.get().hooks, response)
    });
    tokio::task::spawn(async move {
        let result = response_io.await;
        debug!(?result, "p3 HTTP response body I/O future finished");
    });
    http_store.with(|mut access| {
        access
            .get()
            .table
            .push(response)
            .context("failed to push p3 HTTP response to table")
            .map_err(wasmtime::Error::from_anyhow)
            .map_err(HttpError::trap)
    })
}

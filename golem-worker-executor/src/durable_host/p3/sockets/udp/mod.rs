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

use crate::durable_host::DurableWorkerCtx;
use crate::durable_host::authorization::targets::udp_target;
use crate::durable_host::concurrent::{
    AccessClaimOptions, CallReplayOutcome, Cancellable, DurableCallSession, NotCancellable,
    ResolvedCall, reserve_invocation_id_before_await,
};
use crate::durable_host::durability::DurabilityHost;
use crate::durable_host::p3::{
    DurableP3, DurableP3View, durable_worker_ctx, observe_function_call, wasi_sockets_view,
};
use crate::durable_host::sockets::{SocketEntry, SocketPhase};
use crate::services::HasWorker;
use crate::services::oplog::CommitLevel;
use crate::workerctx::WorkerCtx;
use futures::FutureExt;
use golem_common::model::oplog::host_functions::{
    P3SocketsTypesUdpSocketBind, P3SocketsTypesUdpSocketConnect, P3SocketsTypesUdpSocketReceive,
    P3SocketsTypesUdpSocketSend,
};
use golem_common::model::oplog::types::{
    SerializableP3IpSocketAddress, SerializableP3SocketErrorCode, SerializableP3UdpDatagram,
    SerializableP3UdpEndpoint,
};
use golem_common::model::oplog::{
    DeniablePair, DurableFunctionType, HostRequestNoInput, HostRequestP3SocketsBind,
    HostRequestP3SocketsConnect, HostRequestP3SocketsUdpSend, HostResponseP3SocketsBind,
    HostResponseP3SocketsUdpConnect, HostResponseP3SocketsUdpReceive, HostResponseP3SocketsUdpSend,
};
use golem_service_base::error::worker_executor::InterruptKind;
use std::future::Future;
use wasmtime::component::{Accessor, Resource};
use wasmtime_wasi::p3::bindings::sockets::types;
use wasmtime_wasi::p3::sockets::{SocketError, SocketResult};
use wasmtime_wasi::sockets::{UdpSocket, WasiSockets, WasiSocketsView};

use super::serialize_socket_error;
use super::tcp::UNSUPPORTED_SOCKET_TARGET;

#[cfg(test)]
mod tests;

async fn interruptible_receive<T>(
    native: impl Future<Output = SocketResult<T>>,
    interrupt: impl Future<Output = InterruptKind>,
) -> wasmtime::Result<SocketResult<T>> {
    tokio::select! {
        result = native => Ok(result),
        kind = interrupt => Err(wasmtime::Error::from_anyhow(kind.into())),
    }
}

pub(super) fn socket_target(
    address: &types::IpSocketAddress,
) -> Option<golem_common::model::card::PermissionTarget> {
    match address {
        types::IpSocketAddress::Ipv4(address) => {
            let (a, b, c, d) = address.address;
            let host = std::net::Ipv4Addr::new(a, b, c, d).to_string();
            udp_target(&host, address.port).ok()
        }
        types::IpSocketAddress::Ipv6(_) => None,
    }
}

fn udp_entry<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    rep: u32,
) -> wasmtime::Result<&mut SocketEntry> {
    ctx.state
        .socket_entries
        .get_mut(&rep)
        .ok_or_else(|| wasmtime::Error::msg("UDP socket has no socket entry"))
}

/// Applies the endpoint change a UDP call recorded, live and on replay.
fn apply_endpoint(entry: &mut SocketEntry, endpoint: &Option<SerializableP3UdpEndpoint>) {
    match endpoint {
        Some(SerializableP3UdpEndpoint::Bound(local_address)) => {
            entry.local_address = Some(local_address.clone());
            if entry.phase == SocketPhase::Created {
                entry.phase = SocketPhase::Bound;
            }
        }
        Some(SerializableP3UdpEndpoint::Rebound(local_address)) => {
            entry.bind_request = Some(SerializableP3IpSocketAddress {
                port: local_address.port,
                ..entry
                    .bind_request
                    .clone()
                    .unwrap_or_else(|| unspecified_address(entry.family))
            });
            entry.local_address = Some(local_address.clone());
            if entry.phase == SocketPhase::Created {
                entry.phase = SocketPhase::Bound;
            }
        }
        Some(SerializableP3UdpEndpoint::Lost) => entry.phase = SocketPhase::Lost,
        None => {}
    }
}

/// Polls a native bind or connect once. They wait only for the socket address check, which
/// Golem answers at once, and they must run inside one synchronous store access so that no other
/// operation on the socket sees a half-rebuilt endpoint.
fn run_at_once(future: impl Future<Output = SocketResult<()>>) -> SocketResult<()> {
    future.now_or_never().unwrap_or_else(|| {
        Err(SocketError::trap(wasmtime::Error::msg(
            "the socket address check of a UDP endpoint rebuild did not complete at once",
        )))
    })
}

/// Prepares the native socket for a live operation. Replay rebuilds a socket without its
/// endpoint, so at its first live operation the native socket is bound on the recorded local
/// address, and connected again when it was connected; neither sends a packet. Returns `false`
/// when the socket is `Lost`: the recorded address is not free, or it was not free before.
fn ensure_native_endpoint<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    rep: u32,
) -> wasmtime::Result<bool> {
    let entry = udp_entry(ctx, rep)?;
    if entry.phase == SocketPhase::Lost {
        return Ok(false);
    }
    if entry.native_bound {
        return Ok(true);
    }
    // The native socket is bound as the guest bound it: on the address of its bind request, or
    // the unspecified address after an implicit bind, with the recorded port. A connect then
    // selects the same interface address again.
    let unspecified = unspecified_address(entry.family);
    let bind_address =
        entry
            .local_address
            .clone()
            .map(|local_address| SerializableP3IpSocketAddress {
                port: local_address.port,
                ..entry.bind_request.clone().unwrap_or(unspecified.clone())
            });
    let mut rebound = false;
    let restored = match bind_address {
        // No endpoint, or one that a disconnect released: the next send binds implicitly.
        None => Ok(()),
        Some(bind_address) if bind_address == unspecified => Ok(()),
        Some(bind_address) => {
            // A bind on a fixed port keeps that port on a disconnect, unlike the implicit bind or
            // the bind on port 0 that selected it, so the next endpoint change records it.
            rebound = bind_address.port != 0;
            let remote_address = entry.remote_address.clone();
            let mut wasi = ctx.as_wasi_view();
            let mut view = wasi.sockets();
            run_at_once(types::HostUdpSocket::bind(
                &mut view,
                Resource::new_borrow(rep),
                bind_address.into(),
            ))
            .and_then(|()| match remote_address {
                Some(remote_address) => run_at_once(types::HostUdpSocket::connect(
                    &mut view,
                    Resource::new_borrow(rep),
                    remote_address.into(),
                )),
                None => Ok(()),
            })
        }
    };
    let entry = udp_entry(ctx, rep)?;
    match restored {
        Ok(()) => {
            entry.native_bound = true;
            entry.native_rebound = rebound;
            Ok(true)
        }
        Err(error) => {
            // Traps propagate; any socket error means the endpoint is gone.
            serialize_socket_error(error)?;
            entry.phase = SocketPhase::Lost;
            Ok(false)
        }
    }
}

/// The new local address of the native socket after a live connect, send or receive, when it
/// differs from the entry: an implicit bind, the interface address that a connect selects for a
/// wildcard bind, or the port that a rebuild on port 0 selected. After a rebuild on a fixed port
/// it is always `Rebound`.
fn endpoint_change<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    rep: u32,
) -> wasmtime::Result<Option<SerializableP3UdpEndpoint>> {
    let native = {
        let mut wasi = ctx.as_wasi_view();
        let mut view = wasi.sockets();
        types::HostUdpSocket::get_local_address(&mut view, Resource::new_borrow(rep))
            .ok()
            .map(SerializableP3IpSocketAddress::from)
    };
    let entry = udp_entry(ctx, rep)?;
    let rebound = std::mem::take(&mut entry.native_rebound);
    Ok(match native {
        Some(address) if rebound => Some(SerializableP3UdpEndpoint::Rebound(address)),
        Some(address) if entry.local_address.as_ref() != Some(&address) => {
            Some(SerializableP3UdpEndpoint::Bound(address))
        }
        _ => None,
    })
}

fn unspecified_address(family: types::IpAddressFamily) -> SerializableP3IpSocketAddress {
    match family {
        types::IpAddressFamily::Ipv4 => types::IpSocketAddress::Ipv4(types::Ipv4SocketAddress {
            address: (0, 0, 0, 0),
            port: 0,
        }),
        types::IpAddressFamily::Ipv6 => types::IpSocketAddress::Ipv6(types::Ipv6SocketAddress {
            address: (0, 0, 0, 0, 0, 0, 0, 0),
            port: 0,
            flow_info: 0,
            scope_id: 0,
        }),
    }
    .into()
}

/// Updates the entry from a connect result, live and on replay.
fn apply_connect<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    rep: u32,
    remote_address: types::IpSocketAddress,
    response: HostResponseP3SocketsUdpConnect,
) -> SocketResult<()> {
    let entry = udp_entry(ctx, rep).map_err(SocketError::trap)?;
    apply_endpoint(entry, &response.endpoint);
    if response.disconnected && entry.phase == SocketPhase::Connected {
        entry.remote_address = None;
        entry.phase = SocketPhase::Bound;
    }
    match response.result {
        Ok(()) => {
            entry.remote_address = Some(remote_address.into());
            entry.phase = SocketPhase::Connected;
            Ok(())
        }
        Err(error) => Err(types::ErrorCode::from(error).into()),
    }
}

fn apply_recorded_endpoint<Ctx: WorkerCtx, U: Send + 'static>(
    store: &Accessor<U, DurableP3<Ctx>>,
    rep: u32,
    endpoint: &Option<SerializableP3UdpEndpoint>,
) -> SocketResult<()> {
    store
        .with(|mut access| -> wasmtime::Result<()> {
            let entry = udp_entry(durable_worker_ctx::<Ctx, U>(access.data_mut()), rep)?;
            apply_endpoint(entry, endpoint);
            Ok(())
        })
        .map_err(SocketError::trap)
}

fn datagram_result(
    result: Result<SerializableP3UdpDatagram, SerializableP3SocketErrorCode>,
) -> SocketResult<(Vec<u8>, types::IpSocketAddress)> {
    match result {
        Ok(SerializableP3UdpDatagram {
            data,
            remote_address,
        }) => Ok((data, remote_address.into())),
        Err(error) => Err(types::ErrorCode::from(error).into()),
    }
}

fn lost_response() -> (
    Result<(), SerializableP3SocketErrorCode>,
    Option<SerializableP3UdpEndpoint>,
) {
    (
        Err(SerializableP3SocketErrorCode::ConnectionBroken),
        Some(SerializableP3UdpEndpoint::Lost),
    )
}

/// Sends a datagram live, after the socket got its endpoint back. Returns the result and the
/// endpoint change to record.
async fn send_live<Ctx: WorkerCtx, U: Send + 'static>(
    store: &Accessor<U, DurableP3<Ctx>>,
    socket: Resource<UdpSocket>,
    data: Vec<u8>,
    remote_address: Option<types::IpSocketAddress>,
) -> SocketResult<(
    Result<(), SerializableP3SocketErrorCode>,
    Option<SerializableP3UdpEndpoint>,
)> {
    let rep = socket.rep();
    let ready = store
        .with(|mut access| {
            ensure_native_endpoint(durable_worker_ctx::<Ctx, U>(access.data_mut()), rep)
        })
        .map_err(SocketError::trap)?;
    if !ready {
        return Ok(lost_response());
    }
    let sockets = store.with_getter::<WasiSockets>(wasi_sockets_view::<Ctx, U>);
    let result = <WasiSockets as types::HostUdpSocketWithStore<U>>::send(
        &sockets,
        socket,
        data,
        remote_address,
    )
    .await;
    let result = match result {
        Ok(()) => Ok(()),
        Err(error) => Err(serialize_socket_error(error).map_err(SocketError::trap)?),
    };
    let endpoint = store
        .with(|mut access| endpoint_change(durable_worker_ctx::<Ctx, U>(access.data_mut()), rep))
        .map_err(SocketError::trap)?;
    Ok((result, endpoint))
}

impl<Ctx: WorkerCtx> types::HostUdpSocket for DurableP3View<'_, Ctx> {
    async fn bind(
        &mut self,
        socket: Resource<UdpSocket>,
        local_address: types::IpSocketAddress,
    ) -> SocketResult<()> {
        observe_function_call(&*self.0, "sockets::types::udp-socket", "bind");
        let rep = socket.rep();
        let response = DurableCallSession::<P3SocketsTypesUdpSocketBind, NotCancellable>::invoke(
            self.0.durable_ctx_mut(),
            HostRequestP3SocketsBind {
                local_address: local_address.into(),
            },
            DurableFunctionType::WriteLocal,
            async |ctx| -> wasmtime::Result<_> {
                let result = if udp_entry(ctx, rep)?.local_address.is_some()
                    || !ensure_native_endpoint(ctx, rep)?
                {
                    Err(types::ErrorCode::InvalidState.into())
                } else {
                    let mut wasi = ctx.as_wasi_view();
                    let mut view = wasi.sockets();
                    match types::HostUdpSocket::bind(&mut view, socket, local_address).await {
                        Ok(()) => types::HostUdpSocket::get_local_address(
                            &mut view,
                            Resource::new_borrow(rep),
                        ),
                        Err(error) => Err(error),
                    }
                };
                Ok(HostResponseP3SocketsBind {
                    result: match result {
                        Ok(address) => Ok(address.into()),
                        Err(error) => Err(serialize_socket_error(error)?),
                    },
                })
            },
        )
        .await
        .map_err(SocketError::trap)?;
        match response.result {
            Ok(address) => {
                let entry = udp_entry(self.0.durable_ctx_mut(), rep).map_err(SocketError::trap)?;
                entry.local_address = Some(address);
                entry.bind_request = Some(local_address.into());
                entry.phase = SocketPhase::Bound;
                Ok(())
            }
            Err(error) => Err(types::ErrorCode::from(error).into()),
        }
    }

    async fn connect(
        &mut self,
        socket: Resource<UdpSocket>,
        remote_address: types::IpSocketAddress,
    ) -> SocketResult<()> {
        observe_function_call(&*self.0, "sockets::types::udp-socket", "connect");
        let rep = socket.rep();
        let begun = DurableCallSession::<P3SocketsTypesUdpSocketConnect, NotCancellable>::begin(
            self.0.durable_ctx_mut(),
            DurableFunctionType::WriteRemote,
        )
        .await
        .map_err(SocketError::trap)?;
        let request = HostRequestP3SocketsConnect {
            remote_address: remote_address.into(),
        };
        let handle = match begun
            .resolve(self.0.durable_ctx_mut())
            .await
            .map_err(SocketError::trap)?
        {
            ResolvedCall::Live(begun) => {
                let denial = match socket_target(&remote_address) {
                    Some(target) => match self
                        .0
                        .durable_ctx_mut()
                        .authorize_live_permission(&target)
                        .await
                    {
                        Ok(Ok(_)) => None,
                        Ok(Err(error)) => Some(error.to_string()),
                        Err(error) => Some(error.to_string()),
                    },
                    None => Some(UNSUPPORTED_SOCKET_TARGET.to_string()),
                };
                match denial {
                    Some(denial) => {
                        let response = begun
                            .start_live_denied(self.0.durable_ctx_mut(), request, denial)
                            .await
                            .map_err(|error| {
                                SocketError::trap(wasmtime::Error::from_anyhow(error))
                            })?;
                        return apply_connect(
                            self.0.durable_ctx_mut(),
                            rep,
                            remote_address,
                            response,
                        );
                    }
                    None => begun
                        .start_live(self.0.durable_ctx_mut(), request)
                        .await
                        .map_err(SocketError::trap)?,
                }
            }
            ResolvedCall::Replay(mut handle) => {
                match handle
                    .replay(self.0.durable_ctx_mut())
                    .await
                    .map_err(SocketError::trap)?
                {
                    CallReplayOutcome::Replayed(response) => {
                        return apply_connect(
                            self.0.durable_ctx_mut(),
                            rep,
                            remote_address,
                            response,
                        );
                    }
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
                handle
            }
        };
        let (result, endpoint, disconnected) =
            if ensure_native_endpoint(self.0.durable_ctx_mut(), rep).map_err(SocketError::trap)? {
                let mut view = WasiSocketsView::sockets(self.0);
                let result = match types::HostUdpSocket::connect(
                    &mut view,
                    Resource::new_borrow(rep),
                    remote_address,
                )
                .await
                {
                    Ok(()) => Ok(()),
                    Err(error) => Err(serialize_socket_error(error).map_err(SocketError::trap)?),
                };
                // On Linux a connect that fails with `EINVAL` dissolves the old association before
                // it tries again, so a failed connect can leave the socket disconnected.
                let disconnected = result.is_err()
                    && types::HostUdpSocket::get_remote_address(&mut view, socket).is_err();
                (
                    result,
                    endpoint_change(self.0.durable_ctx_mut(), rep).map_err(SocketError::trap)?,
                    disconnected,
                )
            } else {
                let (result, endpoint) = lost_response();
                (result, endpoint, false)
            };
        let response = handle
            .complete(
                self.0.durable_ctx_mut(),
                HostResponseP3SocketsUdpConnect {
                    result,
                    endpoint,
                    disconnected,
                },
            )
            .await
            .map_err(SocketError::trap)?;
        apply_connect(self.0.durable_ctx_mut(), rep, remote_address, response)
    }

    async fn create(
        &mut self,
        address_family: types::IpAddressFamily,
    ) -> SocketResult<Resource<UdpSocket>> {
        observe_function_call(&*self.0, "sockets::types::udp-socket", "create");
        let socket =
            types::HostUdpSocket::create(&mut WasiSocketsView::sockets(self.0), address_family)
                .await?;
        self.0
            .durable_ctx_mut()
            .state
            .socket_entries
            .insert(socket.rep(), SocketEntry::new(address_family));
        Ok(socket)
    }

    fn disconnect(&mut self, socket: Resource<UdpSocket>) -> SocketResult<()> {
        observe_function_call(&*self.0, "sockets::types::udp-socket", "disconnect");
        // Not recorded: the entry decides the result, live and on replay. The native socket is
        // disconnected only when it holds the entry's endpoint; otherwise the endpoint rebuild
        // binds it without connecting. Linux keeps only the address and port that a native bind
        // named, so the local address goes back to the bind request (with the port of a recorded
        // rebuild), or to the unspecified address and port 0 after an implicit bind. The next send
        // records its new endpoint.
        let rep = socket.rep();
        let entry = udp_entry(self.0.durable_ctx_mut(), rep).map_err(SocketError::trap)?;
        if entry.phase == SocketPhase::Lost {
            return Err(types::ErrorCode::ConnectionBroken.into());
        }
        if entry.remote_address.is_none() {
            return Err(types::ErrorCode::InvalidState.into());
        }
        if entry.native_bound {
            types::HostUdpSocket::disconnect(&mut WasiSocketsView::sockets(self.0), socket)?;
        }
        let entry = udp_entry(self.0.durable_ctx_mut(), rep).map_err(SocketError::trap)?;
        entry.remote_address = None;
        entry.local_address = Some(
            entry
                .bind_request
                .clone()
                .unwrap_or_else(|| unspecified_address(entry.family)),
        );
        entry.phase = SocketPhase::Bound;
        Ok(())
    }

    fn get_local_address(
        &mut self,
        socket: Resource<UdpSocket>,
    ) -> SocketResult<types::IpSocketAddress> {
        observe_function_call(&*self.0, "sockets::types::udp-socket", "get-local-address");
        udp_entry(self.0.durable_ctx_mut(), socket.rep())
            .map_err(SocketError::trap)?
            .local_address
            .clone()
            .map(Into::into)
            .ok_or_else(|| types::ErrorCode::InvalidState.into())
    }

    fn get_remote_address(
        &mut self,
        socket: Resource<UdpSocket>,
    ) -> SocketResult<types::IpSocketAddress> {
        observe_function_call(&*self.0, "sockets::types::udp-socket", "get-remote-address");
        udp_entry(self.0.durable_ctx_mut(), socket.rep())
            .map_err(SocketError::trap)?
            .remote_address
            .clone()
            .map(Into::into)
            .ok_or_else(|| types::ErrorCode::InvalidState.into())
    }

    fn get_address_family(
        &mut self,
        socket: Resource<UdpSocket>,
    ) -> wasmtime::Result<types::IpAddressFamily> {
        observe_function_call(&*self.0, "sockets::types::udp-socket", "get-address-family");
        Ok(udp_entry(self.0.durable_ctx_mut(), socket.rep())?.family)
    }

    async fn get_unicast_hop_limit(&mut self, socket: Resource<UdpSocket>) -> SocketResult<u8> {
        observe_function_call(
            &*self.0,
            "sockets::types::udp-socket",
            "get-unicast-hop-limit",
        );
        types::HostUdpSocket::get_unicast_hop_limit(&mut WasiSocketsView::sockets(self.0), socket)
            .await
    }

    fn set_unicast_hop_limit(
        &mut self,
        socket: Resource<UdpSocket>,
        value: u8,
    ) -> SocketResult<()> {
        observe_function_call(
            &*self.0,
            "sockets::types::udp-socket",
            "set-unicast-hop-limit",
        );
        types::HostUdpSocket::set_unicast_hop_limit(
            &mut WasiSocketsView::sockets(self.0),
            socket,
            value,
        )
    }

    async fn get_receive_buffer_size(&mut self, socket: Resource<UdpSocket>) -> SocketResult<u64> {
        observe_function_call(
            &*self.0,
            "sockets::types::udp-socket",
            "get-receive-buffer-size",
        );
        types::HostUdpSocket::get_receive_buffer_size(&mut WasiSocketsView::sockets(self.0), socket)
            .await
    }

    fn set_receive_buffer_size(
        &mut self,
        socket: Resource<UdpSocket>,
        value: u64,
    ) -> SocketResult<()> {
        observe_function_call(
            &*self.0,
            "sockets::types::udp-socket",
            "set-receive-buffer-size",
        );
        types::HostUdpSocket::set_receive_buffer_size(
            &mut WasiSocketsView::sockets(self.0),
            socket,
            value,
        )
    }

    async fn get_send_buffer_size(&mut self, socket: Resource<UdpSocket>) -> SocketResult<u64> {
        observe_function_call(
            &*self.0,
            "sockets::types::udp-socket",
            "get-send-buffer-size",
        );
        types::HostUdpSocket::get_send_buffer_size(&mut WasiSocketsView::sockets(self.0), socket)
            .await
    }

    fn set_send_buffer_size(
        &mut self,
        socket: Resource<UdpSocket>,
        value: u64,
    ) -> SocketResult<()> {
        observe_function_call(
            &*self.0,
            "sockets::types::udp-socket",
            "set-send-buffer-size",
        );
        types::HostUdpSocket::set_send_buffer_size(
            &mut WasiSocketsView::sockets(self.0),
            socket,
            value,
        )
    }

    fn drop(&mut self, sock: Resource<UdpSocket>) -> wasmtime::Result<()> {
        observe_function_call(&*self.0, "sockets::types::udp-socket", "drop");
        self.0
            .durable_ctx_mut()
            .state
            .socket_entries
            .remove(&sock.rep());
        types::HostUdpSocket::drop(&mut WasiSocketsView::sockets(self.0), sock)
    }
}

impl<U: Send + 'static, Ctx: WorkerCtx> types::HostUdpSocketWithStore<U> for DurableP3<Ctx> {
    async fn send(
        store: &Accessor<U, Self>,
        socket: Resource<UdpSocket>,
        data: Vec<u8>,
        remote_address: Option<types::IpSocketAddress>,
    ) -> SocketResult<()> {
        reserve_invocation_id_before_await(store, durable_worker_ctx::<Ctx, U>)
            .map_err(|error| SocketError::trap(wasmtime::Error::from(error)))?;
        let rep = socket.rep();
        let targets = match remote_address.as_ref() {
            Some(remote_address) => socket_target(remote_address)
                .map(|target| vec![target])
                .ok_or_else(|| UNSUPPORTED_SOCKET_TARGET.to_string()),
            None => Ok(Vec::new()),
        };
        // The datagram is never stored: a completed or ambiguous send is never repeated.
        let request = HostRequestP3SocketsUdpSend {
            remote_address: remote_address.map(Into::into),
        };
        let (mut call, decision) =
            DurableCallSession::<P3SocketsTypesUdpSocketSend, Cancellable>::start_authorized_access(
                store,
                durable_worker_ctx::<Ctx, U>,
                DurableFunctionType::WriteRemote,
                AccessClaimOptions {
                    incomplete_repair_authorized_by_scope: true,
                    ..Default::default()
                },
                request.clone(),
                targets,
            )
            .await
            .map_err(SocketError::trap)?;
        let denied = decision
            .as_ref()
            .and_then(|decision| decision.as_ref().err())
            .map(|denial| P3SocketsTypesUdpSocketSend::denied(&request, denial));
        let live = call.is_live();
        if !live {
            match call
                .replay_access(store, durable_worker_ctx::<Ctx, U>)
                .await
                .map_err(SocketError::trap)?
            {
                CallReplayOutcome::Replayed(response) => {
                    apply_recorded_endpoint(store, rep, &response.endpoint)?;
                    return response
                        .result
                        .map_err(|error| types::ErrorCode::from(error).into());
                }
                CallReplayOutcome::Incomplete(incomplete) => call = incomplete,
            }
        }
        let response = {
            {
                // An incomplete send completes with `connection-broken` and is never sent again.
                let (result, endpoint) = if !live {
                    (Err(SerializableP3SocketErrorCode::ConnectionBroken), None)
                } else if let Some(denied) = denied {
                    (denied.result, denied.endpoint)
                } else {
                    if call.is_persisted() {
                        // Fence the original send Start before the datagram can leave the worker.
                        let public_state = store.with(|mut access| {
                            durable_worker_ctx::<Ctx, U>(access.data_mut())
                                .public_state
                                .clone()
                        });
                        public_state
                            .worker()
                            .commit_oplog_and_update_state(CommitLevel::Always)
                            .await
                            .map_err(SocketError::trap)?;
                    }
                    send_live(store, socket, data, remote_address).await?
                };
                call.complete_access(
                    store,
                    durable_worker_ctx::<Ctx, U>,
                    HostResponseP3SocketsUdpSend { result, endpoint },
                )
                .await
                .map_err(SocketError::trap)?
            }
        };
        apply_recorded_endpoint(store, rep, &response.endpoint)?;
        response
            .result
            .map_err(|error| types::ErrorCode::from(error).into())
    }

    async fn receive(
        store: &Accessor<U, Self>,
        socket: Resource<UdpSocket>,
    ) -> SocketResult<(Vec<u8>, types::IpSocketAddress)> {
        let rep = socket.rep();
        let mut call = DurableCallSession::<P3SocketsTypesUdpSocketReceive, Cancellable>::start_access_with_options(
            store,
            durable_worker_ctx::<Ctx, U>,
            DurableFunctionType::ReadRemote,
            AccessClaimOptions::default(),
            HostRequestNoInput {},
        )
        .await
        .map_err(SocketError::trap)?;
        if !call.is_live() {
            match call
                .replay_access(store, durable_worker_ctx::<Ctx, U>)
                .await
                .map_err(SocketError::trap)?
            {
                CallReplayOutcome::Replayed(response) => {
                    apply_recorded_endpoint(store, rep, &response.endpoint)?;
                    return datagram_result(response.result);
                }
                CallReplayOutcome::Incomplete(incomplete) => call = incomplete,
            }
        }
        let response = call
            .run_live_action_access(store, durable_worker_ctx::<Ctx, U>, || async {
                let ready = store.with(|mut access| {
                    ensure_native_endpoint(durable_worker_ctx::<Ctx, U>(access.data_mut()), rep)
                })?;
                if !ready {
                    return Ok(HostResponseP3SocketsUdpReceive {
                        result: Err(SerializableP3SocketErrorCode::ConnectionBroken),
                        endpoint: Some(SerializableP3UdpEndpoint::Lost),
                    });
                }
                let sockets = store.with_getter::<WasiSockets>(wasi_sockets_view::<Ctx, U>);
                let native =
                    <WasiSockets as types::HostUdpSocketWithStore<U>>::receive(&sockets, socket);
                let interrupt = store.with(|mut access| {
                    durable_worker_ctx::<Ctx, U>(access.data_mut()).create_interrupt_signal()
                });
                // Only native receive is interruptible. Replay, terminal persistence and
                // completion delivery remain owned by the durable call outside this select.
                let result = interruptible_receive(native, interrupt).await?;
                let endpoint = store.with(|mut access| {
                    endpoint_change(durable_worker_ctx::<Ctx, U>(access.data_mut()), rep)
                })?;

                Ok::<_, wasmtime::Error>(HostResponseP3SocketsUdpReceive {
                    result: match result {
                        Ok((data, remote_address)) => Ok(SerializableP3UdpDatagram {
                            data,
                            remote_address: remote_address.into(),
                        }),
                        Err(error) => Err(serialize_socket_error(error)?),
                    },
                    endpoint,
                })
            })
            .await
            .map_err(SocketError::trap)?;
        apply_recorded_endpoint(store, rep, &response.endpoint)?;
        datagram_result(response.result)
    }
}

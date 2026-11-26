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

//! P2 UDP keeps its endpoint in the shared [`SocketEntries`](super::SocketEntries) table.
//!
//! `start-bind`, `stream`, `receive`, `check-send` and `send` are recorded. Replay updates the
//! entry only and does not bind, stream or send natively; the address getters answer from the
//! entry. The guest's datagram streams are [`DurableIncomingDatagramStream`] and
//! [`DurableOutgoingDatagramStream`]: a replayed `stream` returns them without native streams.
//! At the first live operation the host binds the native socket on the recorded local address and
//! streams it again to the recorded remote address. When the address is not free, the socket
//! becomes `Lost`, and every later operation fails with `connection-reset`.

use wasmtime::component::Resource;

use crate::durable_host::concurrent::{DurableCallSession, NotCancellable, ResolvedCall};
use crate::durable_host::sockets::{SocketEntry, SocketPhase};
use crate::durable_host::{DurabilityHost, DurableWorkerCtx};
use crate::services::HasWorker;
use crate::services::oplog::CommitLevel;
use crate::workerctx::WorkerCtx;
use golem_common::SafeDisplay;
use golem_common::model::oplog::host_functions::{
    SocketsUdpCheckSend, SocketsUdpPermissionCheck, SocketsUdpReceive, SocketsUdpSend,
    SocketsUdpStartBind, SocketsUdpStream,
};
use golem_common::model::oplog::types::{
    SerializableP3IpSocketAddress, SerializableP3UdpDatagram, SerializableSocketError,
};
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestNoInput, HostRequestSocketsUdpBind,
    HostRequestSocketsUdpPermissionCheck, HostRequestSocketsUdpReceive, HostRequestSocketsUdpSend,
    HostRequestSocketsUdpStream, HostResponsePermissionCheck, HostResponseSocketsUdpBind,
    HostResponseSocketsUdpCheckSend, HostResponseSocketsUdpReceive, HostResponseSocketsUdpSend,
    HostResponseSocketsUdpStream,
};
use wasmtime_wasi::p2::SocketError;
use wasmtime_wasi::p2::bindings::sockets::instance_network::Host as _;
use wasmtime_wasi::p2::bindings::sockets::network::ErrorCode;
use wasmtime_wasi::p2::bindings::sockets::udp::{
    Host, HostIncomingDatagramStream, HostOutgoingDatagramStream, HostUdpSocket, IncomingDatagram,
    IncomingDatagramStream, IpAddressFamily, IpSocketAddress, Network, OutgoingDatagram,
    OutgoingDatagramStream, Pollable, UdpSocket,
};
use wasmtime_wasi::p3::bindings::sockets::types::IpAddressFamily as P3IpAddressFamily;
use wasmtime_wasi::sockets::WasiSocketsView as _;

/// The error a P2 socket reports when its endpoint was lost. P2 has no `connection-broken`
/// code; `connection-reset` is what P2 TCP reports for a lost connection too.
const P2_CONNECTION_LOST: ErrorCode = ErrorCode::ConnectionReset;

/// The guest's incoming datagram stream. It has a native stream once the native socket holds the
/// endpoint of the entry; a replayed `stream` returns it without one.
pub(crate) struct DurableIncomingDatagramStream {
    socket: u32,
    native: Option<IncomingDatagramStream>,
}

/// The guest's outgoing datagram stream; see [`DurableIncomingDatagramStream`].
pub(crate) struct DurableOutgoingDatagramStream {
    socket: u32,
    native: Option<OutgoingDatagramStream>,
    /// The number of datagrams that the last `check-send`, live or replayed, permitted.
    permit: u64,
}

/// Without a native stream the stream is ready at once: its next operation rebuilds the endpoint
/// or reports the loss.
#[async_trait::async_trait]
#[golem_async_trait::region_simple]
impl wasmtime_wasi::p2::Pollable for DurableIncomingDatagramStream {
    async fn ready(&mut self) {
        if let Some(native) = &mut self.native {
            native.ready().await
        }
    }
}

#[async_trait::async_trait]
#[golem_async_trait::region_simple]
impl wasmtime_wasi::p2::Pollable for DurableOutgoingDatagramStream {
    async fn ready(&mut self) {
        if let Some(native) = &mut self.native {
            native.ready().await
        }
    }
}

pub(crate) fn p3_family(family: IpAddressFamily) -> P3IpAddressFamily {
    match family {
        IpAddressFamily::Ipv4 => P3IpAddressFamily::Ipv4,
        IpAddressFamily::Ipv6 => P3IpAddressFamily::Ipv6,
    }
}

fn p2_family(family: P3IpAddressFamily) -> IpAddressFamily {
    match family {
        P3IpAddressFamily::Ipv4 => IpAddressFamily::Ipv4,
        P3IpAddressFamily::Ipv6 => IpAddressFamily::Ipv6,
    }
}

/// Splits a native socket error into its recorded error code; anything else traps.
fn recorded_error(error: SocketError) -> wasmtime::Result<SerializableSocketError> {
    Ok(SerializableSocketError::from_result(Ok(error.downcast()?)))
}

fn lost_error() -> SerializableSocketError {
    SerializableSocketError::from_result(Ok(P2_CONNECTION_LOST))
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

fn apply_lost<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    socket: u32,
    lost: bool,
) -> Result<(), SocketError> {
    if lost {
        udp_entry(ctx, socket).map_err(SocketError::trap)?.phase = SocketPhase::Lost;
    }
    Ok(())
}

fn table_entry<T: 'static, Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    rep: u32,
) -> wasmtime::Result<&mut T> {
    ctx.table()
        .get_any_mut(rep)
        .map_err(wasmtime::Error::from)?
        .downcast_mut::<T>()
        .ok_or_else(|| wasmtime::Error::msg("the resource is not a UDP datagram stream"))
}

/// Pushes the two guest streams of a `stream` call as children of the socket, in the order of the
/// native call, so they get the reps that the native streams had.
fn push_streams<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    socket: u32,
    natives: Option<(IncomingDatagramStream, OutgoingDatagramStream)>,
) -> wasmtime::Result<(
    Resource<IncomingDatagramStream>,
    Resource<OutgoingDatagramStream>,
)> {
    let (incoming, outgoing) = match natives {
        Some((incoming, outgoing)) => (Some(incoming), Some(outgoing)),
        None => (None, None),
    };
    let parent = Resource::<UdpSocket>::new_borrow(socket);
    let incoming = ctx.table().push_child(
        DurableIncomingDatagramStream {
            socket,
            native: incoming,
        },
        &parent,
    )?;
    let outgoing = ctx.table().push_child(
        DurableOutgoingDatagramStream {
            socket,
            native: outgoing,
            permit: 0,
        },
        &parent,
    )?;
    Ok((
        Resource::new_own(incoming.rep()),
        Resource::new_own(outgoing.rep()),
    ))
}

/// Runs native `stream` and takes its two native streams out of the resource table. They are
/// deleted in the reverse order of their push, which gives the table's free list back its order.
async fn native_stream<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    socket: u32,
    remote_address: Option<IpSocketAddress>,
) -> wasmtime::Result<Result<(IncomingDatagramStream, OutgoingDatagramStream), SocketError>> {
    let streams = {
        let mut wasi = ctx.as_wasi_view();
        HostUdpSocket::stream(
            &mut wasi.sockets(),
            Resource::new_borrow(socket),
            remote_address,
        )
        .await
    };
    match streams {
        Ok((incoming, outgoing)) => {
            let outgoing = ctx.table().delete(outgoing)?;
            let incoming = ctx.table().delete(incoming)?;
            Ok(Ok((incoming, outgoing)))
        }
        Err(error) => Ok(Err(error)),
    }
}

/// Binds the native socket on `local_address` with a network resource that exists only for the
/// call, as the guest's network may be gone.
async fn native_bind<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    socket: u32,
    local_address: IpSocketAddress,
    finish: bool,
) -> wasmtime::Result<Result<(), SocketError>> {
    let network = ctx.as_wasi_view().sockets().instance_network()?;
    let bound = {
        let mut wasi = ctx.as_wasi_view();
        let mut view = wasi.sockets();
        match HostUdpSocket::start_bind(
            &mut view,
            Resource::new_borrow(socket),
            Resource::new_borrow(network.rep()),
            local_address,
        )
        .await
        {
            Ok(()) if finish => {
                HostUdpSocket::finish_bind(&mut view, Resource::new_borrow(socket)).await
            }
            other => other,
        }
    };
    ctx.table().delete::<Network>(network)?;
    Ok(bound)
}

/// Prepares the native socket for a live operation. Replay rebuilds a socket without its
/// endpoint, so at its first live operation the native socket is bound as P3 UDP binds it, on
/// the bind request's address with the recorded port, and, when the guest holds streams of it, streamed again to the recorded remote
/// address; neither sends a packet. Returns `false` when the socket is `Lost`: the recorded
/// address is not free, or it was not free before.
async fn ensure_native_endpoint<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    socket: u32,
) -> wasmtime::Result<bool> {
    let entry = udp_entry(ctx, socket)?;
    if entry.phase == SocketPhase::Lost {
        return Ok(false);
    }
    if entry.native_bound {
        return Ok(true);
    }
    let Some(local_address) = entry.udp_rebuild_bind_address() else {
        return Ok(true);
    };
    let remote_address = entry.remote_address.clone().map(Into::into);
    let finish = !entry.bind_pending;
    let has_streams = has_streams(ctx, socket)?;

    let mut restored = native_bind(ctx, socket, local_address.into(), finish).await?;
    if restored.is_ok() && has_streams {
        match native_stream(ctx, socket, remote_address).await? {
            Ok((incoming, outgoing)) => {
                let mut incoming = Some(incoming);
                let mut outgoing = Some(outgoing);
                for (_, resource) in ctx.table().iter_mut() {
                    if let Some(stream) = resource.downcast_mut::<DurableIncomingDatagramStream>()
                        && stream.socket == socket
                    {
                        stream.native = incoming.take();
                    } else if let Some(stream) =
                        resource.downcast_mut::<DurableOutgoingDatagramStream>()
                        && stream.socket == socket
                    {
                        stream.native = outgoing.take();
                    }
                }
            }
            Err(error) => restored = Err(error),
        }
    }
    let entry = udp_entry(ctx, socket)?;
    match restored {
        Ok(()) => {
            entry.native_bound = true;
            Ok(true)
        }
        Err(error) => {
            // Traps propagate; any socket error means the endpoint is gone.
            recorded_error(error)?;
            entry.phase = SocketPhase::Lost;
            Ok(false)
        }
    }
}

/// The native local and remote addresses of the socket. Native connect and disconnect can change
/// both: a connect fixes a wildcard local address, a disconnect can release an ephemeral port, and
/// a failed connect can leave the socket disconnected.
fn native_addresses<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    socket: u32,
) -> (
    Option<SerializableP3IpSocketAddress>,
    Option<SerializableP3IpSocketAddress>,
) {
    let mut wasi = ctx.as_wasi_view();
    let mut view = wasi.sockets();
    let local = HostUdpSocket::local_address(&mut view, Resource::new_borrow(socket))
        .ok()
        .map(Into::into);
    let remote = HostUdpSocket::remote_address(&mut view, Resource::new_borrow(socket))
        .ok()
        .map(Into::into);
    (local, remote)
}

/// The local address of the entry after a `stream` call, before the recorded change: a
/// disconnect resets it as P3 `disconnect` does.
fn stream_local_address(
    entry: &SocketEntry,
    disconnects: bool,
) -> Option<SerializableP3IpSocketAddress> {
    if disconnects {
        Some(entry.udp_local_address_after_disconnect())
    } else {
        entry.local_address.clone()
    }
}

/// The new native local address after a live call, when it differs from the entry: an implicit
/// bind, or the port that an endpoint rebuild on port 0 selected.
fn local_address_change<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    socket: u32,
) -> wasmtime::Result<Option<SerializableP3IpSocketAddress>> {
    let (native, _) = native_addresses(ctx, socket);
    Ok(SocketEntry::udp_local_address_change(
        udp_entry(ctx, socket)?.local_address.as_ref(),
        native,
    ))
}

/// Applies a recorded local address change, live and on replay.
fn apply_local_address<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    socket: u32,
    local_address: Option<SerializableP3IpSocketAddress>,
) -> Result<(), SocketError> {
    if let Some(address) = local_address {
        udp_entry(ctx, socket)
            .map_err(SocketError::trap)?
            .local_address = Some(address);
    }
    Ok(())
}

fn has_streams<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    socket: u32,
) -> wasmtime::Result<bool> {
    Ok(ctx
        .table()
        .iter_children(&Resource::<UdpSocket>::new_borrow(socket))?
        .any(|child| {
            child.is::<DurableIncomingDatagramStream>()
                || child.is::<DurableOutgoingDatagramStream>()
        }))
}

impl<Ctx: WorkerCtx> HostUdpSocket for DurableWorkerCtx<Ctx> {
    async fn start_bind(
        &mut self,
        self_: Resource<UdpSocket>,
        network: Resource<Network>,
        local_address: IpSocketAddress,
    ) -> Result<(), SocketError> {
        self.observe_function_call("sockets::udp", "start_bind");
        let rep = self_.rep();
        let mut bound_live = false;
        let request = HostRequestSocketsUdpBind {
            local_address: local_address.into(),
        };
        let bind_request = request.local_address.clone();
        let response = DurableCallSession::<SocketsUdpStartBind, NotCancellable>::invoke(
            self,
            request,
            DurableFunctionType::WriteLocal,
            async |ctx| -> wasmtime::Result<_> {
                let entry = udp_entry(ctx, rep)?;
                // A reconstruction rebuilds the entry without its native endpoint, and that
                // native socket would accept a second bind.
                let refused = match entry.phase {
                    SocketPhase::Lost => Some(ErrorCode::InvalidState),
                    _ if entry.local_address.is_some() && !entry.native_bound => {
                        Some(if entry.bind_pending {
                            ErrorCode::ConcurrencyConflict
                        } else {
                            ErrorCode::InvalidState
                        })
                    }
                    _ => None,
                };
                if let Some(code) = refused {
                    return Ok(HostResponseSocketsUdpBind {
                        result: Err(SerializableSocketError::from_result(Ok(code))),
                    });
                }
                let mut wasi = ctx.as_wasi_view();
                let mut view = wasi.sockets();
                let result =
                    match HostUdpSocket::start_bind(&mut view, self_, network, local_address).await
                    {
                        Ok(()) => {
                            bound_live = true;
                            match HostUdpSocket::local_address(&mut view, Resource::new_borrow(rep))
                            {
                                Ok(address) => Ok(address.into()),
                                Err(error) => Err(recorded_error(error)?),
                            }
                        }
                        Err(error) => Err(recorded_error(error)?),
                    };
                Ok(HostResponseSocketsUdpBind { result })
            },
        )
        .await
        .map_err(SocketError::trap)?;
        match response.result {
            Ok(address) => {
                let entry = udp_entry(self, rep).map_err(SocketError::trap)?;
                entry.phase = SocketPhase::Bound;
                entry.local_address = Some(address);
                entry.bind_request = Some(bind_request);
                entry.native_bound = bound_live;
                entry.bind_pending = true;
                Ok(())
            }
            Err(error) => Err(error.into()),
        }
    }

    async fn finish_bind(&mut self, self_: Resource<UdpSocket>) -> Result<(), SocketError> {
        self.observe_function_call("sockets::udp", "finish_bind");
        let rep = self_.rep();
        let entry = udp_entry(self, rep).map_err(SocketError::trap)?;
        let (native_bound, bind_pending) = (entry.native_bound, entry.bind_pending);
        let result = if native_bound {
            HostUdpSocket::finish_bind(&mut self.as_wasi_view().sockets(), self_).await
        } else if bind_pending {
            Ok(())
        } else {
            Err(ErrorCode::NotInProgress.into())
        };
        if result.is_ok() {
            udp_entry(self, rep)
                .map_err(SocketError::trap)?
                .bind_pending = false;
        }
        result
    }

    async fn stream(
        &mut self,
        self_: Resource<UdpSocket>,
        remote_address: Option<IpSocketAddress>,
    ) -> Result<
        (
            Resource<IncomingDatagramStream>,
            Resource<OutgoingDatagramStream>,
        ),
        SocketError,
    > {
        if let Some(remote_address) = remote_address.as_ref() {
            authorize_addresses(self, std::slice::from_ref(remote_address)).await?;
        }
        self.observe_function_call("sockets::udp", "stream");
        let rep = self_.rep();
        if has_streams(self, rep).map_err(SocketError::trap)? {
            return Err(SocketError::trap(wasmtime::Error::msg(
                "UDP streams not dropped yet",
            )));
        }
        // `stream(none)` disconnects a connected socket, as P3 `disconnect` does.
        let disconnects = remote_address.is_none()
            && udp_entry(self, rep)
                .map_err(SocketError::trap)?
                .remote_address
                .is_some();
        let mut natives = None;
        let response = DurableCallSession::<SocketsUdpStream, NotCancellable>::invoke(
            self,
            HostRequestSocketsUdpStream {
                remote_address: remote_address.map(Into::into),
            },
            DurableFunctionType::WriteRemote,
            async |ctx| -> wasmtime::Result<_> {
                if !ensure_native_endpoint(ctx, rep).await? {
                    return Ok(HostResponseSocketsUdpStream {
                        result: Err(lost_error()),
                        lost: true,
                        local_address: None,
                        disconnected: false,
                    });
                }
                let result = match native_stream(ctx, rep, remote_address).await? {
                    Ok(streams) => {
                        natives = Some(streams);
                        Ok(())
                    }
                    Err(error) => Err(recorded_error(error)?),
                };
                // Also after a failed call. On Linux a connect that fails with `EINVAL` dissolves
                // the old association before it tries again, so it can leave the socket
                // disconnected, as in P3 UDP connect.
                let (native_local, native_remote) = native_addresses(ctx, rep);
                let disconnected = result.is_err() && native_remote.is_none();
                let expected = stream_local_address(udp_entry(ctx, rep)?, disconnects);
                Ok(HostResponseSocketsUdpStream {
                    result,
                    lost: false,
                    local_address: SocketEntry::udp_local_address_change(
                        expected.as_ref(),
                        native_local,
                    ),
                    disconnected,
                })
            },
        )
        .await
        .map_err(SocketError::trap)?;
        apply_lost(self, rep, response.lost)?;
        if !response.lost {
            let entry = udp_entry(self, rep).map_err(SocketError::trap)?;
            entry.local_address = stream_local_address(entry, disconnects);
            if let Some(address) = response.local_address {
                entry.local_address = Some(address);
            }
            if response.result.is_ok() {
                entry.remote_address = remote_address.map(Into::into);
            } else if response.disconnected {
                entry.remote_address = None;
            }
            entry.phase = if entry.remote_address.is_some() {
                SocketPhase::Connected
            } else {
                SocketPhase::Bound
            };
        }
        response.result.map_err(SocketError::from)?;
        push_streams(self, rep, natives).map_err(SocketError::trap)
    }

    fn local_address(
        &mut self,
        self_: Resource<UdpSocket>,
    ) -> Result<IpSocketAddress, SocketError> {
        self.observe_function_call("sockets::udp", "local_address");
        udp_entry(self, self_.rep())
            .map_err(SocketError::trap)?
            .local_address
            .clone()
            .map(Into::into)
            .ok_or_else(|| ErrorCode::InvalidState.into())
    }

    fn remote_address(
        &mut self,
        self_: Resource<UdpSocket>,
    ) -> Result<IpSocketAddress, SocketError> {
        self.observe_function_call("sockets::udp", "remote_address");
        udp_entry(self, self_.rep())
            .map_err(SocketError::trap)?
            .remote_address
            .clone()
            .map(Into::into)
            .ok_or_else(|| ErrorCode::InvalidState.into())
    }

    fn address_family(&mut self, self_: Resource<UdpSocket>) -> wasmtime::Result<IpAddressFamily> {
        self.observe_function_call("sockets::udp", "address_family");
        Ok(p2_family(udp_entry(self, self_.rep())?.family))
    }

    async fn unicast_hop_limit(&mut self, self_: Resource<UdpSocket>) -> Result<u8, SocketError> {
        self.observe_function_call("sockets::udp", "unicast_hop_limit");
        HostUdpSocket::unicast_hop_limit(&mut self.as_wasi_view().sockets(), self_).await
    }

    fn set_unicast_hop_limit(
        &mut self,
        self_: Resource<UdpSocket>,
        value: u8,
    ) -> Result<(), SocketError> {
        self.observe_function_call("sockets::udp", "set_unicast_hop_limit");
        HostUdpSocket::set_unicast_hop_limit(&mut self.as_wasi_view().sockets(), self_, value)
    }

    async fn receive_buffer_size(
        &mut self,
        self_: Resource<UdpSocket>,
    ) -> Result<u64, SocketError> {
        self.observe_function_call("sockets::udp", "receive_buffer_size");
        HostUdpSocket::receive_buffer_size(&mut self.as_wasi_view().sockets(), self_).await
    }

    fn set_receive_buffer_size(
        &mut self,
        self_: Resource<UdpSocket>,
        value: u64,
    ) -> Result<(), SocketError> {
        self.observe_function_call("sockets::udp", "set_receive_buffer_size");
        HostUdpSocket::set_receive_buffer_size(&mut self.as_wasi_view().sockets(), self_, value)
    }

    async fn send_buffer_size(&mut self, self_: Resource<UdpSocket>) -> Result<u64, SocketError> {
        self.observe_function_call("sockets::udp", "send_buffer_size");
        HostUdpSocket::send_buffer_size(&mut self.as_wasi_view().sockets(), self_).await
    }

    fn set_send_buffer_size(
        &mut self,
        self_: Resource<UdpSocket>,
        value: u64,
    ) -> Result<(), SocketError> {
        self.observe_function_call("sockets::udp", "set_send_buffer_size");
        HostUdpSocket::set_send_buffer_size(&mut self.as_wasi_view().sockets(), self_, value)
    }

    fn subscribe(&mut self, self_: Resource<UdpSocket>) -> wasmtime::Result<Resource<Pollable>> {
        self.observe_function_call("sockets::udp", "subscribe");
        HostUdpSocket::subscribe(&mut self.as_wasi_view().sockets(), self_)
    }

    fn drop(&mut self, rep: Resource<UdpSocket>) -> wasmtime::Result<()> {
        self.observe_function_call("sockets::udp", "drop");
        let socket = rep.rep();
        HostUdpSocket::drop(&mut self.as_wasi_view().sockets(), rep)?;
        // Only after the native delete: a failed drop leaves the socket and its rep live.
        self.state.socket_entries.remove(&socket);
        Ok(())
    }
}

impl<Ctx: WorkerCtx> HostIncomingDatagramStream for DurableWorkerCtx<Ctx> {
    async fn receive(
        &mut self,
        self_: Resource<IncomingDatagramStream>,
        max_results: u64,
    ) -> Result<Vec<IncomingDatagram>, SocketError> {
        self.observe_function_call("sockets::udp", "receive");
        let rep = self_.rep();
        let socket = table_entry::<DurableIncomingDatagramStream, _>(self, rep)
            .map_err(SocketError::trap)?
            .socket;
        let response = DurableCallSession::<SocketsUdpReceive, NotCancellable>::invoke(
            self,
            HostRequestSocketsUdpReceive { max_results },
            DurableFunctionType::ReadRemote,
            async |ctx| -> wasmtime::Result<_> {
                if !ensure_native_endpoint(ctx, socket).await? {
                    return Ok(HostResponseSocketsUdpReceive {
                        result: Err(lost_error()),
                        lost: true,
                        local_address: None,
                    });
                }
                let native = table_entry::<DurableIncomingDatagramStream, _>(ctx, rep)?
                    .native
                    .take()
                    .ok_or_else(|| wasmtime::Error::msg("UDP stream has no native stream"))?;
                // The native stream is in the table only for this call. The push takes the head
                // of the free list and the delete gives it back, so no other rep changes.
                let native = ctx.table().push(native)?;
                let received = {
                    let mut wasi = ctx.as_wasi_view();
                    HostIncomingDatagramStream::receive(
                        &mut wasi.sockets(),
                        Resource::new_borrow(native.rep()),
                        max_results,
                    )
                    .await
                };
                let native = ctx.table().delete(native)?;
                table_entry::<DurableIncomingDatagramStream, _>(ctx, rep)?.native = Some(native);
                let result = match received {
                    Ok(datagrams) => Ok(datagrams
                        .into_iter()
                        .map(|datagram| SerializableP3UdpDatagram {
                            data: datagram.data,
                            remote_address: datagram.remote_address.into(),
                        })
                        .collect()),
                    Err(error) => Err(recorded_error(error)?),
                };
                Ok(HostResponseSocketsUdpReceive {
                    result,
                    lost: false,
                    local_address: local_address_change(ctx, socket)?,
                })
            },
        )
        .await
        .map_err(SocketError::trap)?;
        apply_lost(self, socket, response.lost)?;
        apply_local_address(self, socket, response.local_address)?;
        Ok(response
            .result
            .map_err(SocketError::from)?
            .into_iter()
            .map(|datagram| IncomingDatagram {
                data: datagram.data,
                remote_address: datagram.remote_address.into(),
            })
            .collect())
    }

    fn subscribe(
        &mut self,
        self_: Resource<IncomingDatagramStream>,
    ) -> wasmtime::Result<Resource<Pollable>> {
        self.observe_function_call("sockets::udp", "subscribe");
        wasmtime_wasi::p2::subscribe(
            self.table(),
            Resource::<DurableIncomingDatagramStream>::new_borrow(self_.rep()),
            None,
        )
    }

    async fn drop(&mut self, rep: Resource<IncomingDatagramStream>) -> wasmtime::Result<()> {
        self.observe_function_call("sockets::udp", "drop");
        let stream = self
            .table()
            .delete(Resource::<DurableIncomingDatagramStream>::new_own(
                rep.rep(),
            ))?;
        if let Some(native) = stream.native {
            let native = self.table().push(native)?;
            HostIncomingDatagramStream::drop(&mut self.as_wasi_view().sockets(), native).await?;
        }
        Ok(())
    }
}

impl<Ctx: WorkerCtx> HostOutgoingDatagramStream for DurableWorkerCtx<Ctx> {
    async fn check_send(
        &mut self,
        self_: Resource<OutgoingDatagramStream>,
    ) -> Result<u64, SocketError> {
        self.observe_function_call("sockets::udp", "check_send");
        let rep = self_.rep();
        let socket = table_entry::<DurableOutgoingDatagramStream, _>(self, rep)
            .map_err(SocketError::trap)?
            .socket;
        let response = DurableCallSession::<SocketsUdpCheckSend, NotCancellable>::invoke(
            self,
            HostRequestNoInput {},
            DurableFunctionType::ReadLocal,
            async |ctx| -> wasmtime::Result<_> {
                if !ensure_native_endpoint(ctx, socket).await? {
                    return Ok(HostResponseSocketsUdpCheckSend {
                        result: Err(lost_error()),
                        lost: true,
                        local_address: None,
                    });
                }
                let result = match native_check_send(ctx, rep).await? {
                    Ok(permit) => Ok(permit),
                    Err(error) => Err(recorded_error(error)?),
                };
                Ok(HostResponseSocketsUdpCheckSend {
                    result,
                    lost: false,
                    local_address: local_address_change(ctx, socket)?,
                })
            },
        )
        .await
        .map_err(SocketError::trap)?;
        apply_lost(self, socket, response.lost)?;
        apply_local_address(self, socket, response.local_address)?;
        let permit = response.result.map_err(SocketError::from)?;
        table_entry::<DurableOutgoingDatagramStream, _>(self, rep)
            .map_err(SocketError::trap)?
            .permit = permit;
        Ok(permit)
    }

    async fn send(
        &mut self,
        self_: Resource<OutgoingDatagramStream>,
        datagrams: Vec<OutgoingDatagram>,
    ) -> Result<u64, SocketError> {
        let addresses: Vec<_> = datagrams
            .iter()
            .filter_map(|datagram| datagram.remote_address)
            .collect();
        if !addresses.is_empty() {
            authorize_addresses(self, &addresses).await?;
        }
        self.observe_function_call("sockets::udp", "send");
        let rep = self_.rep();
        if datagrams.is_empty() {
            return Ok(0);
        }
        let stream = table_entry::<DurableOutgoingDatagramStream, _>(self, rep)
            .map_err(SocketError::trap)?;
        if datagrams.len() as u64 > stream.permit {
            return Err(SocketError::trap(wasmtime::Error::msg(
                "unpermitted: argument exceeds permitted size",
            )));
        }
        // Each `send` must be permitted by a preceding `check-send`.
        stream.permit = 0;
        let socket = stream.socket;

        // The datagrams are never stored: a completed or ambiguous send is never repeated.
        let request = HostRequestSocketsUdpSend {
            remote_addresses: datagrams
                .iter()
                .map(|datagram| datagram.remote_address.map(Into::into))
                .collect(),
        };
        let call = DurableCallSession::<SocketsUdpSend, NotCancellable>::start(
            self,
            request,
            DurableFunctionType::WriteRemote,
        )
        .await
        .map_err(SocketError::trap)?;
        let started_live = call.is_live();
        let persisted = call.is_persisted();
        let response = call
            .run(self, async |ctx| -> wasmtime::Result<_> {
                if !started_live {
                    // An incomplete send completes with `connection-reset` and is never sent
                    // again.
                    return Ok(HostResponseSocketsUdpSend {
                        result: Err(lost_error()),
                        lost: false,
                        local_address: None,
                    });
                }
                if !ensure_native_endpoint(ctx, socket).await? {
                    return Ok(HostResponseSocketsUdpSend {
                        result: Err(lost_error()),
                        lost: true,
                        local_address: None,
                    });
                }
                if persisted {
                    // Fence the send's Start before the datagrams can leave the worker.
                    ctx.public_state
                        .worker()
                        .commit_oplog_and_update_state(CommitLevel::Always)
                        .await?;
                }
                let result = match native_send(ctx, rep, datagrams).await? {
                    Ok(count) => Ok(count),
                    Err(error) => Err(recorded_error(error)?),
                };
                // A send on a socket whose disconnect released its port binds it implicitly.
                Ok(HostResponseSocketsUdpSend {
                    result,
                    lost: false,
                    local_address: local_address_change(ctx, socket)?,
                })
            })
            .await
            .map_err(SocketError::trap)?;
        apply_lost(self, socket, response.lost)?;
        apply_local_address(self, socket, response.local_address.clone())?;
        response.result.map_err(SocketError::from)
    }

    fn subscribe(
        &mut self,
        self_: Resource<OutgoingDatagramStream>,
    ) -> wasmtime::Result<Resource<Pollable>> {
        self.observe_function_call("sockets::udp", "subscribe");
        wasmtime_wasi::p2::subscribe(
            self.table(),
            Resource::<DurableOutgoingDatagramStream>::new_borrow(self_.rep()),
            None,
        )
    }

    async fn drop(&mut self, rep: Resource<OutgoingDatagramStream>) -> wasmtime::Result<()> {
        self.observe_function_call("sockets::udp", "drop");
        let stream = self
            .table()
            .delete(Resource::<DurableOutgoingDatagramStream>::new_own(
                rep.rep(),
            ))?;
        if let Some(native) = stream.native {
            // The native drop waits for a send that is still in progress.
            let native = self.table().push(native)?;
            HostOutgoingDatagramStream::drop(&mut self.as_wasi_view().sockets(), native).await?;
        }
        Ok(())
    }
}

/// Takes the native outgoing stream of the guest stream at `rep` into the resource table for one
/// native call, then puts it back. The push takes the head of the table's free list and the
/// delete gives it back, so no other rep changes.
async fn with_native_outgoing<Ctx: WorkerCtx, R>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    rep: u32,
    call: impl AsyncFnOnce(
        &mut DurableWorkerCtx<Ctx>,
        Resource<OutgoingDatagramStream>,
    ) -> Result<R, SocketError>,
) -> wasmtime::Result<Result<R, SocketError>> {
    let native = table_entry::<DurableOutgoingDatagramStream, _>(ctx, rep)?
        .native
        .take()
        .ok_or_else(|| wasmtime::Error::msg("UDP stream has no native stream"))?;
    let native = ctx.table().push(native)?;
    let result = call(ctx, Resource::new_borrow(native.rep())).await;
    let native = ctx.table().delete(native)?;
    table_entry::<DurableOutgoingDatagramStream, _>(ctx, rep)?.native = Some(native);
    Ok(result)
}

async fn native_check_send<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    rep: u32,
) -> wasmtime::Result<Result<u64, SocketError>> {
    with_native_outgoing(ctx, rep, async |ctx, native| {
        HostOutgoingDatagramStream::check_send(&mut ctx.as_wasi_view().sockets(), native).await
    })
    .await
}

/// Sends natively. The native stream needs its own permit, which a replayed `check-send` did not
/// give it, so the native `check-send` runs first; it sends nothing.
async fn native_send<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    rep: u32,
    datagrams: Vec<OutgoingDatagram>,
) -> wasmtime::Result<Result<u64, SocketError>> {
    with_native_outgoing(ctx, rep, async |ctx, native| {
        let mut wasi = ctx.as_wasi_view();
        let mut view = wasi.sockets();
        HostOutgoingDatagramStream::check_send(&mut view, Resource::new_borrow(native.rep()))
            .await?;
        HostOutgoingDatagramStream::send(&mut view, native, datagrams).await
    })
    .await
}

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {}

async fn authorize_addresses<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    addresses: &[IpSocketAddress],
) -> Result<(), SocketError> {
    let begun = DurableCallSession::<SocketsUdpPermissionCheck, NotCancellable>::begin(
        ctx,
        DurableFunctionType::ReadLocal,
    )
    .await
    .map_err(SocketError::trap)?;
    let request = HostRequestSocketsUdpPermissionCheck {
        addresses: addresses
            .iter()
            .map(|address| std::net::SocketAddr::from(*address).to_string())
            .collect(),
    };
    let response = 'response: {
        let handle = match begun.resolve(ctx).await.map_err(SocketError::trap)? {
            ResolvedCall::Live(begun) => {
                let targets = addresses
                    .iter()
                    .map(|address| ctx.socket_target(std::net::SocketAddr::from(*address)))
                    .collect::<Result<Vec<_>, _>>();
                let denial = match targets {
                    Ok(targets) => ctx
                        .authorize_live_permissions(&targets)
                        .await
                        .map_err(SocketError::trap)?
                        .err()
                        .map(|error| error.to_safe_string()),
                    Err(denial) => Some(denial),
                };
                if let Some(denial) = denial {
                    break 'response begun
                        .start_live_denied(ctx, request, denial)
                        .await
                        .map_err(|error| SocketError::trap(wasmtime::Error::from_anyhow(error)))?;
                }
                begun
                    .start_live(ctx, request)
                    .await
                    .map_err(SocketError::trap)?
            }
            ResolvedCall::Replay(handle) => handle,
        };
        handle
            .run(ctx, async |_| -> wasmtime::Result<_> {
                Ok(HostResponsePermissionCheck { result: Ok(()) })
            })
            .await
            .map_err(SocketError::trap)?
    };
    response.result.map_err(|_| ErrorCode::AccessDenied.into())
}

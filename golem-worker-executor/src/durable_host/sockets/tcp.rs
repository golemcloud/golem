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

use bytes::Bytes;
use wasmtime::component::Resource;

use crate::durable_host::concurrent::ResolvedCall;
use crate::durable_host::sockets::{SocketEntry, SocketPhase};
use crate::durable_host::{DurabilityHost, DurableCallSession, DurableWorkerCtx, NotCancellable};
use crate::workerctx::WorkerCtx;
use golem_common::SafeDisplay;
use golem_common::model::oplog::host_functions::{
    SocketsTcpConnectPermissionCheck, SocketsTcpFinishConnect, SocketsTcpStartBind,
    SocketsTcpStartConnect,
};
use golem_common::model::oplog::types::{SerializableP3SocketErrorCode, SerializableSocketError};
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestNoInput, HostRequestSocketsTcpBind,
    HostRequestSocketsTcpConnect, HostRequestSocketsTcpConnectPermissionCheck,
    HostResponsePermissionCheck, HostResponseSocketsTcpBind, HostResponseSocketsTcpFinishConnect,
    HostResponseSocketsTcpStartConnect,
};
use wasmtime_wasi::p2::bindings::sockets::network::ErrorCode;
use wasmtime_wasi::p2::bindings::sockets::tcp::{
    Duration, Host, HostTcpSocket, InputStream, IpAddressFamily, IpSocketAddress, Network,
    OutputStream, Pollable, ShutdownType, TcpSocket,
};
use wasmtime_wasi::p2::{SocketError, StreamError, StreamResult};
use wasmtime_wasi::p3::bindings::sockets::types::IpAddressFamily as P3IpAddressFamily;
use wasmtime_wasi::sockets::WasiSocketsView as _;

/// The error a P2 socket reports when its connection was lost. P2 has no `connection-broken`
/// code; `connection-reset` is what a guest sees when a peer resets a connection.
const P2_CONNECTION_LOST: ErrorCode = ErrorCode::ConnectionReset;

fn lost_stream_error() -> StreamError {
    StreamError::LastOperationFailed(wasmtime::Error::from(std::io::Error::from(
        std::io::ErrorKind::ConnectionReset,
    )))
}

/// The P2 input stream of a connection that a reconstruction lost. Its first operation fails
/// with `connection-reset`, the next ones report a closed stream, and it is always ready.
#[derive(Default)]
pub(crate) struct LostTcpInputStream {
    failed: bool,
}

/// The P2 output stream of a connection that a reconstruction lost; see [`LostTcpInputStream`].
#[derive(Default)]
pub(crate) struct LostTcpOutputStream {
    failed: bool,
}

fn fail_once(failed: &mut bool) -> StreamError {
    if std::mem::replace(failed, true) {
        StreamError::Closed
    } else {
        lost_stream_error()
    }
}

#[async_trait::async_trait]
impl wasmtime_wasi::p2::Pollable for LostTcpInputStream {
    async fn ready(&mut self) {}
}

#[async_trait::async_trait]
impl wasmtime_wasi::p2::InputStream for LostTcpInputStream {
    fn read(&mut self, _size: usize) -> StreamResult<Bytes> {
        Err(fail_once(&mut self.failed))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[async_trait::async_trait]
impl wasmtime_wasi::p2::Pollable for LostTcpOutputStream {
    async fn ready(&mut self) {}
}

#[async_trait::async_trait]
impl wasmtime_wasi::p2::OutputStream for LostTcpOutputStream {
    fn write(&mut self, _bytes: Bytes) -> StreamResult<()> {
        Err(fail_once(&mut self.failed))
    }

    fn flush(&mut self) -> StreamResult<()> {
        Err(fail_once(&mut self.failed))
    }

    fn check_write(&mut self) -> StreamResult<usize> {
        Err(fail_once(&mut self.failed))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
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

fn closed_code(code: ErrorCode) -> SerializableP3SocketErrorCode {
    use SerializableP3SocketErrorCode as P3;
    match code {
        ErrorCode::AccessDenied => P3::AccessDenied,
        ErrorCode::NotSupported => P3::NotSupported,
        ErrorCode::InvalidArgument => P3::InvalidArgument,
        ErrorCode::OutOfMemory => P3::OutOfMemory,
        ErrorCode::Timeout => P3::Timeout,
        ErrorCode::InvalidState => P3::InvalidState,
        ErrorCode::AddressNotBindable => P3::AddressNotBindable,
        ErrorCode::AddressInUse => P3::AddressInUse,
        ErrorCode::RemoteUnreachable => P3::RemoteUnreachable,
        ErrorCode::ConnectionRefused => P3::ConnectionRefused,
        ErrorCode::ConnectionReset => P3::ConnectionReset,
        ErrorCode::ConnectionAborted => P3::ConnectionAborted,
        ErrorCode::DatagramTooLarge => P3::DatagramTooLarge,
        other => P3::Other(Some(format!("{other:?}"))),
    }
}

fn p2_code(code: &SerializableP3SocketErrorCode) -> ErrorCode {
    use SerializableP3SocketErrorCode as P3;
    match code {
        P3::AccessDenied => ErrorCode::AccessDenied,
        P3::NotSupported => ErrorCode::NotSupported,
        P3::InvalidArgument => ErrorCode::InvalidArgument,
        P3::OutOfMemory => ErrorCode::OutOfMemory,
        P3::Timeout => ErrorCode::Timeout,
        P3::InvalidState => ErrorCode::InvalidState,
        P3::AddressNotBindable => ErrorCode::AddressNotBindable,
        P3::AddressInUse => ErrorCode::AddressInUse,
        P3::RemoteUnreachable => ErrorCode::RemoteUnreachable,
        P3::ConnectionRefused => ErrorCode::ConnectionRefused,
        P3::ConnectionReset | P3::ConnectionBroken => ErrorCode::ConnectionReset,
        P3::ConnectionAborted => ErrorCode::ConnectionAborted,
        P3::DatagramTooLarge => ErrorCode::DatagramTooLarge,
        P3::Other(Some(name)) => [
            ErrorCode::ConcurrencyConflict,
            ErrorCode::NotInProgress,
            ErrorCode::WouldBlock,
            ErrorCode::NewSocketLimit,
            ErrorCode::NameUnresolvable,
            ErrorCode::TemporaryResolverFailure,
            ErrorCode::PermanentResolverFailure,
        ]
        .into_iter()
        .find(|code| format!("{code:?}") == *name)
        .unwrap_or(ErrorCode::Unknown),
        P3::Other(None) => ErrorCode::Unknown,
    }
}

/// Splits a native socket error into its recorded error code; anything else traps.
fn recorded_error(error: SocketError) -> wasmtime::Result<SerializableSocketError> {
    Ok(SerializableSocketError::from_result(Ok(error.downcast()?)))
}

fn recorded_code(error: &SerializableSocketError) -> Option<ErrorCode> {
    SocketError::from(error.clone()).downcast().ok()
}

/// The entry of a P2 TCP socket. A socket from native `accept` has no entry until its first use.
fn tcp_entry<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    rep: u32,
) -> wasmtime::Result<&mut SocketEntry> {
    if !ctx.state.socket_entries.contains_key(&rep) {
        let entry = {
            let mut wasi = ctx.as_wasi_view();
            let mut view = wasi.sockets();
            let family = HostTcpSocket::address_family(&mut view, Resource::new_borrow(rep))?;
            let mut entry = SocketEntry::new(p3_family(family));
            entry.local_address =
                HostTcpSocket::local_address(&mut view, Resource::new_borrow(rep))
                    .ok()
                    .map(Into::into);
            entry.remote_address =
                HostTcpSocket::remote_address(&mut view, Resource::new_borrow(rep))
                    .ok()
                    .map(Into::into);
            entry.native_bound = entry.local_address.is_some();
            entry.phase = if entry.remote_address.is_some() {
                SocketPhase::Connected
            } else if entry.local_address.is_some() {
                SocketPhase::Bound
            } else {
                SocketPhase::Created
            };
            entry
        };
        ctx.state.socket_entries.insert(rep, entry);
    }
    Ok(ctx
        .state
        .socket_entries
        .get_mut(&rep)
        .expect("TCP socket entry"))
}

/// The answer to `start-bind` or `start-connect` when the entry holds a state that a
/// reconstruction rebuilt without the native socket. The native socket of such an entry is
/// unbound and unconnected, so it would accept either operation.
fn reconstructed_state_error(entry: &SocketEntry, connect: bool) -> Option<ErrorCode> {
    match &entry.phase {
        SocketPhase::Bound if !entry.native_bound && entry.bind_pending => {
            Some(ErrorCode::ConcurrencyConflict)
        }
        SocketPhase::Bound if !entry.native_bound => Some(if connect {
            // The recorded endpoint is not a reserved native port.
            P2_CONNECTION_LOST
        } else {
            ErrorCode::InvalidState
        }),
        SocketPhase::Connecting if !entry.native_connect => Some(ErrorCode::ConcurrencyConflict),
        SocketPhase::Lost | SocketPhase::Closed(_) => Some(ErrorCode::InvalidState),
        _ => None,
    }
}

/// Socket options follow the native socket, which a reconstruction leaves unconnected. The
/// entry answers for the states that the native socket no longer holds.
fn check_option_access<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    rep: u32,
) -> Result<(), SocketError> {
    match &tcp_entry(ctx, rep).map_err(SocketError::trap)?.phase {
        SocketPhase::Connecting => Err(ErrorCode::InvalidState.into()),
        SocketPhase::Closed(code) => Err(p2_code(code).into()),
        _ => Ok(()),
    }
}

impl<Ctx: WorkerCtx> HostTcpSocket for DurableWorkerCtx<Ctx> {
    async fn start_bind(
        &mut self,
        self_: Resource<TcpSocket>,
        network: Resource<Network>,
        local_address: IpSocketAddress,
    ) -> Result<(), SocketError> {
        self.observe_function_call("sockets::tcp", "start_bind");
        let rep = self_.rep();
        let mut bound_live = false;
        let response = DurableCallSession::<SocketsTcpStartBind, NotCancellable>::invoke(
            self,
            HostRequestSocketsTcpBind {
                local_address: local_address.into(),
            },
            DurableFunctionType::WriteLocal,
            async |ctx| -> wasmtime::Result<_> {
                if let Some(code) = reconstructed_state_error(tcp_entry(ctx, rep)?, false) {
                    return Ok(HostResponseSocketsTcpBind {
                        result: Err(SerializableSocketError::from_result(Ok(code))),
                    });
                }
                let mut wasi = ctx.as_wasi_view();
                let mut view = wasi.sockets();
                let result =
                    match HostTcpSocket::start_bind(&mut view, self_, network, local_address).await
                    {
                        Ok(()) => {
                            bound_live = true;
                            match HostTcpSocket::local_address(&mut view, Resource::new_borrow(rep))
                            {
                                Ok(address) => Ok(address.into()),
                                Err(error) => Err(recorded_error(error)?),
                            }
                        }
                        Err(error) => Err(recorded_error(error)?),
                    };
                Ok(HostResponseSocketsTcpBind { result })
            },
        )
        .await
        .map_err(SocketError::trap)?;
        match response.result {
            Ok(address) => {
                let entry = tcp_entry(self, rep).map_err(SocketError::trap)?;
                entry.phase = SocketPhase::Bound;
                entry.local_address = Some(address);
                entry.native_bound = bound_live;
                entry.bind_pending = true;
                Ok(())
            }
            Err(error) => Err(error.into()),
        }
    }

    async fn finish_bind(&mut self, self_: Resource<TcpSocket>) -> Result<(), SocketError> {
        self.observe_function_call("sockets::tcp", "finish_bind");
        let rep = self_.rep();
        let entry = tcp_entry(self, rep).map_err(SocketError::trap)?;
        let (native_bound, bind_pending) = (entry.native_bound, entry.bind_pending);
        let result = if native_bound {
            HostTcpSocket::finish_bind(&mut self.as_wasi_view().sockets(), self_).await
        } else if bind_pending {
            Ok(())
        } else {
            Err(ErrorCode::NotInProgress.into())
        };
        if result.is_ok() {
            tcp_entry(self, rep)
                .map_err(SocketError::trap)?
                .bind_pending = false;
        }
        result
    }

    async fn start_connect(
        &mut self,
        self_: Resource<TcpSocket>,
        network: Resource<Network>,
        remote_address: IpSocketAddress,
    ) -> Result<(), SocketError> {
        authorize_connect(self, &remote_address).await?;
        self.observe_function_call("sockets::tcp", "start_connect");
        let rep = self_.rep();
        let mut connect_live = false;
        // `WriteLocal`: the read-only guard refuses the connect here, before a poll of the socket
        // can drive the native connect.
        let response = DurableCallSession::<SocketsTcpStartConnect, NotCancellable>::invoke(
            self,
            HostRequestSocketsTcpConnect {
                remote_address: remote_address.into(),
            },
            DurableFunctionType::WriteLocal,
            async |ctx| -> wasmtime::Result<_> {
                if let Some(code) = reconstructed_state_error(tcp_entry(ctx, rep)?, true) {
                    return Ok(HostResponseSocketsTcpStartConnect {
                        result: Err(SerializableSocketError::from_result(Ok(code))),
                    });
                }
                let mut view = ctx.as_wasi_view();
                let result = match HostTcpSocket::start_connect(
                    &mut view.sockets(),
                    self_,
                    network,
                    remote_address,
                )
                .await
                {
                    Ok(()) => {
                        connect_live = true;
                        Ok(())
                    }
                    Err(error) => Err(recorded_error(error)?),
                };
                Ok(HostResponseSocketsTcpStartConnect { result })
            },
        )
        .await
        .map_err(SocketError::trap)?;
        match response.result {
            Ok(()) => {
                let entry = tcp_entry(self, rep).map_err(SocketError::trap)?;
                entry.phase = SocketPhase::Connecting;
                entry.native_connect = connect_live;
                Ok(())
            }
            Err(error) => Err(error.into()),
        }
    }

    async fn finish_connect(
        &mut self,
        self_: Resource<TcpSocket>,
    ) -> Result<(Resource<InputStream>, Resource<OutputStream>), SocketError> {
        self.observe_function_call("sockets::tcp", "finish_connect");
        let rep = self_.rep();
        let was_connecting =
            tcp_entry(self, rep).map_err(SocketError::trap)?.phase == SocketPhase::Connecting;
        let mut native_streams = None;
        let response = DurableCallSession::<SocketsTcpFinishConnect, NotCancellable>::invoke(
            self,
            HostRequestNoInput {},
            DurableFunctionType::WriteRemote,
            async |ctx| -> wasmtime::Result<_> {
                let entry = tcp_entry(ctx, rep)?;
                if entry.phase == SocketPhase::Connecting && !entry.native_connect {
                    // The native connect started in a Store that is gone.
                    return Ok(HostResponseSocketsTcpFinishConnect {
                        result: Err(SerializableSocketError::from_result(Ok(P2_CONNECTION_LOST))),
                    });
                }
                let mut wasi = ctx.as_wasi_view();
                let mut view = wasi.sockets();
                let result = match HostTcpSocket::finish_connect(&mut view, self_).await {
                    Ok(streams) => {
                        native_streams = Some(streams);
                        let local =
                            HostTcpSocket::local_address(&mut view, Resource::new_borrow(rep))
                                .map_err(|error| wasmtime::Error::msg(error.to_string()))?;
                        let remote =
                            HostTcpSocket::remote_address(&mut view, Resource::new_borrow(rep))
                                .map_err(|error| wasmtime::Error::msg(error.to_string()))?;
                        Ok((local.into(), remote.into()))
                    }
                    Err(error) => Err(recorded_error(error)?),
                };
                Ok(HostResponseSocketsTcpFinishConnect { result })
            },
        )
        .await
        .map_err(SocketError::trap)?;
        match response.result {
            Ok((local, remote)) => {
                let live = native_streams.is_some();
                let (input, output) = match native_streams {
                    Some(streams) => streams,
                    None => {
                        // Stand-ins take the reps that the native streams had, in the same order.
                        let socket = Resource::<TcpSocket>::new_borrow(rep);
                        let input: InputStream = Box::<LostTcpInputStream>::default();
                        let output: OutputStream = Box::<LostTcpOutputStream>::default();
                        let input = self
                            .table()
                            .push_child(input, &socket)
                            .map_err(|error| SocketError::trap(wasmtime::Error::from(error)))?;
                        let output = self
                            .table()
                            .push_child(output, &socket)
                            .map_err(|error| SocketError::trap(wasmtime::Error::from(error)))?;
                        (input, output)
                    }
                };
                let entry = tcp_entry(self, rep).map_err(SocketError::trap)?;
                entry.phase = if live {
                    SocketPhase::Connected
                } else {
                    SocketPhase::Lost
                };
                entry.local_address = Some(local);
                entry.remote_address = Some(remote);
                entry.native_connect = false;
                self.state.open_tcp_input_streams.insert(input.rep());
                self.state.open_tcp_output_streams.insert(output.rep());
                Ok((input, output))
            }
            Err(error) => {
                let code = recorded_code(&error);
                if was_connecting && code != Some(ErrorCode::WouldBlock) {
                    let entry = tcp_entry(self, rep).map_err(SocketError::trap)?;
                    entry.phase =
                        SocketPhase::Closed(closed_code(code.unwrap_or(ErrorCode::Unknown)));
                    entry.native_connect = false;
                }
                Err(error.into())
            }
        }
    }

    /// Agents expose no ports, so `listen` is always refused.
    async fn start_listen(&mut self, _socket: Resource<TcpSocket>) -> Result<(), SocketError> {
        self.observe_function_call("sockets::tcp", "start_listen");
        Err(ErrorCode::AccessDenied.into())
    }

    fn finish_listen(&mut self, self_: Resource<TcpSocket>) -> Result<(), SocketError> {
        self.observe_function_call("sockets::tcp", "finish_listen");
        HostTcpSocket::finish_listen(&mut self.as_wasi_view().sockets(), self_)
    }

    fn accept(
        &mut self,
        self_: Resource<TcpSocket>,
    ) -> Result<
        (
            Resource<TcpSocket>,
            Resource<InputStream>,
            Resource<OutputStream>,
        ),
        SocketError,
    > {
        self.observe_function_call("sockets::tcp", "accept");
        let result = HostTcpSocket::accept(&mut self.as_wasi_view().sockets(), self_)?;
        self.state.open_tcp_input_streams.insert(result.1.rep());
        self.state.open_tcp_output_streams.insert(result.2.rep());
        Ok(result)
    }

    fn local_address(
        &mut self,
        self_: Resource<TcpSocket>,
    ) -> Result<IpSocketAddress, SocketError> {
        self.observe_function_call("sockets::tcp", "local_address");
        let entry = tcp_entry(self, self_.rep()).map_err(SocketError::trap)?;
        match &entry.phase {
            SocketPhase::Closed(code) => Err(p2_code(code).into()),
            SocketPhase::Connecting => Err(ErrorCode::InvalidState.into()),
            _ => entry
                .local_address
                .clone()
                .map(Into::into)
                .ok_or_else(|| ErrorCode::InvalidState.into()),
        }
    }

    fn remote_address(
        &mut self,
        self_: Resource<TcpSocket>,
    ) -> Result<IpSocketAddress, SocketError> {
        self.observe_function_call("sockets::tcp", "remote_address");
        let entry = tcp_entry(self, self_.rep()).map_err(SocketError::trap)?;
        match &entry.phase {
            SocketPhase::Closed(code) => Err(p2_code(code).into()),
            SocketPhase::Connected | SocketPhase::Lost => entry
                .remote_address
                .clone()
                .map(Into::into)
                .ok_or_else(|| ErrorCode::InvalidState.into()),
            _ => Err(ErrorCode::InvalidState.into()),
        }
    }

    fn is_listening(&mut self, self_: Resource<TcpSocket>) -> wasmtime::Result<bool> {
        self.observe_function_call("sockets::tcp", "is_listening");
        HostTcpSocket::is_listening(&mut self.as_wasi_view().sockets(), self_)
    }

    fn address_family(&mut self, self_: Resource<TcpSocket>) -> wasmtime::Result<IpAddressFamily> {
        self.observe_function_call("sockets::tcp", "address_family");
        Ok(p2_family(tcp_entry(self, self_.rep())?.family))
    }
    fn set_listen_backlog_size(
        &mut self,
        self_: Resource<TcpSocket>,
        value: u64,
    ) -> Result<(), SocketError> {
        self.observe_function_call("sockets::tcp", "set_listen_backlog_size");
        HostTcpSocket::set_listen_backlog_size(&mut self.as_wasi_view().sockets(), self_, value)
    }

    async fn keep_alive_enabled(
        &mut self,
        self_: Resource<TcpSocket>,
    ) -> Result<bool, SocketError> {
        self.observe_function_call("sockets::tcp", "keep_alive_enabled");
        check_option_access(self, self_.rep())?;
        HostTcpSocket::keep_alive_enabled(&mut self.as_wasi_view().sockets(), self_).await
    }

    fn set_keep_alive_enabled(
        &mut self,
        self_: Resource<TcpSocket>,
        value: bool,
    ) -> Result<(), SocketError> {
        self.observe_function_call("sockets::tcp", "set_keep_alive_enabled");
        check_option_access(self, self_.rep())?;
        HostTcpSocket::set_keep_alive_enabled(&mut self.as_wasi_view().sockets(), self_, value)
    }

    async fn keep_alive_idle_time(
        &mut self,
        self_: Resource<TcpSocket>,
    ) -> Result<Duration, SocketError> {
        self.observe_function_call("sockets::tcp", "keep_alive_idle_time");
        check_option_access(self, self_.rep())?;
        HostTcpSocket::keep_alive_idle_time(&mut self.as_wasi_view().sockets(), self_).await
    }

    fn set_keep_alive_idle_time(
        &mut self,
        self_: Resource<TcpSocket>,
        value: Duration,
    ) -> Result<(), SocketError> {
        self.observe_function_call("sockets::tcp", "set_keep_alive_idle_time");
        check_option_access(self, self_.rep())?;
        HostTcpSocket::set_keep_alive_idle_time(&mut self.as_wasi_view().sockets(), self_, value)
    }

    async fn keep_alive_interval(
        &mut self,
        self_: Resource<TcpSocket>,
    ) -> Result<Duration, SocketError> {
        self.observe_function_call("sockets::tcp", "keep_alive_interval");
        check_option_access(self, self_.rep())?;
        HostTcpSocket::keep_alive_interval(&mut self.as_wasi_view().sockets(), self_).await
    }

    fn set_keep_alive_interval(
        &mut self,
        self_: Resource<TcpSocket>,
        value: Duration,
    ) -> Result<(), SocketError> {
        self.observe_function_call("sockets::tcp", "set_keep_alive_interval");
        check_option_access(self, self_.rep())?;
        HostTcpSocket::set_keep_alive_interval(&mut self.as_wasi_view().sockets(), self_, value)
    }

    async fn keep_alive_count(&mut self, self_: Resource<TcpSocket>) -> Result<u32, SocketError> {
        self.observe_function_call("sockets::tcp", "keep_alive_count");
        check_option_access(self, self_.rep())?;
        HostTcpSocket::keep_alive_count(&mut self.as_wasi_view().sockets(), self_).await
    }

    fn set_keep_alive_count(
        &mut self,
        self_: Resource<TcpSocket>,
        value: u32,
    ) -> Result<(), SocketError> {
        self.observe_function_call("sockets::tcp", "set_keep_alive_count");
        check_option_access(self, self_.rep())?;
        HostTcpSocket::set_keep_alive_count(&mut self.as_wasi_view().sockets(), self_, value)
    }

    async fn hop_limit(&mut self, self_: Resource<TcpSocket>) -> Result<u8, SocketError> {
        self.observe_function_call("sockets::tcp", "hop_limit");
        check_option_access(self, self_.rep())?;
        HostTcpSocket::hop_limit(&mut self.as_wasi_view().sockets(), self_).await
    }

    fn set_hop_limit(&mut self, self_: Resource<TcpSocket>, value: u8) -> Result<(), SocketError> {
        self.observe_function_call("sockets::tcp", "set_hop_limit");
        check_option_access(self, self_.rep())?;
        HostTcpSocket::set_hop_limit(&mut self.as_wasi_view().sockets(), self_, value)
    }

    async fn receive_buffer_size(
        &mut self,
        self_: Resource<TcpSocket>,
    ) -> Result<u64, SocketError> {
        self.observe_function_call("sockets::tcp", "receive_buffer_size");
        check_option_access(self, self_.rep())?;
        HostTcpSocket::receive_buffer_size(&mut self.as_wasi_view().sockets(), self_).await
    }

    fn set_receive_buffer_size(
        &mut self,
        self_: Resource<TcpSocket>,
        value: u64,
    ) -> Result<(), SocketError> {
        self.observe_function_call("sockets::tcp", "set_receive_buffer_size");
        check_option_access(self, self_.rep())?;
        HostTcpSocket::set_receive_buffer_size(&mut self.as_wasi_view().sockets(), self_, value)
    }

    async fn send_buffer_size(&mut self, self_: Resource<TcpSocket>) -> Result<u64, SocketError> {
        self.observe_function_call("sockets::tcp", "send_buffer_size");
        check_option_access(self, self_.rep())?;
        HostTcpSocket::send_buffer_size(&mut self.as_wasi_view().sockets(), self_).await
    }

    fn set_send_buffer_size(
        &mut self,
        self_: Resource<TcpSocket>,
        value: u64,
    ) -> Result<(), SocketError> {
        self.observe_function_call("sockets::tcp", "set_send_buffer_size");
        check_option_access(self, self_.rep())?;
        HostTcpSocket::set_send_buffer_size(&mut self.as_wasi_view().sockets(), self_, value)
    }

    fn subscribe(&mut self, self_: Resource<TcpSocket>) -> wasmtime::Result<Resource<Pollable>> {
        self.observe_function_call("sockets::tcp", "subscribe");
        HostTcpSocket::subscribe(&mut self.as_wasi_view().sockets(), self_)
    }

    async fn shutdown(
        &mut self,
        self_: Resource<TcpSocket>,
        shutdown_type: ShutdownType,
    ) -> Result<(), SocketError> {
        self.observe_function_call("sockets::tcp", "shutdown");
        if tcp_entry(self, self_.rep())
            .map_err(SocketError::trap)?
            .phase
            == SocketPhase::Lost
        {
            // The native socket of a lost connection has no streams to shut down.
            return Ok(());
        }
        HostTcpSocket::shutdown(&mut self.as_wasi_view().sockets(), self_, shutdown_type).await
    }

    fn drop(&mut self, rep: Resource<TcpSocket>) -> wasmtime::Result<()> {
        self.observe_function_call("sockets::tcp", "drop");
        let socket = rep.rep();
        HostTcpSocket::drop(&mut self.as_wasi_view().sockets(), rep)?;
        // Only after the native delete: a failed drop leaves the socket and its rep live.
        self.state.socket_entries.remove(&socket);
        Ok(())
    }
}

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {}

/// Checks the permission of a connect in a durable call, so that replay returns the recorded
/// decision instead of checking again. An address that the socket policy refuses is denied. A
/// denied check records the denial in its `Start`, so the recovery of an incomplete denied check
/// denies the connect too; an incomplete check without a denial was allowed.
async fn authorize_connect<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    remote_address: &IpSocketAddress,
) -> Result<(), SocketError> {
    let begun = DurableCallSession::<SocketsTcpConnectPermissionCheck, NotCancellable>::begin(
        ctx,
        DurableFunctionType::ReadLocal,
    )
    .await
    .map_err(SocketError::trap)?;
    let request = HostRequestSocketsTcpConnectPermissionCheck {
        address: std::net::SocketAddr::from(*remote_address).to_string(),
    };
    let response = 'resp: {
        let handle = match begun.resolve(ctx).await.map_err(SocketError::trap)? {
            ResolvedCall::Live(begun) => {
                let target = ctx.socket_target(std::net::SocketAddr::from(*remote_address));
                let denial = match target {
                    Ok(target) => ctx
                        .authorize_live_permission(&target)
                        .await
                        .map_err(SocketError::trap)?
                        .err()
                        .map(|error| error.to_safe_string()),
                    Err(refusal) => Some(refusal),
                };
                if let Some(denial) = denial {
                    break 'resp begun
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

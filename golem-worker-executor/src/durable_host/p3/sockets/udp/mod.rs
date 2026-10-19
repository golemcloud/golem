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

use crate::durable_host::authorization::targets::udp_target;
use crate::durable_host::concurrent::{
    AccessClaimOptions, CallReplayOutcome, Cancellable, DurableCallSession, NotCancellable,
    ResolvedCall, reserve_invocation_id_before_await,
};
use crate::durable_host::durability::DurabilityHost;
use crate::durable_host::p3::{
    DurableP3, DurableP3View, durable_worker_ctx, observe_function_call, wasi_sockets_view,
};
use crate::services::HasWorker;
use crate::services::oplog::CommitLevel;
use crate::workerctx::WorkerCtx;
use golem_common::model::oplog::host_functions::{
    P3SocketsTypesUdpSocketConnect, P3SocketsTypesUdpSocketReceive, P3SocketsTypesUdpSocketSend,
};
use golem_common::model::oplog::types::{SerializableP3SocketErrorCode, SerializableP3UdpDatagram};
use golem_common::model::oplog::{
    DeniablePair, DurableFunctionType, HostRequestNoInput, HostRequestP3SocketsConnect,
    HostRequestP3SocketsUdpSend, HostResponseP3SocketsConnect, HostResponseP3SocketsUdpReceive,
    HostResponseP3SocketsUdpSend,
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

impl<Ctx: WorkerCtx> types::HostUdpSocket for DurableP3View<'_, Ctx> {
    async fn bind(
        &mut self,
        socket: Resource<UdpSocket>,
        local_address: types::IpSocketAddress,
    ) -> SocketResult<()> {
        observe_function_call(&*self.0, "sockets::types::udp-socket", "bind");
        let mut view = WasiSocketsView::sockets(self.0);
        types::HostUdpSocket::bind(&mut view, socket, local_address).await
    }

    async fn connect(
        &mut self,
        socket: Resource<UdpSocket>,
        remote_address: types::IpSocketAddress,
    ) -> SocketResult<()> {
        observe_function_call(&*self.0, "sockets::types::udp-socket", "connect");
        let begun = DurableCallSession::<P3SocketsTypesUdpSocketConnect, NotCancellable>::begin(
            self.0.durable_ctx_mut(),
            DurableFunctionType::WriteRemote,
        )
        .await
        .map_err(SocketError::trap)?;
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
                let request = HostRequestP3SocketsConnect {
                    remote_address: remote_address.into(),
                };
                if let Some(denial) = denial {
                    return begun
                        .start_live_denied(self.0.durable_ctx_mut(), request, denial)
                        .await
                        .map_err(|error| SocketError::trap(wasmtime::Error::from_anyhow(error)))?
                        .result
                        .map_err(|error| types::ErrorCode::from(error).into());
                }
                begun
                    .start_live(self.0.durable_ctx_mut(), request)
                    .await
                    .map_err(SocketError::trap)?
            }
            ResolvedCall::Replay(mut handle) => {
                match handle
                    .replay(self.0.durable_ctx_mut())
                    .await
                    .map_err(SocketError::trap)?
                {
                    CallReplayOutcome::Replayed(response) => {
                        return response
                            .result
                            .map_err(|error| types::ErrorCode::from(error).into());
                    }
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
                handle
            }
        };
        let result = {
            let mut view = WasiSocketsView::sockets(self.0);
            match types::HostUdpSocket::connect(&mut view, socket, remote_address).await {
                Ok(()) => Ok(()),
                Err(error) => Err(serialize_socket_error(error).map_err(SocketError::trap)?),
            }
        };
        handle
            .complete(
                self.0.durable_ctx_mut(),
                HostResponseP3SocketsConnect { result },
            )
            .await
            .map_err(SocketError::trap)?
            .result
            .map_err(|error| types::ErrorCode::from(error).into())
    }

    fn create(
        &mut self,
        address_family: types::IpAddressFamily,
    ) -> SocketResult<Resource<UdpSocket>> {
        observe_function_call(&*self.0, "sockets::types::udp-socket", "create");
        types::HostUdpSocket::create(&mut WasiSocketsView::sockets(self.0), address_family)
    }

    fn disconnect(&mut self, socket: Resource<UdpSocket>) -> SocketResult<()> {
        observe_function_call(&*self.0, "sockets::types::udp-socket", "disconnect");
        types::HostUdpSocket::disconnect(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn get_local_address(
        &mut self,
        socket: Resource<UdpSocket>,
    ) -> SocketResult<types::IpSocketAddress> {
        observe_function_call(&*self.0, "sockets::types::udp-socket", "get-local-address");
        types::HostUdpSocket::get_local_address(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn get_remote_address(
        &mut self,
        socket: Resource<UdpSocket>,
    ) -> SocketResult<types::IpSocketAddress> {
        observe_function_call(&*self.0, "sockets::types::udp-socket", "get-remote-address");
        types::HostUdpSocket::get_remote_address(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn get_address_family(
        &mut self,
        socket: Resource<UdpSocket>,
    ) -> wasmtime::Result<types::IpAddressFamily> {
        observe_function_call(&*self.0, "sockets::types::udp-socket", "get-address-family");
        types::HostUdpSocket::get_address_family(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn get_unicast_hop_limit(&mut self, socket: Resource<UdpSocket>) -> SocketResult<u8> {
        observe_function_call(
            &*self.0,
            "sockets::types::udp-socket",
            "get-unicast-hop-limit",
        );
        types::HostUdpSocket::get_unicast_hop_limit(&mut WasiSocketsView::sockets(self.0), socket)
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

    fn get_receive_buffer_size(&mut self, socket: Resource<UdpSocket>) -> SocketResult<u64> {
        observe_function_call(
            &*self.0,
            "sockets::types::udp-socket",
            "get-receive-buffer-size",
        );
        types::HostUdpSocket::get_receive_buffer_size(&mut WasiSocketsView::sockets(self.0), socket)
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

    fn get_send_buffer_size(&mut self, socket: Resource<UdpSocket>) -> SocketResult<u64> {
        observe_function_call(
            &*self.0,
            "sockets::types::udp-socket",
            "get-send-buffer-size",
        );
        types::HostUdpSocket::get_send_buffer_size(&mut WasiSocketsView::sockets(self.0), socket)
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
        let targets = match remote_address.as_ref() {
            Some(remote_address) => socket_target(remote_address)
                .map(|target| vec![target])
                .ok_or_else(|| UNSUPPORTED_SOCKET_TARGET.to_string()),
            None => Ok(Vec::new()),
        };
        let request = HostRequestP3SocketsUdpSend {
            data: data.clone(),
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
        let result = if !call.is_live() {
            match call
                .replay_access(store, durable_worker_ctx::<Ctx, U>)
                .await
                .map_err(SocketError::trap)?
            {
                CallReplayOutcome::Replayed(response) => {
                    return response
                        .result
                        .map_err(|error| types::ErrorCode::from(error).into());
                }
                CallReplayOutcome::Incomplete(incomplete) => {
                    call = incomplete;
                    Err(SerializableP3SocketErrorCode::ConnectionBroken)
                }
            }
        } else if let Some(denied) = denied {
            denied.result
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
            let sockets = store.with_getter::<WasiSockets>(wasi_sockets_view::<Ctx, U>);
            match <WasiSockets as types::HostUdpSocketWithStore<U>>::send(
                &sockets,
                socket,
                data,
                remote_address,
            )
            .await
            {
                Ok(()) => Ok(()),
                Err(error) => Err(serialize_socket_error(error).map_err(SocketError::trap)?),
            }
        };
        let response = call
            .complete_access(
                store,
                durable_worker_ctx::<Ctx, U>,
                HostResponseP3SocketsUdpSend { result },
            )
            .await
            .map_err(SocketError::trap)?;
        match response.result {
            Ok(()) => Ok(()),
            Err(error) => Err(types::ErrorCode::from(error).into()),
        }
    }

    async fn receive(
        store: &Accessor<U, Self>,
        socket: Resource<UdpSocket>,
    ) -> SocketResult<(Vec<u8>, types::IpSocketAddress)> {
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
                    return response
                        .result
                        .map(|datagram| (datagram.data, datagram.remote_address.into()))
                        .map_err(|error| types::ErrorCode::from(error).into());
                }
                CallReplayOutcome::Incomplete(incomplete) => call = incomplete,
            }
        }
        let response = call
            .run_live_action_access(store, durable_worker_ctx::<Ctx, U>, || async {
                let sockets = store.with_getter::<WasiSockets>(wasi_sockets_view::<Ctx, U>);
                let native =
                    <WasiSockets as types::HostUdpSocketWithStore<U>>::receive(&sockets, socket);
                let interrupt = store.with(|mut access| {
                    durable_worker_ctx::<Ctx, U>(access.data_mut()).create_interrupt_signal()
                });
                // Only native receive is interruptible. Replay, terminal persistence and
                // completion delivery remain owned by the durable call outside this select.
                let result = interruptible_receive(native, interrupt).await?;

                Ok::<_, wasmtime::Error>(HostResponseP3SocketsUdpReceive {
                    result: match result {
                        Ok((data, remote_address)) => Ok(SerializableP3UdpDatagram {
                            data,
                            remote_address: remote_address.into(),
                        }),
                        Err(error) => Err(serialize_socket_error(error)?),
                    },
                })
            })
            .await
            .map_err(SocketError::trap)?;

        match response.result {
            Ok(SerializableP3UdpDatagram {
                data,
                remote_address,
            }) => Ok((data, remote_address.into())),
            Err(error) => Err(types::ErrorCode::from(error).into()),
        }
    }
}

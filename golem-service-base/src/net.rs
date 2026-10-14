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

use socket2::{Domain, Protocol, Socket, Type};
use std::io;
use std::net::SocketAddr;

/// The backlog `tokio::net::TcpListener::bind` uses.
const LISTEN_BACKLOG: i32 = 128;

/// Binds a TCP listener for a service API.
///
/// An IPv4 address binds an IPv4-only listener. An IPv6 address always binds with `IPV6_V6ONLY`
/// cleared, so `[::]` accepts both IPv6 and IPv4 connections regardless of the operating system
/// default. IPv4 clients of a dual-stack listener appear with IPv4-mapped IPv6 peer addresses
/// (`::ffff:a.b.c.d`); use [`std::net::IpAddr::to_canonical`] where a peer address identifies a
/// client.
///
/// Must be called within a Tokio runtime.
pub fn bind_tcp_listener(addr: SocketAddr) -> io::Result<tokio::net::TcpListener> {
    bind(addr).map_err(|err| io::Error::new(err.kind(), format!("failed to bind {addr}: {err}")))
}

/// [`bind_tcp_listener`] as a poem acceptor.
pub fn bind_poem_acceptor(addr: SocketAddr) -> io::Result<poem::listener::TcpAcceptor> {
    poem::listener::TcpAcceptor::from_tokio(bind_tcp_listener(addr)?)
}

fn bind(addr: SocketAddr) -> io::Result<tokio::net::TcpListener> {
    let socket = Socket::new(Domain::for_address(addr), Type::STREAM, Some(Protocol::TCP))?;
    if addr.is_ipv6() {
        socket.set_only_v6(false)?;
    }
    // Matches `tokio::net::TcpListener::bind`, so a restarted service can rebind its port while
    // connections of the previous process are in TIME_WAIT.
    #[cfg(not(windows))]
    socket.set_reuse_address(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&addr.into())?;
    socket.listen(LISTEN_BACKLOG)?;
    tokio::net::TcpListener::from_std(socket.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use socket2::SockRef;
    use std::net::{Ipv4Addr, Ipv6Addr};
    use std::time::Duration;
    use test_r::test;
    use tokio::net::TcpStream;

    /// Connects to `listener` from `client_ip` and returns the peer address the listener reports.
    async fn connect_from(
        listener: &tokio::net::TcpListener,
        client_ip: std::net::IpAddr,
    ) -> SocketAddr {
        let port = listener.local_addr().unwrap().port();
        let client = TcpStream::connect(SocketAddr::new(client_ip, port))
            .await
            .unwrap_or_else(|err| panic!("connect from {client_ip}: {err}"));
        let (_, peer) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
            .await
            .expect("accept timed out")
            .expect("accept failed");
        assert_eq!(peer.port(), client.local_addr().unwrap().port());
        peer
    }

    async fn accepts_from(listener: &tokio::net::TcpListener, client_ip: std::net::IpAddr) {
        let peer = connect_from(listener, client_ip).await;
        assert_eq!(peer.ip().to_canonical(), client_ip);
    }

    #[test]
    async fn unspecified_ipv6_listener_is_dual_stack() {
        let listener = bind_tcp_listener(SocketAddr::new(Ipv6Addr::UNSPECIFIED.into(), 0)).unwrap();

        assert!(!SockRef::from(&listener).only_v6().unwrap());
        accepts_from(&listener, Ipv6Addr::LOCALHOST.into()).await;
        accepts_from(&listener, Ipv4Addr::LOCALHOST.into()).await;
    }

    #[test]
    async fn ipv4_peer_of_dual_stack_listener_is_mapped() {
        let listener = bind_tcp_listener(SocketAddr::new(Ipv6Addr::UNSPECIFIED.into(), 0)).unwrap();

        let peer = connect_from(&listener, Ipv4Addr::LOCALHOST.into()).await;

        assert_eq!(
            peer.ip(),
            std::net::IpAddr::V6(Ipv4Addr::LOCALHOST.to_ipv6_mapped())
        );
    }

    #[test]
    async fn unspecified_ipv4_listener_is_ipv4_only() {
        let listener = bind_tcp_listener(SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 0)).unwrap();

        assert!(listener.local_addr().unwrap().is_ipv4());
        accepts_from(&listener, Ipv4Addr::LOCALHOST.into()).await;
    }

    #[test]
    async fn specific_ipv6_listener_binds() {
        let listener = bind_tcp_listener(SocketAddr::new(Ipv6Addr::LOCALHOST.into(), 0)).unwrap();

        assert_eq!(
            listener.local_addr().unwrap().ip(),
            std::net::IpAddr::V6(Ipv6Addr::LOCALHOST)
        );
        accepts_from(&listener, Ipv6Addr::LOCALHOST.into()).await;
    }

    #[test]
    async fn bind_error_names_the_address() {
        let first = bind_tcp_listener(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0)).unwrap();
        let addr = first.local_addr().unwrap();

        let err = bind_tcp_listener(addr).unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::AddrInUse);
        assert!(err.to_string().contains(&addr.to_string()), "{err}");
    }
}

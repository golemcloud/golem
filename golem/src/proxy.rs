// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use anyhow::Context;
use tokio::task::JoinSet;
use tracing::info;

#[cfg(unix)]
use sozu_command_lib::proto::command::WorkerResponse;
#[cfg(unix)]
use sozu_command_lib::{
    channel::Channel,
    config::ListenerBuilder,
    proto::command::{
        request::RequestType, AddBackend, Cluster, LoadBalancingAlgorithms, PathRule,
        RequestHttpFrontend, RulePosition, SocketAddress, WorkerRequest,
    },
};

#[cfg(unix)]
use std::net::Ipv4Addr;

use crate::AllRunDetails;

#[cfg(windows)]
use std::sync::mpsc;
#[cfg(windows)]
use windows_sys::Win32::Networking::HttpServer;
#[cfg(windows)]
use windows_sys::Win32::Foundation::*;
#[cfg(windows)]
use windows_sys::Win32::Networking::HttpServer::HTTPAPI_VERSION;

#[cfg(windows)]
#[derive(Debug)]
pub struct Channel<T, U> {
    sender: mpsc::Sender<T>,
    receiver: mpsc::Receiver<U>,
}

#[cfg(windows)]
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct WorkerRequest {
    pub id: String,
    pub content: RequestType,
}

#[cfg(windows)]
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum RequestType {
    AddCluster(Cluster),
    AddBackend(AddBackend),
    AddHttpFrontend(RequestHttpFrontend),
}

#[cfg(windows)]
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Cluster {
    pub cluster_id: String,
    pub sticky_session: bool,
    pub https_redirect: bool,
    pub load_balancing: i32,
}

#[cfg(windows)]
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct AddBackend {
    pub cluster_id: String,
    pub backend_id: String,
    pub address: SocketAddress,
}

#[cfg(windows)]
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct RequestHttpFrontend {
    pub cluster_id: Option<String>,
    pub address: SocketAddress,
    pub hostname: String,
    pub path: PathRule,
    pub position: i32,
}

#[cfg(windows)]
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SocketAddress {
    pub ip: [u8; 4],
    pub port: u16,
}

#[cfg(windows)]
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum PathRule {
    Equals(String),
    Prefix(String),
    Regex(String),
}

#[cfg(windows)]
impl PathRule {
    pub fn equals(path: &str) -> Self {
        PathRule::Equals(path.to_string())
    }

    pub fn prefix(path: &str) -> Self {
        PathRule::Prefix(path.to_string())
    }

    pub fn regex(path: &str) -> Self {
        PathRule::Regex(path.to_string())
    }
}

#[cfg(windows)]
#[derive(Debug)]
pub enum RulePosition {
    Post,
}

#[cfg(windows)]
impl From<RulePosition> for i32 {
    fn from(pos: RulePosition) -> Self {
        match pos {
            RulePosition::Post => 1,
        }
    }
}

#[cfg(windows)]
impl SocketAddress {
    pub fn new_v4(a: u8, b: u8, c: u8, d: u8, port: u16) -> Self {
        Self {
            ip: [a, b, c, d],
            port,
        }
    }
}

#[cfg(windows)]
impl Default for Cluster {
    fn default() -> Self {
        Self {
            cluster_id: String::new(),
            sticky_session: false,
            https_redirect: false,
            load_balancing: 0,
        }
    }
}

#[cfg(windows)]
impl Default for RequestHttpFrontend {
    fn default() -> Self {
        Self {
            cluster_id: None,
            address: SocketAddress::new_v4(0, 0, 0, 0, 0),
            hostname: String::new(),
            path: PathRule::Prefix("/".to_string()),
            position: 0,
        }
    }
}

#[cfg(windows)]
pub enum LoadBalancingAlgorithms {
    Random = 0,
}

#[cfg(windows)]
#[derive(Debug)]
#[allow(dead_code)]
pub struct WorkerResponse {
    pub id: String,
    pub status: i32,
}

#[cfg(windows)]
impl<T, U> Channel<T, U> {
    pub fn new() -> (Channel<T, U>, Channel<U, T>) {
        let (tx1, rx1) = mpsc::channel();
        let (tx2, rx2) = mpsc::channel();
        (
            Channel {
                sender: tx1,
                receiver: rx2,
            },
            Channel {
                sender: tx2,
                receiver: rx1,
            },
        )
    }

    pub fn write_message(&self, msg: &T) -> Result<(), anyhow::Error> 
    where T: Clone {
        self.sender.send(msg.clone())
            .map_err(|e| anyhow::anyhow!("Failed to send message: {}", e))
    }

    pub fn read_message(&self) -> Result<U, anyhow::Error> {
        self.receiver.recv()
            .map_err(|e| anyhow::anyhow!("Failed to receive message: {}", e))
    }
}

#[cfg(windows)]
pub fn start_proxy(
    listener_addr: &str,
    listener_port: u16,
    healthcheck_port: u16,
    all_run_details: &AllRunDetails,
    join_set: &mut JoinSet<Result<(), anyhow::Error>>,
) -> Result<Channel<WorkerRequest, WorkerResponse>, anyhow::Error> {
    info!("Starting Windows HTTP Server proxy");

    let ipv4_addr: std::net::Ipv4Addr = listener_addr.parse().context(format!(
        "Failed at parsing the listener host address {}",
        listener_addr
    ))?;

    unsafe {
        let mut queue_handle = 0;
        let version = HTTPAPI_VERSION { HttpApiMajorVersion: 2, HttpApiMinorVersion: 0 };
        let result = HttpServer::HttpCreateRequestQueue(
            version,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
            &mut queue_handle,
        );

        if result != ERROR_SUCCESS {
            return Err(anyhow::anyhow!("Failed to create HTTP request queue: {}", result));
        }

        let (command_channel, _worker_channel) = Channel::new();

        let dispatch = |request: WorkerRequest| {
            command_channel.write_message(&request)?;
            let response = command_channel.read_message();
            info!("Proxy response: {:?}", response);
            Ok::<(), anyhow::Error>(())
        };

        let queue_handle_clone = queue_handle;
        join_set.spawn(async move {
            let mut url_group = 0;
            let result = HttpServer::HttpCreateUrlGroup(
                queue_handle_clone as u64,
                &mut url_group,
                0,
            );
            
            if result != ERROR_SUCCESS {
                return Err(anyhow::anyhow!("Failed to create URL group: {}", result));
            }

            Ok(())
        });

        let component_backend = "golem-component";
        let worker_backend = "golem-worker";
        let health_backend = "golem-health";

        // set up the clusters
        {
            let add_backend = |(name, port): (&str, u16)| {
                dispatch(WorkerRequest {
                    id: format!("add-{name}-cluster"),
                    content: RequestType::AddCluster(Cluster {
                        cluster_id: name.to_string(),
                        sticky_session: false,
                        https_redirect: false,
                        load_balancing: LoadBalancingAlgorithms::Random as i32,
                        ..Default::default()
                    }),
                })?;

                dispatch(WorkerRequest {
                    id: format!("add-{name}-backend"),
                    content: RequestType::AddBackend(AddBackend {
                        cluster_id: name.to_string(),
                        backend_id: name.to_string(),
                        address: SocketAddress::new_v4(
                            ipv4_addr.octets()[0],
                            ipv4_addr.octets()[1],
                            ipv4_addr.octets()[2],
                            ipv4_addr.octets()[3],
                            port,
                        ),
                    }),
                })
            };

            add_backend((health_backend, healthcheck_port))?;
            add_backend((
                component_backend,
                all_run_details.component_service.http_port,
            ))?;
            add_backend((worker_backend, all_run_details.worker_service.http_port))?;
        }

        // set up routing
        {
            let mut route_counter = -1;
            let mut add_route = |(path, cluster_id): (PathRule, &str)| {
                route_counter += 1;
                dispatch(WorkerRequest {
                    id: format!("add-golem-frontend-${route_counter}"),
                    content: RequestType::AddHttpFrontend(RequestHttpFrontend {
                        cluster_id: Some(cluster_id.to_string()),
                        address: SocketAddress::new_v4(
                            ipv4_addr.octets()[0],
                            ipv4_addr.octets()[1],
                            ipv4_addr.octets()[2],
                            ipv4_addr.octets()[3],
                            listener_port,
                        ),
                        hostname: "*".to_string(),
                        path,
                        position: RulePosition::Post.into(),
                        ..Default::default()
                    }),
                })
            };

            add_route((PathRule::equals("/healthcheck"), health_backend))?;
            add_route((PathRule::equals("/metrics"), health_backend))?;

            add_route((
                PathRule::regex("/v1/components/[^/]+/workers/[^/]+/connect$"),
                worker_backend,
            ))?;
            add_route((PathRule::prefix("/v1/api"), worker_backend))?;
            add_route((
                PathRule::regex("/v1/components/[^/]+/workers"),
                worker_backend,
            ))?;
            add_route((
                PathRule::regex("/v1/components/[^/]+/invoke"),
                worker_backend,
            ))?;
            add_route((
                PathRule::regex("/v1/components/[^/]+/invoke-and-await"),
                worker_backend,
            ))?;
            add_route((PathRule::prefix("/v1/components"), component_backend))?;
            add_route((PathRule::prefix("/"), component_backend))?;
        }

        Ok(command_channel)
    }
}

#[cfg(unix)]
pub fn start_proxy(
    listener_addr: &str,
    listener_port: u16,
    healthcheck_port: u16,
    all_run_details: &AllRunDetails,
    join_set: &mut JoinSet<Result<(), anyhow::Error>>,
) -> Result<Channel<WorkerRequest, WorkerResponse>, anyhow::Error> {
    info!("Starting proxy on Unix");

    let ipv4_addr: Ipv4Addr = listener_addr.parse().context(format!(
        "Failed at parsing the listener host address {}",
        listener_addr
    ))?;
    let listener_socket_addr = SocketAddress::new_v4(
        ipv4_addr.octets()[0],
        ipv4_addr.octets()[1],
        ipv4_addr.octets()[2],
        ipv4_addr.octets()[3],
        listener_port,
    );
    let http_listener = ListenerBuilder::new_http(listener_socket_addr).to_http(None)?;

    let (mut command_channel, proxy_channel) =
        Channel::generate(1000, 10000).with_context(|| "should create a channel")?;

    let mut dispatch = |request| {
        command_channel.write_message(&request)?;
        let response = command_channel.read_message();
        info!("Proxy response: {:?}", response);
        Ok::<(), anyhow::Error>(())
    };

    let _join_handle = join_set.spawn_blocking(move || {
        let span = tracing::info_span!("proxy");
        let _enter = span.enter();
        let max_buffers = 500;
        let buffer_size = 16384;
        sozu_lib::http::testing::start_http_worker(
            http_listener,
            proxy_channel,
            max_buffers,
            buffer_size,
        )
    });

    let component_backend = "golem-component";
    let worker_backend = "golem-worker";
    let health_backend = "golem-health";

    // set up the clusters. We'll have one per service with a single backend per cluster
    {
        let mut add_backend = |(name, port): (&str, u16)| {
            dispatch(WorkerRequest {
                id: format!("add-{name}-cluster"),
                content: RequestType::AddCluster(Cluster {
                    cluster_id: name.to_string(),
                    sticky_session: false,
                    https_redirect: false,
                    load_balancing: LoadBalancingAlgorithms::Random as i32,
                    ..Default::default()
                })
                .into(),
            })?;

            dispatch(WorkerRequest {
                id: format!("add-{name}-backend"),
                content: RequestType::AddBackend(AddBackend {
                    cluster_id: name.to_string(),
                    backend_id: name.to_string(),
                    address: SocketAddress::new_v4(
                        ipv4_addr.octets()[0],
                        ipv4_addr.octets()[1],
                        ipv4_addr.octets()[2],
                        ipv4_addr.octets()[3],
                        port,
                    ),
                    ..Default::default()
                })
                .into(),
            })
        };

        add_backend((health_backend, healthcheck_port))?;
        add_backend((
            component_backend,
            all_run_details.component_service.http_port,
        ))?;
        add_backend((worker_backend, all_run_details.worker_service.http_port))?;
    }

    // set up routing
    {
        let mut route_counter = -1;
        let mut add_route = |(path, cluster_id): (PathRule, &str)| {
            route_counter += 1;
            dispatch(WorkerRequest {
                id: format!("add-golem-frontend-${route_counter}"),
                content: RequestType::AddHttpFrontend(RequestHttpFrontend {
                    cluster_id: Some(cluster_id.to_string()),
                    address: listener_socket_addr,
                    hostname: "*".to_string(),
                    path,
                    position: RulePosition::Post.into(),
                    ..Default::default()
                })
                .into(),
            })
        };

        add_route((PathRule::equals("/healthcheck"), health_backend))?;
        add_route((PathRule::equals("/metrics"), health_backend))?;

        add_route((
            PathRule::regex("/v1/components/[^/]+/workers/[^/]+/connect$"),
            worker_backend,
        ))?;
        add_route((PathRule::prefix("/v1/api"), worker_backend))?;
        add_route((
            PathRule::regex("/v1/components/[^/]+/workers"),
            worker_backend,
        ))?;
        add_route((
            PathRule::regex("/v1/components/[^/]+/invoke"),
            worker_backend,
        ))?;
        add_route((
            PathRule::regex("/v1/components/[^/]+/invoke-and-await"),
            worker_backend,
        ))?;
        add_route((PathRule::prefix("/v1/components"), component_backend))?;
        add_route((PathRule::prefix("/"), component_backend))?;
    }

    Ok(command_channel)
}


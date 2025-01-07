// Copyright 2024-2025 Golem Cloud
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
use sozu_command_lib::proto::command::WorkerResponse;
use sozu_command_lib::{
    channel::Channel,
    config::ListenerBuilder,
    proto::command::{
        request::RequestType, AddBackend, Cluster, LoadBalancingAlgorithms, PathRule,
        RequestHttpFrontend, RulePosition, SocketAddress, WorkerRequest,
    },
};
use std::net::Ipv4Addr;
use tokio::task::JoinSet;
use tracing::info;

use crate::AllRunDetails;

pub fn start_proxy(
    listener_addr: &str,
    listener_port: u16,
    healthcheck_port: u16,
    all_run_details: &AllRunDetails,
    join_set: &mut JoinSet<Result<(), anyhow::Error>>,
) -> Result<Channel<WorkerRequest, WorkerResponse>, anyhow::Error> {
    info!("Starting proxy");

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

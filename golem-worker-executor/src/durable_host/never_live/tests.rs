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

use super::allows_execution;
use golem_common::model::oplog::host_functions::HostFunctionName;
use std::collections::BTreeSet;
use test_r::test;

/// The interfaces whose functions a never-live agent may execute: local clocks, random, the
/// pinned environment and configuration, the private filesystem, polling, spans and its own
/// metadata.
const LOCAL_INTERFACES: &[&str] = &[
    "wall_clock",
    "monotonic_clock",
    "clocks::monotonic-clock",
    "clocks::system-clock",
    "random",
    "random::random",
    "random::insecure",
    "random::insecure_seed",
    "random::insecure-seed",
    "cli::environment",
    "filesystem::types::descriptor",
    "filesystem::input_stream",
    "filesystem::output_stream",
    "filesystem::permissions",
    "io::poll",
    "io::poll::pollable",
    "golem::api::context",
    "golem::api::context::span",
    "wasi:config/store",
];

fn interface_of(name: &HostFunctionName) -> String {
    let fqfn = name.to_string();
    fqfn.rsplit_once("::")
        .map(|(interface, _)| interface.to_string())
        .unwrap_or(fqfn)
}

#[test]
fn only_the_local_host_functions_are_allowed_and_custom_is_refused() {
    let allowed = HostFunctionName::NAMED
        .iter()
        .filter(|name| allows_execution(name))
        .map(|name| name.to_string())
        .collect::<BTreeSet<_>>();

    let expected = HostFunctionName::NAMED
        .iter()
        .filter(|name| {
            LOCAL_INTERFACES.contains(&interface_of(name).as_str())
                || name.to_string() == "golem::api::get_self_metadata"
        })
        .map(|name| name.to_string())
        .collect::<BTreeSet<_>>();

    assert_eq!(allowed, expected);
    assert_eq!(allowed.len(), 40);
    assert!(!allows_execution(&HostFunctionName::Custom(
        "wall_clock::now".to_string()
    )));
    assert!(!allows_execution(&HostFunctionName::Custom(
        "my::custom::effect".to_string()
    )));
}

#[test]
fn remote_reads_and_socket_checks_are_refused() {
    for name in [
        HostFunctionName::KeyvalueEventualGet,
        HostFunctionName::BlobstoreContainerGetData,
        HostFunctionName::P3HttpClientSend,
        HostFunctionName::GolemRpcWasmRpcInvokeAndAwaitResult,
        HostFunctionName::GolemRpcFutureInvokeResultCancel,
        HostFunctionName::GolemRpcCancellationTokenCancel,
        HostFunctionName::SocketsTcpConnectPermissionCheck,
        HostFunctionName::SocketsUdpPermissionCheck,
        HostFunctionName::SocketsIpNameLookupResolveAddresses,
        HostFunctionName::P3SocketsIpNameLookupResolveAddresses,
        HostFunctionName::GolemApiCreatePromise,
        HostFunctionName::GolemApiCompletePromise,
        HostFunctionName::GolemApiGetAgentMetadata,
        HostFunctionName::GolemSecretsReveal,
        HostFunctionName::RdbmsPostgresDbConnectionBeginTransaction,
        HostFunctionName::GolemPermissionsCardLiveness,
    ] {
        assert!(!allows_execution(&name), "{name}");
    }
}

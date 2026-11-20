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

use super::{Execution, execution};

fn allows_execution(name: &HostFunctionName) -> bool {
    execution(name) != Execution::Refused
}
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

/// The allowed functions whose result the oplog does not record when they execute now: clocks,
/// random, the environment and the filesystem. They taint the generation. Polling, spans, the
/// pinned configuration and the agent's own metadata do not.
#[test]
fn clock_random_environment_and_filesystem_reads_taint_the_generation() {
    let tainting = HostFunctionName::NAMED
        .iter()
        .filter(|name| execution(name) == Execution::AllowedUnrecorded)
        .map(|name| name.to_string())
        .collect::<BTreeSet<_>>();
    let expected = HostFunctionName::NAMED
        .iter()
        .filter(|name| allows_execution(name))
        .filter(|name| {
            let interface = interface_of(name);
            ["clock", "random", "environment", "filesystem"]
                .iter()
                .any(|family| interface.contains(family))
        })
        .map(|name| name.to_string())
        .collect::<BTreeSet<_>>();
    assert_eq!(tainting, expected);
    for untainting in [
        "io::poll::pollable::ready",
        "golem::api::get_self_metadata",
        "wasi:config/store::get",
    ] {
        assert!(
            HostFunctionName::NAMED.iter().any(|name| {
                name.to_string() == untainting && execution(name) == Execution::Allowed
            }),
            "{untainting}"
        );
    }
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

fn is_refusal(error: &golem_service_base::error::worker_executor::WorkerExecutorError) -> bool {
    matches!(
        error,
        golem_service_base::error::worker_executor::WorkerExecutorError::NeverLiveEffectRefused { .. }
    )
}

#[test]
async fn the_refusing_key_value_service_lets_reads_through_and_never_writes() {
    use crate::services::key_value::refusing::RefusingKeyValueService;
    use crate::services::key_value::{DefaultKeyValueService, KeyValueService};
    use crate::storage::keyvalue::memory::InMemoryKeyValueStorage;
    use golem_common::model::environment::EnvironmentId;
    use std::sync::Arc;

    let environment_id = EnvironmentId::new();
    let inner: Arc<dyn KeyValueService> = Arc::new(DefaultKeyValueService::new(Arc::new(
        InMemoryKeyValueStorage::new(),
    )));
    inner
        .set(environment_id, "b".into(), "k".into(), b"before".to_vec())
        .await
        .unwrap();
    let refusing = RefusingKeyValueService {
        inner: inner.clone(),
    };

    for result in [
        refusing
            .set(environment_id, "b".into(), "k".into(), b"after".to_vec())
            .await,
        refusing
            .set_many(
                environment_id,
                "b".into(),
                Arc::from(vec![("k".to_string(), b"after".to_vec())]),
            )
            .await,
        refusing
            .delete(environment_id, "b".into(), "k".into())
            .await,
        refusing
            .delete_many(environment_id, "b".into(), Arc::from(vec!["k".to_string()]))
            .await,
    ] {
        let error = result.unwrap_err();
        assert!(is_refusal(error.downcast_ref().unwrap()), "{error:?}");
    }

    assert_eq!(
        refusing
            .get(environment_id, "b".into(), "k".into())
            .await
            .unwrap(),
        Some(b"before".to_vec())
    );
    assert_eq!(
        inner
            .get(environment_id, "b".into(), "k".into())
            .await
            .unwrap(),
        Some(b"before".to_vec())
    );
}

#[test]
async fn the_refusing_promise_service_never_creates_or_completes() {
    use crate::services::promise::refusing::RefusingPromiseService;
    use crate::services::promise::{PromiseService, PromiseServiceMock};
    use golem_common::base_model::component::ComponentId;
    use golem_common::model::oplog::OplogIndex;
    use golem_common::model::{AgentFingerprint, AgentId, PromiseId};
    use std::sync::Arc;

    let mock = Arc::new(PromiseServiceMock::new());
    let refusing = RefusingPromiseService {
        inner: mock.clone(),
    };
    let agent_id = AgentId {
        component_id: ComponentId(uuid::Uuid::new_v4()),
        agent_id: "never-live".to_string(),
    };
    let promise_id = PromiseId {
        agent_id: agent_id.clone(),
        fingerprint: AgentFingerprint::new(),
        oplog_idx: OplogIndex::from_u64(1),
    };

    // The mock panics on `create`, so a refusal proves the inner service was not called.
    let created = refusing
        .create(&agent_id, AgentFingerprint::new(), OplogIndex::from_u64(1))
        .await;
    assert!(is_refusal(&created.unwrap_err()));
    assert!(is_refusal(
        &refusing
            .complete(promise_id.clone(), vec![1])
            .await
            .unwrap_err()
    ));
    assert!(is_refusal(
        &refusing
            .complete_without_wakeup(promise_id, vec![1])
            .await
            .unwrap_err()
    ));
    assert!(mock.all_completed().await.is_empty());
}

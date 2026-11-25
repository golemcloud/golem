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

use super::admits;
use golem_common::model::oplog::DurableFunctionType;
use golem_common::model::oplog::host_functions::HostFunctionName;
use std::collections::BTreeSet;
use test_r::test;

const ALL_TYPES: [DurableFunctionType; 4] = [
    DurableFunctionType::ReadLocal,
    DurableFunctionType::WriteLocal,
    DurableFunctionType::ReadRemote,
    DurableFunctionType::WriteRemote,
];

fn every_host_function() -> Vec<HostFunctionName> {
    HostFunctionName::NAMED
        .iter()
        .cloned()
        .chain([HostFunctionName::Custom("wall_clock::now".to_string())])
        .collect()
}

/// A never-live owner admits a call that executes now only when it is `ReadLocal` and not
/// `Custom`: a write, a remote read and a guest-named function are refused, also a `Custom`
/// function that the guest names after a local one.
#[test]
fn a_call_that_executes_now_is_admitted_only_when_it_is_a_local_read() {
    for name in every_host_function() {
        for function_type in &ALL_TYPES {
            let expected = matches!(function_type, DurableFunctionType::ReadLocal)
                && !matches!(name, HostFunctionName::Custom(_));
            assert_eq!(
                admits(true, true, function_type, &name),
                expected,
                "{name} {function_type:?}"
            );
        }
    }
    assert!(!admits(
        true,
        true,
        &DurableFunctionType::ReadRemote,
        &HostFunctionName::KeyvalueEventualGet
    ));
    assert!(admits(
        true,
        true,
        &DurableFunctionType::ReadLocal,
        &HostFunctionName::WallClockNow
    ));
}

/// GOL-2143 P2: a never-live owner admits a call that only returns its recorded result, of every
/// function and type, `Custom` included.
#[test]
fn a_recorded_result_is_admitted_for_every_call() {
    for name in every_host_function() {
        for function_type in &ALL_TYPES {
            assert!(admits(true, false, function_type, &name), "{name}");
        }
    }
}

/// GOL-2143 P9: an unmarked owner admits every call, recorded or executing, so its snapshot hooks
/// run as before.
#[test]
fn an_unmarked_owner_admits_every_call() {
    for name in every_host_function() {
        for function_type in &ALL_TYPES {
            for executes_now in [false, true] {
                assert!(admits(false, executes_now, function_type, &name), "{name}");
            }
        }
    }
}

/// What a never-live agent's guest import does, for the import audit below.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Import {
    /// The import's peer effects are durable calls, which a never-live agent executes only if
    /// they are `ReadLocal`; the rest of the interface only builds or reads local values.
    Durable,
    /// No peer effect: a local value, the private filesystem or a socket that is not connected.
    Local,
    /// A native peer effect: its wrapper refuses it for a never-live agent.
    PeerBoundary,
}

/// The audit of every guest import the executor links. A key is an interface, or an interface
/// and one of its functions, which overrides the interface.
const IMPORT_AUDIT: &[(&str, Import)] = &[
    ("golem:agent/durable-streams@2.0.0", Import::Durable),
    ("golem:agent/host@2.0.0", Import::Durable),
    ("golem:agent/invocation-context@2.0.0", Import::Durable),
    ("golem:api/context@1.5.0", Import::Durable),
    ("golem:api/host@1.5.0", Import::Durable),
    ("golem:api/oplog@1.5.0", Import::Durable),
    ("golem:api/retry-scope@1.5.0", Import::Durable),
    ("golem:api/retry@1.5.0", Import::Durable),
    // UUID conversions, and the wrapping of a guest stream in a resource: local values only.
    ("golem:core/types@2.0.0", Import::Local),
    ("golem:durability/durability@1.6.0", Import::Durable),
    ("golem:network/dns@0.1.0", Import::Durable),
    ("golem:permissions/derive@0.1.0", Import::Durable),
    ("golem:permissions/inspect@0.1.0", Import::Durable),
    (
        "golem:permissions/kernel-introspection@0.1.0",
        Import::Durable,
    ),
    ("golem:permissions/revoke@0.1.0", Import::Durable),
    ("golem:permissions/types@0.1.0", Import::Durable),
    ("golem:permissions/wallet@0.1.0", Import::Durable),
    ("golem:quota/types@1.5.0", Import::Durable),
    ("golem:rdbms/ignite2@1.5.0", Import::Durable),
    ("golem:rdbms/mysql@1.5.0", Import::Durable),
    ("golem:rdbms/postgres@1.5.0", Import::Durable),
    ("golem:secrets/reveal@0.1.0", Import::Durable),
    ("golem:secrets/types@0.1.0", Import::Durable),
    ("golem:tool/host@0.1.0", Import::Durable),
    ("golem:tool/streams@0.1.0", Import::Durable),
    ("golem:tool/underlying@0.1.0", Import::Durable),
    ("golem:websocket/client@1.5.0", Import::Durable),
    ("wasi:blobstore/blobstore", Import::Durable),
    ("wasi:blobstore/container", Import::Durable),
    ("wasi:blobstore/types", Import::Durable),
    ("wasi:keyvalue/cache@0.1.0", Import::Durable),
    ("wasi:keyvalue/eventual-batch@0.1.0", Import::Durable),
    ("wasi:keyvalue/eventual@0.1.0", Import::Durable),
    ("wasi:keyvalue/types@0.1.0", Import::Durable),
    ("wasi:keyvalue/wasi-keyvalue-error@0.1.0", Import::Local),
    ("wasi:logging/logging", Import::Durable),
    ("wasi:config/store@0.2.0-draft", Import::Durable),
    ("wasi:cli/environment@0.2.6", Import::Durable),
    ("wasi:cli/environment@0.3.0", Import::Durable),
    ("wasi:cli/exit@0.2.6", Import::Local),
    ("wasi:cli/exit@0.3.0", Import::Local),
    ("wasi:cli/stdin@0.2.6", Import::Local),
    ("wasi:cli/stdin@0.3.0", Import::Local),
    ("wasi:cli/stdout@0.2.6", Import::Local),
    ("wasi:cli/stdout@0.3.0", Import::Local),
    ("wasi:cli/stderr@0.2.6", Import::Local),
    ("wasi:cli/stderr@0.3.0", Import::Local),
    ("wasi:cli/terminal-stdin@0.2.6", Import::Local),
    ("wasi:cli/terminal-stdin@0.3.0", Import::Local),
    ("wasi:cli/terminal-stdout@0.2.6", Import::Local),
    ("wasi:cli/terminal-stdout@0.3.0", Import::Local),
    ("wasi:cli/terminal-stderr@0.2.6", Import::Local),
    ("wasi:cli/terminal-stderr@0.3.0", Import::Local),
    ("wasi:clocks/monotonic-clock@0.2.6", Import::Durable),
    ("wasi:clocks/monotonic-clock@0.3.0", Import::Durable),
    ("wasi:clocks/wall-clock@0.2.6", Import::Durable),
    ("wasi:clocks/system-clock@0.3.0", Import::Durable),
    ("wasi:random/random@0.2.6", Import::Durable),
    ("wasi:random/random@0.3.0", Import::Durable),
    ("wasi:random/insecure@0.2.6", Import::Durable),
    ("wasi:random/insecure@0.3.0", Import::Durable),
    ("wasi:random/insecure-seed@0.2.6", Import::Durable),
    ("wasi:random/insecure-seed@0.3.0", Import::Durable),
    ("wasi:filesystem/preopens@0.2.6", Import::Local),
    ("wasi:filesystem/preopens@0.3.0", Import::Local),
    ("wasi:filesystem/types@0.2.6", Import::Local),
    ("wasi:filesystem/types@0.3.0", Import::Local),
    ("wasi:io/error@0.2.6", Import::Local),
    ("wasi:io/poll@0.2.6", Import::Durable),
    // A socket stream exists only after a connect or accept, which a never-live agent cannot
    // execute; the other streams are of private files.
    ("wasi:io/streams@0.2.6", Import::Local),
    ("wasi:http/types@0.3.0", Import::Durable),
    ("wasi:http/client@0.3.0", Import::Durable),
    ("wasi:sockets/instance-network@0.2.6", Import::Local),
    ("wasi:sockets/ip-name-lookup@0.2.6", Import::Durable),
    ("wasi:sockets/ip-name-lookup@0.3.0", Import::Durable),
    (
        "wasi:sockets/tcp-create-socket@0.2.6#create-tcp-socket",
        Import::Local,
    ),
    (
        "wasi:sockets/udp-create-socket@0.2.6#create-udp-socket",
        Import::Local,
    ),
    // A socket's options and addresses are local observations.
    ("wasi:sockets/tcp@0.2.6", Import::Local),
    (
        "wasi:sockets/tcp@0.2.6#[method]tcp-socket.start-bind",
        Import::Local,
    ),
    (
        "wasi:sockets/tcp@0.2.6#[method]tcp-socket.start-connect",
        Import::PeerBoundary,
    ),
    (
        "wasi:sockets/tcp@0.2.6#[method]tcp-socket.start-listen",
        Import::PeerBoundary,
    ),
    (
        "wasi:sockets/tcp@0.2.6#[method]tcp-socket.accept",
        Import::PeerBoundary,
    ),
    ("wasi:sockets/udp@0.2.6", Import::Local),
    (
        "wasi:sockets/udp@0.2.6#[method]udp-socket.start-bind",
        Import::Durable,
    ),
    (
        "wasi:sockets/udp@0.2.6#[method]udp-socket.stream",
        Import::Durable,
    ),
    (
        "wasi:sockets/udp@0.2.6#[method]incoming-datagram-stream.receive",
        Import::Durable,
    ),
    (
        "wasi:sockets/udp@0.2.6#[method]outgoing-datagram-stream.check-send",
        Import::Durable,
    ),
    (
        "wasi:sockets/udp@0.2.6#[method]outgoing-datagram-stream.send",
        Import::Durable,
    ),
    ("wasi:sockets/types@0.3.0", Import::Local),
    (
        "wasi:sockets/types@0.3.0#[static]tcp-socket.create",
        Import::Local,
    ),
    (
        "wasi:sockets/types@0.3.0#[static]udp-socket.create",
        Import::Local,
    ),
    (
        "wasi:sockets/types@0.3.0#[method]tcp-socket.listen",
        Import::PeerBoundary,
    ),
    (
        "wasi:sockets/types@0.3.0#[method]tcp-socket.bind",
        Import::Durable,
    ),
    (
        "wasi:sockets/types@0.3.0#[method]tcp-socket.connect",
        Import::Durable,
    ),
    (
        "wasi:sockets/types@0.3.0#[method]tcp-socket.send",
        Import::Durable,
    ),
    (
        "wasi:sockets/types@0.3.0#[method]tcp-socket.receive",
        Import::Durable,
    ),
    (
        "wasi:sockets/types@0.3.0#[method]udp-socket.bind",
        Import::Durable,
    ),
    (
        "wasi:sockets/types@0.3.0#[method]udp-socket.connect",
        Import::Durable,
    ),
    (
        "wasi:sockets/types@0.3.0#[method]udp-socket.disconnect",
        Import::Durable,
    ),
    (
        "wasi:sockets/types@0.3.0#[method]udp-socket.send",
        Import::Durable,
    ),
    (
        "wasi:sockets/types@0.3.0#[method]udp-socket.receive",
        Import::Durable,
    ),
];

/// The worlds whose imports a guest can have: the Golem world and the WASI Preview 2 and
/// Preview 3 command and HTTP worlds.
const GUEST_WORLDS: &[&str] = &[
    "golem:api/golem",
    "wasi:cli/imports@0.2.6",
    "wasi:http/imports@0.2.6",
    "wasi:cli/imports@0.3.0",
    "wasi:http/service@0.3.0",
];

/// U15: every function that the executor links for a guest has a class in the import audit, and
/// every audit entry names a linked import. A new import fails here until someone classifies it.
#[test]
fn every_linked_guest_import_is_in_the_import_audit() {
    use crate::Bootstrap;
    use crate::bootstrap::ServerBootstrap;
    use crate::workerctx::default::Context;
    use std::collections::BTreeMap;

    let engine = wasmtime::Engine::new(
        &golem_common::wasmtime_config::create_wasmtime_config_without_fs_cache(),
    )
    .unwrap();
    let mut linker =
        <ServerBootstrap as Bootstrap<Context>>::create_wasmtime_linker(&ServerBootstrap, &engine)
            .unwrap();
    linker.allow_shadowing(false);

    let mut resolve = wit_parser::Resolve::default();
    let (package, _) = resolve
        .push_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../wit"))
        .unwrap();
    let mut interfaces = BTreeMap::new();
    for world in GUEST_WORLDS {
        let world = resolve.select_world(&[package], Some(world)).unwrap();
        for item in resolve.worlds[world].imports.values() {
            if let wit_parser::WorldItem::Interface { id, .. } = item {
                interfaces.insert(resolve.id_of(*id).unwrap(), *id);
            }
        }
    }

    // A guest's `0.2.x` import resolves to any `0.2` patch version that the linker defines, and
    // wasmtime-wasi links its own patch version, so the probe tries each one.
    let candidates = |interface: &str| -> Vec<String> {
        match interface.split_once("@0.2.") {
            Some((name, patch)) if patch.parse::<u32>().is_ok() => (0..=40)
                .map(|patch| format!("{name}@0.2.{patch}"))
                .collect(),
            _ => vec![interface.to_string()],
        }
    };
    let mut linked = BTreeSet::new();
    for (interface, id) in &interfaces {
        for function in resolve.interfaces[*id].functions.keys() {
            let is_linked = candidates(interface).iter().any(|candidate| {
                let mut instance = linker.instance(candidate).unwrap();
                instance.func_new(function, |_, _, _, _| Ok(())).is_err()
            });
            if is_linked {
                linked.insert(format!("{interface}#{function}"));
            }
        }
    }
    assert!(
        linked.contains("wasi:sockets/tcp@0.2.6#[method]tcp-socket.start-connect"),
        "the probe found no linked socket import: {linked:?}"
    );

    let class_of = |import: &str| {
        let interface = import
            .split_once('#')
            .map_or(import, |(interface, _)| interface);
        IMPORT_AUDIT
            .iter()
            .find(|(key, _)| *key == import)
            .or_else(|| IMPORT_AUDIT.iter().find(|(key, _)| *key == interface))
            .map(|(_, class)| *class)
    };
    let unclassified = linked
        .iter()
        .filter(|import| class_of(import).is_none())
        .collect::<Vec<_>>();
    assert!(
        unclassified.is_empty(),
        "unclassified imports: {unclassified:#?}"
    );

    let stale = IMPORT_AUDIT
        .iter()
        .map(|(key, _)| *key)
        .filter(|key| {
            !linked
                .iter()
                .any(|import| import == key || import.split_once('#').unwrap().0 == *key)
        })
        .collect::<Vec<_>>();
    assert!(
        stale.is_empty(),
        "audit entries that name no linked import: {stale:#?}"
    );
}
